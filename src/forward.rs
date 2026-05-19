use grammers_client::{Client, message::Message, peer::Peer};
use grammers_session::types::PeerRef;
use anyhow::Context;
use std::time::Duration;

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

pub async fn forward_message(
    client: &Client,
    msg: &Message,
    target: &str,
) -> anyhow::Result<()> {
    let target_peer = resolve_peer(client, target).await?;
    tracing::info!("Resolving target peer: {}", target);

    let target_ref = target_peer
        .to_ref()
        .await
        .context("Failed to get target peer reference")?;
    
    tracing::debug!("Forwarding message {} to target: {}", msg.id(), target);
    
    msg.forward_to(target_ref)
        .await
        .map_err(|e| {
            tracing::error!("Forward error details: {:?}", e);
            if e.to_string().contains("CHAT_ADMIN_REQUIRED") {
                anyhow::anyhow!("目标频道缺少管理员权限，无法转发消息")
            } else {
                anyhow::anyhow!("Failed to forward message: {:?}", e)
            }
        })?;

    Ok(())
}

pub async fn forward_album(
    client: &Client,
    messages: &[Message],
    target: &str,
) -> anyhow::Result<()> {
    if messages.is_empty() {
        return Ok(());
    }

    let target_peer = resolve_peer(client, target).await?;
    tracing::debug!(
        "Forwarding album with {} messages to target: {}",
        messages.len(),
        target
    );

    let target_ref = target_peer
        .to_ref()
        .await
        .context("Failed to get target peer reference")?;

    let source_ref = messages[0]
        .peer_ref()
        .await
        .context("Failed to get source peer reference")?;
    let message_ids: Vec<i32> = messages.iter().map(|m| m.id()).collect();

    client
        .forward_messages(target_ref, &message_ids, source_ref)
        .await
        .map_err(|e| {
            tracing::error!("Album forward error details: {:?}", e);
            anyhow::anyhow!("Failed to forward album: {:?}", e)
        })?;

    Ok(())
}

fn rand_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    (now % i64::MAX as u128) as i64
}

pub async fn send_with_retry(
    client: &Client,
    msg: &Message,
    target: &str,
    max_retries: u32,
) -> anyhow::Result<bool> {
    let mut attempt = 0;
    let mut base_delay = 1.0f64;

    loop {
        match forward_message(client, msg, target).await {
            Ok(_) => return Ok(true),
            Err(e) => {
                if e.to_string().contains("目标频道缺少管理员权限") {
                    tracing::error!("任务终止: {}", e);
                    return Err(e);
                }
                
                attempt += 1;
                if attempt > max_retries {
                    tracing::warn!(
                        "Failed to forward message {} after {} attempts: {}",
                        msg.id(),
                        max_retries,
                        e
                    );
                    return Ok(false);
                }

                let error_str = e.to_string();
                if let Some(wait_seconds) = parse_flood_wait(&error_str) {
                    let delay = Duration::from_secs(wait_seconds as u64 + 1);
                    tracing::warn!("Flood wait {}s", wait_seconds);
                    tokio::time::sleep(delay).await;
                    base_delay = 1.0;
                    continue;
                }

                base_delay *= 2.0;
                let jitter = (rand_id() % 1000) as f64 / 1000.0;
                let delay = Duration::from_millis(((base_delay + jitter) * 1000.0) as u64);
                
                tracing::warn!(
                    "Retry {}/{}: {} (retry in {:.1}s)",
                    attempt,
                    max_retries,
                    e,
                    delay.as_secs_f64()
                );
                
                tokio::time::sleep(delay).await;
            }
        }
    }
}

fn parse_flood_wait(error: &str) -> Option<i32> {
    let prefix = "FLOOD_WAIT_";
    if let Some(pos) = error.find(prefix) {
        let after = &error[pos + prefix.len()..];
        after.parse().ok()
    } else if error.contains("FLOOD_WAIT") {
        let digits: String = error.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    } else {
        None
    }
}
