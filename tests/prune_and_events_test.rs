use gravhive::events::{EventJournal, EventType};
use gravhive::model::{ExecMode, ProcessConfig};
use gravhive::process::ProcessManager;
use gravhive::ring_buffer::LogHub;
use std::collections::HashMap;

#[test]
fn test_event_journal_recording_and_filtering() {
    let journal = EventJournal::new(50);
    journal.record_event("web-api", Some(0), EventType::Spawned, "PID 10001");
    journal.record_event("worker", Some(1), EventType::Crashed, "SIGSEGV exit 139");
    journal.record_event(
        "web-api",
        Some(0),
        EventType::Reloaded,
        "Zero-downtime worker #2 online",
    );

    let all = journal.get_events(None, 10);
    assert_eq!(all.len(), 3);

    let web_events = journal.get_events(Some("web-api"), 10);
    assert_eq!(web_events.len(), 2);
    assert_eq!(web_events[0].event_type, EventType::Spawned);
    assert_eq!(web_events[1].event_type, EventType::Reloaded);
}

#[tokio::test]
async fn test_process_manager_prune_stopped_and_errored() {
    let log_hub = LogHub::new(100);
    let mut mgr = ProcessManager::new(log_hub);

    let config = ProcessConfig {
        name: "test-prune-app".to_string(),
        script: "echo".to_string(),
        args: vec!["done".to_string()],
        cwd: None,
        exec_mode: ExecMode::Fork,
        instances: 1,
        autorestart: false,
        max_restarts: 1,
        restart_delay_ms: 10,
        max_memory_restart: None,
        watch: false,
        watch_paths: Vec::new(),
        watch_ignore: Vec::new(),
        env: HashMap::new(),
        interpreter: None,
        out_log: None,
        err_log: None,
        cron: None,
        user: None,
        group: None,
        cgroup_memory_max: None,
        cgroup_cpu_max: None,
        cgroup_pids_max: None,
        listen_port: None,
        target_port: None,
        idle_timeout_secs: None,
        log_rotate_max_size: None,
        log_rotate_keep: None,
        log_compress: true,
        health_check: None,
    };

    let started = mgr.start_app(config).await.expect("Failed to start app");
    assert_eq!(started.len(), 1);

    // Stop worker to simulate stopped state
    let _ = mgr
        .stop(
            &gravhive::model::TargetSelector::Id(started[0]),
            std::time::Duration::from_millis(500),
        )
        .await;

    // 1. Dry run should report worker but NOT delete
    let (dry_procs, _, _) = mgr.prune(true, false);
    assert_eq!(dry_procs.len(), 1);
    assert_eq!(dry_procs[0].1, "test-prune-app");
    assert_eq!(mgr.list().len(), 1); // Still in list

    // 2. Real prune should remove the stopped worker
    let (pruned_procs, _, _) = mgr.prune(false, false);
    assert_eq!(pruned_procs.len(), 1);
    assert_eq!(mgr.list().len(), 0); // Removed from list
}
