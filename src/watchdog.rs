use crate::model::TargetSelector;
use crate::process::ProcessManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System};
use tokio::sync::RwLock;
use tracing::warn;

pub struct Watchdog {
    manager: Arc<RwLock<ProcessManager>>,
    system: System,
    interval: Duration,
}

impl Watchdog {
    pub fn new(manager: Arc<RwLock<ProcessManager>>, interval_secs: u64) -> Self {
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        Self {
            manager,
            system,
            interval: Duration::from_secs(interval_secs),
        }
    }

    pub async fn run_loop(mut self) {
        let mut ticker = tokio::time::interval(self.interval);
        loop {
            ticker.tick().await;

            // 1. Refresh system processes
            self.system.refresh_processes(ProcessesToUpdate::All, true);

            // 2. Collect metrics for all active pids
            let mut pids_metrics = HashMap::new();
            for (pid, process) in self.system.processes() {
                let pid_u32 = pid.as_u32();
                let cpu = process.cpu_usage();
                let memory = process.memory(); // in bytes
                pids_metrics.insert(pid_u32, (cpu, memory));
            }

            // 3. Update metrics and check for crashes
            let exceeding_workers = {
                let mut mgr = self.manager.write().await;
                mgr.update_metrics(&pids_metrics);
                mgr.check_health_and_restart().await;
                mgr.get_workers_exceeding_memory()
            };

            // 4. Handle max memory reloads
            for worker_id in exceeding_workers {
                warn!(
                    "[WATCHDOG] Worker #{} exceeded max_memory_restart! Initiating graceful restart...",
                    worker_id
                );
                let mut mgr = self.manager.write().await;
                let _ = mgr
                    .restart(&TargetSelector::Id(worker_id), Duration::from_millis(5000))
                    .await;
            }
        }
    }
}
