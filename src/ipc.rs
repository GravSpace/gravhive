use crate::common::socket_path;
use crate::cron::CronManager;
use crate::model::{LogEntry, LogStreamType, TargetSelector};
use crate::process::ProcessManager;
use crate::protocol::{Request, Response};
use crate::ring_buffer::LogHub;
use crate::wasm::WasmPluginManager;
use anyhow::{bail, Context, Result};
use futures::{SinkExt, StreamExt};
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use tracing::{debug, error, info};

pub struct IpcServer {
    manager: Arc<RwLock<ProcessManager>>,
    log_hub: LogHub,
    cron_manager: Option<CronManager>,
    wasm_manager: Option<Arc<WasmPluginManager>>,
}

impl IpcServer {
    pub fn new(
        manager: Arc<RwLock<ProcessManager>>,
        log_hub: LogHub,
        cron_manager: Option<CronManager>,
        wasm_manager: Option<Arc<WasmPluginManager>>,
    ) -> Self {
        Self {
            manager,
            log_hub,
            cron_manager,
            wasm_manager,
        }
    }

    pub async fn run(self) -> Result<()> {
        let sock_path = socket_path();
        if sock_path.exists() {
            // Check if another daemon is already responding
            if IpcClient::ping().await.is_ok() {
                bail!("Another instance of ghived is already running on {:?}", sock_path);
            }
            let _ = fs::remove_file(&sock_path);
        }

        let listener = UnixListener::bind(&sock_path)
            .with_context(|| format!("Failed to bind Unix socket at {:?}", sock_path))?;

        info!("Gravhive daemon IPC server listening on {:?}", sock_path);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let manager = self.manager.clone();
                    let log_hub = self.log_hub.clone();
                    let cron_mgr = self.cron_manager.clone();
                    let wasm_mgr = self.wasm_manager.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_client(stream, manager, log_hub, cron_mgr, wasm_mgr).await {
                            debug!("Client connection closed: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Error accepting IPC client connection: {}", e);
                }
            }
        }
    }
}

