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
                e.app_name == app
            } else {
                true
            }
        });
        let total = iter.clone().count();
        let skip = if total > limit { total - limit } else { 0 };
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

