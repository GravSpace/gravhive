use crate::model::{ExecMode, ProcessConfig, TargetSelector};
use crate::path_utils::resolve_lexical_path;
use crate::process::ProcessManager;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);

type PendingReloadMap =
    Arc<tokio::sync::Mutex<HashMap<String, (tokio::time::Instant, PathBuf, ExecMode)>>>;

#[derive(Clone)]
pub struct FileWatcher {
    tx: mpsc::UnboundedSender<(String, PathBuf, ExecMode)>,
    watchers: Arc<Mutex<HashMap<String, RecommendedWatcher>>>,
}

impl FileWatcher {
    pub fn new(manager: Arc<RwLock<ProcessManager>>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let watcher = Self {
            tx,
            watchers: Arc::new(Mutex::new(HashMap::new())),
        };
        spawn_debounce_loop(manager, rx);
        watcher
    }

    /// Legacy starter preserved for compatibility; debounce loop starts in `new()`.
    pub fn start(self) -> mpsc::Sender<(String, PathBuf)> {
        let (legacy_tx, mut legacy_rx) = mpsc::channel::<(String, PathBuf)>(100);
        let fwd_tx = self.tx.clone();
        tokio::spawn(async move {
            while let Some((app_name, path)) = legacy_rx.recv().await {
                let _ = fwd_tx.send((app_name, path, ExecMode::Fork));
            }
        });
        legacy_tx
    }

    /// Registers or updates file/binary watches for an application when `config.watch` is enabled.
    pub fn register_app(&self, config: &ProcessConfig) {
        if !config.watch {
            self.unregister_app(&config.name);
            return;
        }

        let spec = WatchSpec::from_config(config);
        if spec.targets.is_empty() {
            return;
        }

        match build_app_watcher(config.name.clone(), config.exec_mode, spec, self.tx.clone()) {
            Ok(w) => {
                if let Ok(mut guard) = self.watchers.lock() {
                    guard.insert(config.name.clone(), w);
                }
                info!("[WATCH] Watching '{}' for rebuild/file changes", config.name);
            }
            Err(e) => {
                warn!("[WATCH] Failed to start watcher for '{}': {}", config.name, e);
            }
        }
    }

    pub fn unregister_app(&self, app_name: &str) {
        if let Ok(mut guard) = self.watchers.lock() {
            guard.remove(app_name);
        }
    }

    pub fn sync_apps(&self, active_configs: &[ProcessConfig]) {
        let active_watched: HashSet<&str> = active_configs
            .iter()
            .filter(|c| c.watch)
            .map(|c| c.name.as_str())
            .collect();

        if let Ok(mut guard) = self.watchers.lock() {
            guard.retain(|name, _| active_watched.contains(name.as_str()));
        }

        for cfg in active_configs {
            if cfg.watch {
                let already_watched = self
                    .watchers
                    .lock()
                    .map(|g| g.contains_key(&cfg.name))
                    .unwrap_or(false);
                if !already_watched {
                    self.register_app(cfg);
                }
            }
        }
    }
}

struct WatchSpec {
    /// Directories to watch mapped to their recursive mode.
    targets: Vec<(PathBuf, RecursiveMode)>,
    /// Explicit file paths (such as rebuilt Rust binaries in `/target/`) that bypass ignores.
    explicit_files: HashSet<PathBuf>,
    /// Explicit file names matched in parent directories (handles atomic rename/unlink+create).
    explicit_names: HashSet<std::ffi::OsString>,
    user_ignores: Vec<String>,
}

impl WatchSpec {
    fn from_config(config: &ProcessConfig) -> Self {
        let mut targets = Vec::new();
        let mut explicit_files = HashSet::new();
        let mut explicit_names = HashSet::new();
        let mut seen_dirs = HashSet::new();

        let script_cmd = config.script.split_whitespace().next().unwrap_or(&config.script);
        let resolved_script = PathBuf::from(resolve_lexical_path(script_cmd, config.cwd.as_deref()));

        if config.watch_paths.is_empty() {
            add_path_target(&resolved_script, &mut targets, &mut explicit_files, &mut explicit_names, &mut seen_dirs);
            if is_interpreted_script(&resolved_script) {
                if let Some(ref cwd) = config.cwd {
                    if cwd.is_dir() && seen_dirs.insert(cwd.clone()) {
                        targets.push((cwd.clone(), RecursiveMode::Recursive));
                    }
                }
            }
        } else {
            for wp in &config.watch_paths {
                let resolved = PathBuf::from(resolve_lexical_path(
                    &wp.to_string_lossy(),
                    config.cwd.as_deref(),
                ));
                add_path_target(&resolved, &mut targets, &mut explicit_files, &mut explicit_names, &mut seen_dirs);
            }
        }

        Self {
            targets,
            explicit_files,
            explicit_names,
            user_ignores: config.watch_ignore.clone(),
        }
    }

    fn matches_event(&self, event: &Event) -> Option<PathBuf> {
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return None;
        }

