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

    ensure_dirs().context("Failed to initialize Gravhive directories")?;

    // Write daemon PID
    let pid = std::process::id();
    fs::write(pid_file_path(), pid.to_string()).context("Failed to write PID file")?;

    // Load alert configuration if available
    let mut alert_config = AlertConfig::default();
    if Path::new("ghive.toml").exists() {
        if let Ok((_, Some(alerts), _)) = load_ecosystem(Path::new("ghive.toml")) {
            info!("Loaded alert configuration from ghive.toml");
            alert_config = alerts;
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
    let cron_manager = CronManager::new(log_hub.clone());
    cron_manager.clone().start_loop();

    // Initialize Socket Activation Manager
    let socket_activation = SocketActivationManager::new(manager.clone());
    socket_activation.start_idle_checker();

    // Initialize WASM Plugin Manager
    let wasm_manager = Arc::new(WasmPluginManager::new());
    let plugins_dir = gravhive::common::gravhive_dir().join("plugins");
    let _ = wasm_manager.load_directory(&plugins_dir);

    // Spawn Resource Watchdog (CPU, RAM, max_memory_restart)
    let watchdog = Watchdog::new(manager.clone(), 3, Some(alert_manager));
    tokio::spawn(async move {
        watchdog.run_loop().await;
    });

    // Handle OS shutdown signals (SIGINT, SIGTERM)
    let manager_shutdown = manager.clone();
    tokio::spawn(async move {
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("Failed to register SIGINT handler");
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to register SIGTERM handler");

        tokio::select! {
            _ = sigint.recv() => info!("Received SIGINT, initiating graceful shutdown..."),
            _ = sigterm.recv() => info!("Received SIGTERM, initiating graceful shutdown..."),
        }

        let mut mgr = manager_shutdown.write().await;
        let _ = mgr
            .stop(&TargetSelector::All, Duration::from_millis(3000))
            .await;

        let _ = fs::remove_file(pid_file_path());
        let _ = fs::remove_file(gravhive::common::socket_path());
        std::process::exit(0);
    });

    // Run IPC server
    let ipc_server = IpcServer::new(manager, log_hub, Some(cron_manager), Some(wasm_manager));
    ipc_server.run().await?;

    Ok(())
}
