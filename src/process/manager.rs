use crate::alerts::{AlertEvent, AlertManager};
use crate::model::{
    ExecMode, LogEntry, ProcessConfig, ProcessItem, ProcessStatus, TargetSelector,
};
use crate::process::worker::Worker;
use crate::ring_buffer::LogHub;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tracing::{error, info, warn};

pub struct ProcessManager {
    apps: HashMap<String, ProcessConfig>,
    workers: Vec<Worker>,
    next_id: u32,
    log_hub: LogHub,
    alert_manager: Option<AlertManager>,
}

impl ProcessManager {
    pub fn new(log_hub: LogHub) -> Self {
        Self {
            apps: HashMap::new(),
            workers: Vec::new(),
            next_id: 0,
            log_hub,
            alert_manager: None,
        }
    }

    pub fn set_alert_manager(&mut self, alert_mgr: AlertManager) {
        self.alert_manager = Some(alert_mgr);
    }

    pub async fn start_app(&mut self, mut config: ProcessConfig) -> Result<Vec<u32>> {
        let app_name = config.name.clone();

        // If app already exists, delete old instances first
        if self.apps.contains_key(&app_name) {
            info!("App '{}' already exists, stopping existing instances...", app_name);
            let _ = self.delete(&TargetSelector::Name(app_name.clone()), Duration::from_millis(3000)).await;
        }

        let instances = match config.exec_mode {
            ExecMode::Cluster => {
                if config.instances == 0 {
                    std::thread::available_parallelism()
                        .map(|p| p.get())
                        .unwrap_or(2)
                } else {
                    config.instances
                }
            }
            ExecMode::Fork => 1,
        };
        config.instances = instances;

        self.apps.insert(app_name.clone(), config.clone());

        let mut started_ids = Vec::new();

        for i in 0..instances {
            let id = self.next_id;
            self.next_id += 1;

            let mut worker = Worker::new(
                id,
                app_name.clone(),
                i,
                config.clone(),
                self.log_hub.clone(),
            );

            if let Err(e) = worker.spawn().await {
                error!("Failed to spawn worker #{} for app '{}': {}", id, app_name, e);
                worker.status = ProcessStatus::Errored;
            } else {
                started_ids.push(id);
            }

            self.workers.push(worker);
        }

        Ok(started_ids)
    }

    pub async fn stop(&mut self, target: &TargetSelector, timeout: Duration) -> Result<usize> {
        let mut affected = 0;
        for worker in self.workers.iter_mut() {
            if matches_target(worker, target) && worker.status == ProcessStatus::Online {
                worker.stop(timeout).await?;
                affected += 1;
            }
        }
        Ok(affected)
    }

    pub async fn restart(&mut self, target: &TargetSelector, timeout: Duration) -> Result<usize> {
        let mut affected = 0;
        for worker in self.workers.iter_mut() {
            if matches_target(worker, target) {
                if worker.status == ProcessStatus::Online {
                    let _ = worker.stop(timeout).await;
                }
                if let Err(e) = worker.spawn().await {
                    error!("Failed to restart worker #{}: {}", worker.id, e);
                    worker.status = ProcessStatus::Errored;
                } else {
                    worker.restarts += 1;
                    affected += 1;
                }
            }
        }
        Ok(affected)
    }

    pub async fn reload(&mut self, target: &TargetSelector, timeout: Duration) -> Result<usize> {
        // Zero-Downtime Rolling Reload
        // Target can be a cluster app
        let target_workers: Vec<(u32, String, usize, ProcessConfig)> = self
            .workers
            .iter()
            .filter(|w| matches_target(w, target))
            .map(|w| (w.id, w.app_name.clone(), w.instance_index, w.config.clone()))
            .collect();

        if target_workers.is_empty() {
            bail!("No workers found for target: {}", target);
        }

        let mut reloaded_count = 0;

        for (old_id, app_name, instance_idx, config) in target_workers {
            info!("Rolling reload: spawning new worker for app '{}' (index {})", app_name, instance_idx);

            // 1. Spawn new worker
            let new_id = self.next_id;
            self.next_id += 1;

            let mut new_worker = Worker::new(
                new_id,
                app_name.clone(),
                instance_idx,
                config,
                self.log_hub.clone(),
            );

            if let Err(e) = new_worker.spawn().await {
                error!("Failed to spawn replacement worker: {}", e);
                continue;
            }

            // 2. Allow brief readiness stabilization
            tokio::time::sleep(Duration::from_millis(300)).await;

            // 3. Stop old worker
            if let Some(pos) = self.workers.iter().position(|w| w.id == old_id) {
                let mut old_worker = self.workers.remove(pos);
                let _ = old_worker.stop(timeout).await;
            }

            self.workers.push(new_worker);
            reloaded_count += 1;
        }

        if let Some(ref alerts) = self.alert_manager {
            alerts.send(AlertEvent::ReloadSuccess {
                app: format!("{}", target),
                count: reloaded_count,
            });
        }

        Ok(reloaded_count)
    }

    pub async fn delete(&mut self, target: &TargetSelector, timeout: Duration) -> Result<usize> {
        let mut affected = 0;
        let mut remaining_workers = Vec::new();

        // Also identify apps to remove if deleting by name or all
        match target {
            TargetSelector::All => {
                self.apps.clear();
            }
            TargetSelector::Name(name) => {
                self.apps.remove(name);
            }
            TargetSelector::Id(_) => {}
        }

        for mut worker in self.workers.drain(..) {
            if matches_target(&worker, target) {
                if worker.status == ProcessStatus::Online {
                    let _ = worker.stop(timeout).await;
                }
                affected += 1;
            } else {
                remaining_workers.push(worker);
            }
        }

        self.workers = remaining_workers;
        Ok(affected)
    }

