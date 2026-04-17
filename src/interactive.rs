use grammers_client::Client;
use grammers_client::peer::Peer;
use crate::config::ChannelConfig;
use std::fs;
use std::path::Path;

pub struct ChannelInfo {
    pub id: i64,
    pub name: String,
    pub username: Option<String>,
    pub is_channel: bool,
}

pub async fn list_channels(client: &Client) -> anyhow::Result<Vec<ChannelInfo>> {
    let mut channels = Vec::new();
    let mut dialogs = client.iter_dialogs();
    
    while let Some(dialog) = dialogs.next().await? {
        let peer = dialog.peer();
        match peer {
            Peer::Channel(ch) => {
                let id = ch.id().bare_id();
                let name = ch.title().to_string();
                let username = ch.username().map(String::from);
                tracing::debug!("Channel '{}': id={}, username={:?}", name, id, username);
                channels.push(ChannelInfo {
                    id,
                    name,
                    username,
                    is_channel: true,
                });
            }
            Peer::Group(g) => {
                let id = g.id().bare_id();
                let name = g.title().unwrap_or("Unknown Group").to_string();
                let username = g.username().map(String::from);
                tracing::debug!("Group '{}': id={}, username={:?}", name, id, username);
                channels.push(ChannelInfo {
                    id,
                    name,
                    username,
                    is_channel: false,
                });
            }
            Peer::User(_) => {}
        }
    }
    
    channels.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(channels)
}

pub fn select_from_list(channels: &[ChannelInfo], prompt: &str) -> anyhow::Result<usize> {
    if channels.is_empty() {
        return Err(anyhow::anyhow!("没有可用的频道或群组"));
    }
    
    let choices: Vec<String> = channels
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let type_tag = if c.is_channel { "频道" } else { "群组" };
            let username = c.username.as_deref().unwrap_or("无用户名");
            format!("[{}] {} ({}) @{}", i + 1, c.name, type_tag, username)
        })
        .collect();
    
    let selection = dialoguer::Select::new()
        .with_prompt(prompt)
        .items(&choices)
        .default(0)
        .interact()?;
    
    Ok(selection)
}

pub struct FilterSettings {
    pub mode: String,
    pub min_reactions: i32,
    pub min_comments: i32,
}

pub fn ask_filter_settings() -> anyhow::Result<FilterSettings> {
    println!("\n=== 设置过滤条件 ===");
    println!("过滤模式决定如何组合 reactions 和 comments 条件:");
    println!("  any (OR): 满足任一条件即转发");
    println!("  all (AND): 必须同时满足所有条件");
    
    let mode_options = vec!["any (满足任一条件)", "all (同时满足所有条件)"];
    let mode_idx = dialoguer::Select::new()
        .with_prompt("选择过滤模式")
        .items(&mode_options)
        .default(0)
        .interact()?;
    let mode = if mode_idx == 0 { "any" } else { "all" }.to_string();
    
    let min_reactions: i32 = dialoguer::Input::<String>::new()
        .with_prompt("最低 reaction 数 (0 = 不过滤)")
        .default("0".to_string())
        .validate_with(|input: &String| -> Result<(), String> {
            input.parse::<i32>()
                .map(|_| ())
                .map_err(|_| "请输入有效数字".to_string())
        })
        .interact_text()?
        .parse()
        .unwrap_or(0);
    
    let min_comments: i32 = dialoguer::Input::<String>::new()
        .with_prompt("最低评论数 (0 = 不过滤)")
        .default("0".to_string())
        .validate_with(|input: &String| -> Result<(), String> {
            input.parse::<i32>()
                .map(|_| ())
                .map_err(|_| "请输入有效数字".to_string())
        })
        .interact_text()?
        .parse()
        .unwrap_or(0);
    
    Ok(FilterSettings {
        mode,
        min_reactions,
        min_comments,
    })
}

pub fn create_channel_config(
    source: &str,
    target: &str,
    filter: &FilterSettings,
) -> anyhow::Result<()> {
    let channels_dir = Path::new("channels");
    if !channels_dir.exists() {
        fs::create_dir_all(channels_dir)?;
    }
    
    let filename = format!("{}.toml", source);
    let filepath = channels_dir.join(&filename);
    
    if filepath.exists() {
        println!("频道配置 {} 已存在，跳过创建", filename);
        return Ok(());
    }
    
    let content = format!(
        r#"source = "{}"
target = "{}"
limit = 0

[filter]
mode = "{}"

[filter.reactions]
specific = ["👍", "❤️"]
min_specific_total = {}

[filter.comments]
min = {}
"#,
        source, target, filter.mode, filter.min_reactions, filter.min_comments
    );
    
    fs::write(&filepath, content)?;
    println!("已创建频道配置: {}", filepath.display());
    Ok(())
}

pub fn list_existing_configs() -> anyhow::Result<Vec<ChannelConfig>> {
    let channels_dir = Path::new("channels");
    crate::config::load_channel_configs(channels_dir)
}

pub fn update_channel_config(
    source: &str,
    target: &str,
    filter: &FilterSettings,
) -> anyhow::Result<()> {
    let channels_dir = Path::new("channels");
    let filename = format!("{}.toml", source);
    let filepath = channels_dir.join(&filename);
    
    if !filepath.exists() {
        return Err(anyhow::anyhow!("配置文件 {} 不存在", filename));
    }
    
    let content = format!(
        r#"source = "{}"
target = "{}"
limit = 0

[filter]
mode = "{}"

[filter.reactions]
specific = ["👍", "❤️"]
min_specific_total = {}

[filter.comments]
min = {}
"#,
        source, target, filter.mode, filter.min_reactions, filter.min_comments
    );
    
    fs::write(&filepath, content)?;
    println!("已更新频道配置: {}", filepath.display());
    Ok(())
}

pub fn delete_channel_config(source: &str) -> anyhow::Result<()> {
    let channels_dir = Path::new("channels");
    let filename = format!("{}.toml", source);
    let filepath = channels_dir.join(&filename);
    
    if filepath.exists() {
        fs::remove_file(&filepath)?;
        println!("已删除频道配置: {}", filepath.display());
    } else {
        println!("配置文件 {} 不存在", filename);
    }
    Ok(())
}

pub async fn get_channel_display_name(client: &Client, identifier: &str) -> String {
    use grammers_client::peer::Peer;
    use grammers_session::types::PeerRef;
    
    if let Ok(id) = identifier.parse::<i64>() {
        let peer_ref = PeerRef {
            id: grammers_session::types::PeerId::channel(id),
            auth: grammers_session::types::PeerAuth::default(),
        };
        if let Ok(peer) = client.resolve_peer(peer_ref).await {
            match peer {
                Peer::Channel(ch) => return ch.title().to_string(),
                Peer::Group(g) => return g.title().unwrap_or(identifier).to_string(),
                Peer::User(_) => {}
            }
        }
    } else {
        if let Ok(Some(peer)) = client.resolve_username(identifier).await {
            match peer {
                Peer::Channel(ch) => return ch.title().to_string(),
                Peer::Group(g) => return g.title().unwrap_or(identifier).to_string(),
                Peer::User(_) => {}
            }
        }
    }
    
    identifier.to_string()
}
