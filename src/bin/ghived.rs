use anyhow::{Context, Result};
use gravhive::alerts::{AlertConfig, AlertManager};
use gravhive::common::{ensure_dirs, pid_file_path, DEFAULT_LOG_RING_BUFFER_SIZE};
use gravhive::config::load_ecosystem;
use gravhive::cron::CronManager;
use gravhive::ipc::IpcServer;
use gravhive::model::TargetSelector;
use gravhive::process::ProcessManager;
use gravhive::ring_buffer::LogHub;
use gravhive::socket_activation::SocketActivationManager;
use gravhive::wasm::WasmPluginManager;
use gravhive::watchdog::Watchdog;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Setup logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gravhive=info,ghived=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!(
        "Starting Gravhive Daemon (ghived) v{}",
        gravhive::common::VERSION
    );

    // Setup subreaper for container & background zombie reaping
    gravhive::process::setup_subreaper();

    ensure_dirs().context("Failed to initialize Gravhive directories")?;

    // Write daemon PID
    let pid = std::process::id();
    fs::write(pid_file_path(), pid.to_string()).context("Failed to write PID file")?;

    // Load alert configuration if available
    let mut alert_config = AlertConfig::default();
    if let Ok(Some(global_alerts)) = AlertConfig::load_global() {
        info!("Loaded alert configuration from ~/.gravhive/alerts.json");
        alert_config = global_alerts;
    }
    if Path::new("ghive.toml").exists() {
        if let Ok((_, Some(alerts), _)) = load_ecosystem(Path::new("ghive.toml")) {
            info!("Loaded alert configuration from ghive.toml");
            alert_config.merge(alerts);
        }
    }
    let alert_manager = AlertManager::new(alert_config);

    let log_hub = LogHub::new(DEFAULT_LOG_RING_BUFFER_SIZE);
    let manager = Arc::new(RwLock::new(ProcessManager::new(log_hub.clone())));
    {
        let mut mgr = manager.write().await;
        mgr.set_alert_manager(alert_manager.clone());
    }

    // Initialize Cron Engine
    let mut cron_timezone = "UTC".to_string();
    if let Ok(Some(saved_cron_config)) = gravhive::cron::CronConfig::load_global() {
        info!(
            "Loaded cron configuration from ~/.gravhive/cron.json (timezone: {})",
            saved_cron_config.timezone
        );
        cron_timezone = saved_cron_config.timezone;
    }
    if Path::new("ghive.toml").exists() {
        if let Some(eco_tz) =
            gravhive::config::load_cron_config_from_ecosystem(Path::new("ghive.toml"))
        {
            info!("Loaded cron timezone '{}' from ghive.toml", eco_tz);
            cron_timezone = eco_tz;
        }
    }
    let cron_manager = CronManager::new_with_timezone(log_hub.clone(), &cron_timezone);
    if let Ok(count) = cron_manager.load_from_disk().await {
        if count > 0 {
            info!(
                "Restored {} scheduled cron jobs from ~/.gravhive/cron.json",
                count
            );
        }
    }
    cron_manager.clone().start_loop();

    // Auto-resurrect processes from dump.json if available
    let state_file = gravhive::common::state_file_path();
    if state_file.exists() {
        let mut mgr = manager.write().await;
        match mgr.resurrect_state(&state_file).await {
            Ok(count) if count > 0 => {
                info!(
                    "Auto-resurrected {} applications from {:?}",
                    count, state_file
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    "Failed to auto-resurrect applications from {:?}: {}",
                    state_file,
                    e
                );
            }
        }
    }

    // Initialize Socket Activation Manager
    let socket_activation = Arc::new(SocketActivationManager::new(manager.clone()));
    socket_activation.start_idle_checker();

    // Auto-register resurrected socket activation apps if any
    {
        let mgr = manager.read().await;
        for config in mgr.get_active_configs() {
            if let (Some(lp), Some(tp), Some(idle_secs)) = (
                config.listen_port,
                config.target_port,
                config.idle_timeout_secs,
            ) {
                let sa = socket_activation.clone();
                let name = config.name.clone();
                let mode = config.idle_mode.unwrap_or_default();
                tokio::spawn(async move {
                    let _ = sa.register(&name, lp, tp, idle_secs, mode).await;
                });
            }
        }
    }

    // Initialize WASM Plugin Manager
    let wasm_manager = Arc::new(WasmPluginManager::new());
    let plugins_dir = gravhive::common::gravhive_dir().join("plugins");
    let _ = wasm_manager.load_directory(&plugins_dir);

    // Spawn Resource Watchdog (CPU, RAM, max_memory_restart)
    let watchdog = Watchdog::new(manager.clone(), 3, Some(alert_manager.clone()));
    tokio::spawn(async move {
        watchdog.run_loop().await;
    });

    // Spawn Zombie Process Reaper Loop
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            gravhive::process::reap_zombies();
        }
    });

    // Handle OS shutdown signals (SIGINT, SIGTERM)
    let manager_shutdown = manager.clone();
    let cron_shutdown = cron_manager.clone();
    tokio::spawn(async move {
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("Failed to register SIGINT handler");
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to register SIGTERM handler");

        tokio::select! {
            _ = sigint.recv() => info!("Received SIGINT, initiating graceful shutdown..."),
            _ = sigterm.recv() => info!("Received SIGTERM, initiating graceful shutdown..."),
        }

        // Auto-save running apps and cron jobs before terminating
        let state_file = gravhive::common::state_file_path();
        {
            let mgr = manager_shutdown.read().await;
            let _ = mgr.save_state(&state_file);
        }
        let _ = cron_shutdown.save_to_disk().await;

        let mut mgr = manager_shutdown.write().await;
        let _ = mgr
            .stop(&TargetSelector::All, Duration::from_millis(3000))
            .await;

        let _ = fs::remove_file(pid_file_path());
        let _ = fs::remove_file(gravhive::common::socket_path());
        std::process::exit(0);
    });

    // Run IPC server
    let ipc_server = IpcServer::new(
        manager,
        log_hub,
        Some(cron_manager),
        Some(wasm_manager),
        Some(alert_manager),
        Some(socket_activation),
    );
    ipc_server.run().await?;

    Ok(())
}
