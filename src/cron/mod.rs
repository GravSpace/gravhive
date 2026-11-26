pub mod runner;
pub mod timezone;
pub mod types;

#[cfg(test)]
mod tests;

pub use timezone::{append_cron_log, format_timezone_offset, resolve_timezone};
pub use types::{
    default_timezone_str, CronConfig, CronExecutionContext, CronJob, CronJobInfo, CronJobRecord,
    DEFAULT_CRON_TIMEOUT, MAX_CRON_LOG_CAPTURE_BYTES,
};

use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff_cron::Schedule;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Clone)]
pub struct CronManager {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    default_timezone: Arc<RwLock<String>>,
    log_hub: LogHubProxy,
}

type LogHubProxy = crate::ring_buffer::LogHub;

impl CronManager {
    pub fn new(log_hub: LogHubProxy) -> Self {
        Self::new_with_timezone(log_hub, "UTC")
    }

    pub fn new_with_timezone(log_hub: LogHubProxy, default_tz: &str) -> Self {
        let canonical_tz = resolve_timezone(default_tz)
            .ok()
            .and_then(|tz| tz.iana_name().map(str::to_string))
            .unwrap_or_else(|| "UTC".to_string());
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            default_timezone: Arc::new(RwLock::new(canonical_tz)),
            log_hub,
        }
    }

    pub async fn get_timezone(&self) -> (String, String) {
        let tz_name = self.default_timezone.read().await.clone();
        let offset = if let Ok(tz) = resolve_timezone(&tz_name) {
            format_timezone_offset(&tz)
        } else {
            "+00:00".to_string()
        };
        (tz_name, offset)
    }

    pub async fn set_default_timezone(&self, tz_str: &str) -> Result<(String, String)> {
        let tz = resolve_timezone(tz_str)?;
        let canonical_tz = tz.iana_name().unwrap_or("UTC").to_string();
        let offset = format_timezone_offset(&tz);

        *self.default_timezone.write().await = canonical_tz.clone();

        {
            let mut lock = self.jobs.write().await;
            for job in lock.values_mut() {
                if job.timezone.is_none() && !job.paused {
                    job.next_run = Self::calculate_next_run(&job.schedule, &canonical_tz);
                }
            }
        }

        let _ = self.save_to_disk().await;
        info!(
            "[CRON] Set default timezone to '{}' ({})",
            canonical_tz, offset
        );
        Ok((canonical_tz, offset))
    }

    /// Save current cron configuration and all scheduled jobs to ~/.gravhive/cron.json
    pub async fn save_to_disk(&self) -> Result<()> {
        let timezone = self.default_timezone.read().await.clone();
        let jobs_lock = self.jobs.read().await;
        let job_records = jobs_lock
            .values()
            .map(|job| CronJobRecord {
                name: job.name.clone(),
                expression: job.expression.clone(),
                command: job.command.clone(),
                args: job.args.clone(),
                timezone: job.timezone.clone(),
                paused: job.paused,
                cwd: job.context.cwd.clone(),
                env: job.context.env.clone(),
                interpreter: job.context.interpreter.clone(),
            })
            .collect();
        drop(jobs_lock);

        CronConfig {
            timezone,
            jobs: job_records,
        }
        .save_global()
    }

    /// Reload cron configuration and restore all scheduled jobs from ~/.gravhive/cron.json
    pub async fn load_from_disk(&self) -> Result<usize> {
        let Some(cfg) = CronConfig::load_global()? else {
            return Ok(0);
        };

        if let Ok(tz) = resolve_timezone(&cfg.timezone) {
            *self.default_timezone.write().await = tz.iana_name().unwrap_or("UTC").to_string();
        }

        let mut count = 0;
        for rec in cfg.jobs {
            let ctx = CronExecutionContext {
                cwd: rec.cwd,
                env: rec.env,
                interpreter: rec.interpreter,
            };
            if self
                .add_job_internal(
                    &rec.name,
                    &rec.expression,
                    rec.timezone.as_deref(),
                    &rec.command,
                    &rec.args,
                    ctx,
                    rec.paused,
                )
                .await
                .is_ok()
            {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn calculate_next_run(schedule: &Schedule, tz_str: &str) -> Option<Timestamp> {
        timezone::calculate_next_run(schedule, tz_str)
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_job_internal(
        &self,
        name: &str,
        expression: &str,
        tz_override: Option<&str>,
        command: &str,
        args: &[String],
        context: CronExecutionContext,
        paused: bool,
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
            Some(tz.iana_name().unwrap_or(tz_str.trim()).to_string())
        } else {
            None
        };

        let default_tz = self.default_timezone.read().await.clone();
        let next_run = {
            let effective_tz = canonical_tz_override.as_deref().unwrap_or(&default_tz);
            if paused {
                None
            } else {
                Self::calculate_next_run(&schedule, effective_tz)
            }
        };

        let last_status = if paused { "Stopped" } else { "Scheduled" }.to_string();

        let job = CronJob {
            name: name.to_string(),
            expression: expression.to_string(),
            schedule,
            command: command.to_string(),
            args: args.to_vec(),
            context,
            last_run: None,
            next_run,
            runs_count: 0,
            last_status,
            paused,
            timezone: canonical_tz_override,
            last_error: None,
        };

        info!(
            "[CRON] Registered job '{}' with schedule '{}' (Timezone: {}, Paused: {}, Next run: {:?})",
            name,
            expression,
            job.timezone.as_deref().unwrap_or(&default_tz),
            paused,
            next_run
        );
        self.jobs.write().await.insert(name.to_string(), job);
        Ok(())
    }

    pub async fn add_job(
        &self,
        name: &str,
        expression: &str,
        tz_override: Option<&str>,
        command: &str,
        args: &[String],
    ) -> Result<()> {
        self.add_job_with_context(
            name,
            expression,
            tz_override,
            command,
            args,
            CronExecutionContext::default(),
        )
        .await
    }

    pub async fn add_job_with_context(
        &self,
        name: &str,
        expression: &str,
        tz_override: Option<&str>,
        command: &str,
        args: &[String],
        context: CronExecutionContext,
    ) -> Result<()> {
        self.add_job_internal(name, expression, tz_override, command, args, context, false)
            .await?;
        let _ = self.save_to_disk().await;
        Ok(())
    }

    pub async fn stop_job(&self, name: &str) -> Result<usize> {
        let count = {
            let mut lock = self.jobs.write().await;
            if name == "all" {
                let mut n = 0;
                for job in lock.values_mut() {
                    if !job.paused {
                        job.paused = true;
                        job.last_status = "Stopped".to_string();
                        n += 1;
                    }
                }
                info!("[CRON] Stopped all ({}) cron jobs", n);
                n
            } else if let Some(job) = lock.get_mut(name) {
                job.paused = true;
                job.last_status = "Stopped".to_string();
                info!("[CRON] Stopped cron job '{}'", name);
                1
            } else {
                anyhow::bail!("Cron job '{}' not found", name);
            }
        };
        let _ = self.save_to_disk().await;
        Ok(count)
    }

    pub async fn restart_job(&self, name: &str) -> Result<usize> {
        let default_tz = self.default_timezone.read().await.clone();
        let count = {
            let mut lock = self.jobs.write().await;
            if name == "all" {
                let mut n = 0;
                for job in lock.values_mut() {
                    job.paused = false;
                    job.last_status = "Scheduled".to_string();
                    let effective_tz = job.timezone.as_deref().unwrap_or(&default_tz);
                    job.next_run = Self::calculate_next_run(&job.schedule, effective_tz);
                    n += 1;
                }
                info!("[CRON] Resumed all ({}) cron jobs", n);
                n
            } else if let Some(job) = lock.get_mut(name) {
                job.paused = false;
                job.last_status = "Scheduled".to_string();
                let effective_tz = job.timezone.as_deref().unwrap_or(&default_tz);
                job.next_run = Self::calculate_next_run(&job.schedule, effective_tz);
                info!("[CRON] Resumed cron job '{}'", name);
                1
            } else {
                anyhow::bail!("Cron job '{}' not found", name);
            }
        };
        let _ = self.save_to_disk().await;
        Ok(count)
    }

    pub async fn delete_job(&self, name: &str) -> Result<usize> {
        let count = {
            let mut lock = self.jobs.write().await;
            if name == "all" {
                let n = lock.len();
                lock.clear();
                info!("[CRON] Deleted all ({}) cron jobs", n);
                n
            } else if lock.remove(name).is_some() {
                info!("[CRON] Deleted cron job '{}'", name);
                1
            } else {
                anyhow::bail!("Cron job '{}' not found", name);
            }
        };
        let _ = self.save_to_disk().await;
        Ok(count)
    }

    pub async fn list_jobs(&self) -> Vec<CronJobInfo> {
        let default_tz = self.default_timezone.read().await.clone();
        let lock = self.jobs.read().await;
        lock.values()
            .map(|j| CronJobInfo {
                name: j.name.clone(),
                expression: j.expression.clone(),
                command: if j.args.is_empty() {
                    j.command.clone()
                } else {
                    format!("{} {}", j.command, j.args.join(" "))
                },
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
        runner::spawn_cron_loop(self.jobs, self.default_timezone, self.log_hub);
    }
}
