use crate::model::LogEntry;
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct LogHub {
    buffer: Arc<RwLock<VecDeque<LogEntry>>>,
    max_size: usize,
    tx: broadcast::Sender<LogEntry>,
}

impl LogHub {
    pub fn new(max_size: usize) -> Self {
        let (tx, _rx) = broadcast::channel(1000);
        Self {
            buffer: Arc::new(RwLock::new(VecDeque::with_capacity(max_size))),
            max_size,
            tx,
        }
    }

    pub fn push(&self, entry: LogEntry) {
        {
            let mut buf = self.buffer.write().unwrap();
            if buf.len() >= self.max_size {
                buf.pop_front();
            }
            buf.push_back(entry.clone());
        }
        // Broadcast to any active subscribers (ignore if no receivers)
        let _ = self.tx.send(entry);
    }

    pub fn get_recent(&self, app_name: Option<&str>, limit: usize) -> Vec<LogEntry> {
        let buf = self.buffer.read().unwrap();
        let iter = buf.iter().filter(|e| {
            if let Some(app) = app_name {
                if app == "cron" {
                    e.app_name.starts_with("cron:")
                } else if app.starts_with("cron:") {
                    e.app_name == app
                } else {
                    e.app_name == app || e.app_name == format!("cron:{}", app)
                }
            } else {
                true
            }
        });
        let total = iter.clone().count();
        let skip = total.saturating_sub(limit);
        iter.skip(skip).cloned().collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }

    pub fn clear(&self) {
        let mut buf = self.buffer.write().unwrap();
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_ring_buffer_cron_filtering() {
        let hub = LogHub::new(50);
        hub.push(LogEntry {
            timestamp: Utc::now(),
            app_name: "web-api".to_string(),
            worker_id: 0,
            stream: "out".to_string(),
            message: "Listening on :3000".to_string(),
        });
        hub.push(LogEntry {
            timestamp: Utc::now(),
            app_name: "cron:backup".to_string(),
            worker_id: 0,
            stream: "out".to_string(),
            message: "Backup DB created".to_string(),
        });
        hub.push(LogEntry {
            timestamp: Utc::now(),
            app_name: "cron:cleanup".to_string(),
            worker_id: 0,
            stream: "err".to_string(),
            message: "Cleanup failed: disk full".to_string(),
        });

        // 1. All cron logs via "cron"
        let cron_logs = hub.get_recent(Some("cron"), 10);
        assert_eq!(cron_logs.len(), 2);

        // 2. Exact cron job via "cron:backup"
        let backup_logs = hub.get_recent(Some("cron:backup"), 10);
        assert_eq!(backup_logs.len(), 1);
        assert_eq!(backup_logs[0].message, "Backup DB created");

        // 3. Exact cron job via bare name "backup"
        let bare_backup = hub.get_recent(Some("backup"), 10);
        assert_eq!(bare_backup.len(), 1);
        assert_eq!(bare_backup[0].message, "Backup DB created");

        // 4. Non-cron app
        let web_logs = hub.get_recent(Some("web-api"), 10);
        assert_eq!(web_logs.len(), 1);

        // 5. All logs
        let all_logs = hub.get_recent(None, 10);
        assert_eq!(all_logs.len(), 3);
    }
}
