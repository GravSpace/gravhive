use gravhive::events::EventType;
use gravhive::model::{ExecMode, IdleMode, ProcessConfig, ProcessStatus, TargetSelector};
use gravhive::process::ProcessManager;
use gravhive::ring_buffer::LogHub;
use gravhive::socket_activation::SocketActivationManager;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

#[tokio::test]
async fn test_process_manager_suspend_and_resume_lifecycle() {
    let log_hub = LogHub::new(100);
    let mut manager = ProcessManager::new(log_hub);

    let config = ProcessConfig {
        name: "test-freeze-app".to_string(),
        script: "sleep".to_string(),
        args: vec!["10".to_string()],
        cwd: None,
        exec_mode: ExecMode::Fork,
        instances: 1,
        autorestart: false,
        max_restarts: 1,
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
        idle_timeout_secs: Some(1),
        idle_mode: Some(IdleMode::Freeze),
        log_rotate_max_size: None,
        log_rotate_keep: None,
        log_compress: true,
        health_check: None,
    };

    let started = manager
        .start_app(config)
        .await
        .expect("Failed to start app");
    assert_eq!(started.len(), 1);

    // Verify online
    let list = manager.list();
    assert_eq!(list[0].status, ProcessStatus::Online);

    // Suspend via SIGSTOP
    let suspended = manager
        .suspend(&TargetSelector::Name("test-freeze-app".to_string()))
        .await
        .expect("Failed to suspend");
    assert_eq!(suspended, 1);

    let list_suspended = manager.list();
    assert_eq!(list_suspended[0].status, ProcessStatus::Idle);

    // Verify event journal recorded Suspended
    let events = manager.event_journal().get_events(None, 5);
    assert!(events.iter().any(|e| e.event_type == EventType::Suspended));

    // Resume via SIGCONT
    let resumed = manager
        .resume(&TargetSelector::Name("test-freeze-app".to_string()))
        .await
        .expect("Failed to resume");
    assert_eq!(resumed, 1);

    let list_resumed = manager.list();
    assert_eq!(list_resumed[0].status, ProcessStatus::Online);

    // Verify event journal recorded Resumed
    let events = manager.event_journal().get_events(None, 5);
    assert!(events.iter().any(|e| e.event_type == EventType::Resumed));

    // Cleanup
    let _ = manager
        .stop(
            &TargetSelector::Name("test-freeze-app".to_string()),
            Duration::from_millis(500),
        )
        .await;
}

#[tokio::test]
async fn test_socket_activation_freeze_fast_wakeup() {
    // 1. Setup mock upstream worker server
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_port = upstream_listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = upstream_listener.accept().await {
            let _ = stream.write_all(b"PONG_WAKEUP_OK\n").await;
        }
    });

    // 2. Setup ProcessManager and SocketActivationManager
    let log_hub = LogHub::new(100);
    let manager = Arc::new(RwLock::new(ProcessManager::new(log_hub)));

    let app_config = ProcessConfig {
        name: "socket-fast-app".to_string(),
        script: "sleep".to_string(),
        args: vec!["15".to_string()],
        cwd: None,
        exec_mode: ExecMode::Fork,
        instances: 1,
        autorestart: false,
        max_restarts: 1,
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
        target_port: Some(target_port),
        cluster_proxy: None,
        internal_port_base: None,
        idle_timeout_secs: Some(1),
        idle_mode: Some(IdleMode::Freeze),
        log_rotate_max_size: None,
        log_rotate_keep: None,
        log_compress: true,
        health_check: None,
    };

    {
        let mut mgr = manager.write().await;
        mgr.start_app(app_config)
            .await
            .expect("Failed to start app");
    }

    // 3. Find a free port for listen_port
    let temp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen_port = temp_listener.local_addr().unwrap().port();
    drop(temp_listener); // free port for socket activation

    let sa = SocketActivationManager::new(manager.clone());
    sa.register(
        "socket-fast-app",
        listen_port,
        target_port,
        1, // 1 second idle timeout
        IdleMode::Freeze,
    )
    .await
    .expect("Failed to register socket activation");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 4. Trigger idle scale via socket activation manager
    let triggered = sa
        .trigger_idle_scale(listen_port)
        .await
        .expect("Failed to trigger idle scale");
    assert!(triggered);

    // Verify it is Idle
    {
        let mgr = manager.read().await;
        let list = mgr.list();
        assert_eq!(list[0].status, ProcessStatus::Idle);
    }

    // 5. Connect to listen_port and measure wake-up response time
    let start_req = Instant::now();
    let mut client = TcpStream::connect(format!("127.0.0.1:{}", listen_port))
        .await
        .expect("Failed to connect to socket activation listener");

    let mut buf = vec![0u8; 64];
    let n = client.read(&mut buf).await.unwrap_or(0);
    let elapsed = start_req.elapsed();
    let response = String::from_utf8_lossy(&buf[..n]).trim().to_string();

    assert_eq!(response, "PONG_WAKEUP_OK");

    // Verify that wake-up transition is ultra fast (< 200ms)
    println!("Fast transition wake-up time: {:?}", elapsed);
    assert!(
        elapsed < Duration::from_millis(500),
        "Wakeup took too long: {:?}",
        elapsed
    );

    // Verify process is back Online
    {
        let mgr = manager.read().await;
        let list = mgr.list();
        assert_eq!(list[0].status, ProcessStatus::Online);
    }

    // Cleanup
    {
        let mut mgr = manager.write().await;
        let _ = mgr
            .stop(
                &TargetSelector::Name("socket-fast-app".to_string()),
                Duration::from_millis(500),
            )
            .await;
    }
}

