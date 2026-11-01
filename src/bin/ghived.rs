use anyhow::{Context, Result};
use gravhive::common::{ensure_dirs, pid_file_path, DEFAULT_LOG_RING_BUFFER_SIZE};
use gravhive::ipc::IpcServer;
use gravhive::model::TargetSelector;
use gravhive::process::ProcessManager;
use gravhive::ring_buffer::LogHub;
use gravhive::watchdog::Watchdog;
use std::fs;
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

    info!("Starting Gravhive Daemon (ghived) v{}", gravhive::common::VERSION);

    ensure_dirs().context("Failed to initialize Gravhive directories")?;

    // Write daemon PID
    let pid = std::process::id();
    fs::write(pid_file_path(), pid.to_string())
        .context("Failed to write PID file")?;

    let log_hub = LogHub::new(DEFAULT_LOG_RING_BUFFER_SIZE);
    let manager = Arc::new(RwLock::new(ProcessManager::new(log_hub.clone())));

    // Spawn Resource Watchdog (CPU, RAM, max_memory_restart)
    let watchdog = Watchdog::new(manager.clone(), 3);
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
        let _ = mgr.stop(&TargetSelector::All, Duration::from_millis(3000)).await;

        let _ = fs::remove_file(pid_file_path());
        let _ = fs::remove_file(gravhive::common::socket_path());
        std::process::exit(0);
    });

    // Run IPC server
    let ipc_server = IpcServer::new(manager, log_hub);
    ipc_server.run().await?;

    Ok(())
}
