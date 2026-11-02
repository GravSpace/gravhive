use crate::ipc::IpcClient;
use crate::model::{ProcessItem, ProcessStatus};
use crate::protocol::{Request, Response};
use anyhow::{Context, Result};
use std::fmt::Write;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::debug;

/// Formats the given processes into Prometheus / OpenMetrics text format.
/// Includes PM2-compatible metric names for seamless Grafana integration.
pub fn render_prometheus_metrics(processes: &[ProcessItem], daemon_up: bool) -> String {
    let mut out = String::new();

    let _ = writeln!(
        out,
        "# HELP gravhive_up GravHive daemon status (1 = reachable, 0 = unreachable)"
    );
    let _ = writeln!(out, "# TYPE gravhive_up gauge");
    let _ = writeln!(out, "gravhive_up {}", if daemon_up { 1 } else { 0 });

    let _ = writeln!(
        out,
        "# HELP pm2_up Process manager daemon status (PM2 compatibility)"
    );
    let _ = writeln!(out, "# TYPE pm2_up gauge");
    let _ = writeln!(out, "pm2_up {}", if daemon_up { 1 } else { 0 });

    if !daemon_up {
        return out;
    }

    let _ = writeln!(
        out,
        "# HELP gravhive_processes_total Total number of registered processes"
    );
    let _ = writeln!(out, "# TYPE gravhive_processes_total gauge");
    let _ = writeln!(out, "gravhive_processes_total {}", processes.len());

    // Process Status
    let _ = writeln!(
        out,
        "# HELP gravhive_process_status Process online status (1 = online, 0 = other)"
    );
    let _ = writeln!(out, "# TYPE gravhive_process_status gauge");
    for p in processes {
        let is_online = if p.status == ProcessStatus::Online {
            1
        } else {
            0
        };
        let _ = writeln!(
            out,
            r#"gravhive_process_status{{id="{}",name="{}",instance="{}",status="{}"}} {}"#,
            p.id, p.name, p.instance_index, p.status, is_online
        );
        let _ = writeln!(
            out,
            r#"pm2_process_status{{name="{}",id="{}"}} {}"#,
            p.name, p.id, is_online
        );
    }

    // Process CPU
    let _ = writeln!(
        out,
        "# HELP gravhive_process_cpu_percent Process CPU utilization percentage"
    );
    let _ = writeln!(out, "# TYPE gravhive_process_cpu_percent gauge");
    for p in processes {
        let _ = writeln!(
            out,
            r#"gravhive_process_cpu_percent{{id="{}",name="{}",instance="{}",pid="{}"}} {:.2}"#,
            p.id, p.name, p.instance_index, p.pid, p.cpu_percent
        );
        let _ = writeln!(
            out,
            r#"pm2_process_cpu{{name="{}",id="{}"}} {:.2}"#,
            p.name, p.id, p.cpu_percent
        );
    }

    // Process Memory
    let _ = writeln!(
        out,
        "# HELP gravhive_process_memory_bytes Process RSS memory usage in bytes"
    );
    let _ = writeln!(out, "# TYPE gravhive_process_memory_bytes gauge");
    for p in processes {
        let _ = writeln!(
            out,
            r#"gravhive_process_memory_bytes{{id="{}",name="{}",instance="{}",pid="{}"}} {}"#,
            p.id, p.name, p.instance_index, p.pid, p.memory_bytes
        );
        let _ = writeln!(
            out,
            r#"pm2_process_memory{{name="{}",id="{}"}} {}"#,
            p.name, p.id, p.memory_bytes
        );
    }

    // Process Uptime
    let _ = writeln!(
        out,
        "# HELP gravhive_process_uptime_seconds Process uptime in seconds"
    );
    let _ = writeln!(out, "# TYPE gravhive_process_uptime_seconds gauge");
    for p in processes {
        let _ = writeln!(
            out,
            r#"gravhive_process_uptime_seconds{{id="{}",name="{}",instance="{}"}} {}"#,
            p.id, p.name, p.instance_index, p.uptime_secs
        );
    }

    // Process Restarts
    let _ = writeln!(
        out,
        "# HELP gravhive_process_restarts_total Process restart count"
    );
    let _ = writeln!(out, "# TYPE gravhive_process_restarts_total counter");
    for p in processes {
        let _ = writeln!(
            out,
            r#"gravhive_process_restarts_total{{id="{}",name="{}",instance="{}"}} {}"#,
            p.id, p.name, p.instance_index, p.restarts
        );
        let _ = writeln!(
            out,
            r#"pm2_process_restarts{{name="{}",id="{}"}} {}"#,
            p.name, p.id, p.restarts
        );
    }

    out
}

