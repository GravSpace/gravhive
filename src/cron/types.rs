use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff_cron::Schedule;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_CRON_TIMEOUT: Duration = Duration::from_secs(1800); // 30 minutes
pub const MAX_CRON_LOG_CAPTURE_BYTES: usize = 65536; // 64 KB limit to prevent memory bloat

pub fn default_timezone_str() -> String {
    "UTC".to_string()
}

/// Optional execution environment captured at job registration time.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronExecutionContext {
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub interpreter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronJobRecord {
    pub name: String,
    pub expression: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub interpreter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronConfig {
    #[serde(default = "default_timezone_str")]
    pub timezone: String,
    #[serde(default)]
    pub jobs: Vec<CronJobRecord>,
}

impl CronConfig {
    pub fn load_global() -> Result<Option<Self>> {
        let path = crate::common::cron_config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let cfg = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse JSON from {}", path.display()))?;
            Ok(Some(cfg))
        } else {
            Ok(None)
        }
    }

    pub fn save_global(&self) -> Result<()> {
        let path = crate::common::cron_config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobInfo {
    pub name: String,
    pub expression: String,
    pub command: String,
    pub last_run: Option<Timestamp>,
    pub next_run: Option<Timestamp>,
    pub runs_count: u64,
    pub last_status: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default = "default_timezone_str")]
    pub timezone: String,
    #[serde(default)]
    pub last_error: Option<String>,
}

pub struct CronJob {
    pub name: String,
    pub expression: String,
    pub schedule: Schedule,
    pub command: String,
    pub args: Vec<String>,
    pub context: CronExecutionContext,
    pub last_run: Option<Timestamp>,
    pub next_run: Option<Timestamp>,
    pub runs_count: u64,
    pub last_status: String,
    pub paused: bool,
    pub timezone: Option<String>,
    pub last_error: Option<String>,
}
