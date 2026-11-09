use gravhive::config::load_cron_config_from_ecosystem;
use gravhive::cron::{CronConfig, CronManager};
use gravhive::protocol::{Request, Response};
use gravhive::ring_buffer::LogHub;

#[test]
fn test_cron_config_serialization() {
    let cfg = CronConfig {
        timezone: "Asia/Jakarta".to_string(),
        jobs: vec![],
    };
    let json = serde_json::to_string(&cfg).expect("Failed to serialize");
    let deserialized: CronConfig = serde_json::from_str(&json).expect("Failed to deserialize");
    assert_eq!(deserialized.timezone, "Asia/Jakarta");
}

#[test]
fn test_protocol_cron_timezone_messages() {
    let req = Request::CronSetTimezone("Europe/London".to_string());
    let req_json = serde_json::to_string(&req).expect("Failed to serialize request");
    let req_de: Request = serde_json::from_str(&req_json).expect("Failed to deserialize request");
    match req_de {
        Request::CronSetTimezone(tz) => assert_eq!(tz, "Europe/London"),
        _ => panic!("Expected Request::CronSetTimezone"),
    }

    let resp = Response::CronTimezone {
        timezone: "Asia/Jakarta".to_string(),
        offset: "+07:00".to_string(),
    };
    let resp_json = serde_json::to_string(&resp).expect("Failed to serialize response");
    let resp_de: Response =
        serde_json::from_str(&resp_json).expect("Failed to deserialize response");
    match resp_de {
        Response::CronTimezone { timezone, offset } => {
            assert_eq!(timezone, "Asia/Jakarta");
            assert_eq!(offset, "+07:00");
        }
        _ => panic!("Expected Response::CronTimezone"),
    }
}

#[tokio::test]
async fn test_cron_manager_timezone_lifecycle() {
    let hub = LogHub::new(100);
    let cron = CronManager::new_with_timezone(hub, "UTC");

    let (initial_tz, initial_offset) = cron.get_timezone().await;
    assert_eq!(initial_tz, "UTC");
    assert_eq!(initial_offset, "+00:00");

    // Add scheduled job in default timezone (UTC)
    cron.add_job("nightly-task", "0 0 4 * * *", None, "echo", &[])
        .await
        .expect("Failed to add job");

    let jobs = cron.list_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].timezone, "UTC");
    let next_utc = jobs[0].next_run.expect("next_run expected");

    // Switch timezone to Asia/Jakarta (UTC+7)
    let (new_tz, new_offset) = cron
        .set_default_timezone("Asia/Jakarta")
        .await
        .expect("Failed to set timezone");
    assert_eq!(new_tz, "Asia/Jakarta");
    assert_eq!(new_offset, "+07:00");

    let jobs_updated = cron.list_jobs().await;
    assert_eq!(jobs_updated[0].timezone, "Asia/Jakarta");
    let next_jakarta = jobs_updated[0].next_run.expect("next_run expected");

    // Next run must be re-computed and not equal to the UTC calculation
    assert_ne!(next_utc, next_jakarta);

    // Testing invalid timezone rejects with error
    let err_res = cron.set_default_timezone("Invalid/Nowhere_Land").await;
    assert!(err_res.is_err());
}

#[test]
fn test_load_cron_timezone_from_toml_ecosystem() {
    let temp_dir = std::env::temp_dir();
    let path = temp_dir.join(format!("test_cron_eco_{}.toml", std::process::id()));
    let content = r#"
[cron]
timezone = "Asia/Jakarta"

[app.sample]
script = "echo"
cron = "0 2 * * *"
"#;
    std::fs::write(&path, content).unwrap();

    let loaded_tz = load_cron_config_from_ecosystem(&path);
    let _ = std::fs::remove_file(path);

    assert_eq!(loaded_tz, Some("Asia/Jakarta".to_string()));
}

#[tokio::test]
async fn test_cron_compound_shell_command() {
    let hub = LogHub::new(100);
    let cron = CronManager::new_with_timezone(hub, "UTC");

    let cmd = "docker restart surrealdb && /usr/bin/pm2 restart 7 8 11";
    cron.add_job("maintenance", "0 2 * * *", Some("Asia/Jakarta"), cmd, &[])
        .await
        .expect("Failed to add compound shell cron job");

    let jobs = cron.list_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "maintenance");
    assert_eq!(jobs[0].command, cmd);
    assert_eq!(jobs[0].timezone, "Asia/Jakarta");
}
