use crate::model::{LogEntry, LogStreamType, ProcessConfig, ProcessItem, TargetSelector};
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
    LogLine(LogEntry),
    Error {
        message: String,
    },
}

