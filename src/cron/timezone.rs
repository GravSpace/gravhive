use anyhow::Result;
use chrono::{DateTime, Utc};
use cron::Schedule;
use tracing::warn;

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
