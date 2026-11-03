use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecMode {
    Fork,
    Cluster,
}

impl fmt::Display for ExecMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecMode::Fork => write!(f, "fork"),
            ExecMode::Cluster => write!(f, "cluster"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    Online,
    Stopping,
    Stopped,
    Errored,
    Restarting,
}

impl fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessStatus::Online => write!(f, "online"),
            ProcessStatus::Stopping => write!(f, "stopping"),
            ProcessStatus::Stopped => write!(f, "stopped"),
            ProcessStatus::Errored => write!(f, "errored"),
            ProcessStatus::Restarting => write!(f, "restarting"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TargetSelector {
    Id(u32),
    Name(String),
    All,
}

impl fmt::Display for TargetSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetSelector::Id(id) => write!(f, "ID:{}", id),
            TargetSelector::Name(name) => write!(f, "Name:{}", name),
            TargetSelector::All => write!(f, "all"),
        }
    }
}

impl TargetSelector {
    pub fn parse(input: &str) -> Self {
        if input.eq_ignore_ascii_case("all") {
            TargetSelector::All
        } else if let Ok(id) = input.parse::<u32>() {
            TargetSelector::Id(id)
        } else {
            TargetSelector::Name(input.to_string())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessConfig {
    pub name: String,
    pub script: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default = "default_exec_mode")]
    pub exec_mode: ExecMode,
    #[serde(default = "default_instances")]
    pub instances: usize,
    #[serde(default = "default_true")]
    pub autorestart: bool,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
    #[serde(default = "default_restart_delay_ms")]
    pub restart_delay_ms: u64,
    #[serde(default)]
    pub max_memory_restart: Option<u64>,
    #[serde(default)]
    pub watch: bool,
    #[serde(default)]
    pub watch_paths: Vec<PathBuf>,
    #[serde(default)]
    pub watch_ignore: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub interpreter: Option<String>,
    #[serde(default)]
    pub out_log: Option<PathBuf>,
    #[serde(default)]
    pub err_log: Option<PathBuf>,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub cron_timezone: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub cgroup_memory_max: Option<u64>,
    #[serde(default)]
    pub cgroup_cpu_max: Option<String>,
    #[serde(default)]
    pub cgroup_pids_max: Option<u32>,
    #[serde(default)]
    pub listen_port: Option<u16>,
    #[serde(default)]
    pub target_port: Option<u16>,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub log_rotate_max_size: Option<u64>,
    #[serde(default)]
    pub log_rotate_keep: Option<u32>,
    #[serde(default = "default_true")]
    pub log_compress: bool,
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthCheckConfig {
    pub http_path: Option<String>,
    pub tcp_port: Option<u16>,
    #[serde(default = "default_hc_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_hc_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_hc_retries")]
    pub max_retries: u32,
}

fn default_hc_interval() -> u64 {
    10
}
fn default_hc_timeout() -> u64 {
    3
}
fn default_hc_retries() -> u32 {
    3
}

fn default_exec_mode() -> ExecMode {
    ExecMode::Fork
}
fn default_instances() -> usize {
    1
}
fn default_true() -> bool {
    true
}
fn default_max_restarts() -> u32 {
    15
}
fn default_restart_delay_ms() -> u64 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessItem {
    pub id: u32,
    pub name: String,
    pub instance_index: usize,
    pub pid: u32,
    pub status: ProcessStatus,
    pub exec_mode: ExecMode,
    pub uptime_secs: u64,
    pub restarts: u32,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStreamType {
    Stdout,
    Stderr,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub app_name: String,
    pub worker_id: u32,
    pub stream: String,
    pub message: String,
}

pub fn parse_memory_string(val: &str) -> Option<u64> {
    let s = val.trim();
    if s.is_empty() {
        return None;
    }
    let s_upper = s.to_uppercase();
    if let Some(num_str) = s_upper.strip_suffix("GB") {
        num_str
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| (n * 1024.0 * 1024.0 * 1024.0) as u64)
    } else if let Some(num_str) = s_upper.strip_suffix("G") {
        num_str
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| (n * 1024.0 * 1024.0 * 1024.0) as u64)
    } else if let Some(num_str) = s_upper.strip_suffix("MB") {
        num_str
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| (n * 1024.0 * 1024.0) as u64)
    } else if let Some(num_str) = s_upper.strip_suffix("M") {
        num_str
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| (n * 1024.0 * 1024.0) as u64)
    } else if let Some(num_str) = s_upper.strip_suffix("KB") {
        num_str
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| (n * 1024.0) as u64)
    } else if let Some(num_str) = s_upper.strip_suffix("K") {
        num_str
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| (n * 1024.0) as u64)
    } else if let Some(num_str) = s_upper.strip_suffix("B") {
        num_str.trim().parse::<u64>().ok()
    } else {
        s.parse::<u64>().ok()
    }
}

pub fn format_memory_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let kb = bytes as f64 / 1024.0;
    let mb = kb / 1024.0;
    let gb = mb / 1024.0;

    if gb >= 1.0 {
        format!("{:.1} GB", gb)
    } else if mb >= 1.0 {
        format!("{:.1} MB", mb)
    } else if kb >= 1.0 {
        format!("{:.1} KB", kb)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_duration(seconds: u64) -> String {
    if seconds == 0 {
        return "0s".to_string();
    }
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if days > 0 {
        format!("{}D {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}