    pub fn list(&self) -> Vec<ProcessItem> {
        self.workers
            .iter()
            .map(|w| ProcessItem {
                id: w.id,
                name: w.app_name.clone(),
                instance_index: w.instance_index,
                pid: w.pid.unwrap_or(0),
                status: w.status,
                exec_mode: w.config.exec_mode,
                uptime_secs: w.uptime_secs(),
                restarts: w.restarts,
                cpu_percent: w.cpu_percent,
                memory_bytes: w.memory_bytes,
                created_at: w.created_at,
            })
            .collect()
    }

    pub fn describe(&self, target: &TargetSelector) -> Option<(ProcessItem, ProcessConfig, Vec<String>)> {
        let worker = self.workers.iter().find(|w| matches_target(w, target))?;
        let item = ProcessItem {
            id: worker.id,
            name: worker.app_name.clone(),
            instance_index: worker.instance_index,
            pid: worker.pid.unwrap_or(0),
            status: worker.status,
            exec_mode: worker.config.exec_mode,
            uptime_secs: worker.uptime_secs(),
            restarts: worker.restarts,
            cpu_percent: worker.cpu_percent,
            memory_bytes: worker.memory_bytes,
            created_at: worker.created_at,
        };
        let logs = self
            .log_hub
            .get_recent(Some(&worker.app_name), 20)
            .into_iter()
            .map(|l| format!("[{}] {}", l.timestamp.format("%H:%M:%S"), l.message))
            .collect();

        Some((item, worker.config.clone(), logs))
    }

    pub async fn check_health_and_restart(&mut self) {
        for worker in self.workers.iter_mut() {
            if worker.status == ProcessStatus::Online && !worker.is_stopping() {
                match worker.try_wait().await {
                    Ok(Some(status)) => {
                        warn!(
                            "[app: {}] Worker #{} (PID {:?}) exited unexpectedly with status: {}",
                            worker.app_name, worker.id, worker.pid, status
                        );

                        worker.pid = None;

                        if let Some(ref alerts) = self.alert_manager {
                            alerts.send(AlertEvent::ProcessCrash {
                                app: worker.app_name.clone(),
                                worker_id: worker.id,
                                exit_code: status.to_string(),
                                restarts: worker.restarts,
                            });
                        }

                        if worker.config.autorestart && worker.restarts < worker.config.max_restarts {
                            worker.restarts += 1;
                            worker.status = ProcessStatus::Restarting;

                            // Exponential backoff with jitter
                            let backoff_factor = (worker.restarts as u32).min(6);
                            let delay_ms = (100u64 * 2u64.pow(backoff_factor)).min(10000);

                            info!(
                                "[app: {}] Worker #{} backing off for {}ms before restart (attempt {}/{})",
                                worker.app_name, worker.id, delay_ms, worker.restarts, worker.config.max_restarts
                            );

                            let app_name = worker.app_name.clone();
                            let worker_id = worker.id;
                            self.log_hub.push(LogEntry {
                                timestamp: Utc::now(),
                                app_name: app_name.clone(),
                                worker_id,
                                stream: "err".to_string(),
                                message: format!("Process crashed (exit: {}). Restarting in {}ms...", status, delay_ms),
                            });

                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;

                            if let Err(e) = worker.spawn().await {
                                error!("[app: {}] Restart failed: {}", app_name, e);
                                worker.status = ProcessStatus::Errored;
                            }
                        } else {
                            worker.status = ProcessStatus::Errored;
                            error!(
                                "[app: {}] Worker #{} reached max restarts limit or autorestart is disabled",
                                worker.app_name, worker.id
                            );
                        }
                    }
                    Ok(None) => {
                        // Still running healthy
                    }
                    Err(e) => {
                        error!("[app: {}] Error polling worker #{}: {}", worker.app_name, worker.id, e);
                    }
                }
            }
        }
    }

    pub fn update_metrics(&mut self, pids_metrics: &HashMap<u32, (f32, u64)>) {
        for worker in self.workers.iter_mut() {
            if let Some(pid) = worker.pid {
                if let Some(&(cpu, mem)) = pids_metrics.get(&pid) {
                    worker.cpu_percent = cpu;
                    worker.memory_bytes = mem;
                }
            }
        }
    }

    pub fn get_workers_exceeding_memory(&self) -> Vec<u32> {
        let mut exceeding = Vec::new();
        for worker in &self.workers {
            if worker.status == ProcessStatus::Online {
                if let Some(limit) = worker.config.max_memory_restart {
                    if worker.memory_bytes > limit {
                        exceeding.push(worker.id);
                    }
                }
            }
        }
        exceeding
    }

    pub fn get_active_configs(&self) -> Vec<ProcessConfig> {
        self.apps.values().cloned().collect()
    }

    pub fn save_state(&self, path: &Path) -> Result<()> {
        let configs: Vec<ProcessConfig> = self.apps.values().cloned().collect();
        let json = serde_json::to_string_pretty(&configs)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub async fn resurrect_state(&mut self, path: &Path) -> Result<usize> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read state dump file: {}", path.display()))?;
        let configs: Vec<ProcessConfig> = serde_json::from_str(&content)?;
        let count = configs.len();
        for config in configs {
            let _ = self.start_app(config).await;
        }
        Ok(count)
    }
}

fn matches_target(worker: &Worker, target: &TargetSelector) -> bool {
    match target {
        TargetSelector::All => true,
        TargetSelector::Id(id) => worker.id == *id,
        TargetSelector::Name(name) => worker.app_name == *name,
    }
}
