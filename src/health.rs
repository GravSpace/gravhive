use crate::model::HealthCheckConfig;
use crate::process::Worker;
use reqwest::Client;
use std::time::Duration;
use tokio::net::TcpStream;
use tracing::{debug, warn};

/// Probes an HTTP endpoint and returns true if status is 2xx.
pub async fn probe_http(url: &str, timeout: Duration) -> bool {
    let client = match Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(_) => return false,
    };

    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(e) => {
            debug!("HTTP health check probe failed for {}: {}", url, e);
            false
        }
    }
}

/// Probes a TCP port on localhost and returns true if connection is accepted.
pub async fn probe_tcp(port: u16, timeout: Duration) -> bool {
    let addr = format!("127.0.0.1:{}", port);
    matches!(
        tokio::time::timeout(timeout, TcpStream::connect(&addr)).await,
        Ok(Ok(_))
    )
}

/// Probes a worker for readiness according to its `HealthCheckConfig`.
/// Retries up to `max_retries` with brief delays between attempts.
pub async fn probe_worker_readiness(worker: &Worker, hc: &HealthCheckConfig) -> bool {
    let timeout = Duration::from_secs(hc.timeout_secs.max(1));
    let retries = hc.max_retries.max(1);
    let interval = Duration::from_millis((hc.interval_secs * 1000).max(200));

    let port_val = worker
        .config
        .target_port
        .or(worker.config.listen_port)
        .unwrap_or(8080);

    for attempt in 1..=retries {
        let mut healthy = true;

        if let Some(ref path_template) = hc.http_path {
            let resolved_url = path_template
                .replace("{port}", &port_val.to_string())
                .replace("{worker_id}", &worker.id.to_string())
                .replace("{instance_index}", &worker.instance_index.to_string());

            if !probe_http(&resolved_url, timeout).await {
                healthy = false;
            }
        }

        if let Some(port) = hc.tcp_port {
            if !probe_tcp(port, timeout).await {
                healthy = false;
            }
        }

        if healthy {
            debug!(
                "[app: {}] Worker #{} passed readiness probe on attempt {}/{}",
                worker.app_name, worker.id, attempt, retries
            );
            return true;
        }

        warn!(
            "[app: {}] Worker #{} failed readiness probe attempt {}/{}",
            worker.app_name, worker.id, attempt, retries
        );

        if attempt < retries {
            tokio::time::sleep(interval).await;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_tcp_probe() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        assert!(probe_tcp(port, Duration::from_millis(500)).await);
        // Non-existent port
        assert!(!probe_tcp(1, Duration::from_millis(100)).await);
    }
}
