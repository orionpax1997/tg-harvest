use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Clone)]
pub struct GlobalConfig {
    pub api_id: i32,
    pub api_hash: String,
    #[serde(default = "default_forward_delay_ms")]
    pub forward_delay_ms: u64,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_forward_delay_ms() -> u64 {
    1500
}

fn default_batch_size() -> usize {
    100
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChannelConfig {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub limit: usize,
    #[serde(default)]
    pub filter: FilterConfig,
}

impl ChannelConfig {
    #[allow(dead_code)]
    pub fn is_source_id(&self) -> bool {
        self.source.chars().all(|c| c.is_ascii_digit())
    }

    #[allow(dead_code)]
    pub fn is_target_id(&self) -> bool {
        self.target.chars().all(|c| c.is_ascii_digit())
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct FilterConfig {
    #[serde(default)]
    pub mode: FilterMode,
    #[serde(default)]
    pub reactions: Option<ReactionFilterConfig>,
    #[serde(default)]
    pub comments: Option<CommentFilterConfig>,
}

#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    #[default]
    Any,
    All,
}

impl std::fmt::Display for FilterMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterMode::Any => write!(f, "any"),
            FilterMode::All => write!(f, "all"),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ReactionFilterConfig {
    #[serde(default)]
    pub specific: Vec<String>,
    #[serde(default)]
    pub min_specific_total: i32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CommentFilterConfig {
    #[serde(default)]
    pub min: i32,
}

pub fn load_global_config(path: &Path) -> anyhow::Result<GlobalConfig> {
    let content = fs::read_to_string(path)?;
    let config: GlobalConfig = toml::from_str(&content)?;
    Ok(config)
}

pub fn load_channel_configs(dir: &Path) -> anyhow::Result<Vec<ChannelConfig>> {
    let mut configs = Vec::new();
    if !dir.exists() {
        return Ok(configs);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            let content = fs::read_to_string(&path)?;
            match toml::from_str::<ChannelConfig>(&content) {
                Ok(config) => configs.push(config),
                Err(e) => tracing::warn!("Failed to parse channel config {:?}: {}", path, e),
            }
        }
    }
    Ok(configs)
}

impl GlobalConfig {
    pub fn load_default() -> anyhow::Result<Self> {
        let config_path = PathBuf::from("config.toml");
        if config_path.exists() {
            load_global_config(&config_path)
        } else {
            Err(anyhow::anyhow!(
                "config.toml not found. Please create one with your API credentials."
            ))
        }
    }
}
