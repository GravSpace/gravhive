use crate::alerts::{AlertEvent, AlertManager};
use crate::events::{EventJournal, EventType};
use crate::model::{ExecMode, LogEntry, ProcessConfig, ProcessItem, ProcessStatus, TargetSelector};
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
    event_journal: EventJournal,
    cluster_balancers: HashMap<String, std::sync::Arc<crate::cluster_proxy::ClusterLoadBalancer>>,
}

impl ProcessManager {
    pub fn new(log_hub: LogHub) -> Self {
        Self {
            apps: HashMap::new(),
            workers: Vec::new(),
            next_id: 0,
            log_hub,
            alert_manager: None,
            event_journal: EventJournal::default(),
            cluster_balancers: HashMap::new(),
        }
    }

    pub fn set_alert_manager(&mut self, alert_mgr: AlertManager) {
        self.alert_manager = Some(alert_mgr);
    }

    pub fn set_event_journal(&mut self, journal: EventJournal) {
        self.event_journal = journal;
    }

    pub fn event_journal(&self) -> &EventJournal {
        &self.event_journal
    }

    pub fn get_cluster_balancer(
        &self,
        app_name: &str,
    ) -> Option<std::sync::Arc<crate::cluster_proxy::ClusterLoadBalancer>> {
        self.cluster_balancers.get(app_name).cloned()
    }

    pub async fn update_cluster_proxy_upstreams(&self, app_name: &str) {
        if let Some(balancer) = self.cluster_balancers.get(app_name) {
            let current_ports: Vec<u16> = self
                .workers
                .iter()
                .filter(|w| w.app_name == app_name && w.status == ProcessStatus::Online)
                .filter_map(|w| w.config.target_port)
                .collect();
            balancer.update_upstreams(current_ports).await;
        }
    }

