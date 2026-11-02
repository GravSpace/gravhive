#![allow(clippy::large_enum_variant)]

use crate::alerts::AlertConfig;
use crate::cron::CronJobInfo;
use crate::events::LifecycleEvent;
use crate::model::{LogEntry, LogStreamType, ProcessConfig, ProcessItem, TargetSelector};
use crate::wasm::PluginInfo;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", content = "data", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Start(ProcessConfig),
    Stop(TargetSelector),
    Restart(TargetSelector),
    Reload {
        target: TargetSelector,
        graceful_timeout_ms: Option<u64>,
        readiness_delay_ms: Option<u64>,
    },
    Delete(TargetSelector),
    List,
    Describe(TargetSelector),
    Logs {
        target: Option<TargetSelector>,
        lines: usize,
        follow: bool,
        stream: LogStreamType,
    },
    FlushLogs(TargetSelector),
    Save,
    Resurrect,
    CronList,
    CronStop(String),
    CronRestart(String),
    CronDelete(String),
    PluginList,
    UpdateAlertConfig(AlertConfig),
    GetAlertConfig,
    TestAlert {
        message: String,
    },
    Scale {
        app: String,
        instances: String,
    },
    LogRotate(TargetSelector),
    Prune {
        dry_run: bool,
        only_logs: bool,
    },
    GetEvents {
        target: Option<TargetSelector>,
        limit: usize,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum Response {
    Ok {
        message: String,
    },
    Pong,
    ProcessList {
        processes: Vec<ProcessItem>,
    },
    ProcessDetails {
        info: ProcessItem,
        config: ProcessConfig,
        logs_preview: Vec<String>,
    },
    CronList {
        jobs: Vec<CronJobInfo>,
    },
    PluginList {
        plugins: Vec<PluginInfo>,
    },
    AlertConfig(AlertConfig),
    TestAlertResult {
        results: Vec<(String, bool, String)>,
    },
    ScaleResult {
        app: String,
        previous: usize,
        current: usize,
    },
    PruneResult {
        removed_processes: Vec<(u32, String)>,
        deleted_log_files: Vec<String>,
        freed_bytes: u64,
    },
    EventList {
        events: Vec<LifecycleEvent>,
    },
    LogLine(LogEntry),
    Error {
        message: String,
    },
}
