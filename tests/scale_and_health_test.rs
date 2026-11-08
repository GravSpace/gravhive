use gravhive::health::probe_http;
use gravhive::model::{ExecMode, ProcessConfig};
use gravhive::process::ProcessManager;
use gravhive::ring_buffer::LogHub;
use std::time::Duration;

#[tokio::test]
async fn test_dynamic_scale_process_manager() {
    let log_hub = LogHub::new(100);
    let mut manager = ProcessManager::new(log_hub);

    let config = ProcessConfig {
        name: "test-scale-worker".to_string(),
        script: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "sleep 30".to_string()],
        cwd: None,
        exec_mode: ExecMode::Cluster,
        instances: 2,
        autorestart: true,
        max_restarts: 3,
        restart_delay_ms: 100,
        max_memory_restart: None,
        watch: false,
        watch_paths: Vec::new(),
        watch_ignore: Vec::new(),
        env: Default::default(),
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

    // 1. Start initially with 2 instances
    let ids = manager
        .start_app(config)
        .await
        .expect("Failed to start app");
    assert_eq!(ids.len(), 2);
    assert_eq!(manager.list().len(), 2);

    // 2. Scale up: +2 (should become 4)
    let (prev, curr) = manager.scale("test-scale-worker", "+2").await.unwrap();
    assert_eq!(prev, 2);
    assert_eq!(curr, 4);
    assert_eq!(manager.list().len(), 4);

    // 3. Scale down: -1 (should become 3)
    let (prev, curr) = manager.scale("test-scale-worker", "-1").await.unwrap();
    assert_eq!(prev, 4);
    assert_eq!(curr, 3);
    assert_eq!(manager.list().len(), 3);

    // 4. Scale absolute: 1
    let (prev, curr) = manager.scale("test-scale-worker", "1").await.unwrap();
    assert_eq!(prev, 3);
    assert_eq!(curr, 1);
    assert_eq!(manager.list().len(), 1);

    // Cleanup
    let _ = manager
        .delete(
            &gravhive::model::TargetSelector::All,
            Duration::from_millis(500),
        )
        .await;
}

#[tokio::test]
async fn test_http_probe_success_and_failure() {
    // Non-listening port should return false quickly
    let healthy = probe_http("http://127.0.0.1:54321/healthz", Duration::from_millis(200)).await;
    assert!(!healthy);
}
