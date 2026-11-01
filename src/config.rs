use crate::model::{parse_memory_string, ExecMode, ProcessConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct EcosystemConfig {
    #[serde(default)]
    pub app: HashMap<String, SingleAppConfig>,
    #[serde(default)]
    pub apps: Vec<SingleAppConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleAppConfig {
    pub name: Option<String>,
    pub script: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub exec_mode: Option<String>,
    pub instances: Option<toml::Value>,
    pub autorestart: Option<bool>,
    pub max_restarts: Option<u32>,
    pub restart_delay: Option<String>,
    pub max_memory_restart: Option<String>,
    pub watch: Option<bool>,
    pub watch_paths: Option<Vec<PathBuf>>,
    pub watch_ignore: Option<Vec<String>>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub interpreter: Option<String>,
    pub log: Option<LogConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    pub out: Option<PathBuf>,
    pub err: Option<PathBuf>,
    pub rotate_max_size: Option<String>,
    pub rotate_keep: Option<u32>,
}

pub fn parse_config_file(path: &Path) -> Result<Vec<ProcessConfig>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read configuration file: {}", path.display()))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let configs: Vec<ProcessConfig> = match ext.to_lowercase().as_str() {
        "json" => {
            let eco: EcosystemConfig = serde_json::from_str(&content)
                .with_context(|| "Failed to parse JSON config")?;
            convert_ecosystem_to_configs(eco, path.parent())?
        }
        "yaml" | "yml" => {
            let eco: EcosystemConfig = serde_json::from_str(&content)
                .or_else(|_| toml::from_str(&content))
                .with_context(|| "Failed to parse YAML/TOML config")?;
            convert_ecosystem_to_configs(eco, path.parent())?
        }
        _ => {
            // Default TOML
            let eco: EcosystemConfig = toml::from_str(&content)
                .with_context(|| "Failed to parse TOML config")?;
            convert_ecosystem_to_configs(eco, path.parent())?
        }
    };

    Ok(configs)
}

fn convert_ecosystem_to_configs(
    eco: EcosystemConfig,
    base_dir: Option<&Path>,
) -> Result<Vec<ProcessConfig>> {
    let mut results = Vec::new();

    // From app map
    for (name_key, mut single) in eco.app {
        if single.name.is_none() {
            single.name = Some(name_key);
        }
        results.push(convert_single_app(single, base_dir)?);
    }

    // From apps list
    for single in eco.apps {
        results.push(convert_single_app(single, base_dir)?);
    }

    Ok(results)
}

fn convert_single_app(single: SingleAppConfig, base_dir: Option<&Path>) -> Result<ProcessConfig> {
    let name = single.name.unwrap_or_else(|| {
        Path::new(&single.script)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("app")
            .to_string()
    });

    let exec_mode = match single.exec_mode.as_deref() {
        Some("cluster") => ExecMode::Cluster,
        _ => ExecMode::Fork,
    };

    let instances = match single.instances {
        Some(toml::Value::String(s)) if s.eq_ignore_ascii_case("max") => {
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(2)
        }
        Some(toml::Value::Integer(n)) if n > 0 => n as usize,
        _ => 1,
    };

    let max_memory_restart = single
        .max_memory_restart
        .as_deref()
        .and_then(parse_memory_string);

    let cwd = single.cwd.or_else(|| base_dir.map(|p| p.to_path_buf()));

    let (out_log, err_log) = if let Some(log) = single.log {
        (log.out, log.err)
    } else {
        (None, None)
    };

    Ok(ProcessConfig {
        name,
        script: single.script,
        args: single.args,
        cwd,
        exec_mode,
        instances,
        autorestart: single.autorestart.unwrap_or(true),
        max_restarts: single.max_restarts.unwrap_or(15),
        restart_delay_ms: 100,
        max_memory_restart,
        watch: single.watch.unwrap_or(false),
        watch_paths: single.watch_paths.unwrap_or_default(),
        watch_ignore: single.watch_ignore.unwrap_or_default(),
        env: single.env,
        interpreter: single.interpreter,
        out_log,
        err_log,
    })
}

pub fn generate_ecosystem_template(format: &str) -> &'static str {
    match format.to_lowercase().as_str() {
        "json" => r#"{
  "apps": [
    {
      "name": "web-api",
      "script": "app.js",
      "instances": "max",
      "exec_mode": "cluster",
      "autorestart": true,
      "max_memory_restart": "300MB",
      "watch": false,
      "env": {
        "NODE_ENV": "production",
        "PORT": "3000"
      }
    },
    {
      "name": "worker",
      "script": "./worker-bin",
      "exec_mode": "fork",
      "instances": 1,
      "autorestart": true,
      "env": {
        "QUEUE": "tasks"
      }
    }
  ]
}"#,
        "yaml" | "yml" => r#"apps:
  - name: web-api
    script: app.js
    instances: max
    exec_mode: cluster
    autorestart: true
    max_memory_restart: 300MB
    watch: false
    env:
      NODE_ENV: production
      PORT: "3000"

  - name: worker
    script: ./worker-bin
    exec_mode: fork
    instances: 1
    autorestart: true
"#,
        _ => r#"# Gravhive Ecosystem Configuration File (ghive.toml)
# Run with: ghive start ghive.toml

[app.web-service]
script = "app.js"                   # Script entry point, command, or native binary
name = "web-service"                # Application name
instances = "max"                   # "max" or fixed number of workers
exec_mode = "cluster"               # "cluster" or "fork"
autorestart = true                  # Auto-restart on crash
max_memory_restart = "300MB"        # Automatic restart if RSS exceeds threshold
watch = false                       # Set to true for live file watching
env = { NODE_ENV = "production", PORT = "8080" }

[app.web-service.log]
out = "logs/web-out.log"
err = "logs/web-err.log"

[app.worker-service]
script = "./target/release/worker"
exec_mode = "fork"
instances = 1
autorestart = true
max_restarts = 10
env = { WORKER_THREADS = "4" }
"#,
    }
}

