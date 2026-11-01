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
use tracing::{error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobInfo {
    pub name: String,
    pub expression: String,
    pub command: String,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub runs_count: u64,
    pub last_status: String,
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
        let schedule = Schedule::from_str(expression)
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
        };

        let mut lock = self.jobs.write().await;
        lock.insert(name.to_string(), job);
        info!(
            "[CRON] Registered job '{}' with schedule '{}' (Next run: {:?})",
            name, expression, next_run
        );
        Ok(())
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

                        let output = cmd.output().await;
                        let status_str = match output {
                            Ok(out) => {
                                let stdout = String::from_utf8_lossy(&out.stdout);
                                let stderr = String::from_utf8_lossy(&out.stderr);

                                if !stdout.trim().is_empty() {
                                    log_hub.push(LogEntry {
                                        timestamp: Utc::now(),
                                        app_name: format!("cron:{}", name),
                                        worker_id: 0,
                                        stream: "out".to_string(),
                                        message: stdout.trim().to_string(),
                                    });
                                }

                                if !stderr.trim().is_empty() {
                                    log_hub.push(LogEntry {
                                        timestamp: Utc::now(),
                                        app_name: format!("cron:{}", name),
                                        worker_id: 0,
                                        stream: "err".to_string(),
                                        message: stderr.trim().to_string(),
                                    });
                                }

                                if out.status.success() {
                                    "Success".to_string()
                                } else {
                                    format!("Failed ({})", out.status)
                                }
                            }
                            Err(e) => {
                                error!("[CRON] Failed to spawn job '{}': {}", name, e);
                                format!("Error: {}", e)
                            }
                        };

                        let mut lock = jobs_lock.write().await;
                        if let Some(j) = lock.get_mut(&name) {
                            j.last_status = status_str;
                        }
                    });
                }
            }
        });
    }
}
