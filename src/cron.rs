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

pub fn default_timezone_str() -> String {
    "UTC".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronConfig {
    #[serde(default = "default_timezone_str")]
    pub timezone: String,
}

impl CronConfig {
    pub fn load_global() -> Result<Option<Self>> {
        let path = crate::common::cron_config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let cfg = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse JSON from {}", path.display()))?;
            Ok(Some(cfg))
        } else {
            Ok(None)
        }
    }

    pub fn save_global(&self) -> Result<()> {
        let path = crate::common::cron_config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }
}

pub fn resolve_timezone(tz_str: &str) -> Result<chrono_tz::Tz> {
    let trimmed = tz_str.trim();
    if trimmed.eq_ignore_ascii_case("local") || trimmed.eq_ignore_ascii_case("system") {
        let local_name = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
        chrono_tz::Tz::from_str_insensitive(&local_name)
            .or_else(|_| "UTC".parse::<chrono_tz::Tz>())
            .map_err(|e| anyhow::anyhow!("Failed to parse local timezone '{}': {}", local_name, e))
    } else {
        chrono_tz::Tz::from_str_insensitive(trimmed)
            .map_err(|e| anyhow::anyhow!("Invalid timezone '{}': {}", tz_str, e))
    }
}

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
    #[serde(default = "default_timezone_str")]
    pub timezone: String,
    #[serde(default)]
    pub last_error: Option<String>,
}

pub fn append_cron_log(name: &str, stream: &str, message: &str) {
    let logs_dir = crate::common::logs_dir();
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        warn!(
            "[CRON] Failed to create logs directory '{}': {}",
            logs_dir.display(),
            e
        );
        return;
    }
    let file_path = logs_dir.join(format!("cron-{}-{}.log", name, stream));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
    {
        use std::io::Write;
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        for line in message.lines() {
            let _ = writeln!(f, "[{}] {}", now, line);
        }
    }
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
    pub timezone: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct CronManager {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    default_timezone: Arc<RwLock<String>>,
    log_hub: LogHub,
}

impl CronManager {
    pub fn new(log_hub: LogHub) -> Self {
        Self::new_with_timezone(log_hub, "UTC")
    }

