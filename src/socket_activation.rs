use crate::model::TargetSelector;
use crate::process::ProcessManager;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

pub const MAX_CONCURRENT_SOCKET_CONNECTIONS: usize = 1024;

pub struct SocketActivationManager {
    manager: Arc<RwLock<ProcessManager>>,
    proxies: Arc<RwLock<HashMap<u16, ProxyEntry>>>,
}

#[allow(dead_code)]
struct ProxyEntry {
    app_name: String,
    listen_port: u16,
    target_port: u16,
    idle_timeout: Duration,
    last_active: Arc<RwLock<Instant>>,
    is_running: Arc<AtomicBool>,
}

impl SocketActivationManager {
    pub fn new(manager: Arc<RwLock<ProcessManager>>) -> Self {
        Self {
            manager,
            proxies: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(
        &self,
        app_name: &str,
        listen_port: u16,
        target_port: u16,
        idle_timeout_secs: u64,
    ) -> Result<()> {
        let entry = ProxyEntry {
            app_name: app_name.to_string(),
            listen_port,
            target_port,
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            last_active: Arc::new(RwLock::new(Instant::now())),
            is_running: Arc::new(AtomicBool::new(false)),
        };

        {
            let mut lock = self.proxies.write().await;
            lock.insert(listen_port, entry);
        }

        let manager = self.manager.clone();
        let proxies = self.proxies.clone();
        let app = app_name.to_string();

        info!(
            "[SOCKET-ACTIVATION] Registered listener on port {} -> worker port {} for app '{}' (Idle timeout: {}s)",
            listen_port, target_port, app, idle_timeout_secs
        );

        let connection_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_SOCKET_CONNECTIONS));

        // Spawn TCP Listener for on-demand activation
        tokio::spawn(async move {
            let bind_host =
                std::env::var("GHIVE_SOCKET_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
            let addr = format!("{}:{}", bind_host, listen_port);
            let listener = match TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    error!(
                        "[SOCKET-ACTIVATION] Failed to bind {} (port {}): {}",
                        addr, listen_port, e
                    );
                    return;
                }
            };

            loop {
                match listener.accept().await {
                    Ok((mut client_stream, client_addr)) => {
                        let permit = match connection_semaphore.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                warn!(
                                    "[SOCKET-ACTIVATION] Max concurrent connections ({}) reached on port {}, rejecting connection from {}",
                                    MAX_CONCURRENT_SOCKET_CONNECTIONS, listen_port, client_addr
                                );
                                drop(client_stream);
                                continue;
                            }
                        };

                        debug!(
                            "[SOCKET-ACTIVATION] Incoming connection from {} on port {}",
                            client_addr, listen_port
                        );

                        let mgr = manager.clone();
                        let app_c = app.clone();
                        let target_p = target_port;

                        // Find entry metadata
                        let (last_active_arc, is_running_arc) = {
                            let lock = proxies.read().await;
                            if let Some(e) = lock.get(&listen_port) {
                                (e.last_active.clone(), e.is_running.clone())
                            } else {
                                continue;
                            }
                        };

                        tokio::spawn(async move {
                            let _permit = permit;
                            // Update last active
                            {
                                let mut la = last_active_arc.write().await;
                                *la = Instant::now();
                            }

                            // 1. If not running, trigger wake-up
                            if !is_running_arc.load(Ordering::SeqCst) {
                                info!("[SOCKET-ACTIVATION] Waking up app '{}' on-demand...", app_c);
                                let mut m = mgr.write().await;
                                let _ = m
                                    .restart(
                                        &TargetSelector::Name(app_c.clone()),
                                        Duration::from_millis(3000),
                                    )
                                    .await;
                                is_running_arc.store(true, Ordering::SeqCst);
                            }

                            // 2. Wait for worker to be ready on target_port (up to 5s)
                            let target_addr = format!("127.0.0.1:{}", target_p);
                            let mut upstream_conn = None;
                            let start_wait = Instant::now();

                            while start_wait.elapsed() < Duration::from_secs(5) {
                                if let Ok(s) = TcpStream::connect(&target_addr).await {
                                    upstream_conn = Some(s);
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(50)).await;
                            }

                            // 3. Proxy bytes bidirectionally
                            if let Some(mut upstream_stream) = upstream_conn {
                                if let Err(e) =
                                    copy_bidirectional(&mut client_stream, &mut upstream_stream)
                                        .await
                                {
                                    debug!("[SOCKET-ACTIVATION] Connection closed: {}", e);
                                }
                                // Update last active after connection finishes
                                let mut la = last_active_arc.write().await;
                                *la = Instant::now();
                            } else {
                                warn!(
                                    "[SOCKET-ACTIVATION] Timed out waiting for app '{}' on {}",
                                    app_c, target_addr
                                );
                            }
                        });
                    }
                    Err(e) => {
                        error!("[SOCKET-ACTIVATION] Error accepting connection: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    pub fn start_idle_checker(self) {
        let proxies = self.proxies;
        let manager = self.manager;

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;

                let lock = proxies.read().await;
                for (port, entry) in lock.iter() {
                    if entry.is_running.load(Ordering::SeqCst) {
                        let last_act = *entry.last_active.read().await;
                        if last_act.elapsed() >= entry.idle_timeout {
                            info!(
                                "[SOCKET-ACTIVATION] App '{}' (port {}) idle for {:?}. Scaling to zero (stopping process)...",
                                entry.app_name, port, last_act.elapsed()
                            );
                            let mut mgr = manager.write().await;
                            let _ = mgr
                                .stop(
                                    &TargetSelector::Name(entry.app_name.clone()),
                                    Duration::from_millis(3000),
                                )
                                .await;
                            entry.is_running.store(false, Ordering::SeqCst);
                        }
                    }
                }
            }
        });
    }
}
