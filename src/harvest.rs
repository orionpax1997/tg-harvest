use grammers_client::{Client, message::Message};
use crate::config::{ChannelConfig, GlobalConfig};
use crate::db;
use crate::filter::{ChannelFilter, MessageFilter};
use crate::forward::{send_with_retry, forward_album, resolve_peer, PeerCache, PeerNames};
use anyhow::Context;
use chrono::Utc;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

pub struct HarvestStats {
    pub total_scanned: i64,
    pub total_forwarded: i64,
    pub total_skipped: i64,
}

pub async fn harvest_channel(
    client: &Client,
    db_conn: &Arc<Mutex<Connection>>,
    channel_config: &ChannelConfig,
    global_config: &GlobalConfig,
    peer_cache: &PeerCache,
    peer_names: &PeerNames,
) -> anyhow::Result<HarvestStats> {
    let source = channel_config.source.clone();
    let target = channel_config.target.clone();
    let limit = channel_config.limit;
    let batch_size = global_config.batch_size;
    let forward_delay_ms = global_config.forward_delay_ms;

    let source_label = lookup_name(peer_names, &source, &source);
    let target_label = lookup_name(peer_names, &target, &target);

    // Load cursor outside of network calls
    let mut cursor = {
        let db = db_conn.lock().await;
        db::load_cursor(&db, &source, &target).context("Failed to load cursor")?
    };

    tracing::info!(
        "Harvest: {} -> {} (from msg {})",
        source_label,
        target_label,
        cursor.last_msg_id
    );

    let source_peer = resolve_peer(client, &source, peer_cache).await?;

    // Get latest message ID first
    let source_ref = source_peer.to_ref().await.context("Failed to get peer ref")?;
    let mut iter = client.iter_messages(source_ref);
    iter = iter.limit(1);
    let latest_msg_id = match iter.next().await {
        Ok(Some(msg)) => msg.id(),
        Ok(None) => {
            tracing::warn!("No messages in channel '{}'", source);
            return Ok(HarvestStats {
                total_scanned: 0,
                total_forwarded: 0,
                total_skipped: 0,
            });
        }
        Err(e) => return Err(e).context("Failed to get latest msg"),
    };


    let filter = ChannelFilter::new(&channel_config.filter);

    let mut total_scanned = cursor.total_scanned;
    let mut total_forwarded = cursor.total_forwarded;
    let mut total_skipped = 0i64;
    let mut processed_count = 0usize;

    // Start from last forwarded msg_id + 100
    let mut offset: i32 = if cursor.last_msg_id > 0 {
        (cursor.last_msg_id + 100) as i32
    } else {
        100
    };

    let mut empty_count = 0;

    loop {
        if limit > 0 && processed_count >= limit {
            tracing::info!("Limit {} reached for {}", limit, source);
            break;
        }

        if offset > latest_msg_id as i32 + 100 {
            tracing::info!("Reached latest+100 for {}", source);
            break;
        }

        if cursor.last_msg_id > 0 && empty_count >= 3 {
            tracing::info!("Empty batches, stopping");
            break;
        }

        let batch_start = Instant::now();
        tracing::debug!("Fetching batch with offset_id={}", offset);

        let source_ref = source_peer.to_ref().await.context("Failed to get peer ref")?;
        let mut iter = client.iter_messages(source_ref);
        iter = iter.offset_id(offset).limit(batch_size);

        let mut batch: Vec<Message> = Vec::new();
        let mut raw_count = 0usize;
        let mut deleted_count = 0usize;
        loop {
            match iter.next().await {
                Ok(Some(msg)) => {
                    raw_count += 1;
                    let text = msg.text();
                    let media = msg.media();
                    if text.is_empty() && media.is_none() {
                        deleted_count += 1;
                        continue;
                    }
                    batch.push(msg);
                }
                Ok(None) => break,
                Err(e) => {
                    let err_msg = e.to_string();
                    if err_msg.contains("CHAT_READ_FORBIDDEN") || err_msg.contains("ACCESS_DENIED") {
                        let err = format!("来源频道/群组无读取权限: {}", err_msg);
                        tracing::error!("{}", err);
                        return Err(anyhow::anyhow!(err));
                    }
                    return Err(e).context("获取消息失败");
                }
            }
        }

        let fetch_elapsed = batch_start.elapsed();
        tracing::info!(
            "Batch [{}, {}) fetched: raw={}, deleted={}, kept={}, fetch={}ms",
            offset - batch_size as i32,
            offset,
            raw_count,
            deleted_count,
            batch.len(),
            fetch_elapsed.as_millis(),
        );

        if batch.is_empty() {
            if cursor.last_msg_id > 0 {
                empty_count += 1;
                tracing::debug!("Empty batch #{}, continuing...", empty_count);
                if empty_count >= 3 {
                    tracing::info!("Empty batches, stopping");
                    break;
                }
            }
            offset += batch_size as i32;
            continue;
        }

        empty_count = 0;
        batch.sort_by_key(|m| m.id());

        let mut processed_group_ids: HashSet<i64> = HashSet::new();
        let mut i = 0;
        while i < batch.len() {
            if limit > 0 && processed_count >= limit {
                break;
            }

            let msg = &batch[i];
            let grouped_id = msg.grouped_id();


            if let Some(gid) = grouped_id {
                if processed_group_ids.contains(&gid) {
                    i += 1;
                    continue;
                }
                processed_group_ids.insert(gid);

                let group_start = i;
                let mut group_end = i + 1;
                while group_end < batch.len() && batch[group_end].grouped_id() == Some(gid) {
                    group_end += 1;
                }

                let group: Vec<Message> = batch[group_start..group_end].to_vec();



                let first_accepted = filter.accept(&group[0]);
                tracing::debug!(
                    "Album check: grouped_id={}, first_msg_id={}, last_msg_id={}, first_accepted={}, reactions={:?}, comments={:?}",
                    gid, group[0].id(), cursor.last_msg_id, first_accepted,
                    group[0].reaction_count(), group[0].reply_count()
                );

                let first_msg_id = group[0].id();
                tracing::debug!(
                    "Album check: grouped_id={}, first_msg_id={}, last_msg_id={}, first_accepted={}",
                    gid, first_msg_id, cursor.last_msg_id, first_accepted
                );
                if first_msg_id as i64 <= cursor.last_msg_id {
                    total_skipped += group.len() as i64;
                    tracing::debug!("Skipped album with grouped_id {} (first msg {} already forwarded)", gid, first_msg_id);
                    total_scanned += group.len() as i64;
                    processed_count += group.len();
                    i = group_end;
                    continue;
                }

                if !first_accepted {
                    total_skipped += group.len() as i64;
                    tracing::debug!("Skipped album with grouped_id {} (first msg rejected)", gid);
                } else {
                    tracing::info!(
                        "Forwarding album of {} msgs (msg {}, total forwarded so far: {})",
                        group.len(),
                        group[0].id(),
                        total_forwarded
                    );
                    match forward_album(client, &group, &target, peer_cache).await {
                        Ok(_) => {
                            total_forwarded += group.len() as i64;
                            let last_msg = &group[group.len() - 1];
                            cursor.last_msg_id = last_msg.id() as i64;
                            cursor.last_msg_date = last_msg.date().to_rfc3339();
                            tracing::debug!(
                                "Forwarded album with {} msgs, grouped_id={} (reactions: {:?}, comments: {:?})",
                                group.len(),
                                gid,
                                msg.reaction_count(),
                                msg.reply_count()
                            );
                        }
                        Err(e) => {
                            tracing::error!("任务终止: {}", e);
                            return Err(e);
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(forward_delay_ms)).await;
                }

                total_scanned += group.len() as i64;
                processed_count += group.len();
                i = group_end;
            } else {
                total_scanned += 1;
                processed_count += 1;

                let accepted = filter.accept(msg);
                tracing::debug!(
                    "Msg check: msg_id={}, last_msg_id={}, accepted={}, reactions={:?}, comments={:?}",
                    msg.id(), cursor.last_msg_id, accepted,
                    msg.reaction_count(), msg.reply_count()
                );
                if msg.id() as i64 <= cursor.last_msg_id {
                    total_skipped += 1;
                    tracing::debug!("Skipped msg_id {} (already forwarded)", msg.id());
                    i += 1;
                    continue;
                } else if accepted {
                    tracing::info!(
                        "Forwarding msg {} (total forwarded so far: {})",
                        msg.id(),
                        total_forwarded
                    );
                    match send_with_retry(client, msg, &target, 5, peer_cache).await {
                        Ok(true) => {
                            total_forwarded += 1;
                            cursor.last_msg_id = msg.id() as i64;
                            cursor.last_msg_date = msg.date().to_rfc3339();
                            tracing::debug!(
                                "Forwarded msg_id {} (reactions: {:?}, comments: {:?})",
                                msg.id(),
                                msg.reaction_count(),
                                msg.reply_count()
                            );
                        }
                        Ok(false) => {
                            total_skipped += 1;
                        }
                        Err(e) => {
                            tracing::error!("任务终止: {}", e);
                            return Err(e);
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(forward_delay_ms)).await;
                } else {
                    total_skipped += 1;
                    tracing::trace!("Skipped msg_id {}", msg.id());
                }

                i += 1;
            }

            cursor.total_scanned = total_scanned;
            cursor.total_forwarded = total_forwarded;
            cursor.last_run_at = Utc::now().to_rfc3339();
        }

        {
            let db = db_conn.lock().await;
            db::save_cursor(&db, &source, &target, &cursor).context("Failed to save cursor")?;
        }
        let total_elapsed = batch_start.elapsed();
        let process_elapsed = total_elapsed.saturating_sub(fetch_elapsed);
        tracing::info!(
            "Batch [{}, {}) processed: scanned={}, forwarded={}, process={}ms, total={}ms",
            offset - batch_size as i32,
            offset,
            total_scanned,
            total_forwarded,
            process_elapsed.as_millis(),
            total_elapsed.as_millis(),
        );

        offset += batch_size as i32;
    }

    Ok(HarvestStats {
        total_scanned,
        total_forwarded,
        total_skipped,
    })
}

fn lookup_name(names: &PeerNames, identifier: &str, fallback: &str) -> String {
    if let Ok(id) = identifier.parse::<i64>() {
        names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    } else {
        fallback.to_string()
    }
}
