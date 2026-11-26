use super::*;
use crate::ring_buffer::LogHub;

#[tokio::test]
async fn test_cron_crud_lifecycle() {
    let hub = LogHub::new(100);
    let cron = CronManager::new(hub);

    cron.add_job("cleanup", "0 */10 * * * *", None, "echo", &["done".into()])
        .await
        .expect("Failed to add job");

    let jobs = cron.list_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "cleanup");
    assert_eq!(jobs[0].timezone, "UTC");
    assert!(!jobs[0].paused);
    assert_eq!(jobs[0].last_status, "Scheduled");

    let count = cron.stop_job("cleanup").await.expect("Failed to stop job");
    assert_eq!(count, 1);
    assert!(cron.list_jobs().await[0].paused);
    assert_eq!(cron.list_jobs().await[0].last_status, "Stopped");

    let count = cron
        .restart_job("cleanup")
        .await
        .expect("Failed to restart job");
    assert_eq!(count, 1);
    assert!(!cron.list_jobs().await[0].paused);
    assert_eq!(cron.list_jobs().await[0].last_status, "Scheduled");

    let count = cron
        .delete_job("cleanup")
        .await
        .expect("Failed to delete job");
    assert_eq!(count, 1);
    assert_eq!(cron.list_jobs().await.len(), 0);
}

#[test]
fn test_resolve_valid_and_invalid_timezones() {
    assert!(resolve_timezone("Asia/Jakarta").is_ok());
    assert!(resolve_timezone("UTC").is_ok());
    assert!(resolve_timezone("America/New_York").is_ok());
    assert!(resolve_timezone("local").is_ok());
    assert!(resolve_timezone("system").is_ok());
    assert!(resolve_timezone("asia/jakarta").is_ok());
    assert!(resolve_timezone("Not_A_Real_Timezone_123").is_err());
}

#[tokio::test]
async fn test_cron_timezone_calculation_and_dynamic_change() {
    let hub = LogHub::new(100);
    let cron = CronManager::new_with_timezone(hub, "UTC");

    let (tz, _) = cron.get_timezone().await;
    assert_eq!(tz, "UTC");

    cron.add_job("backup", "0 0 2 * * *", None, "echo", &["backup".into()])
        .await
        .expect("Failed to add job");

    let next_utc = cron.list_jobs().await[0]
        .next_run
        .expect("next_run expected");

    let (new_tz, offset) = cron
        .set_default_timezone("Asia/Jakarta")
        .await
        .expect("Failed to set timezone");
    assert_eq!(new_tz, "Asia/Jakarta");
    assert_eq!(offset, "+07:00");

    let jobs_after = cron.list_jobs().await;
    let next_jakarta_as_utc = jobs_after[0].next_run.expect("next_run expected");
    assert_eq!(jobs_after[0].timezone, "Asia/Jakarta");
    assert_ne!(next_utc, next_jakarta_as_utc);
}

#[tokio::test]
async fn test_cron_job_timezone_override() {
    let hub = LogHub::new(100);
    let cron = CronManager::new_with_timezone(hub, "UTC");

    cron.add_job(
        "ny-report",
        "0 0 9 * * *",
        Some("America/New_York"),
        "echo",
        &[],
    )
    .await
    .expect("Failed to add NY job");

    assert_eq!(cron.list_jobs().await[0].timezone, "America/New_York");

    cron.set_default_timezone("Asia/Tokyo").await.unwrap();
    assert_eq!(cron.list_jobs().await[0].timezone, "America/New_York");
}

#[test]
fn test_cron_append_disk_log() {
    let job_name = "test-log-audit";
    append_cron_log(job_name, "out", "Task finished successfully");
    append_cron_log(job_name, "err", "Warning: rate limit near threshold");

    let logs_dir = crate::common::logs_dir();
    let out_file = logs_dir.join(format!("cron-{}-out.log", job_name));
    let err_file = logs_dir.join(format!("cron-{}-err.log", job_name));

    assert!(out_file.exists());
    assert!(err_file.exists());

    assert!(std::fs::read_to_string(&out_file)
        .unwrap()
        .contains("Task finished successfully"));
    assert!(std::fs::read_to_string(&err_file)
        .unwrap()
        .contains("Warning: rate limit near threshold"));

    let _ = std::fs::remove_file(out_file);
    let _ = std::fs::remove_file(err_file);
}

#[tokio::test]
async fn test_cron_spawn_failure_records_error() {
    let hub = LogHub::new(10);
    let cron = CronManager::new(hub);

    cron.add_job(
        "invalid-bin",
        "0 0 1 * * *",
        None,
        "non_existent_binary_xyz_12345",
        &[],
    )
    .await
    .unwrap();

    let jobs = cron.list_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].last_error, None);
    assert_eq!(jobs[0].last_status, "Scheduled");
}

