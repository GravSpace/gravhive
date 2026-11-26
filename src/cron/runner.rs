use crate::cron::timezone::{append_cron_log, calculate_next_run};
use crate::cron::types::{
    CronExecutionContext, CronJob, DEFAULT_CRON_TIMEOUT, MAX_CRON_LOG_CAPTURE_BYTES,
};
use crate::detector::{detect_runtime, DetectedExecution};
use crate::model::LogEntry;
use crate::ring_buffer::LogHub;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Builds a `tokio::process::Command` applying the job's runtime, working directory, and env.
pub(crate) fn build_cron_command(
    cmd_str: &str,
    args: &[String],
    ctx: &CronExecutionContext,
) -> Command {
    let detected = if let Some(ref interp) = ctx.interpreter {
        DetectedExecution {
            interpreter: Some(interp.clone()),
            script: cmd_str.to_string(),
            args: args.to_vec(),
        }
    } else {
        detect_runtime(cmd_str, args)
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

    if let Some(ref cwd) = ctx.cwd {
        cmd.current_dir(cwd);
    }

    for (k, v) in &ctx.env {
        let resolved_val = if v.starts_with("vault:") {
            match crate::vault::SecretVault::resolve_secret_value(v) {
                Ok(sec) => sec,
                Err(e) => {
                    warn!(
                        "[CRON] Failed to resolve vault secret for env '{}': {}",
                        k, e
                    );
                    v.clone()
                }
            }
        } else {
            v.clone()
        };
        cmd.env(k, resolved_val);
    }

    cmd
}

pub(crate) fn spawn_cron_loop(
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    default_timezone: Arc<RwLock<String>>,
    log_hub: LogHub,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            let to_run = collect_due_jobs(&jobs, &default_timezone).await;

            for (name, cmd_str, args, ctx) in to_run {
                let log_hub = log_hub.clone();
                let jobs_lock = jobs.clone();
                tokio::spawn(async move {
                    run_single_job(name, cmd_str, args, ctx, log_hub, jobs_lock).await;
                });
            }
        }
    });
}

async fn collect_due_jobs(
    jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
    default_timezone: &Arc<RwLock<String>>,
) -> Vec<(String, String, Vec<String>, CronExecutionContext)> {
    let now = Utc::now();
    let default_tz = default_timezone.read().await.clone();
    let mut to_run = Vec::new();
    let mut lock = jobs.write().await;

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
                job.next_run = calculate_next_run(&job.schedule, effective_tz);

                to_run.push((
                    job.name.clone(),
                    job.command.clone(),
                    job.args.clone(),
                    job.context.clone(),
                ));
            }
        }
    }

    to_run
}

pub(crate) async fn run_single_job(
    name: String,
    cmd_str: String,
    args: Vec<String>,
    ctx: CronExecutionContext,
    log_hub: LogHub,
    jobs_lock: Arc<RwLock<HashMap<String, CronJob>>>,
) {
    info!("[CRON] Executing job '{}': {} {:?}", name, cmd_str, args);
    let mut cmd = build_cron_command(&cmd_str, &args, &ctx);

    let (status_str, last_err) =
        match tokio::time::timeout(DEFAULT_CRON_TIMEOUT, cmd.output()).await {
            Ok(Ok(out)) => handle_process_output(&name, out, &log_hub),
            Ok(Err(e)) => {
                let err_msg = format!("[CRON SPAWN ERROR] Failed to spawn job '{}': {}", name, e);
                error!("{}", err_msg);
                push_cron_log(&log_hub, &name, "err", &err_msg);
                (format!("Error: {}", e), Some(err_msg))
            }
            Err(_) => {
                let timeout_msg =
                    "[CRON ERROR] Job exceeded maximum execution timeout of 30 minutes".to_string();
                warn!(
                    "[CRON] Job '{}' exceeded maximum timeout of 30 minutes, terminated",
                    name
                );
                push_cron_log(&log_hub, &name, "err", &timeout_msg);
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
}

fn handle_process_output(
    name: &str,
    out: std::process::Output,
    log_hub: &LogHub,
) -> (String, Option<String>) {
    let stdout = truncate_lossy(&out.stdout);
    let stderr = truncate_lossy(&out.stderr);

    if !stdout.is_empty() {
        push_cron_log(log_hub, name, "out", &stdout);
    }
    if !stderr.is_empty() {
        push_cron_log(log_hub, name, "err", &stderr);
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
            push_cron_log(log_hub, name, "err", &fail_entry);
            status_msg.clone()
        };
        (status_msg, Some(err_desc))
    }
}

fn truncate_lossy(bytes: &[u8]) -> String {
    let slice = if bytes.len() > MAX_CRON_LOG_CAPTURE_BYTES {
        &bytes[..MAX_CRON_LOG_CAPTURE_BYTES]
    } else {
        bytes
    };
    let mut text = String::from_utf8_lossy(slice).trim().to_string();
    if bytes.len() > MAX_CRON_LOG_CAPTURE_BYTES {
        text.push_str("\n... [output truncated due to 64KB log limit]");
    }
    text
}

fn push_cron_log(log_hub: &LogHub, name: &str, stream: &str, message: &str) {
    log_hub.push(LogEntry {
        timestamp: Utc::now(),
        app_name: format!("cron:{}", name),
        worker_id: 0,
        stream: stream.to_string(),
        message: message.to_string(),
    });
    append_cron_log(name, stream, message);
}