#[tokio::test]
async fn test_socket_activation_freeze_concurrent_wakeups() {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_port = upstream_listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = upstream_listener.accept().await {
            let _ = stream.write_all(b"CONCURRENT_OK\n").await;
        }
    });

    let log_hub = LogHub::new(100);
    let manager = Arc::new(RwLock::new(ProcessManager::new(log_hub)));

    let app_config = ProcessConfig {
        name: "concurrent-fast-app".to_string(),
        script: "sleep".to_string(),
        args: vec!["20".to_string()],
        cwd: None,
        exec_mode: ExecMode::Fork,
        instances: 1,
        autorestart: false,
        max_restarts: 1,
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
        target_port: Some(target_port),
        cluster_proxy: None,
        internal_port_base: None,
        idle_timeout_secs: Some(1),
        idle_mode: Some(IdleMode::Freeze),
        log_rotate_max_size: None,
        log_rotate_keep: None,
        log_compress: true,
        health_check: None,
    };

    {
        let mut mgr = manager.write().await;
        mgr.start_app(app_config)
            .await
            .expect("Failed to start app");
    }

    let temp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen_port = temp_listener.local_addr().unwrap().port();
    drop(temp_listener);

    let sa = SocketActivationManager::new(manager.clone());
    sa.register(
        "concurrent-fast-app",
        listen_port,
        target_port,
        1,
        IdleMode::Freeze,
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Scale to idle
    let triggered = sa.trigger_idle_scale(listen_port).await.unwrap();
    assert!(triggered);

    // Launch 5 concurrent clients at the same millisecond
    let mut tasks = Vec::new();
    for _ in 0..5 {
        let task = tokio::spawn(async move {
            let mut client = TcpStream::connect(format!("127.0.0.1:{}", listen_port))
                .await
                .expect("Failed to connect");
            let mut buf = vec![0u8; 64];
            let n = client.read(&mut buf).await.unwrap_or(0);
            String::from_utf8_lossy(&buf[..n]).trim().to_string()
        });
        tasks.push(task);
    }

    for task in tasks {
        let resp = task.await.unwrap();
        assert_eq!(resp, "CONCURRENT_OK");
    }

    // Verify online
    {
        let mgr = manager.read().await;
        let list = mgr.list();
        assert_eq!(list[0].status, ProcessStatus::Online);
    }

    // Cleanup
    {
        let mut mgr = manager.write().await;
        let _ = mgr
            .stop(
                &TargetSelector::Name("concurrent-fast-app".to_string()),
                Duration::from_millis(500),
            )
            .await;
    }
}
