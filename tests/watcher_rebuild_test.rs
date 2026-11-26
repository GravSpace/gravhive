use gravhive::model::{ExecMode, ProcessConfig};
use gravhive::process::ProcessManager;
use gravhive::ring_buffer::LogHub;
use gravhive::watcher::FileWatcher;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[tokio::test]
async fn test_watcher_restarts_rebuilt_binary_in_target_dir() {
    let root = std::env::temp_dir().join(format!("ghive_watch_test_{}", rand::random::<u64>()));
    let bin_dir = root.join("target/release");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let bin_path = bin_dir.join("demo_service");
    std::fs::write(&bin_path, "#!/bin/sh\necho \"v1\"\nsleep 30\n").unwrap();
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let hub = LogHub::new(100);
    let manager = Arc::new(RwLock::new(ProcessManager::new(hub.clone())));
    let watcher = FileWatcher::new(manager.clone());

    let cfg = ProcessConfig {
        name: "demo-watch".to_string(),
        script: bin_path.to_string_lossy().into_owned(),
        args: vec![],
        cwd: Some(root.clone()),
        exec_mode: ExecMode::Fork,
        instances: 1,
        autorestart: false,
        max_restarts: 5,
        restart_delay_ms: 50,
        max_memory_restart: None,
        watch: true,
        watch_paths: vec![],
        watch_ignore: vec![],
        env: HashMap::new(),
        interpreter: None,
        out_log: None,
        err_log: None,
        cron: None,
        cron_timezone: None,
        user: None,
        group: None,
        cgroup_memory_max: None,
        cgroup_cpu_max: None,
        cgroup_pids_max: None,
        listen_port: None,
        target_port: None,
        cluster_proxy: None,
        internal_port_base: None,
        idle_timeout_secs: None,
        idle_mode: None,
        log_rotate_max_size: None,
        log_rotate_keep: None,
        log_compress: true,
        health_check: None,
    };

    {
        let mut mgr = manager.write().await;
        mgr.start_app(cfg.clone()).await.expect("start_app failed");
    }
    watcher.register_app(&cfg);

    // Poll up to 2 seconds for initial v1 output
    let mut saw_v1 = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let initial_logs = hub.get_recent(Some("demo-watch"), 20);
        if initial_logs.iter().any(|l| l.message.trim() == "v1") {
            saw_v1 = true;
            break;
        }
    }
    assert!(
        saw_v1,
        "Expected v1 log, got: {:?}",
        hub.get_recent(Some("demo-watch"), 20)
    );

    // Give FSEvents a brief moment to settle its watch stream before atomic rename
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Atomically rebuild binary (write temp + rename, like cargo build)
    let tmp_bin = bin_dir.join("demo_service.new");
    std::fs::write(&tmp_bin, "#!/bin/sh\necho \"v2\"\nsleep 30\n").unwrap();
    std::fs::set_permissions(&tmp_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp_bin, &bin_path).unwrap();

    // Poll up to 4 seconds for debounced restart (500ms debounce + spawn)
    let mut saw_v2 = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let logs = hub.get_recent(Some("demo-watch"), 30);
        if logs.iter().any(|l| l.message.trim() == "v2") {
            saw_v2 = true;
            break;
        }
    }
    assert!(
        saw_v2,
        "Expected watcher to restart binary and log v2, got: {:?}",
        hub.get_recent(Some("demo-watch"), 30)
    );

    // Cleanup
    watcher.unregister_app("demo-watch");
    let mut mgr = manager.write().await;
    let _ = mgr
        .delete(
            &gravhive::model::TargetSelector::All,
            Duration::from_millis(1000),
        )
        .await;
    let _ = std::fs::remove_dir_all(&root);
}
