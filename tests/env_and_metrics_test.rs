use chrono::Utc;
use gravhive::env_loader::parse_env_str;
use gravhive::metrics::render_prometheus_metrics;
use gravhive::model::{ExecMode, ProcessItem, ProcessStatus};

#[test]
fn test_env_loader_parsing_and_injection() {
    let raw = r#"
# Database settings
DATABASE_URL=postgres://user:pass@localhost:5432/testdb
PORT=4000

# Quoted strings
APP_SECRET="very-secret-token"
GREETING='Hello GravHive'
"#;

    let envs = parse_env_str(raw);
    assert_eq!(
        envs.get("DATABASE_URL").map(|s| s.as_str()),
        Some("postgres://user:pass@localhost:5432/testdb")
    );
    assert_eq!(envs.get("PORT").map(|s| s.as_str()), Some("4000"));
    assert_eq!(
        envs.get("APP_SECRET").map(|s| s.as_str()),
        Some("very-secret-token")
    );
    assert_eq!(
        envs.get("GREETING").map(|s| s.as_str()),
        Some("Hello GravHive")
    );
}

#[test]
fn test_prometheus_metrics_output_conformance() {
    let procs = vec![
        ProcessItem {
            id: 0,
            name: "api".to_string(),
            instance_index: 0,
            pid: 101,
            status: ProcessStatus::Online,
            exec_mode: ExecMode::Fork,
            uptime_secs: 120,
            restarts: 0,
            cpu_percent: 1.25,
            memory_bytes: 32_000_000,
            created_at: Utc::now(),
        },
        ProcessItem {
            id: 1,
            name: "worker".to_string(),
            instance_index: 0,
            pid: 102,
            status: ProcessStatus::Errored,
            exec_mode: ExecMode::Fork,
            uptime_secs: 0,
            restarts: 3,
            cpu_percent: 0.0,
            memory_bytes: 0,
            created_at: Utc::now(),
        },
    ];

    let rendered = render_prometheus_metrics(&procs, true);

    // Verify OpenMetrics / Prometheus standard headers
    assert!(rendered.contains("# HELP gravhive_up"));
    assert!(rendered.contains("# TYPE gravhive_up gauge"));
    assert!(rendered.contains("gravhive_up 1"));

    // Verify PM2 compatibility metrics
    assert!(rendered.contains("# HELP pm2_up"));
    assert!(rendered.contains("pm2_up 1"));
    assert!(rendered.contains(r#"pm2_process_status{name="api",id="0"} 1"#));
    assert!(rendered.contains(r#"pm2_process_status{name="worker",id="1"} 0"#));

    // Verify process metrics
    assert!(rendered.contains("gravhive_processes_total 2"));
    assert!(rendered
        .contains(r#"gravhive_process_status{id="0",name="api",instance="0",status="online"} 1"#));
    assert!(rendered.contains(
        r#"gravhive_process_status{id="1",name="worker",instance="0",status="errored"} 0"#
    ));
    assert!(rendered.contains(
        r#"gravhive_process_cpu_percent{id="0",name="api",instance="0",pid="101"} 1.25"#
    ));
    assert!(rendered.contains(
        r#"gravhive_process_memory_bytes{id="0",name="api",instance="0",pid="101"} 32000000"#
    ));
    assert!(rendered
        .contains(r#"gravhive_process_restarts_total{id="1",name="worker",instance="0"} 3"#));
}

#[tokio::test]
async fn test_metrics_http_server_endpoint() {
    let port = 19615;
    tokio::spawn(async move {
        let _ = gravhive::metrics::start_metrics_server("127.0.0.1", port).await;
    });

    // Wait 100ms for server to bind
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Scrape /metrics via HTTP client
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{}/metrics", port))
        .send()
        .await
        .expect("Failed to connect to metrics server");

    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("Failed to read body");
    assert!(body.contains("gravhive_up"));
    assert!(body.contains("pm2_up"));
}
