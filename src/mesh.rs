use crate::model::ProcessItem;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use tracing::{debug, error, info, warn};

pub const DEFAULT_MESH_PORT: u16 = 9443;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub name: String,
    pub address: String,
    pub os: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub processes_count: usize,
    pub last_seen: DateTime<Utc>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum MeshMessage {
    Register {
        name: String,
        token: String,
        os: String,
    },
    Heartbeat {
        name: String,
        token: String,
        cpu: f32,
        mem: u64,
        processes: Vec<ProcessItem>,
    },
    HeartbeatAck,
    ListNodes {
        token: String,
    },
    NodesResponse(Vec<NodeInfo>),
    AggregateProcesses {
        token: String,
    },
    AggregateProcessesResponse(HashMap<String, Vec<ProcessItem>>),
    Error(String),
}

pub type NodeRegistry = Arc<RwLock<HashMap<String, (NodeInfo, Vec<ProcessItem>)>>>;

#[derive(Clone)]
pub struct MeshCoordinator {
    port: u16,
    token: String,
    nodes: NodeRegistry,
}

impl MeshCoordinator {
    pub fn new(port: u16, token: String) -> Self {
        Self {
            port,
            token,
            nodes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn run(self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.port);
        let listener = TcpListener::bind(&addr)
            .await
            .with_context(|| format!("Failed to bind Mesh Coordinator on {}", addr))?;

        info!("Gravhive Mesh Coordinator listening on {}", addr);

        // Spawn offline pruner task
        let nodes_clone = self.nodes.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;
                let now = Utc::now();
                let mut lock = nodes_clone.write().await;
                for (info, _) in lock.values_mut() {
                    let diff = now.signed_duration_since(info.last_seen);
                    if diff.num_seconds() > 15 {
                        info.status = "offline".to_string();
                    } else {
                        info.status = "online".to_string();
                    }
                }
            }
        });

        loop {
            match listener.accept().await {
                Ok((stream, client_addr)) => {
                    let coordinator = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = coordinator
                            .handle_connection(stream, client_addr.to_string())
                            .await
                        {
                            debug!("Mesh connection closed ({}): {}", client_addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Mesh Coordinator accept error: {}", e);
                }
            }
        }
    }

    async fn handle_connection(&self, stream: TcpStream, client_addr: String) -> Result<()> {
        let (read_half, write_half) = stream.into_split();
        let mut reader = FramedRead::new(read_half, LinesCodec::new());
        let mut writer = FramedWrite::new(write_half, LinesCodec::new());

        while let Some(line_res) = reader.next().await {
            let line = line_res?;
            let msg: MeshMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(e) => {
                    let err = MeshMessage::Error(format!("Malformed mesh message: {}", e));
                    writer.send(serde_json::to_string(&err)?).await?;
                    continue;
                }
            };

            match msg {
                MeshMessage::Register { name, token, os } => {
                    if token != self.token {
                        let err = MeshMessage::Error("Invalid cluster token".to_string());
                        writer.send(serde_json::to_string(&err)?).await?;
                        return Ok(());
                    }

                    let node_info = NodeInfo {
                        id: format!("node-{}", name),
                        name: name.clone(),
                        address: client_addr.clone(),
                        os,
                        cpu_percent: 0.0,
                        memory_bytes: 0,
                        processes_count: 0,
                        last_seen: Utc::now(),
                        status: "online".to_string(),
                    };

                    {
                        let mut lock = self.nodes.write().await;
                        lock.insert(name.clone(), (node_info, Vec::new()));
                    }

                    info!("[MESH] Registered node '{}' from {}", name, client_addr);
                    let resp = MeshMessage::HeartbeatAck;
                    writer.send(serde_json::to_string(&resp)?).await?;
                }
                MeshMessage::Heartbeat {
                    name,
                    token,
                    cpu,
                    mem,
                    processes,
                } => {
                    if token != self.token {
                        let err = MeshMessage::Error("Invalid cluster token".to_string());
                        writer.send(serde_json::to_string(&err)?).await?;
                        return Ok(());
                    }

                    {
                        let mut lock = self.nodes.write().await;
                        if let Some((info, procs)) = lock.get_mut(&name) {
                            info.cpu_percent = cpu;
                            info.memory_bytes = mem;
                            info.processes_count = processes.len();
                            info.last_seen = Utc::now();
                            info.status = "online".to_string();
                            *procs = processes;
                        } else {
                            let node_info = NodeInfo {
                                id: format!("node-{}", name),
                                name: name.clone(),
                                address: client_addr.clone(),
                                os: std::env::consts::OS.to_string(),
                                cpu_percent: cpu,
                                memory_bytes: mem,
                                processes_count: processes.len(),
                                last_seen: Utc::now(),
                                status: "online".to_string(),
                            };
                            lock.insert(name, (node_info, processes));
                        }
                    }

                    let resp = MeshMessage::HeartbeatAck;
                    writer.send(serde_json::to_string(&resp)?).await?;
                }
                MeshMessage::ListNodes { token } => {
                    if token != self.token {
                        let err = MeshMessage::Error("Invalid cluster token".to_string());
                        writer.send(serde_json::to_string(&err)?).await?;
                        return Ok(());
                    }

                    let lock = self.nodes.read().await;
                    let list: Vec<NodeInfo> = lock.values().map(|(info, _)| info.clone()).collect();
                    let resp = MeshMessage::NodesResponse(list);
                    writer.send(serde_json::to_string(&resp)?).await?;
                }
                MeshMessage::AggregateProcesses { token } => {
                    if token != self.token {
                        let err = MeshMessage::Error("Invalid cluster token".to_string());
                        writer.send(serde_json::to_string(&err)?).await?;
                        return Ok(());
                    }

                    let lock = self.nodes.read().await;
                    let map: HashMap<String, Vec<ProcessItem>> = lock
                        .iter()
                        .map(|(k, (_, procs))| (k.clone(), procs.clone()))
                        .collect();
                    let resp = MeshMessage::AggregateProcessesResponse(map);
                    writer.send(serde_json::to_string(&resp)?).await?;
                }
                _ => {}
            }
        }

        Ok(())
    }
}

