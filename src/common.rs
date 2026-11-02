use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_GRACEFUL_TIMEOUT_MS: u64 = 5000;
pub const DEFAULT_RESTART_DELAY_MS: u64 = 100;
pub const MAX_RESTART_BACKOFF_MS: u64 = 10000;
pub const DEFAULT_LOG_RING_BUFFER_SIZE: usize = 1000;

pub fn gravhive_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("GHIVE_HOME") {
        return PathBuf::from(custom);
    }
    if let Some(home) = dirs::home_dir() {
        home.join(".gravhive")
    } else {
        PathBuf::from("/tmp/.gravhive")
    }
}

pub fn socket_path() -> PathBuf {
    gravhive_dir().join("ghive.sock")
}

pub fn pid_file_path() -> PathBuf {
    gravhive_dir().join("ghived.pid")
}

pub fn state_file_path() -> PathBuf {
    gravhive_dir().join("dump.json")
}

pub fn logs_dir() -> PathBuf {
    gravhive_dir().join("logs")
}

pub fn alerts_config_path() -> PathBuf {
    gravhive_dir().join("alerts.json")
}

pub fn ensure_dirs() -> std::io::Result<()> {
    let dir = gravhive_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    let logs = logs_dir();
    if !logs.exists() {
        std::fs::create_dir_all(&logs)?;
    }
    Ok(())
}