async fn handle_client(
    stream: UnixStream,
    manager: Arc<RwLock<ProcessManager>>,
    log_hub: LogHub,
    cron_manager: Option<CronManager>,
    wasm_manager: Option<Arc<WasmPluginManager>>,
) -> Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = FramedRead::new(read_half, LinesCodec::new());
    let mut writer = FramedWrite::new(write_half, LinesCodec::new());

    while let Some(line_res) = reader.next().await {
        let line = line_res?;
        if line.trim().is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let err_resp = Response::Error {
                    message: format!("Malformed request JSON: {}", e),
                };
                let resp_str = serde_json::to_string(&err_resp)?;
                writer.send(resp_str).await?;
                continue;
            }
        };

        match req {
            Request::Ping => {
                let resp = Response::Pong;
                writer.send(serde_json::to_string(&resp)?).await?;
            }
            Request::Start(config) => {
                let app_name = config.name.clone();
                let instances = config.instances;
                let mut mgr = manager.write().await;
                match mgr.start_app(config).await {
                    Ok(ids) => {
                        let resp = Response::Ok {
                            message: format!(
                                "App '{}' started successfully (instances: {}, worker IDs: {:?})",
                                app_name, instances, ids
                            ),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                    Err(e) => {
                        let resp = Response::Error {
                            message: format!("Failed to start app '{}': {}", app_name, e),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                }
            }
            Request::Stop(target) => {
                let mut mgr = manager.write().await;
                match mgr.stop(&target, Duration::from_millis(5000)).await {
                    Ok(count) => {
                        let resp = Response::Ok {
                            message: format!("Stopped {} process(es) matching {}", count, target),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                    Err(e) => {
                        let resp = Response::Error {
                            message: format!("Error stopping target {}: {}", target, e),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                }
            }
            Request::Restart(target) => {
                let mut mgr = manager.write().await;
                match mgr.restart(&target, Duration::from_millis(5000)).await {
                    Ok(count) => {
                        let resp = Response::Ok {
                            message: format!("Restarted {} process(es) matching {}", count, target),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                    Err(e) => {
                        let resp = Response::Error {
                            message: format!("Error restarting target {}: {}", target, e),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                }
            }
            Request::Reload {
                target,
                graceful_timeout_ms,
            } => {
                let timeout = Duration::from_millis(graceful_timeout_ms.unwrap_or(5000));
                let mut mgr = manager.write().await;
                match mgr.reload(&target, timeout).await {
                    Ok(count) => {
                        let resp = Response::Ok {
                            message: format!(
                                "Zero-downtime reloaded {} process(es) matching {}",
                                count, target
                            ),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                    Err(e) => {
                        let resp = Response::Error {
                            message: format!("Error reloading target {}: {}", target, e),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                }
            }
            Request::Delete(target) => {
                let mut mgr = manager.write().await;
                match mgr.delete(&target, Duration::from_millis(5000)).await {
                    Ok(count) => {
                        let resp = Response::Ok {
                            message: format!("Deleted {} process(es) matching {}", count, target),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                    Err(e) => {
                        let resp = Response::Error {
                            message: format!("Error deleting target {}: {}", target, e),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                }
            }
            Request::List => {
                let mgr = manager.read().await;
                let processes = mgr.list();
                let resp = Response::ProcessList { processes };
                writer.send(serde_json::to_string(&resp)?).await?;
            }
            Request::Describe(target) => {
                let mgr = manager.read().await;
                if let Some((info, config, logs_preview)) = mgr.describe(&target) {
                    let resp = Response::ProcessDetails {
                        info,
                        config,
                        logs_preview,
                    };
                    writer.send(serde_json::to_string(&resp)?).await?;
                } else {
                    let resp = Response::Error {
                        message: format!("Process matching {} not found", target),
                    };
                    writer.send(serde_json::to_string(&resp)?).await?;
                }
            }
            Request::Logs {
                target,
                lines,
                follow,
                stream,
            } => {
                let app_filter = match target {
                    Some(TargetSelector::Name(name)) => Some(name),
                    _ => None,
                };

                let recent = log_hub.get_recent(app_filter.as_deref(), lines);
                for entry in recent {
                    if matches_stream(&entry, stream) {
                        let resp = Response::LogLine(entry);
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                }

                if follow {
                    let mut rx = log_hub.subscribe();
                    while let Ok(entry) = rx.recv().await {
                        let matches_app = match &app_filter {
                            Some(app) => &entry.app_name == app,
                            None => true,
                        };
                        if matches_app && matches_stream(&entry, stream) {
                            let resp = Response::LogLine(entry);
                            if let Err(_) = writer.send(serde_json::to_string(&resp)?).await {
                                break; // Client disconnected
                            }
                        }
                    }
                } else {
                    let end_resp = Response::Ok {
                        message: "END_OF_LOGS".to_string(),
                    };
                    writer.send(serde_json::to_string(&end_resp)?).await?;
                }
            }
            Request::Save => {
                let mgr = manager.read().await;
                let path = crate::common::state_file_path();
                match mgr.save_state(&path) {
                    Ok(_) => {
                        let resp = Response::Ok {
                            message: format!("Active processes snapshot saved to {:?}", path),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                    Err(e) => {
                        let resp = Response::Error {
                            message: format!("Failed to save state: {}", e),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                }
            }
            Request::Resurrect => {
                let path = crate::common::state_file_path();
                let mut mgr = manager.write().await;
                match mgr.resurrect_state(&path).await {
                    Ok(count) => {
                        let resp = Response::Ok {
                            message: format!("Successfully resurrected {} apps from {:?}", count, path),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                    Err(e) => {
                        let resp = Response::Error {
                            message: format!("Failed to resurrect state: {}", e),
                        };
                        writer.send(serde_json::to_string(&resp)?).await?;
                    }
                }
            }
            Request::CronList => {
                let jobs = if let Some(ref cron) = cron_manager {
                    cron.list_jobs().await
                } else {
                    Vec::new()
                };
                let resp = Response::CronList { jobs };
                writer.send(serde_json::to_string(&resp)?).await?;
            }
            Request::PluginList => {
                let plugins = if let Some(ref wasm) = wasm_manager {
                    wasm.list_plugins()
                } else {
                    Vec::new()
                };
                let resp = Response::PluginList { plugins };
                writer.send(serde_json::to_string(&resp)?).await?;
            }
            Request::FlushLogs(_) => {
                log_hub.clear();
                let resp = Response::Ok {
                    message: "Log buffer flushed successfully".to_string(),
                };
                writer.send(serde_json::to_string(&resp)?).await?;
            }
            Request::Shutdown => {
                let resp = Response::Ok {
                    message: "Gravhive daemon shutting down and stopping all child processes...".to_string(),
                };
                let _ = writer.send(serde_json::to_string(&resp)?).await;

                // Stop all managed workers gracefully
                let mut mgr = manager.write().await;
                let _ = mgr.stop(&TargetSelector::All, Duration::from_millis(3000)).await;

                let _ = fs::remove_file(crate::common::socket_path());
                let _ = fs::remove_file(crate::common::pid_file_path());
                std::process::exit(0);
            }
        }
    }

    Ok(())
}

fn matches_stream(entry: &LogEntry, stream: LogStreamType) -> bool {
    match stream {
        LogStreamType::All => true,
        LogStreamType::Stdout => entry.stream == "out",
        LogStreamType::Stderr => entry.stream == "err",
    }
}

pub struct IpcClient;

impl IpcClient {
    pub async fn send_request(req: Request) -> Result<Response> {
        let sock_path = socket_path();
        let stream = UnixStream::connect(&sock_path)
            .await
            .with_context(|| format!("Could not connect to Gravhive daemon at {:?}", sock_path))?;

        let (read_half, write_half) = stream.into_split();
        let mut reader = FramedRead::new(read_half, LinesCodec::new());
        let mut writer = FramedWrite::new(write_half, LinesCodec::new());

        let req_json = serde_json::to_string(&req)?;
        writer.send(req_json).await?;

        if let Some(resp_line) = reader.next().await {
            let resp: Response = serde_json::from_str(&resp_line?)?;
            Ok(resp)
        } else {
            bail!("Daemon closed connection unexpectedly without response");
        }
    }

    pub async fn ping() -> Result<()> {
        match Self::send_request(Request::Ping).await? {
            Response::Pong => Ok(()),
            Response::Ok { .. } => Ok(()),
            other => bail!("Unexpected ping response: {:?}", other),
        }
    }

    pub async fn stream_logs<F>(req: Request, mut handler: F) -> Result<()>
    where
        F: FnMut(LogEntry),
    {
        let sock_path = socket_path();
        let stream = UnixStream::connect(&sock_path)
            .await
            .with_context(|| format!("Could not connect to Gravhive daemon at {:?}", sock_path))?;

        let (read_half, write_half) = stream.into_split();
        let mut reader = FramedRead::new(read_half, LinesCodec::new());
        let mut writer = FramedWrite::new(write_half, LinesCodec::new());

        let req_json = serde_json::to_string(&req)?;
        writer.send(req_json).await?;

        while let Some(line_res) = reader.next().await {
            let line = line_res?;
            match serde_json::from_str::<Response>(&line) {
                Ok(Response::LogLine(entry)) => {
                    handler(entry);
                }
                Ok(Response::Ok { message }) if message == "END_OF_LOGS" => {
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }
}
