use crate::detector::detect_runtime;
use crate::model::{ExecMode, LogEntry, ProcessConfig, ProcessStatus};
use crate::ring_buffer::LogHub;
use anyhow::{Context, Result};
use chrono::Utc;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

pub struct Worker {
    pub id: u32,
    pub app_name: String,
    pub instance_index: usize,
    pub config: ProcessConfig,
    pub status: ProcessStatus,
    pub pid: Option<u32>,
    pub created_at: chrono::DateTime<Utc>,
    pub start_time: Option<chrono::DateTime<Utc>>,
    pub restarts: u32,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    child: Arc<Mutex<Option<Child>>>,
    stopping: Arc<AtomicBool>,
    log_hub: LogHub,
}

impl Worker {
    pub fn new(
        id: u32,
        app_name: String,
        instance_index: usize,
        config: ProcessConfig,
        log_hub: LogHub,
    ) -> Self {
        Self {
            id,
            app_name,
            instance_index,
            config,
            status: ProcessStatus::Stopped,
            pid: None,
            created_at: Utc::now(),
            start_time: None,
            restarts: 0,
            cpu_percent: 0.0,
            memory_bytes: 0,
            child: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
            log_hub,
        }
    }

    pub async fn spawn(&mut self) -> Result<()> {
        self.stopping.store(false, Ordering::SeqCst);

        let detected = if let Some(ref interp) = self.config.interpreter {
            crate::detector::DetectedExecution {
                interpreter: Some(interp.clone()),
                script: self.config.script.clone(),
                args: self.config.args.clone(),
            }
        } else {
            detect_runtime(&self.config.script, &self.config.args)
        };

        let mut cmd = if let Some(ref interp) = detected.interpreter {
            let mut c = Command::new(interp);
            c.arg(&detected.script);
            c.args(&detected.args);
            c
        } else {
            let mut c = Command::new(&detected.script);
            c.args(&detected.args);
            c
        };

        if let Some(ref cwd) = self.config.cwd {
            cmd.current_dir(cwd);
        }

        // Standard Gravhive environment variables
        cmd.env("GHIVE", "1");
        cmd.env("GHIVE_APP_NAME", &self.app_name);
        cmd.env("GHIVE_WORKER_ID", self.id.to_string());
        cmd.env("GHIVE_INSTANCE_INDEX", self.instance_index.to_string());

        if self.config.exec_mode == ExecMode::Cluster {
            cmd.env("GHIVE_CLUSTER", "1");
        }

        // User custom env variables
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        if let Some(tp) = self.config.target_port {
            cmd.env("PORT", tp.to_string());
        }

        #[cfg(unix)]
        {
            use nix::unistd::{Group, User};
            if let Some(ref grp) = self.config.group {
                if let Ok(Some(g)) = Group::from_name(grp) {
                    cmd.gid(g.gid.as_raw());
                }
            }
            if let Some(ref usr) = self.config.user {
                if let Ok(Some(u)) = User::from_name(usr) {
                    cmd.uid(u.uid.as_raw());
                }
            }
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn process for app '{}'", self.app_name))?;

        let pid = child.id().unwrap_or(0);
        self.pid = Some(pid);
        self.status = ProcessStatus::Online;
        self.start_time = Some(Utc::now());

        // Apply cgroups v2 if configured
        if self.config.cgroup_memory_max.is_some()
            || self.config.cgroup_cpu_max.is_some()
            || self.config.cgroup_pids_max.is_some()
        {
            let cgroup_cfg = crate::isolation::CgroupConfig {
                memory_max: self.config.cgroup_memory_max,
                cpu_max: self.config.cgroup_cpu_max.clone(),
                pids_max: self.config.cgroup_pids_max,
            };
            let _ = crate::isolation::CgroupManager::apply(&self.app_name, pid, &cgroup_cfg);
        }

        info!(
            "[app: {}] Worker #{} spawned with PID {}",
            self.app_name, self.id, pid
        );

        // Setup async log pipes
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let app_name = self.app_name.clone();
        let worker_id = self.id;
        let log_hub = self.log_hub.clone();
        let out_log_file = self.config.out_log.clone().unwrap_or_else(|| {
            crate::common::logs_dir().join(format!("{}-{}-out.log", app_name, worker_id))
        });
        let err_log_file = self.config.err_log.clone().unwrap_or_else(|| {
            crate::common::logs_dir().join(format!("{}-{}-err.log", app_name, worker_id))
        });

        let max_rotate_size = self.config.log_rotate_max_size;
        let rotate_keep = self.config.log_rotate_keep.unwrap_or(5);
        let compress = self.config.log_compress;

        // Spawn stdout reader
        if let Some(pipe) = stdout {
            let log_hub_c = log_hub.clone();
            let app_name_c = app_name.clone();
            let out_file_c = out_log_file.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let entry = LogEntry {
                        timestamp: Utc::now(),
                        app_name: app_name_c.clone(),
                        worker_id,
                        stream: "out".to_string(),
                        message: line.clone(),
                    };
                    log_hub_c.push(entry);

                    // Automatic log rotation check
                    if let Some(max_bytes) = max_rotate_size {
                        let _ = crate::log_rotate::check_and_rotate_file(
                            &out_file_c,
                            max_bytes,
                            rotate_keep,
                            compress,
                        );
                    }

                    // Write to file
                    if let Ok(mut f) = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&out_file_c)
                    {
                        let _ = writeln!(f, "[{}] {}", Utc::now().to_rfc3339(), line);
                    }
                }
            });
        }

        // Spawn stderr reader
        if let Some(pipe) = stderr {
            let log_hub_c = log_hub;
            let app_name_c = app_name;
            let err_file_c = err_log_file.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let entry = LogEntry {
                        timestamp: Utc::now(),
                        app_name: app_name_c.clone(),
                        worker_id,
                        stream: "err".to_string(),
                        message: line.clone(),
                    };
                    log_hub_c.push(entry);

                    // Automatic log rotation check
                    if let Some(max_bytes) = max_rotate_size {
                        let _ = crate::log_rotate::check_and_rotate_file(
                            &err_file_c,
                            max_bytes,
                            rotate_keep,
                            compress,
                        );
                    }

                    // Write to file
                    if let Ok(mut f) = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&err_file_c)
                    {
                        let _ = writeln!(f, "[{}] {}", Utc::now().to_rfc3339(), line);
                    }
                }
            });
        }

        *self.child.lock().await = Some(child);
        Ok(())
    }

    pub async fn stop(&mut self, graceful_timeout: Duration) -> Result<()> {
        self.stopping.store(true, Ordering::SeqCst);
        self.status = ProcessStatus::Stopping;

        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            if let Some(pid) = child.id() {
                debug!(
                    "[app: {}] Stopping worker #{} (PID {})",
                    self.app_name, self.id, pid
                );

                #[cfg(unix)]
                {
                    let nix_pid = Pid::from_raw(pid as i32);
                    // Send SIGTERM
                    let _ = kill(nix_pid, Signal::SIGTERM);
                }

                // Wait up to graceful timeout
                let wait_result = tokio::time::timeout(graceful_timeout, child.wait()).await;

                if wait_result.is_err() {
                    warn!(
                        "[app: {}] Worker #{} (PID {}) did not stop in time, sending SIGKILL",
                        self.app_name, self.id, pid
                    );
                    #[cfg(unix)]
                    {
                        let nix_pid = Pid::from_raw(pid as i32);
                        let _ = kill(nix_pid, Signal::SIGKILL);
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = child.kill().await;
                    }
                    let _ = child.wait().await;
                }
            }
        }

        self.pid = None;
        self.status = ProcessStatus::Stopped;
        self.cpu_percent = 0.0;
        self.memory_bytes = 0;
        Ok(())
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    pub fn get_log_paths(&self) -> (PathBuf, PathBuf) {
        let out_log_file = self.config.out_log.clone().unwrap_or_else(|| {
            crate::common::logs_dir().join(format!("{}-{}-out.log", self.app_name, self.id))
        });
        let err_log_file = self.config.err_log.clone().unwrap_or_else(|| {
            crate::common::logs_dir().join(format!("{}-{}-err.log", self.app_name, self.id))
        });
        (out_log_file, err_log_file)
    }

    pub fn rotate_logs_now(&self) -> (bool, bool) {
        let (out_file, err_file) = self.get_log_paths();
        let keep = self.config.log_rotate_keep.unwrap_or(5);
        let compress = self.config.log_compress;
        let r1 = crate::log_rotate::rotate_file_now(&out_file, keep, compress).unwrap_or(false);
        let r2 = crate::log_rotate::rotate_file_now(&err_file, keep, compress).unwrap_or(false);
        (r1, r2)
    }

    pub async fn try_wait(&self) -> Result<Option<std::process::ExitStatus>> {
        let mut child_guard = self.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            match child.try_wait() {
                Ok(status) => Ok(status),
                Err(e) => Err(anyhow::anyhow!("Error checking process status: {}", e)),
            }
        } else {
            Ok(None)
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        if self.status != ProcessStatus::Online {
            return 0;
        }
        if let Some(start) = self.start_time {
            let now = Utc::now();
            let duration = now.signed_duration_since(start);
            duration.num_seconds().max(0) as u64
        } else {
            0
        }
    }
}
