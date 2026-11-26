use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_PATH: &str = "./config.yaml";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub isro: Isro,
    pub summary: Summary,
    pub telegram: Telegram,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Isro {
    pub url: String,
    /// Canonical site used to resolve links; defaults to `url`.
    pub base_url: String,
    pub poll_interval_secs: u64,
}

impl Default for Isro {
    fn default() -> Self {
        Self {
            url: "https://www.isro.gov.in".to_string(),
            base_url: String::new(),
            poll_interval_secs: 1800,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Summary {
    pub max_chars: usize,
}

impl Default for Summary {
    fn default() -> Self {
        Self { max_chars: 1024 }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Telegram {
    pub api: String,
    pub registry: PathBuf,
    pub admin_username: Option<String>,
    pub welcome: String,
    pub alerts: Alerts,
}

impl Default for Telegram {
    fn default() -> Self {
        Self {
            api: "https://api.telegram.org".to_string(),
            registry: PathBuf::from("./.state/telegram.json"),
            admin_username: None,
            welcome: "👋 You're subscribed. ISRO updates will arrive here.".to_string(),
            alerts: Alerts::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Alerts {
    pub fetch_failed: String,
    pub site_failure: String,
}

impl Default for Alerts {
    fn default() -> Self {
        Self {
            fetch_failed: "fetch failed: {error}".to_string(),
            site_failure: "site failure: {message}".to_string(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut config: Config = if path.exists() {
            let text =
                fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
            serde_norway::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?
        } else {
            Config::default()
        };
        if config.isro.base_url.is_empty() {
            config.isro.base_url = config.isro.url.clone();
        }
        Ok(config)
    }
}
