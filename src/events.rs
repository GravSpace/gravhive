use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Spawned,
    Exited,
    Crashed,
    Restarted,
    Reloaded,
    Scaled,
    MemoryLimit,
    HealthcheckFailed,
    RotatedLogs,
    Suspended,
    Resumed,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventType::Spawned => write!(f, "spawned"),
            EventType::Exited => write!(f, "exited"),
            EventType::Crashed => write!(f, "crashed"),
            EventType::Restarted => write!(f, "restarted"),
            EventType::Reloaded => write!(f, "reloaded"),
            EventType::Scaled => write!(f, "scaled"),
            EventType::MemoryLimit => write!(f, "memory_limit"),
            EventType::HealthcheckFailed => write!(f, "healthcheck_failed"),
            EventType::RotatedLogs => write!(f, "rotated_logs"),
            EventType::Suspended => write!(f, "suspended"),
            EventType::Resumed => write!(f, "resumed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub timestamp: DateTime<Utc>,
    pub app_name: String,
    pub worker_id: Option<u32>,
    pub event_type: EventType,
    pub details: String,
}

#[derive(Clone)]
pub struct EventJournal {
    buffer: Arc<RwLock<VecDeque<LifecycleEvent>>>,
    max_size: usize,
}

impl EventJournal {
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: Arc::new(RwLock::new(VecDeque::with_capacity(max_size))),
            max_size,
        }
    }

    pub fn record(&self, event: LifecycleEvent) {
        let mut buf = self.buffer.write().unwrap();
        if buf.len() >= self.max_size {
            buf.pop_front();
        }
        buf.push_back(event);
    }

    pub fn record_event(
        &self,
        app_name: &str,
        worker_id: Option<u32>,
        event_type: EventType,
        details: &str,
    ) {
        self.record(LifecycleEvent {
            timestamp: Utc::now(),
            app_name: app_name.to_string(),
            worker_id,
            event_type,
            details: details.to_string(),
        });
    }

    pub fn get_events(&self, target_app: Option<&str>, limit: usize) -> Vec<LifecycleEvent> {
        let buf = self.buffer.read().unwrap();
        let iter = buf.iter().filter(|e| {
            if let Some(app) = target_app {
                e.app_name == app
            } else {
                true
            }
        });

        let total = iter.clone().count();
        let skip = total.saturating_sub(limit);
        iter.skip(skip).cloned().collect()
    }

    pub fn clear(&self) {
        let mut buf = self.buffer.write().unwrap();
        buf.clear();
    }
}

impl Default for EventJournal {
    fn default() -> Self {
        Self::new(500)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_get_events() {
        let journal = EventJournal::new(10);
        journal.record_event("api", Some(0), EventType::Spawned, "PID 1234");
        journal.record_event("worker", Some(1), EventType::Crashed, "Exit code 1");
        journal.record_event("api", Some(0), EventType::Restarted, "Backoff 200ms");

        let all = journal.get_events(None, 10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].event_type, EventType::Spawned);
        assert_eq!(all[1].event_type, EventType::Crashed);
        assert_eq!(all[2].event_type, EventType::Restarted);

        let api_only = journal.get_events(Some("api"), 10);
        assert_eq!(api_only.len(), 2);
        assert_eq!(api_only[0].details, "PID 1234");
        assert_eq!(api_only[1].details, "Backoff 200ms");
    }

    #[test]
    fn test_journal_max_capacity() {
        let journal = EventJournal::new(3);
        journal.record_event("app", Some(0), EventType::Spawned, "1");
        journal.record_event("app", Some(0), EventType::Spawned, "2");
        journal.record_event("app", Some(0), EventType::Spawned, "3");
        journal.record_event("app", Some(0), EventType::Spawned, "4");

        let events = journal.get_events(None, 10);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].details, "2");
        assert_eq!(events[2].details, "4");
    }
}
