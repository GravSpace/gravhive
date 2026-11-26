use anyhow::Result;
use jiff::{tz::TimeZone, Timestamp, ZonedDateTime};
use jiff_cron::Schedule;
use tracing::warn;

pub fn resolve_timezone(tz_str: &str) -> Result<TimeZone> {
    let trimmed = tz_str.trim();
    if trimmed.eq_ignore_ascii_case("local") || trimmed.eq_ignore_ascii_case("system") {
        Ok(TimeZone::try_system().unwrap_or(TimeZone::UTC))
    } else {
        TimeZone::get(trimmed).map_err(|e| anyhow::anyhow!("Invalid timezone '{}': {}", tz_str, e))
    }
}

pub fn format_timezone_offset(tz: &TimeZone) -> String {
    let secs = tz.to_offset(Timestamp::now()).seconds();
    let sign = if secs >= 0 { '+' } else { '-' };
    let abs = secs.abs();
    format!("{}{:02}:{:02}", sign, abs / 3600, (abs % 3600) / 60)
}

pub fn calculate_next_run(schedule: &Schedule, tz_str: &str) -> Option<Timestamp> {
    let trimmed = tz_str.trim();
    let tz = if trimmed.eq_ignore_ascii_case("local") || trimmed.eq_ignore_ascii_case("system") {
        jiff_cron::jiff::tz::TimeZone::try_system().unwrap_or(jiff_cron::jiff::tz::TimeZone::UTC)
    } else {
        jiff_cron::jiff::tz::TimeZone::get(trimmed).unwrap_or(jiff_cron::jiff::tz::TimeZone::UTC)
    };
    let next = schedule.upcoming(tz).next()?;
    Timestamp::from_second(next.timestamp().as_second()).ok()
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
        let now = ZonedDateTime::now().strftime("%Y-%m-%d %H:%M:%S");
        for line in message.lines() {
            let _ = writeln!(f, "[{}] {}", now, line);
        }
    }
}