pub struct MeshAgent;

impl MeshAgent {
    pub async fn run_agent(
        coordinator_addr: String,
        node_name: String,
        token: String,
        poll_interval_secs: u64,
    ) -> Result<()> {
        info!(
            "[MESH-AGENT] Starting node '{}', connecting to coordinator at {}...",
            node_name, coordinator_addr
        );

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("[MESH-AGENT] Disconnecting node '{}' on shutdown signal", node_name);
                    break;
                }
                res = Self::agent_connection_loop(&coordinator_addr, &node_name, &token, poll_interval_secs) => {
                    if let Err(e) = res {
                        warn!(
                            "[MESH-AGENT] Coordinator connection interrupted ({}), retrying in 5s...",
                            e
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn agent_connection_loop(
        coordinator_addr: &str,
        node_name: &str,
        token: &str,
        poll_interval_secs: u64,
    ) -> Result<()> {
        let stream = TcpStream::connect(coordinator_addr)
            .await
            .with_context(|| format!("Failed to connect to {}", coordinator_addr))?;

        info!(
            "[MESH-AGENT] Connected to Mesh Coordinator at {}",
            coordinator_addr
        );
        let (read_half, write_half) = stream.into_split();
        let mut reader = FramedRead::new(read_half, LinesCodec::new());
        let mut writer = FramedWrite::new(write_half, LinesCodec::new());

        // Send registration
        let reg = MeshMessage::Register {
            name: node_name.to_string(),
            token: token.to_string(),
            os: std::env::consts::OS.to_string(),
        };
        writer.send(serde_json::to_string(&reg)?).await?;
        if let Some(res) = reader.next().await {
            let line = res?;
            if let Ok(MeshMessage::Error(err)) = serde_json::from_str::<MeshMessage>(&line) {
                bail!("Registration rejected by coordinator: {}", err);
            }
        }

        // Heartbeat loop
        let mut ticker = tokio::time::interval(Duration::from_secs(poll_interval_secs));
        loop {
            ticker.tick().await;

            // Fetch local process list
            let processes =
                match crate::ipc::IpcClient::send_request(crate::protocol::Request::List).await {
                    Ok(crate::protocol::Response::ProcessList { processes }) => processes,
                    _ => Vec::new(),
                };

            let total_mem: u64 = processes.iter().map(|p| p.memory_bytes).sum();
            let avg_cpu: f32 = if processes.is_empty() {
                0.0
            } else {
                processes.iter().map(|p| p.cpu_percent).sum::<f32>() / processes.len() as f32
            };

            let hb = MeshMessage::Heartbeat {
                name: node_name.to_string(),
                token: token.to_string(),
                cpu: avg_cpu,
                mem: total_mem,
                processes,
            };

            writer.send(serde_json::to_string(&hb)?).await?;

            if let Some(res) = reader.next().await {
                let line = res?;
                if let Ok(MeshMessage::Error(err)) = serde_json::from_str::<MeshMessage>(&line) {
                    bail!("Heartbeat error from coordinator: {}", err);
                }
            } else {
                bail!("Coordinator closed connection");
            }
        }
    }

    pub async fn query_nodes(coordinator_addr: &str, token: &str) -> Result<Vec<NodeInfo>> {
        let stream = TcpStream::connect(coordinator_addr)
            .await
            .with_context(|| {
                format!(
                    "Could not connect to Mesh Coordinator at {}",
                    coordinator_addr
                )
            })?;

        let (read_half, write_half) = stream.into_split();
        let mut reader = FramedRead::new(read_half, LinesCodec::new());
        let mut writer = FramedWrite::new(write_half, LinesCodec::new());

        let req = MeshMessage::ListNodes {
            token: token.to_string(),
        };
        writer.send(serde_json::to_string(&req)?).await?;

        if let Some(res) = reader.next().await {
            let line = res?;
            match serde_json::from_str::<MeshMessage>(&line)? {
                MeshMessage::NodesResponse(nodes) => Ok(nodes),
                MeshMessage::Error(err) => bail!("Mesh error: {}", err),
                _ => bail!("Unexpected mesh response"),
            }
        } else {
            bail!("Coordinator closed connection without response");
        }
    }

    pub async fn query_all_processes(
        coordinator_addr: &str,
        token: &str,
    ) -> Result<HashMap<String, Vec<ProcessItem>>> {
        let stream = TcpStream::connect(coordinator_addr)
            .await
            .with_context(|| {
                format!(
                    "Could not connect to Mesh Coordinator at {}",
                    coordinator_addr
                )
            })?;

        let (read_half, write_half) = stream.into_split();
        let mut reader = FramedRead::new(read_half, LinesCodec::new());
        let mut writer = FramedWrite::new(write_half, LinesCodec::new());

        let req = MeshMessage::AggregateProcesses {
            token: token.to_string(),
        };
        writer.send(serde_json::to_string(&req)?).await?;

        if let Some(res) = reader.next().await {
            let line = res?;
            match serde_json::from_str::<MeshMessage>(&line)? {
                MeshMessage::AggregateProcessesResponse(map) => Ok(map),
                MeshMessage::Error(err) => bail!("Mesh error: {}", err),
                _ => bail!("Unexpected mesh response"),
            }
        } else {
            bail!("Coordinator closed connection without response");
        }
    }
}