/// Fetches current process list from the running GravHive daemon.
pub async fn fetch_metrics_from_daemon() -> Result<Vec<ProcessItem>> {
    match IpcClient::send_request(Request::List).await? {
        Response::ProcessList { processes } => Ok(processes),
        Response::Error { message } => anyhow::bail!("Daemon error: {}", message),
        _ => anyhow::bail!("Unexpected response from daemon"),
    }
}

/// Runs a lightweight HTTP server serving Prometheus metrics on the given host and port.
pub async fn start_metrics_server(host: &str, port: u16) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind metrics exporter on {}", addr))?;

    println!(
        "\x1b[32m[GravHive Metrics]\x1b[0m Prometheus exporter listening on http://{}:{}/metrics",
        host, port
    );

    loop {
        let (mut socket, _peer) = match listener.accept().await {
            Ok(res) => res,
            Err(e) => {
                debug!("Failed to accept metrics connection: {}", e);
                continue;
            }
        };

        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let n = match socket.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };

            let req_str = String::from_utf8_lossy(&buf[..n]);
            let first_line = req_str.lines().next().unwrap_or("");
            let parts: Vec<&str> = first_line.split_whitespace().collect();

            if parts.len() >= 2 && parts[0] == "GET" && (parts[1] == "/metrics" || parts[1] == "/")
            {
                let metrics_text = match fetch_metrics_from_daemon().await {
                    Ok(procs) => render_prometheus_metrics(&procs, true),
                    Err(_) => render_prometheus_metrics(&[], false),
                };

                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\r\n{}",
                    metrics_text.len(),
                    metrics_text
                );

                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            } else {
                let not_found = "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found";
                let _ = socket.write_all(not_found.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecMode;
    use chrono::Utc;

    #[test]
    fn test_render_prometheus_metrics_daemon_down() {
        let output = render_prometheus_metrics(&[], false);
        assert!(output.contains("gravhive_up 0"));
        assert!(output.contains("pm2_up 0"));
        assert!(!output.contains("gravhive_process_status"));
    }

    #[test]
    fn test_render_prometheus_metrics_with_processes() {
        let p = ProcessItem {
            id: 1,
            name: "api-server".to_string(),
            instance_index: 0,
            pid: 12345,
            status: ProcessStatus::Online,
            exec_mode: ExecMode::Fork,
            uptime_secs: 360,
            restarts: 1,
            cpu_percent: 2.5,
            memory_bytes: 52428800,
            created_at: Utc::now(),
        };

        let output = render_prometheus_metrics(&[p], true);
        assert!(output.contains("gravhive_up 1"));
        assert!(output.contains("gravhive_processes_total 1"));
        assert!(output.contains(
            r#"gravhive_process_status{id="1",name="api-server",instance="0",status="online"} 1"#
        ));
        assert!(output.contains(r#"gravhive_process_cpu_percent{id="1",name="api-server",instance="0",pid="12345"} 2.50"#));
        assert!(output.contains(r#"gravhive_process_memory_bytes{id="1",name="api-server",instance="0",pid="12345"} 52428800"#));
        assert!(output.contains(
            r#"gravhive_process_uptime_seconds{id="1",name="api-server",instance="0"} 360"#
        ));
        assert!(output.contains(
            r#"gravhive_process_restarts_total{id="1",name="api-server",instance="0"} 1"#
        ));
        assert!(output.contains(r#"pm2_process_status{name="api-server",id="1"} 1"#));
        assert!(output.contains(r#"pm2_process_cpu{name="api-server",id="1"} 2.50"#));
        assert!(output.contains(r#"pm2_process_memory{name="api-server",id="1"} 52428800"#));
        assert!(output.contains(r#"pm2_process_restarts{name="api-server",id="1"} 1"#));
    }
}