        for path in &event.paths {
            if self.explicit_files.contains(path) {
                return Some(path.clone());
            }
            if let Some(fname) = path.file_name() {
                if self.explicit_names.contains(fname) {
                    return Some(path.clone());
                }
            }
            if self.explicit_files.is_empty() && !should_ignore_path(path, &self.user_ignores) {
                return Some(path.clone());
            }
        }
        None
    }
}

fn add_path_target(
    path: &Path,
    targets: &mut Vec<(PathBuf, RecursiveMode)>,
    explicit_files: &mut HashSet<PathBuf>,
    explicit_names: &mut HashSet<std::ffi::OsString>,
    seen_dirs: &mut HashSet<PathBuf>,
) {
    if path.is_dir() {
        if seen_dirs.insert(path.to_path_buf()) {
            targets.push((path.to_path_buf(), RecursiveMode::Recursive));
        }
        return;
    }

    // For a file/binary (including symlinks or not-yet-created targets), watch its parent directory
    // non-recursively so atomic replacements (rename/unlink+create by cargo build) are detected.
    if path.is_absolute() || path.exists() {
        explicit_files.insert(path.to_path_buf());
        if let Some(name) = path.file_name() {
            explicit_names.insert(name.to_os_string());
        }
        if let Some(parent) = path.parent() {
            if parent.exists() && seen_dirs.insert(parent.to_path_buf()) {
                targets.push((parent.to_path_buf(), RecursiveMode::NonRecursive));
            }
            // Also watch any symlinked parent directory's own parent so symlink swaps trigger reload
            if parent
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                if let Some(grandparent) = parent.parent() {
                    if let Some(pname) = parent.file_name() {
                        explicit_names.insert(pname.to_os_string());
                    }
                    if grandparent.exists() && seen_dirs.insert(grandparent.to_path_buf()) {
                        targets.push((grandparent.to_path_buf(), RecursiveMode::NonRecursive));
                    }
                }
            }
        }
    }
}

fn is_interpreted_script(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "js" | "mjs" | "cjs" | "ts" | "mts" | "cts" | "py" | "php" | "sh" | "bash"
    )
}

fn build_app_watcher(
    app_name: String,
    exec_mode: ExecMode,
    spec: WatchSpec,
    tx: mpsc::UnboundedSender<(String, PathBuf, ExecMode)>,
) -> notify::Result<RecommendedWatcher> {
    let spec = Arc::new(spec);
    let spec_cb = spec.clone();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            if let Some(matched) = spec_cb.matches_event(&event) {
                let _ = tx.send((app_name.clone(), matched, exec_mode));
            }
        }
    })?;

    for (dir, mode) in &spec.targets {
        watcher.watch(dir, *mode)?;
    }
    Ok(watcher)
}

fn spawn_debounce_loop(
    manager: Arc<RwLock<ProcessManager>>,
    mut rx: mpsc::UnboundedReceiver<(String, PathBuf, ExecMode)>,
) {
    tokio::spawn(async move {
        let pending: PendingReloadMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        while let Some((app_name, path, exec_mode)) = rx.recv().await {
            let deadline = tokio::time::Instant::now() + WATCH_DEBOUNCE;
            let should_spawn = {
                let mut guard = pending.lock().await;
                let first = !guard.contains_key(&app_name);
                guard.insert(app_name.clone(), (deadline, path, exec_mode));
                first
            };

            if should_spawn {
                let pending_c = pending.clone();
                let mgr_c = manager.clone();
                tokio::spawn(async move {
                    loop {
                        let next_deadline = {
                            let guard = pending_c.lock().await;
                            guard.get(&app_name).map(|(d, _, _)| *d)
                        };
                        let Some(d) = next_deadline else { break };
                        let now = tokio::time::Instant::now();
                        if now < d {
                            tokio::time::sleep_until(d).await;
                            continue;
                        }
                        let entry = pending_c.lock().await.remove(&app_name);
                        if let Some((_, changed_path, mode)) = entry {
                            trigger_app_reload(&mgr_c, &app_name, &changed_path, mode).await;
                        }
                        break;
                    }
                });
            }
        }
    });
}

async fn trigger_app_reload(
    manager: &Arc<RwLock<ProcessManager>>,
    app_name: &str,
    changed_path: &Path,
    mode: ExecMode,
) {
    info!(
        "[WATCH] Change detected at '{}'. Reloading app '{}'...",
        changed_path.display(),
        app_name
    );
    let selector = TargetSelector::Name(app_name.to_string());
    let mut mgr = manager.write().await;
    if mode == ExecMode::Cluster
        && mgr
            .reload(&selector, Duration::from_millis(3000), Duration::from_millis(80))
            .await
            .is_ok()
    {
        return;
    }
    if let Err(e) = mgr.restart(&selector, Duration::from_millis(3000)).await {
        warn!("[WATCH] Failed to restart app '{}': {}", app_name, e);
    }
}

pub fn should_ignore_path(path: &Path, user_ignores: &[String]) -> bool {
    let path_str = path.to_string_lossy();

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
