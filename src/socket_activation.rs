use crate::model::{IdleMode, TargetSelector};
use crate::process::ProcessManager;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, RwLock, Semaphore};
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
    idle_mode: IdleMode,
    last_active: Arc<RwLock<Instant>>,
    is_running: Arc<AtomicBool>,
    is_waking_up: Arc<AtomicBool>,
    wake_notify: Arc<Notify>,
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
        idle_mode: IdleMode,
    ) -> Result<()> {
        // If already registered for this port, check if config matches
        {
            let mut lock = self.proxies.write().await;
            if lock.contains_key(&listen_port) {
                debug!(
                    "[SOCKET-ACTIVATION] Listener on port {} already registered for app '{}', keeping active proxy",
                    listen_port, app_name
                );
                return Ok(());
            }

            let is_initially_online = {
                let mgr = self.manager.read().await;
                mgr.list()
                    .iter()
                    .any(|w| w.name == app_name && w.status == crate::model::ProcessStatus::Online)
            };

            let entry = ProxyEntry {
                app_name: app_name.to_string(),
                listen_port,
                target_port,
                idle_timeout: Duration::from_secs(idle_timeout_secs),
                idle_mode,
                last_active: Arc::new(RwLock::new(Instant::now())),
                is_running: Arc::new(AtomicBool::new(is_initially_online)),
                is_waking_up: Arc::new(AtomicBool::new(false)),
                wake_notify: Arc::new(Notify::new()),
            };

            lock.insert(listen_port, entry);
        }

        let manager = self.manager.clone();
        let proxies = self.proxies.clone();
        let app = app_name.to_string();

        info!(
            "[SOCKET-ACTIVATION] Registered listener on port {} -> worker port {} for app '{}' (Idle timeout: {}s, mode: {})",
            listen_port, target_port, app, idle_timeout_secs, idle_mode
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
                        let (
                            last_active_arc,
                            is_running_arc,
                            is_waking_up_arc,
                            wake_notify_arc,
                            mode,
                        ) = {
                            let lock = proxies.read().await;
                            if let Some(e) = lock.get(&listen_port) {
                                (
                                    e.last_active.clone(),
                                    e.is_running.clone(),
                                    e.is_waking_up.clone(),
                                    e.wake_notify.clone(),
                                    e.idle_mode,
                                )
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

                            // 1. If not running, coordinate atomic wake-up
                            if !is_running_arc.load(Ordering::SeqCst) {
                                if is_waking_up_arc
                                    .compare_exchange(
                                        false,
                                        true,
                                        Ordering::SeqCst,
                                        Ordering::SeqCst,
                                    )
                                    .is_ok()
                                {
                                    // Leader task: perform the actual wake-up
                                    let wake_start = Instant::now();
                                    info!(
                                        "[SOCKET-ACTIVATION] Waking up app '{}' (mode: {}) on-demand...",
                                        app_c, mode
                                    );
                                    let mut m = mgr.write().await;
                                    match mode {
                                        IdleMode::Freeze => {
                                            let _ = m
                                                .resume(&TargetSelector::Name(app_c.clone()))
                                                .await;
                                        }
                                        IdleMode::Kill => {
                                            let _ = m
                                                .restart(
                                                    &TargetSelector::Name(app_c.clone()),
                                                    Duration::from_millis(3000),
                                                )
                                                .await;
                                        }
                                    }
                                    drop(m); // Release lock immediately!

                                    is_running_arc.store(true, Ordering::SeqCst);
                                    is_waking_up_arc.store(false, Ordering::SeqCst);
                                    wake_notify_arc.notify_waiters();
                                    debug!(
                                        "[SOCKET-ACTIVATION] App '{}' wake-up signal completed in {:?}",
                                        app_c,
                                        wake_start.elapsed()
                                    );
                                } else {
                                    // Waiter task: another connection is waking up the app
                                    while !is_running_arc.load(Ordering::SeqCst) {
                                        let notified = wake_notify_arc.notified();
                                        if is_running_arc.load(Ordering::SeqCst) {
                                            break;
                                        }
                                        notified.await;
                                    }
                                }
                            }

                            // 2. Wait for worker to be ready on target_port using adaptive micro-polling
                            let target_addr = format!("127.0.0.1:{}", target_p);
                            let mut upstream_conn = None;
                            let start_wait = Instant::now();
                            let mut poll_interval = Duration::from_millis(1);
                            let max_poll_interval = Duration::from_millis(50);

                            while start_wait.elapsed() < Duration::from_secs(5) {
                                if let Ok(s) = TcpStream::connect(&target_addr).await {
                                    upstream_conn = Some(s);
                                    break;
                                }
                                tokio::time::sleep(poll_interval).await;
                                poll_interval = (poll_interval * 2).min(max_poll_interval);
                            }

                            // 3. Proxy bytes bidirectionally
                            if let Some(mut upstream_stream) = upstream_conn {
                                debug!(
                                    "[SOCKET-ACTIVATION] Connected to upstream {} for '{}' in {:?}",
                                    target_addr,
                                    app_c,
                                    start_wait.elapsed()
                                );
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

    pub async fn trigger_idle_scale(&self, port: u16) -> Result<bool> {
        let (app_name, mode) = {
            let lock = self.proxies.read().await;
            if let Some(entry) = lock.get(&port) {
                (entry.app_name.clone(), entry.idle_mode)
            } else {
                return Ok(false);
            }
        };

        info!(
            "[SOCKET-ACTIVATION] Scaling app '{}' (port {}) to zero (mode: {})...",
            app_name, port, mode
        );

        let mut mgr = self.manager.write().await;
        match mode {
            IdleMode::Freeze => {
                let _ = mgr.suspend(&TargetSelector::Name(app_name.clone())).await;
            }
            IdleMode::Kill => {
                let _ = mgr
                    .stop(
                        &TargetSelector::Name(app_name.clone()),
                        Duration::from_millis(3000),
                    )
                    .await;
            }
        }
        drop(mgr);

        let lock = self.proxies.read().await;
        if let Some(entry) = lock.get(&port) {
            entry.is_running.store(false, Ordering::SeqCst);
        }

        Ok(true)
    }

    pub fn start_idle_checker(&self) {
        let proxies = self.proxies.clone();
        let manager = self.manager.clone();

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;

                let ports_to_idle: Vec<(u16, String, IdleMode)> = {
                    let lock = proxies.read().await;
                    let mut ports = Vec::new();
                    for (port, entry) in lock.iter() {
                        if entry.is_running.load(Ordering::SeqCst) {
                            let last_act = *entry.last_active.read().await;
                            if last_act.elapsed() >= entry.idle_timeout {
                                ports.push((*port, entry.app_name.clone(), entry.idle_mode));
                            }
                        }
                    }
                    ports
                };

                for (port, app_name, mode) in ports_to_idle {
                    info!(
                        "[SOCKET-ACTIVATION] App '{}' (port {}) idle timeout reached. Scaling to zero (mode: {})...",
                        app_name, port, mode
                    );
                    let mut mgr = manager.write().await;
                    match mode {
                        IdleMode::Freeze => {
                            let _ = mgr.suspend(&TargetSelector::Name(app_name.clone())).await;
                        }
                        IdleMode::Kill => {
                            let _ = mgr
                                .stop(
                                    &TargetSelector::Name(app_name.clone()),
                                    Duration::from_millis(3000),
                                )
                                .await;
                        }
                    }
                    drop(mgr);

                    let lock = proxies.read().await;
                    if let Some(entry) = lock.get(&port) {
                        entry.is_running.store(false, Ordering::SeqCst);
                    }
                }
            }
        });
    }
}
