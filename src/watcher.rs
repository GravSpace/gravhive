use crate::model::TargetSelector;
use crate::process::ProcessManager;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing::info;

pub struct FileWatcher {
    manager: Arc<RwLock<ProcessManager>>,
}

impl FileWatcher {
    pub fn new(manager: Arc<RwLock<ProcessManager>>) -> Self {
        Self { manager }
    }

    pub fn start(self) -> mpsc::Sender<(String, PathBuf)> {
        let (tx, mut rx) = mpsc::channel::<(String, PathBuf)>(100);
        let manager = self.manager;

        tokio::spawn(async move {
            while let Some((app_name, path)) = rx.recv().await {
                info!(
                    "[WATCH] File change detected at '{}'. Reloading app '{}'...",
                    path.display(),
                    app_name
                );
                let mut mgr = manager.write().await;
                let _ = mgr
                    .restart(
                        &TargetSelector::Name(app_name),
                        Duration::from_millis(3000),
                    )
                    .await;
            }
        });

        tx
    }
}

pub fn should_ignore_path(path: &Path, user_ignores: &[String]) -> bool {
    let path_str = path.to_string_lossy();

    // Default ignore patterns
    let default_ignores = [
        "/.git/",
        "/node_modules/",
        "/target/",
        "/vendor/",
        "/.gravhive/",
        ".log",
        ".tmp",
        ".swp",
        ".DS_Store",
    ];

    for pattern in default_ignores {
        if path_str.contains(pattern) || path_str.ends_with(pattern) {
            return true;
        }
    }

    for custom in user_ignores {
        if path_str.contains(custom) {
            return true;
        }
    }

    false
}