    pub async fn start_app(&mut self, mut config: ProcessConfig) -> Result<Vec<u32>> {
        let app_name = config.name.clone();

        // If app already exists, delete old instances first
        if self.apps.contains_key(&app_name) {
            info!(
                "App '{}' already exists, stopping existing instances...",
                app_name
            );
            let _ = self
                .delete(
                    &TargetSelector::Name(app_name.clone()),
                    Duration::from_millis(3000),
                )
                .await;
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

        let cluster_proxy_enabled = config.exec_mode == ExecMode::Cluster
            && config.listen_port.is_some()
            && config.cluster_proxy.unwrap_or(true);

        let mut upstream_ports = Vec::new();
        let base_port = config
            .internal_port_base
            .or_else(|| config.listen_port.map(|p| p + 1))
            .unwrap_or(8081);

        let mut started_ids = Vec::new();
        let mut first_spawn_error = None;

        for i in 0..instances {
            let id = self.next_id;
            self.next_id += 1;

            let mut worker_config = config.clone();
            if cluster_proxy_enabled {
                let worker_port = base_port + (i as u16);
                worker_config.target_port = Some(worker_port);
                upstream_ports.push(worker_port);
            }

            let mut worker =
                Worker::new(id, app_name.clone(), i, worker_config, self.log_hub.clone());

            if let Err(e) = worker.spawn().await {
                let err_msg = format!("{:#}", e);
                error!(
                    "Failed to spawn worker #{} for app '{}': {}",
                    id, app_name, err_msg
                );
                worker.status = ProcessStatus::Errored;
                self.log_hub.push(LogEntry {
                    timestamp: Utc::now(),
                    app_name: app_name.clone(),
                    worker_id: id,
                    stream: "err".to_string(),
                    message: format!("Spawn failed: {}", err_msg),
                });
                self.event_journal.record_event(
                    &app_name,
                    Some(id),
                    EventType::Crashed,
                    &format!("Spawn failed: {}", err_msg),
                );
                if first_spawn_error.is_none() {
                    first_spawn_error = Some(err_msg);
                }
            } else {
                started_ids.push(id);
                self.event_journal.record_event(
                    &app_name,
                    Some(id),
                    EventType::Spawned,
                    &format!("PID {:?}", worker.pid),
                );
            }

            self.workers.push(worker);
        }

        if cluster_proxy_enabled {
            if let Some(public_port) = config.listen_port {
                match crate::cluster_proxy::ClusterLoadBalancer::start(
                    app_name.clone(),
                    public_port,
                    upstream_ports,
                )
                .await
                {
                    Ok(balancer) => {
                        self.cluster_balancers
                            .insert(app_name.clone(), std::sync::Arc::new(balancer));
                    }
                    Err(e) => {
                        warn!(
                            "[CLUSTER-PROXY] Failed to start L4 load balancer for app '{}': {}",
                            app_name, e
                        );
                    }
                }
            }
        }

        if started_ids.is_empty() && instances > 0 {
            let err =
                first_spawn_error.unwrap_or_else(|| "Failed to spawn worker instance".to_string());
            bail!("{}", err);
        }

        Ok(started_ids)
    }

    pub async fn stop(&mut self, target: &TargetSelector, timeout: Duration) -> Result<usize> {
        let mut affected = 0;
        for worker in self.workers.iter_mut() {
            if matches_target(worker, target)
                && (worker.status == ProcessStatus::Online || worker.status == ProcessStatus::Idle)
            {
                worker.stop(timeout).await?;
                self.event_journal.record_event(
                    &worker.app_name,
                    Some(worker.id),
                    EventType::Exited,
                    "Manual stop request",
                );
                affected += 1;
            }
        }
        Ok(affected)
    }

    pub async fn suspend(&mut self, target: &TargetSelector) -> Result<usize> {
        let mut affected = 0;
        for worker in self.workers.iter_mut() {
            if matches_target(worker, target) && worker.status == ProcessStatus::Online {
                if let Err(e) = worker.suspend() {
                    warn!(
                        "Failed to suspend worker #{} of app '{}': {}",
                        worker.id, worker.app_name, e
                    );
                } else {
                    affected += 1;
                    self.event_journal.record_event(
                        &worker.app_name,
                        Some(worker.id),
                        EventType::Suspended,
                        "Worker paused via SIGSTOP (idle scale-to-zero)",
                    );
                }
            }
        }
        Ok(affected)
    }

    pub async fn resume(&mut self, target: &TargetSelector) -> Result<usize> {
        let mut affected = 0;
        for worker in self.workers.iter_mut() {
            if matches_target(worker, target) && worker.status == ProcessStatus::Idle {
                if let Err(e) = worker.resume() {
                    warn!(
                        "Failed to resume worker #{} of app '{}': {}",
                        worker.id, worker.app_name, e
                    );
                } else {
                    affected += 1;
                    self.event_journal.record_event(
                        &worker.app_name,
                        Some(worker.id),
                        EventType::Resumed,
                        "Worker resumed via SIGCONT (socket active)",
                    );
                }
            }
        }
        Ok(affected)
    }

    pub async fn restart(&mut self, target: &TargetSelector, timeout: Duration) -> Result<usize> {
        let mut affected = 0;
        for worker in self.workers.iter_mut() {
            if matches_target(worker, target) {
                if worker.status == ProcessStatus::Online || worker.status == ProcessStatus::Idle {
                    let _ = worker.stop(timeout).await;
                }
                if let Err(e) = worker.spawn().await {
                    error!("Failed to restart worker #{}: {}", worker.id, e);
                    worker.status = ProcessStatus::Errored;
                    self.event_journal.record_event(
                        &worker.app_name,
                        Some(worker.id),
                        EventType::Crashed,
                        &format!("Restart failed: {}", e),
                    );
                } else {
                    worker.restarts += 1;
                    self.event_journal.record_event(
                        &worker.app_name,
                        Some(worker.id),
                        EventType::Restarted,
                        &format!("PID {:?}", worker.pid),
                    );
                    affected += 1;
                }
            }
        }
        Ok(affected)
    }

    pub async fn reload(
        &mut self,
        target: &TargetSelector,
        timeout: Duration,
        stabilization_delay: Duration,
    ) -> Result<usize> {
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
            info!(
                "Rolling reload: spawning new worker for app '{}' (index {})",
                app_name, instance_idx
            );

            // 1. Spawn new replacement worker
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

            // 2. Allow brief readiness stabilization & verify process is still alive
            if !stabilization_delay.is_zero() {
                tokio::time::sleep(stabilization_delay).await;
            }

            // Health check: did the replacement worker crash immediately on boot?
            let crashed = matches!(new_worker.try_wait().await, Ok(Some(_)));

            if crashed {
                error!(
                    "Replacement worker #{} crashed immediately during reload! Aborting replacement for instance {} to preserve uptime.",
                    new_id, instance_idx
                );
                continue;
            }

            // Active readiness probe if configured
            if let Some(ref hc) = new_worker.config.health_check {
                let healthy = crate::health::probe_worker_readiness(&new_worker, hc).await;
                if !healthy {
                    error!(
                        "Replacement worker #{} failed readiness probe during reload! Aborting replacement for instance {} to preserve uptime.",
                        new_id, instance_idx
                    );
                    self.event_journal.record_event(
                        &app_name,
                        Some(new_id),
                        EventType::HealthcheckFailed,
                        "Readiness probe failed during rolling reload",
                    );
                    let _ = new_worker.stop(timeout).await;
                    continue;
                }
            }

            // 3. Gracefully stop old worker in background so reload doesn't block
            if let Some(pos) = self.workers.iter().position(|w| w.id == old_id) {
                let mut old_worker = self.workers.remove(pos);
                tokio::spawn(async move {
                    let _ = old_worker.stop(timeout).await;
                });
            }

            self.event_journal.record_event(
                &app_name,
                Some(new_id),
                EventType::Reloaded,
                &format!(
                    "Rolling reload replaced worker #{} with #{}",
                    old_id, new_id
                ),
            );

            self.workers.push(new_worker);
            self.update_cluster_proxy_upstreams(&app_name).await;
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

    pub async fn scale(&mut self, app_name: &str, target_spec: &str) -> Result<(usize, usize)> {
        let mut config = self
            .apps
            .get(app_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("App '{}' not found in registry", app_name))?;

        // Current active/known workers for this app
        let current_count = self
            .workers
            .iter()
            .filter(|w| w.app_name == app_name)
            .count();

        let target_count = if let Some(stripped) = target_spec.strip_prefix('+') {
            let delta: usize = stripped
                .parse()
                .context("Invalid scale increment (e.g. +2)")?;
            current_count + delta
        } else if let Some(stripped) = target_spec.strip_prefix('-') {
            let delta: usize = stripped
                .parse()
                .context("Invalid scale decrement (e.g. -1)")?;
            current_count.saturating_sub(delta).max(1)
        } else {
            let count: usize = target_spec
                .parse()
                .context("Invalid scale count (e.g. 4)")?;
            count.max(1)
        };

        if target_count == current_count {
            return Ok((current_count, current_count));
        }

        if target_count > current_count {
            let to_add = target_count - current_count;
            for i in 0..to_add {
                let id = self.next_id;
                self.next_id += 1;
                let mut worker = Worker::new(
                    id,
                    app_name.to_string(),
                    current_count + i,
                    config.clone(),
                    self.log_hub.clone(),
                );
                if let Err(e) = worker.spawn().await {
                    error!("Failed to spawn worker #{} during scale: {}", id, e);
                }
                self.workers.push(worker);
            }
        } else {
            let to_remove = current_count - target_count;
            let mut removed_ids = Vec::new();
            for worker in self.workers.iter().rev() {
                if worker.app_name == app_name && removed_ids.len() < to_remove {
                    removed_ids.push(worker.id);
                }
            }
            for id in &removed_ids {
                let _ = self
                    .stop(&TargetSelector::Id(*id), Duration::from_millis(3000))
                    .await;
            }
            self.workers.retain(|w| !removed_ids.contains(&w.id));
        }

        config.instances = target_count;
        self.apps.insert(app_name.to_string(), config);
        self.update_cluster_proxy_upstreams(app_name).await;

        self.event_journal.record_event(
            app_name,
            None,
            EventType::Scaled,
            &format!(
                "Scaled from {} to {} instances",
                current_count, target_count
            ),
        );

        Ok((current_count, target_count))
    }

    pub fn rotate_logs(&self, target: &TargetSelector) -> usize {
        let mut count = 0;
        for worker in &self.workers {
            if matches_target(worker, target) {
                let (r1, r2) = worker.rotate_logs_now();
                if r1 || r2 {
                    self.event_journal.record_event(
                        &worker.app_name,
                        Some(worker.id),
                        EventType::RotatedLogs,
                        "Log file rotated and compressed",
                    );
                    count += 1;
                }
            }
        }
        count
    }

    pub async fn delete(&mut self, target: &TargetSelector, timeout: Duration) -> Result<usize> {
        let mut affected = 0;
        let mut remaining_workers = Vec::new();

        // Also identify apps to remove if deleting by name or all
        match target {
            TargetSelector::All => {
                for (_, balancer) in self.cluster_balancers.drain() {
                    balancer.shutdown();
                }
                self.apps.clear();
            }
            TargetSelector::Name(name) => {
                if let Some(balancer) = self.cluster_balancers.remove(name) {
                    balancer.shutdown();
                }
                self.apps.remove(name);
            }
            TargetSelector::Id(_) => {}
        }

        for mut worker in self.workers.drain(..) {
            if matches_target(&worker, target) {
                if worker.status == ProcessStatus::Online || worker.status == ProcessStatus::Idle {
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

    pub fn describe(
        &self,
        target: &TargetSelector,
    ) -> Option<(ProcessItem, ProcessConfig, Vec<String>)> {
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
                        let is_success = status.success();
                        if is_success {
                            info!(
                                "[app: {}] Worker #{} (PID {:?}) exited cleanly with status: {}",
                                worker.app_name, worker.id, worker.pid, status
                            );
                            self.event_journal.record_event(
                                &worker.app_name,
                                Some(worker.id),
                                EventType::Exited,
                                &format!("Clean exit (status: {})", status),
                            );
                        } else {
                            warn!(
                                "[app: {}] Worker #{} (PID {:?}) exited unexpectedly with status: {}",
                                worker.app_name, worker.id, worker.pid, status
                            );

                            self.event_journal.record_event(
                                &worker.app_name,
                                Some(worker.id),
                                EventType::Crashed,
                                &format!(
                                    "Crashed (exit: {}). Attempt {}/{}",
                                    status,
                                    worker.restarts + 1,
                                    worker.config.max_restarts
                                ),
                            );

                            if let Some(ref alerts) = self.alert_manager {
                                alerts.send(AlertEvent::ProcessCrash {
                                    app: worker.app_name.clone(),
                                    worker_id: worker.id,
                                    exit_code: status.to_string(),
                                    restarts: worker.restarts,
                                });
                            }
                        }

                        worker.pid = None;

                        if worker.config.autorestart && worker.restarts < worker.config.max_restarts
                        {
                            worker.restarts += 1;
                            worker.status = ProcessStatus::Restarting;

                            // Exponential backoff with jitter
                            let backoff_factor = worker.restarts.min(6);
                            let delay_ms = (100u64 * 2u64.pow(backoff_factor)).min(10000);

                            info!(
                                "[app: {}] Worker #{} backing off for {}ms before restart (attempt {}/{})",
                                worker.app_name, worker.id, delay_ms, worker.restarts, worker.config.max_restarts
                            );

                            let app_name = worker.app_name.clone();
                            let worker_id = worker.id;
                            let msg = if is_success {
                                format!(
                                    "Process exited (exit: {}). Restarting in {}ms...",
                                    status, delay_ms
                                )
                            } else {
                                format!(
                                    "Process crashed (exit: {}). Restarting in {}ms...",
                                    status, delay_ms
                                )
                            };

                            self.log_hub.push(LogEntry {
                                timestamp: Utc::now(),
                                app_name: app_name.clone(),
                                worker_id,
                                stream: if is_success {
                                    "out".to_string()
                                } else {
                                    "err".to_string()
                                },
                                message: msg,
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
                        error!(
                            "[app: {}] Error polling worker #{}: {}",
                            worker.app_name, worker.id, e
                        );
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

    pub fn prune(
        &mut self,
        dry_run: bool,
        only_logs: bool,
    ) -> (Vec<(u32, String)>, Vec<String>, u64) {
        let mut removed_processes = Vec::new();

        if !only_logs {
            if dry_run {
                for worker in &self.workers {
                    if worker.status == ProcessStatus::Stopped
                        || worker.status == ProcessStatus::Errored
                    {
                        removed_processes.push((worker.id, worker.app_name.clone()));
                    }
                }
            } else {
                let mut remaining_workers = Vec::new();
                for worker in self.workers.drain(..) {
                    if worker.status == ProcessStatus::Stopped
                        || worker.status == ProcessStatus::Errored
                    {
                        removed_processes.push((worker.id, worker.app_name.clone()));
                    } else {
                        remaining_workers.push(worker);
                    }
                }
                self.workers = remaining_workers;
                let active_app_names: std::collections::HashSet<String> =
                    self.workers.iter().map(|w| w.app_name.clone()).collect();
                self.apps.retain(|name, _| active_app_names.contains(name));
            }
        }

        let mut deleted_log_files = Vec::new();
        let mut freed_bytes = 0u64;

        let logs_dir = crate::common::logs_dir();
        if logs_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&logs_dir) {
                let valid_prefixes: Vec<String> = self
                    .workers
                    .iter()
                    .map(|w| format!("{}-{}-", w.app_name, w.id))
                    .collect();

                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        let file_name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();

                        if file_name.ends_with(".log")
                            || file_name.contains(".log.")
                            || file_name.ends_with(".gz")
                        {
                            let is_active = valid_prefixes
                                .iter()
                                .any(|prefix| file_name.starts_with(prefix));

                            if !is_active {
                                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                freed_bytes += size;
                                deleted_log_files.push(file_name);

                                if !dry_run {
                                    let _ = std::fs::remove_file(&path);
                                }
                            }
                        }
                    }
                }
            }
        }

        (removed_processes, deleted_log_files, freed_bytes)
    }

    pub fn save_state(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let configs: Vec<ProcessConfig> = self.apps.values().cloned().collect();
        let json = serde_json::to_string_pretty(&configs)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub async fn resurrect_state(&mut self, path: &Path) -> Result<usize> {
        if !path.exists() {
            return Ok(0);
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read state dump file: {}", path.display()))?;
        if content.trim().is_empty() {
            return Ok(0);
        }
        let configs: Vec<ProcessConfig> = serde_json::from_str(&content)?;
        let mut count = 0;
        for config in configs {
            let already_running = self.workers.iter().any(|w| {
                w.app_name == config.name
                    && (w.status == ProcessStatus::Online || w.status == ProcessStatus::Idle)
            });
            if !already_running {
                if self.start_app(config).await.is_ok() {
                    count += 1;
                }
            } else {
                count += 1;
            }
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