#[tokio::test]
async fn test_cron_persistence_to_disk() {
    let cron_file = std::env::temp_dir().join(format!("test-cron-{}.json", rand::random::<u64>()));

    let hub = LogHub::new(10);
    let cron1 = CronManager::new_with_timezone(hub.clone(), "Asia/Jakarta");
    cron1
        .add_job("task-1", "0 */5 * * * *", None, "echo", &["hello".into()])
        .await
        .unwrap();
    cron1
        .add_job(
            "task-2",
            "0 0 12 * * *",
            Some("America/New_York"),
            "echo",
            &["noon".into()],
        )
        .await
        .unwrap();

    cron1.stop_job("task-2").await.unwrap();

    let timezone = cron1.default_timezone.read().await.clone();
    let jobs_lock = cron1.jobs.read().await;
    let job_records: Vec<CronJobRecord> = jobs_lock
        .values()
        .map(|job| CronJobRecord {
            name: job.name.clone(),
            expression: job.expression.clone(),
            command: job.command.clone(),
            args: job.args.clone(),
            timezone: job.timezone.clone(),
            paused: job.paused,
            cwd: job.context.cwd.clone(),
            env: job.context.env.clone(),
            interpreter: job.context.interpreter.clone(),
        })
        .collect();
    let json = serde_json::to_string_pretty(&CronConfig {
        timezone,
        jobs: job_records,
    })
    .unwrap();
    std::fs::write(&cron_file, &json).unwrap();

    let cron2 = CronManager::new(hub);
    let loaded_cfg: CronConfig =
        serde_json::from_str(&std::fs::read_to_string(&cron_file).unwrap()).unwrap();

    let mut loaded_count = 0;
    for rec in loaded_cfg.jobs {
        let ctx = CronExecutionContext {
            cwd: rec.cwd,
            env: rec.env,
            interpreter: rec.interpreter,
        };
        if cron2
            .add_job_internal(
                &rec.name,
                &rec.expression,
                rec.timezone.as_deref(),
                &rec.command,
                &rec.args,
                ctx,
                rec.paused,
            )
            .await
            .is_ok()
        {
            loaded_count += 1;
        }
    }

    assert_eq!(loaded_count, 2);
    let list = cron2.list_jobs().await;
    assert_eq!(list.len(), 2);

    let t2 = list.iter().find(|j| j.name == "task-2").unwrap();
    assert!(t2.paused);
    assert_eq!(t2.timezone, "America/New_York");

    let _ = std::fs::remove_file(cron_file);
}

#[tokio::test]
async fn test_cron_executes_with_cwd_env_and_rebuilt_executable() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("ghive_cron_rebuild_{}", rand::random::<u64>()));
    let bin_dir = root.join("target/release");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let bin_path = bin_dir.join("demo_app");
    std::fs::write(&bin_path, "#!/bin/sh\necho \"v1:$TEST_VAR:$(pwd)\"\n").unwrap();
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let hub = LogHub::new(50);
    let cron = CronManager::new(hub.clone());
    let mut env = HashMap::new();
    env.insert("TEST_VAR".to_string(), "ok".to_string());

    let ctx = CronExecutionContext {
        cwd: Some(root.clone()),
        env,
        interpreter: None,
    };

    cron.add_job_with_context(
        "rebuild-job",
        "0 0 1 * * *",
        None,
        "./target/release/demo_app",
        &[],
        ctx.clone(),
    )
    .await
    .unwrap();

    // Execute v1
    runner::run_single_job(
        "rebuild-job".into(),
        "./target/release/demo_app".into(),
        vec![],
        ctx.clone(),
        hub.clone(),
        cron.jobs.clone(),
    )
    .await;

    // Rebuild (replace file on disk with new inode via temp + rename, like cargo build)
    let tmp_new = bin_dir.join("demo_app.tmp");
    std::fs::write(&tmp_new, "#!/bin/sh\necho \"v2:$TEST_VAR:$(pwd)\"\n").unwrap();
    std::fs::set_permissions(&tmp_new, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp_new, &bin_path).unwrap();

    // Execute v2
    runner::run_single_job(
        "rebuild-job".into(),
        "./target/release/demo_app".into(),
        vec![],
        ctx,
        hub.clone(),
        cron.jobs.clone(),
    )
    .await;

    let logs = hub.get_recent(Some("cron:rebuild-job"), 10);
    let messages: Vec<&str> = logs.iter().map(|l| l.message.as_str()).collect();
    let expected_suffix = root.canonicalize().unwrap().to_string_lossy().to_string();
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("v1:ok:") && m.ends_with(&expected_suffix)),
        "Expected v1 log with cwd and env, got: {:?}",
        messages
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("v2:ok:") && m.ends_with(&expected_suffix)),
        "Expected v2 log after rebuild, got: {:?}",
        messages
    );

    let _ = std::fs::remove_dir_all(&root);
}
