use crate::detector::detect_runtime;
use crate::model::LogEntry;
use crate::ring_buffer::LogHub;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

pub const DEFAULT_CRON_TIMEOUT: Duration = Duration::from_secs(1800); // 30 minutes
pub const MAX_CRON_LOG_CAPTURE_BYTES: usize = 65536; // 64 KB limit to prevent memory bloat

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobInfo {
    pub name: String,
    pub expression: String,
    pub command: String,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub runs_count: u64,
    pub last_status: String,
    #[serde(default)]
    pub paused: bool,
}

pub struct CronJob {
    pub name: String,
    pub expression: String,
    pub schedule: Schedule,
    pub command: String,
    pub args: Vec<String>,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub runs_count: u64,
    pub last_status: String,
    pub paused: bool,
}

#[derive(Clone)]
pub struct CronManager {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    log_hub: LogHub,
}

impl CronManager {
    pub fn new(log_hub: LogHub) -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            log_hub,
        }
    }

    pub async fn add_job(
        &self,
        name: &str,
        expression: &str,
        command: &str,
        args: &[String],
    ) -> Result<()> {
        let normalized = if expression.split_whitespace().count() == 5 {
            format!("0 {}", expression)
        } else {
            expression.to_string()
        };

        let schedule = Schedule::from_str(&normalized)
            .with_context(|| format!("Invalid cron expression '{}'", expression))?;

        let next_run = schedule.upcoming(Utc).next();

        let job = CronJob {
            name: name.to_string(),
            expression: expression.to_string(),
            schedule,
            command: command.to_string(),
            args: args.to_vec(),
            last_run: None,
            next_run,
            runs_count: 0,
            last_status: "Scheduled".to_string(),
            paused: false,
        };

        let mut lock = self.jobs.write().await;
        lock.insert(name.to_string(), job);
        info!(
            "[CRON] Registered job '{}' with schedule '{}' (Next run: {:?})",
            name, expression, next_run
        );
        Ok(())
    }

    pub async fn stop_job(&self, name: &str) -> Result<usize> {
        let mut lock = self.jobs.write().await;
        if name == "all" {
            let mut count = 0;
            for job in lock.values_mut() {
                if !job.paused {
                    job.paused = true;
                    job.last_status = "Stopped".to_string();
                    count += 1;
                }
            }
            info!("[CRON] Stopped all ({}) cron jobs", count);
            Ok(count)
        } else if let Some(job) = lock.get_mut(name) {
            job.paused = true;
            job.last_status = "Stopped".to_string();
            info!("[CRON] Stopped cron job '{}'", name);
            Ok(1)
        } else {
            anyhow::bail!("Cron job '{}' not found", name);
        }
    }

    pub async fn restart_job(&self, name: &str) -> Result<usize> {
        let mut lock = self.jobs.write().await;
        if name == "all" {
            let mut count = 0;
            for job in lock.values_mut() {
                job.paused = false;
                job.last_status = "Scheduled".to_string();
                job.next_run = job.schedule.upcoming(Utc).next();
                count += 1;
            }
            info!("[CRON] Resumed all ({}) cron jobs", count);
            Ok(count)
        } else if let Some(job) = lock.get_mut(name) {
            job.paused = false;
            job.last_status = "Scheduled".to_string();
            job.next_run = job.schedule.upcoming(Utc).next();
            info!("[CRON] Resumed cron job '{}'", name);
            Ok(1)
        } else {
            anyhow::bail!("Cron job '{}' not found", name);
        }
    }

    pub async fn delete_job(&self, name: &str) -> Result<usize> {
        let mut lock = self.jobs.write().await;
        if name == "all" {
            let count = lock.len();
            lock.clear();
            info!("[CRON] Deleted all ({}) cron jobs", count);
            Ok(count)
        } else if lock.remove(name).is_some() {
            info!("[CRON] Deleted cron job '{}'", name);
            Ok(1)
        } else {
            anyhow::bail!("Cron job '{}' not found", name);
        }
    }

    pub async fn list_jobs(&self) -> Vec<CronJobInfo> {
        let lock = self.jobs.read().await;
        lock.values()
            .map(|j| CronJobInfo {
                name: j.name.clone(),
                expression: j.expression.clone(),
                command: j.command.clone(),
                last_run: j.last_run,
                next_run: j.next_run,
                runs_count: j.runs_count,
                last_status: j.last_status.clone(),
                paused: j.paused,
            })
            .collect()
    }

    pub fn start_loop(self) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let now = Utc::now();

                let mut to_run = Vec::new();
                {
                    let mut lock = self.jobs.write().await;
                    for job in lock.values_mut() {
                        if job.paused {
                            continue;
                        }
                        if let Some(next) = job.next_run {
                            if now >= next {
                                job.last_run = Some(now);
                                job.runs_count += 1;
                                job.last_status = "Running".to_string();
                                job.next_run = job.schedule.upcoming(Utc).next();

                                to_run.push((
                                    job.name.clone(),
                                    job.command.clone(),
                                    job.args.clone(),
                                ));
                            }
                        }
                    }
                }

                // Execute triggered jobs
                for (name, cmd_str, args) in to_run {
                    let log_hub = self.log_hub.clone();
                    let jobs_lock = self.jobs.clone();

                    tokio::spawn(async move {
                        info!("[CRON] Executing job '{}': {} {:?}", name, cmd_str, args);

                        let detected = detect_runtime(&cmd_str, &args);
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

                        // Execute with timeout and capped output buffer to prevent resource exhaustion / DoS
                        let status_str = match tokio::time::timeout(
                            DEFAULT_CRON_TIMEOUT,
                            cmd.output(),
                        )
                        .await
                        {
                            Ok(Ok(out)) => {
                                let stdout_len = out.stdout.len();
                                let stderr_len = out.stderr.len();

                                let stdout_slice = if stdout_len > MAX_CRON_LOG_CAPTURE_BYTES {
                                    &out.stdout[..MAX_CRON_LOG_CAPTURE_BYTES]
                                } else {
                                    &out.stdout[..]
                                };
                                let stderr_slice = if stderr_len > MAX_CRON_LOG_CAPTURE_BYTES {
                                    &out.stderr[..MAX_CRON_LOG_CAPTURE_BYTES]
                                } else {
                                    &out.stderr[..]
                                };

                                let mut stdout =
                                    String::from_utf8_lossy(stdout_slice).trim().to_string();
                                if stdout_len > MAX_CRON_LOG_CAPTURE_BYTES {
                                    stdout
                                        .push_str("\n... [output truncated due to 64KB log limit]");
                                }

                                let mut stderr =
                                    String::from_utf8_lossy(stderr_slice).trim().to_string();
                                if stderr_len > MAX_CRON_LOG_CAPTURE_BYTES {
                                    stderr
                                        .push_str("\n... [output truncated due to 64KB log limit]");
                                }

                                if !stdout.is_empty() {
                                    log_hub.push(LogEntry {
                                        timestamp: Utc::now(),
                                        app_name: format!("cron:{}", name),
                                        worker_id: 0,
                                        stream: "out".to_string(),
                                        message: stdout,
                                    });
                                }

                                if !stderr.is_empty() {
                                    log_hub.push(LogEntry {
                                        timestamp: Utc::now(),
                                        app_name: format!("cron:{}", name),
                                        worker_id: 0,
                                        stream: "err".to_string(),
                                        message: stderr,
                                    });
                                }

                                if out.status.success() {
                                    "Success".to_string()
                                } else {
                                    format!("Failed ({})", out.status)
                                }
                            }
                            Ok(Err(e)) => {
                                error!("[CRON] Failed to spawn job '{}': {}", name, e);
                                format!("Error: {}", e)
                            }
                            Err(_) => {
                                warn!("[CRON] Job '{}' exceeded maximum timeout of 30 minutes, terminated", name);
                                log_hub.push(LogEntry {
                                    timestamp: Utc::now(),
                                    app_name: format!("cron:{}", name),
                                    worker_id: 0,
                                    stream: "err".to_string(),
                                    message: "[CRON ERROR] Job exceeded maximum execution timeout of 30 minutes".to_string(),
                                });
                                "Timed Out (30m)".to_string()
                            }
                        };

                        let mut lock = jobs_lock.write().await;
                        if let Some(j) = lock.get_mut(&name) {
                            if !j.paused {
                                j.last_status = status_str;
                            }
                        }
                    });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cron_crud_lifecycle() {
        let hub = LogHub::new(100);
        let cron = CronManager::new(hub);

        // 1. Add job
        cron.add_job("cleanup", "0 */10 * * * *", "echo", &["done".into()])
            .await
            .expect("Failed to add job");

        let jobs = cron.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "cleanup");
        assert!(!jobs[0].paused);
        assert_eq!(jobs[0].last_status, "Scheduled");

        // 2. Stop job
        let count = cron.stop_job("cleanup").await.expect("Failed to stop job");
        assert_eq!(count, 1);
        let jobs = cron.list_jobs().await;
        assert!(jobs[0].paused);
        assert_eq!(jobs[0].last_status, "Stopped");

        // 3. Resume job
        let count = cron
            .restart_job("cleanup")
            .await
            .expect("Failed to restart job");
        assert_eq!(count, 1);
        let jobs = cron.list_jobs().await;
        assert!(!jobs[0].paused);
        assert_eq!(jobs[0].last_status, "Scheduled");

        // 4. Delete job
        let count = cron
            .delete_job("cleanup")
            .await
            .expect("Failed to delete job");
        assert_eq!(count, 1);
        let jobs = cron.list_jobs().await;
        assert_eq!(jobs.len(), 0);
    }
}
