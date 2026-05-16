use grammers_client::{Client, message::Message, peer::Peer};
use grammers_session::types::PeerRef;
use crate::config::{ChannelConfig, GlobalConfig};
use crate::db;
use crate::filter::{ChannelFilter, MessageFilter};
use crate::forward::{send_with_retry, forward_album};
use anyhow::Context;
use chrono::Utc;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn resolve_peer(client: &Client, identifier: &str) -> anyhow::Result<Peer> {
    if let Ok(id) = identifier.parse::<i64>() {
        let peer_ref = PeerRef {
            id: grammers_session::types::PeerId::channel(id),
            auth: grammers_session::types::PeerAuth::default(),
        };
        client
            .resolve_peer(peer_ref)
            .await
            .context(format!("Failed to resolve peer by ID: {}", identifier))
    } else {
        client
            .resolve_username(identifier)
            .await
            .context(format!("Failed to resolve username: {}", identifier))?
            .ok_or_else(|| anyhow::anyhow!("Peer '{}' not found", identifier))
    }
}

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
) -> anyhow::Result<HarvestStats> {
    let source = channel_config.source.clone();
    let target = channel_config.target.clone();
    let limit = channel_config.limit;
    let batch_size = global_config.batch_size;
    let forward_delay_ms = global_config.forward_delay_ms;

    // Load cursor outside of network calls
    let mut cursor = {
        let db = db_conn.lock().await;
        db::load_cursor(&db, &source, &target).context("Failed to load cursor")?
    };
    
    tracing::info!(
        "Harvest: {} -> {} (from msg {})",
        source,
        target,
        cursor.last_msg_id
    );

    let source_peer = resolve_peer(client, &source).await?;

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

        tracing::debug!("Fetching batch with offset_id={}", offset);

        let source_ref = source_peer.to_ref().await.context("Failed to get peer ref")?;
        let mut iter = client.iter_messages(source_ref);
        iter = iter.offset_id(offset).limit(batch_size);

        let mut batch: Vec<Message> = Vec::new();
        let mut raw_count = 0usize;
        loop {
            match iter.next().await {
                Ok(Some(msg)) => {
                    raw_count += 1;
                    let text = msg.text();
                    let media = msg.media();
                    if text.is_empty() && media.is_none() {
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

        if batch.is_empty() {
            if cursor.last_msg_id > 0 {
                empty_count += 1;
                tracing::debug!("Empty batch #{}, continuing...", empty_count);
                if empty_count >= 3 {
                    tracing::info!("Empty batches, stopping");
                    break;
                }
            }
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

                let first_msg_id = group[0].id();
                if first_msg_id as i64 <= cursor.last_msg_id {
                    total_skipped += group.len() as i64;
                    tracing::trace!("Skipped album with grouped_id {} (first msg {} already forwarded)", gid, first_msg_id);
                    total_scanned += group.len() as i64;
                    processed_count += group.len();
                    i = group_end;
                    continue;
                }

                if !first_accepted {
                    total_skipped += group.len() as i64;
                    tracing::trace!("Skipped album with grouped_id {} (first msg rejected)", gid);
                } else {
                    match forward_album(client, &group, &target).await {
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

                if msg.id() as i64 <= cursor.last_msg_id {
                    total_skipped += 1;
                    tracing::trace!("Skipped msg_id {} (already forwarded)", msg.id());
                    i += 1;
                    continue;
                } else if filter.accept(msg) {
                    match send_with_retry(client, msg, &target, 5).await {
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
        tracing::debug!(
            "Batch [{}, {}): raw={}, filtered={}, scanned={}, forwarded={}",
            offset - batch_size as i32,
            offset,
            raw_count,
            batch.len(),
            total_scanned,
            total_forwarded
        );

        offset += batch_size as i32;
    }

    Ok(HarvestStats {
        total_scanned,
        total_forwarded,
        total_skipped,
    })
}