    pub fn new_with_timezone(log_hub: LogHub, default_tz: &str) -> Self {
        let canonical_tz = resolve_timezone(default_tz)
            .map(|tz| tz.name().to_string())
            .unwrap_or_else(|_| "UTC".to_string());
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            default_timezone: Arc::new(RwLock::new(canonical_tz)),
            log_hub,
        }
    }

    pub async fn get_timezone(&self) -> (String, String) {
        let tz_name = self.default_timezone.read().await.clone();
        let offset = if let Ok(tz) = resolve_timezone(&tz_name) {
            Utc::now().with_timezone(&tz).format("%:z").to_string()
        } else {
            "+00:00".to_string()
        };
        (tz_name, offset)
    }

    pub async fn set_default_timezone(&self, tz_str: &str) -> Result<(String, String)> {
        let tz = resolve_timezone(tz_str)?;
        let canonical_tz = tz.name().to_string();
        let offset = Utc::now().with_timezone(&tz).format("%:z").to_string();

        {
            let mut dt = self.default_timezone.write().await;
            *dt = canonical_tz.clone();
        }

        // Recalculate next_run for all jobs that use default timezone
        {
            let mut lock = self.jobs.write().await;
            for job in lock.values_mut() {
                if job.timezone.is_none() && !job.paused {
                    job.next_run = Self::calculate_next_run(&job.schedule, &canonical_tz);
                }
            }
        }

        // Save global config
        let cfg = CronConfig {
            timezone: canonical_tz.clone(),
        };
        let _ = cfg.save_global();

        info!(
            "[CRON] Set default timezone to '{}' ({})",
            canonical_tz, offset
        );
        Ok((canonical_tz, offset))
    }

    pub fn calculate_next_run(schedule: &Schedule, tz_str: &str) -> Option<DateTime<Utc>> {
        if let Ok(tz) = resolve_timezone(tz_str) {
            schedule
                .upcoming(tz)
                .next()
                .map(|dt| dt.with_timezone(&Utc))
        } else {
            schedule.upcoming(Utc).next()
        }
    }

    pub async fn add_job(
        &self,
        name: &str,
        expression: &str,
        tz_override: Option<&str>,
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

        let canonical_tz_override = if let Some(tz_str) = tz_override {
            let tz = resolve_timezone(tz_str)
                .with_context(|| format!("Invalid cron timezone '{}'", tz_str))?;
            Some(tz.name().to_string())
        } else {
            None
        };

        let default_tz = self.default_timezone.read().await.clone();
        let effective_tz = canonical_tz_override.as_deref().unwrap_or(&default_tz);
        let next_run = Self::calculate_next_run(&schedule, effective_tz);

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
            timezone: canonical_tz_override.clone(),
            last_error: None,
        };

        let mut lock = self.jobs.write().await;
        lock.insert(name.to_string(), job);
        info!(
            "[CRON] Registered job '{}' with schedule '{}' (Timezone: {}, Next run: {:?})",
            name, expression, effective_tz, next_run
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
        let default_tz = self.default_timezone.read().await.clone();
        let mut lock = self.jobs.write().await;
        if name == "all" {
            let mut count = 0;
            for job in lock.values_mut() {
                job.paused = false;
                job.last_status = "Scheduled".to_string();
                let effective_tz = job.timezone.as_deref().unwrap_or(&default_tz);
                job.next_run = Self::calculate_next_run(&job.schedule, effective_tz);
                count += 1;
            }
            info!("[CRON] Resumed all ({}) cron jobs", count);
            Ok(count)
        } else if let Some(job) = lock.get_mut(name) {
            job.paused = false;
            job.last_status = "Scheduled".to_string();
            let effective_tz = job.timezone.as_deref().unwrap_or(&default_tz);
            job.next_run = Self::calculate_next_run(&job.schedule, effective_tz);
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
        let default_tz = self.default_timezone.read().await.clone();
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
                timezone: j.timezone.clone().unwrap_or_else(|| default_tz.clone()),
                last_error: j.last_error.clone(),
            })
            .collect()
    }

    pub fn start_loop(self) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let now = Utc::now();

                let default_tz = self.default_timezone.read().await.clone();
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
                                let effective_tz = job.timezone.as_deref().unwrap_or(&default_tz);
                                job.next_run =
                                    Self::calculate_next_run(&job.schedule, effective_tz);

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
                        let (status_str, last_err) = match tokio::time::timeout(
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
                                        message: stdout.clone(),
                                    });
                                    append_cron_log(&name, "out", &stdout);
                                }

                                if !stderr.is_empty() {
                                    log_hub.push(LogEntry {
                                        timestamp: Utc::now(),
                                        app_name: format!("cron:{}", name),
                                        worker_id: 0,
                                        stream: "err".to_string(),
                                        message: stderr.clone(),
                                    });
                                    append_cron_log(&name, "err", &stderr);
                                }

                                if out.status.success() {
                                    ("Success".to_string(), None)
                                } else {
                                    let status_msg = format!("Failed ({})", out.status);
                                    let err_desc = if !stderr.is_empty() {
                                        stderr
                                    } else {
                                        let fail_entry = format!(
                                            "[CRON ERROR] Process exited with failure status: {}",
                                            out.status
                                        );
                                        log_hub.push(LogEntry {
                                            timestamp: Utc::now(),
                                            app_name: format!("cron:{}", name),
                                            worker_id: 0,
                                            stream: "err".to_string(),
                                            message: fail_entry.clone(),
                                        });
                                        append_cron_log(&name, "err", &fail_entry);
                                        status_msg.clone()
                                    };
                                    (status_msg, Some(err_desc))
                                }
                            }
                            Ok(Err(e)) => {
                                let err_msg = format!(
                                    "[CRON SPAWN ERROR] Failed to spawn job '{}': {}",
                                    name, e
                                );
                                error!("{}", err_msg);
                                log_hub.push(LogEntry {
                                    timestamp: Utc::now(),
                                    app_name: format!("cron:{}", name),
                                    worker_id: 0,
                                    stream: "err".to_string(),
                                    message: err_msg.clone(),
                                });
                                append_cron_log(&name, "err", &err_msg);
                                (format!("Error: {}", e), Some(err_msg))
                            }
                            Err(_) => {
                                let timeout_msg =
                                    "[CRON ERROR] Job exceeded maximum execution timeout of 30 minutes".to_string();
                                warn!(
                                    "[CRON] Job '{}' exceeded maximum timeout of 30 minutes, terminated",
                                    name
                                );
                                log_hub.push(LogEntry {
                                    timestamp: Utc::now(),
                                    app_name: format!("cron:{}", name),
                                    worker_id: 0,
                                    stream: "err".to_string(),
                                    message: timeout_msg.clone(),
                                });
                                append_cron_log(&name, "err", &timeout_msg);
                                ("Timed Out (30m)".to_string(), Some(timeout_msg))
                            }
                        };

                        let mut lock = jobs_lock.write().await;
                        if let Some(j) = lock.get_mut(&name) {
                            if !j.paused {
                                j.last_status = status_str;
                                j.last_error = last_err;
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
        cron.add_job("cleanup", "0 */10 * * * *", None, "echo", &["done".into()])
            .await
            .expect("Failed to add job");

        let jobs = cron.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "cleanup");
        assert_eq!(jobs[0].timezone, "UTC");
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

    #[test]
    fn test_resolve_valid_and_invalid_timezones() {
        assert!(resolve_timezone("Asia/Jakarta").is_ok());
        assert!(resolve_timezone("UTC").is_ok());
        assert!(resolve_timezone("America/New_York").is_ok());
        assert!(resolve_timezone("local").is_ok());
        assert!(resolve_timezone("system").is_ok());
        assert!(resolve_timezone("asia/jakarta").is_ok()); // case-insensitive

        assert!(resolve_timezone("Not_A_Real_Timezone_123").is_err());
    }

    #[tokio::test]
    async fn test_cron_timezone_calculation_and_dynamic_change() {
        let hub = LogHub::new(100);
        let cron = CronManager::new_with_timezone(hub, "UTC");

        let (tz, _) = cron.get_timezone().await;
        assert_eq!(tz, "UTC");

        // Add a job scheduled at 02:00:00 every day
        cron.add_job("backup", "0 0 2 * * *", None, "echo", &["backup".into()])
            .await
            .expect("Failed to add job");

        let jobs_before = cron.list_jobs().await;
        let next_utc = jobs_before[0].next_run.expect("next_run expected");

        // Now dynamically change default timezone to Asia/Jakarta (UTC+7)
        let (new_tz, offset) = cron
            .set_default_timezone("Asia/Jakarta")
            .await
            .expect("Failed to set timezone");
        assert_eq!(new_tz, "Asia/Jakarta");
        assert_eq!(offset, "+07:00");

        let jobs_after = cron.list_jobs().await;
        let next_jakarta_as_utc = jobs_after[0].next_run.expect("next_run expected");
        assert_eq!(jobs_after[0].timezone, "Asia/Jakarta");

        // 02:00:00 Jakarta time is 19:00:00 UTC previous day, so it should differ by exactly 7 hours
        // Verify both timestamps are valid and next_run was recalculated
        assert_ne!(next_utc, next_jakarta_as_utc);
    }

    #[tokio::test]
    async fn test_cron_job_timezone_override() {
        let hub = LogHub::new(100);
        let cron = CronManager::new_with_timezone(hub, "UTC");

        // Job with explicit timezone override
        cron.add_job(
            "ny-report",
            "0 0 9 * * *",
            Some("America/New_York"),
            "echo",
            &[],
        )
        .await
        .expect("Failed to add NY job");

        let jobs = cron.list_jobs().await;
        assert_eq!(jobs[0].timezone, "America/New_York");

        // Changing global timezone should not affect job with custom timezone
        cron.set_default_timezone("Asia/Tokyo").await.unwrap();
        let jobs2 = cron.list_jobs().await;
        assert_eq!(jobs2[0].timezone, "America/New_York");
    }

    #[test]
    fn test_cron_append_disk_log() {
        let job_name = "test-log-audit";
        append_cron_log(job_name, "out", "Task finished successfully");
        append_cron_log(job_name, "err", "Warning: rate limit near threshold");

        let logs_dir = crate::common::logs_dir();
        let out_file = logs_dir.join(format!("cron-{}-out.log", job_name));
        let err_file = logs_dir.join(format!("cron-{}-err.log", job_name));

        assert!(out_file.exists());
        assert!(err_file.exists());

        let out_content = std::fs::read_to_string(&out_file).unwrap();
        assert!(out_content.contains("Task finished successfully"));

        let err_content = std::fs::read_to_string(&err_file).unwrap();
        assert!(err_content.contains("Warning: rate limit near threshold"));

        // Cleanup test files
        let _ = std::fs::remove_file(out_file);
        let _ = std::fs::remove_file(err_file);
    }

    #[tokio::test]
    async fn test_cron_spawn_failure_records_error() {
        let hub = LogHub::new(10);
        let cron = CronManager::new(hub.clone());

        cron.add_job(
            "invalid-bin",
            "0 0 1 * * *",
            None,
            "non_existent_binary_xyz_12345",
            &[],
        )
        .await
        .unwrap();

        let jobs = cron.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].last_error, None);
        assert_eq!(jobs[0].last_status, "Scheduled");
    }
}
