use anyhow::{Context, Result};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info, warn};

/// Universal L4 Cluster TCP Load Balancer using Round-Robin distribution across worker instances.
pub struct ClusterLoadBalancer {
    pub app_name: String,
    pub listen_port: u16,
    upstream_ports: Arc<RwLock<Vec<u16>>>,
    counter: Arc<AtomicUsize>,
    shutdown_tx: watch::Sender<bool>,
}

impl ClusterLoadBalancer {
    pub async fn start(
        app_name: String,
        listen_port: u16,
        initial_upstreams: Vec<u16>,
    ) -> Result<Self> {
        let addr = format!("0.0.0.0:{}", listen_port);
        let listener = TcpListener::bind(&addr)
            .await
            .with_context(|| format!("Failed to bind Cluster L4 Load Balancer on {}", addr))?;
        let actual_port = listener.local_addr()?.port();

        info!(
            "[CLUSTER-PROXY] Started L4 Round-Robin Load Balancer for app '{}' on port {} (initial upstreams: {:?})",
            app_name, actual_port, initial_upstreams
        );

        let upstream_ports = Arc::new(RwLock::new(initial_upstreams));
        let counter = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let upstreams_clone = upstream_ports.clone();
        let counter_clone = counter.clone();
        let app_c = app_name.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("[CLUSTER-PROXY] Shutting down load balancer on port {}", listen_port);
                            break;
                        }
                    }
                    res = listener.accept() => {
                        match res {
                            Ok((mut client_stream, client_addr)) => {
                                let ports = upstreams_clone.read().await.clone();
                                if ports.is_empty() {
                                    warn!(
                                        "[CLUSTER-PROXY] App '{}' received connection from {} but no upstream workers are available",
                                        app_c, client_addr
                                    );
                                    continue;
                                }

                                let total = ports.len();
                                let start_idx = counter_clone.fetch_add(1, Ordering::Relaxed) % total;
                                let app_name_inner = app_c.clone();

                                tokio::spawn(async move {
                                    // Try connecting to selected upstream, with retry to next upstream if failed
                                    let mut connected_worker = None;
                                    for offset in 0..total {
                                        let target_port = ports[(start_idx + offset) % total];
                                        let target_addr = format!("127.0.0.1:{}", target_port);
                                        match TcpStream::connect(&target_addr).await {
                                            Ok(stream) => {
                                                connected_worker = Some((stream, target_port));
                                                break;
                                            }
                                            Err(e) => {
                                                debug!(
                                                    "[CLUSTER-PROXY] Upstream 127.0.0.1:{} unavailable for app '{}': {}",
                                                    target_port, app_name_inner, e
                                                );
                                            }
                                        }
                                    }

                                    if let Some((mut worker_stream, port)) = connected_worker {
                                        debug!(
                                            "[CLUSTER-PROXY] Proxied connection {} -> 127.0.0.1:{} for app '{}'",
                                            client_addr, port, app_name_inner
                                        );
                                        let _ = tokio::io::copy_bidirectional(
                                            &mut client_stream,
                                            &mut worker_stream,
                                        )
                                        .await;
                                    } else {
                                        error!(
                                            "[CLUSTER-PROXY] All upstream workers failed for app '{}'",
                                            app_name_inner
                                        );
                                    }
                                });
                            }
                            Err(e) => {
                                error!("[CLUSTER-PROXY] Listener error on port {}: {}", listen_port, e);
                                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            app_name,
            listen_port: actual_port,
            upstream_ports,
            counter,
            shutdown_tx,
        })
    }

    pub async fn update_upstreams(&self, new_ports: Vec<u16>) {
        info!(
            "[CLUSTER-PROXY] Updating upstreams for app '{}' on port {}: {:?}",
            self.app_name, self.listen_port, new_ports
        );
        let mut lock = self.upstream_ports.write().await;
        *lock = new_ports;
    }

    pub async fn get_upstreams(&self) -> Vec<u16> {
        self.upstream_ports.read().await.clone()
    }

    pub fn request_count(&self) -> usize {
        self.counter.load(Ordering::Relaxed)
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}
