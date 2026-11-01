use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::{Cell, Color as TableColor, ContentArrangement, Table};
use gravhive::common::{ensure_dirs, socket_path};
use gravhive::config::{generate_ecosystem_template, parse_config_file};
use gravhive::detector::detect_runtime;
use gravhive::ipc::IpcClient;
use gravhive::model::{
    format_duration, format_memory_bytes, parse_memory_string, ExecMode, LogStreamType,
    ProcessConfig, ProcessStatus, TargetSelector,
};
use gravhive::protocol::{Request, Response};
use gravhive::startup::{generate_startup_script, remove_startup_script};
use gravhive::tui::run_monit;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ghive",
    author = "Gravhive Team",
    version = gravhive::common::VERSION,
    about = "Next-Generation High-Performance Production Process Manager in Rust",
    long_about = "Gravhive is a lightweight, ultra-fast production process manager with cluster mode, zero-downtime hot reload, auto-restart, resource watchdog, and interactive monitoring."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a process or load an ecosystem file
    Start {
        /// Script file, executable binary, or ecosystem config (ghive.toml / ecosystem.config.json)
        target: String,

        /// Custom application name
        #[arg(short, long)]
        name: Option<String>,

        /// Number of cluster instances (or "max" to match CPU cores)
        #[arg(short = 'i', long)]
        instances: Option<String>,

        /// Enable watch & reload upon file changes
        #[arg(short, long)]
        watch: bool,

        /// Maximum memory limit for automatic restart (e.g. "300MB", "1GB")
        #[arg(long)]
        max_memory_restart: Option<String>,

        /// Custom working directory
        #[arg(long)]
        cwd: Option<PathBuf>,

        /// Custom interpreter override (e.g. node, bun, python3, php)
        #[arg(long)]
        interpreter: Option<String>,

        /// Environment variables in KEY=VAL format
        #[arg(short, long = "env", value_parser = parse_key_val)]
        envs: Vec<(String, String)>,

        /// Arguments passed to the target script
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// Stop an application by ID, Name, or 'all'
    Stop {
        /// Target process ID, name, or 'all'
        target: String,
    },

    /// Restart an application by ID, Name, or 'all'
    Restart {
        /// Target process ID, name, or 'all'
        target: String,
    },

    /// Perform a zero-downtime rolling reload on a cluster
    Reload {
        /// Target process ID, name, or 'all'
        target: String,

        /// Graceful shutdown timeout in milliseconds
        #[arg(long, default_value = "5000")]
        timeout: u64,
    },

    /// Delete an application from Gravhive registry
    Delete {
        /// Target process ID, name, or 'all'
        target: String,
    },

    /// List all managed processes with real-time status and metrics
    #[command(alias = "ls")]
    List {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Open full-screen interactive TUI monitoring dashboard
    Monit,

    /// Stream logs for an application
    Logs {
        /// Target application name or ID (optional, defaults to all)
        target: Option<String>,

        /// Number of initial lines to display
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,

        /// Keep listening and streaming live logs
        #[arg(short = 'f', long)]
        follow: bool,

        /// Show only stderr
        #[arg(long)]
        err: bool,

        /// Show only stdout
        #[arg(long)]
        out: bool,
    },

    /// Show detailed metadata and resource inspection for an application
    Describe {
        /// Target process ID or name
        target: String,
    },

    /// Generate an ecosystem configuration file template
    #[command(alias = "init")]
    Ecosystem {
        /// Format: toml, yaml, or json
        #[arg(default_value = "toml")]
        format: String,
    },

    /// Save currently running processes to a snapshot dump
    Save,

    /// Resurrect previously saved processes from snapshot dump
    Resurrect,

    /// Clear in-memory log buffer
    Flush {
        /// Target application name or ID (optional)
        target: Option<String>,
    },

    /// Generate and install OS startup script (systemd / launchd)
    Startup,

    /// Remove OS startup script
    Unstartup,

    /// Deploy application to an environment configured in ghive.toml
    Deploy {
        /// Target environment name (e.g. production, staging, local)
        environment: String,

        /// Path to ecosystem config file (default: ghive.toml)
        #[arg(short, long, default_value = "ghive.toml")]
        config: PathBuf,
    },

    /// List and inspect scheduled cron jobs
    Cron,

    /// List active WebAssembly (WASM) plugins
    Plugins,

    /// Stop the Gravhive background daemon
    Kill,

    /// Check daemon status and connection
    Ping,
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("Invalid KEY=VALUE: no `=` found in `{}`", s))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    ensure_dirs()?;

    match cli.command {
        Commands::Ecosystem { format } => {
            let filename = match format.to_lowercase().as_str() {
                "json" => "ecosystem.config.json",
                "yaml" | "yml" => "ghive.yaml",
                _ => "ghive.toml",
            };
            let template = generate_ecosystem_template(&format);
            std::fs::write(filename, template)?;
            println!(
                "{} Created ecosystem template at {}",
                "✓".green().bold(),
                filename.bold()
            );
            println!("You can start it with: ghive start {}", filename);
            return Ok(());
        }
        Commands::Startup => {
            generate_startup_script()?;
            return Ok(());
        }
        Commands::Unstartup => {
            remove_startup_script()?;
            return Ok(());
        }
        Commands::Deploy { environment, config } => {
            let (_, alerts, targets) = gravhive::config::load_ecosystem(&config)?;
            let alert_mgr = alerts.map(gravhive::alerts::AlertManager::new);
            if let Some(target) = targets.get(&environment) {
                gravhive::deploy::DeployManager::execute_deploy(&environment, target, alert_mgr.as_ref())?;
            } else {
                eprintln!(
                    "{} Environment '{}' not found in {}",
                    "✗".red().bold(),
                    environment,
                    config.display()
                );
            }
            return Ok(());
        }
        _ => {}
    }

    // Ensure daemon is running; auto-spawn if necessary
    ensure_daemon_running().await?;

    match cli.command {
        Commands::Cron => {
            let resp = IpcClient::send_request(Request::CronList).await?;
            if let Response::CronList { jobs } = resp {
                if jobs.is_empty() {
                    println!("\nNo scheduled cron jobs found.\nAdd one in ghive.toml using `cron = \"...\"`\n");
                } else {
                    let mut table = Table::new();
                    table
                        .load_preset(UTF8_ROUND_CORNERS)
                        .set_content_arrangement(ContentArrangement::Dynamic)
                        .set_header(vec![
                            Cell::new("name").fg(TableColor::Cyan),
                            Cell::new("schedule").fg(TableColor::Cyan),
                            Cell::new("command").fg(TableColor::Cyan),
                            Cell::new("next run").fg(TableColor::Cyan),
                            Cell::new("last run").fg(TableColor::Cyan),
                            Cell::new("runs").fg(TableColor::Cyan),
                            Cell::new("status").fg(TableColor::Cyan),
                        ]);

                    for j in jobs {
                        let next_str = j.next_run.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or("-".into());
                        let last_str = j.last_run.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or("-".into());

                        table.add_row(vec![
                            Cell::new(j.name),
                            Cell::new(j.expression),
                            Cell::new(j.command),
                            Cell::new(next_str),
                            Cell::new(last_str),
                            Cell::new(j.runs_count),
                            Cell::new(j.last_status),
                        ]);
                    }
                    println!("\n{}\n", table);
                }
            }
        }

        Commands::Plugins => {
            let resp = IpcClient::send_request(Request::PluginList).await?;
            if let Response::PluginList { plugins } = resp {
                if plugins.is_empty() {
                    println!("\nNo active WASM plugins found.\nPlace .wasm / .wat modules in ~/.gravhive/plugins/\n");
                } else {
                    let mut table = Table::new();
                    table
                        .load_preset(UTF8_ROUND_CORNERS)
                        .set_content_arrangement(ContentArrangement::Dynamic)
                        .set_header(vec![
                            Cell::new("plugin name").fg(TableColor::Cyan),
                            Cell::new("path").fg(TableColor::Cyan),
                            Cell::new("status").fg(TableColor::Cyan),
                        ]);

                    for p in plugins {
                        table.add_row(vec![
                            Cell::new(p.name),
                            Cell::new(p.path),
                            Cell::new("enabled").fg(TableColor::Green),
                        ]);
                    }
                    println!("\n{}\n", table);
                }
            }
        }
        Commands::Start {
            target,
            name,
            instances,
            watch,
            max_memory_restart,
            cwd,
            interpreter,
            envs,
            args,
        } => {
            let target_path = Path::new(&target);

            // Check if target is a configuration file
            let is_config_file = target.ends_with(".toml")
                || target.ends_with(".yaml")
                || target.ends_with(".yml")
                || target.ends_with(".json");

            if is_config_file && target_path.exists() {
                println!(
                    "{} Loading ecosystem configuration from {}",
                    "ℹ".cyan(),
                    target.bold()
                );
                let configs = parse_config_file(target_path)?;
                for cfg in configs {
                    let resp = IpcClient::send_request(Request::Start(cfg)).await?;
                    print_response(resp);
                }
            } else {
                // Single application direct execution
                let detected = detect_runtime(&target, &args);

                let app_name = name.unwrap_or_else(|| {
                    if let Some(stem) = target_path.file_stem().and_then(|s| s.to_str()) {
                        stem.to_string()
                    } else {
                        "app".to_string()
                    }
                });

                let (exec_mode, instance_count) = match instances.as_deref() {
                    Some("max") => {
                        let cores = std::thread::available_parallelism()
                            .map(|p| p.get())
                            .unwrap_or(2);
                        (ExecMode::Cluster, cores)
                    }
                    Some(num_str) => {
                        let count = num_str.parse::<usize>().unwrap_or(1);
                        if count > 1 {
                            (ExecMode::Cluster, count)
                        } else {
                            (ExecMode::Fork, 1)
                        }
                    }
                    None => (ExecMode::Fork, 1),
                };

                let mem_limit = max_memory_restart
                    .as_deref()
                    .and_then(parse_memory_string);

                let mut env_map = HashMap::new();
                for (k, v) in envs {
                    env_map.insert(k, v);
                }

                let final_interpreter = interpreter.or(detected.interpreter);

                let config = ProcessConfig {
                    name: app_name,
                    script: detected.script,
                    args: detected.args,
                    cwd,
                    exec_mode,
                    instances: instance_count,
                    autorestart: true,
                    max_restarts: 15,
                    restart_delay_ms: 100,
                    max_memory_restart: mem_limit,
                    watch,
                    watch_paths: Vec::new(),
                    watch_ignore: Vec::new(),
                    env: env_map,
                    interpreter: final_interpreter,
                    out_log: None,
                    err_log: None,
                    cron: None,
                    user: None,
                    group: None,
                    cgroup_memory_max: None,
                    cgroup_cpu_max: None,
                    cgroup_pids_max: None,
                    listen_port: None,
                    target_port: None,
                    idle_timeout_secs: None,
                };

                let resp = IpcClient::send_request(Request::Start(config)).await?;
                print_response(resp);
            }

            // Show updated process table
            print_process_table(false).await?;
        }

        Commands::Stop { target } => {
            let selector = TargetSelector::parse(&target);
            let resp = IpcClient::send_request(Request::Stop(selector)).await?;
            print_response(resp);
            print_process_table(false).await?;
        }

        Commands::Restart { target } => {
            let selector = TargetSelector::parse(&target);
            let resp = IpcClient::send_request(Request::Restart(selector)).await?;
            print_response(resp);
            print_process_table(false).await?;
        }

        Commands::Reload { target, timeout } => {
            let selector = TargetSelector::parse(&target);
            let resp = IpcClient::send_request(Request::Reload {
                target: selector,
                graceful_timeout_ms: Some(timeout),
            })
            .await?;
            print_response(resp);
            print_process_table(false).await?;
        }

        Commands::Delete { target } => {
            let selector = TargetSelector::parse(&target);
            let resp = IpcClient::send_request(Request::Delete(selector)).await?;
            print_response(resp);
            print_process_table(false).await?;
        }

        Commands::List { json } => {
            print_process_table(json).await?;
        }

        Commands::Monit => {
            run_monit().await?;
        }

        Commands::Logs {
            target,
            lines,
            follow,
            err,
            out,
        } => {
            let selector = target.map(|t| TargetSelector::parse(&t));
            let stream = if err {
                LogStreamType::Stderr
            } else if out {
                LogStreamType::Stdout
            } else {
                LogStreamType::All
            };

            IpcClient::stream_logs(
                Request::Logs {
                    target: selector,
                    lines,
                    follow,
                    stream,
                },
                |entry| {
                    let stream_tag = if entry.stream == "err" {
                        "[ERR]".red().bold()
                    } else {
                        "[OUT]".green().bold()
                    };
                    println!(
                        "{} {} [{}#{}] {}",
                        entry.timestamp.format("%Y-%m-%d %H:%M:%S").to_string().dimmed(),
                        stream_tag,
                        entry.app_name.cyan(),
                        entry.worker_id,
                        entry.message
                    );
                },
            )
            .await?;
        }

        Commands::Describe { target } => {
            let selector = TargetSelector::parse(&target);
            let resp = IpcClient::send_request(Request::Describe(selector)).await?;
            match resp {
                Response::ProcessDetails {
                    info,
                    config,
                    logs_preview,
                } => {
                    println!("\n{}", "─".repeat(60).cyan());
                    println!(
                        " {} Application Details: {} (Worker #{})",
                        "ℹ".cyan().bold(),
                        info.name.bold(),
                        info.id
                    );
                    println!("{}", "─".repeat(60).cyan());
                    println!("  PID:            {}", info.pid);
                    println!("  Status:         {}", colorize_status(info.status));
                    println!("  Exec Mode:      {}", info.exec_mode);
                    println!("  Uptime:         {}", format_duration(info.uptime_secs));
                    println!("  Restarts:       {}", info.restarts);
                    println!("  CPU Usage:      {:.1}%", info.cpu_percent);
                    println!("  Memory:         {}", format_memory_bytes(info.memory_bytes));
                    println!("  Script:         {}", config.script);
                    println!("  Interpreter:    {:?}", config.interpreter);
                    println!("  Working Dir:    {:?}", config.cwd);
                    println!("  Auto Restart:   {}", config.autorestart);
                    println!(
                        "  Max Memory:     {:?}",
                        config.max_memory_restart.map(format_memory_bytes)
                    );
                    println!("  Watch:          {}", config.watch);

                    if !logs_preview.is_empty() {
                        println!("\n  Recent Logs Preview:");
                        for line in logs_preview {
                            println!("    {}", line.dimmed());
                        }
                    }
                    println!();
                }
                Response::Error { message } => {
                    eprintln!("{} {}", "✗".red().bold(), message);
                }
                _ => {}
            }
        }

        Commands::Save => {
            let resp = IpcClient::send_request(Request::Save).await?;
            print_response(resp);
        }

        Commands::Resurrect => {
            let resp = IpcClient::send_request(Request::Resurrect).await?;
            print_response(resp);
            print_process_table(false).await?;
        }

        Commands::Flush { target } => {
            let selector = target
                .map(|t| TargetSelector::parse(&t))
                .unwrap_or(TargetSelector::All);
            let resp = IpcClient::send_request(Request::FlushLogs(selector)).await?;
            print_response(resp);
        }

        Commands::Kill => {
            let resp = IpcClient::send_request(Request::Shutdown).await?;
            print_response(resp);
        }

        Commands::Ping => {
            IpcClient::ping().await?;
            println!("{} Gravhive daemon is alive and responsive.", "✓".green().bold());
        }

        _ => {}
    }

    Ok(())
}

async fn ensure_daemon_running() -> Result<()> {
    if IpcClient::ping().await.is_ok() {
        return Ok(());
    }

    println!(
        "{} Gravhive daemon is not running. Spawning background daemon...",
        "ℹ".cyan()
    );

    let current_exe = std::env::current_exe()?;
    let ghived_exe = current_exe.with_file_name("ghived");

    if !ghived_exe.exists() {
        // Fallback: check if in target/debug or target/release or PATH
        anyhow::bail!(
            "Could not locate 'ghived' daemon binary adjacent to {:?}",
            current_exe
        );
    }

    // Spawn detached daemon process
    Command::new(&ghived_exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to start background daemon: {:?}", ghived_exe))?;

    // Wait for socket to become ready (up to 3 seconds)
    let sock = socket_path();
    let start_wait = std::time::Instant::now();
    while start_wait.elapsed() < Duration::from_millis(3000) {
        if sock.exists() && IpcClient::ping().await.is_ok() {
            println!("{} Gravhive daemon spawned successfully.", "✓".green().bold());
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    anyhow::bail!("Timeout waiting for Gravhive daemon to start.");
}

async fn print_process_table(as_json: bool) -> Result<()> {
    let resp = IpcClient::send_request(Request::List).await?;
    if let Response::ProcessList { processes } = resp {
        if as_json {
            println!("{}", serde_json::to_string_pretty(&processes)?);
            return Ok(());
        }

        if processes.is_empty() {
            println!("\n{}", "No active processes managed by Gravhive.".dimmed());
            println!("To start an app: ghive start <script|binary|config>\n");
            return Ok(());
        }

        let mut table = Table::new();
        table
            .load_preset(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(vec![
                Cell::new("id").fg(TableColor::Cyan),
                Cell::new("name").fg(TableColor::Cyan),
                Cell::new("mode").fg(TableColor::Cyan),
                Cell::new("pid").fg(TableColor::Cyan),
                Cell::new("uptime").fg(TableColor::Cyan),
                Cell::new("↺").fg(TableColor::Cyan),
                Cell::new("status").fg(TableColor::Cyan),
                Cell::new("cpu").fg(TableColor::Cyan),
                Cell::new("mem").fg(TableColor::Cyan),
            ]);

        let mut total_mem = 0u64;
        let mut active_count = 0usize;

        for p in &processes {
            total_mem += p.memory_bytes;
            if p.status == ProcessStatus::Online {
                active_count += 1;
            }

            let status_cell = match p.status {
                ProcessStatus::Online => Cell::new("online").fg(TableColor::Green),
                ProcessStatus::Stopping | ProcessStatus::Restarting => {
                    Cell::new(format!("{:?}", p.status)).fg(TableColor::Yellow)
                }
                ProcessStatus::Stopped => Cell::new("stopped").fg(TableColor::DarkGrey),
                ProcessStatus::Errored => Cell::new("errored").fg(TableColor::Red),
            };

            table.add_row(vec![
                Cell::new(p.id),
                Cell::new(&p.name),
                Cell::new(p.exec_mode),
                Cell::new(if p.pid == 0 { "-".to_string() } else { p.pid.to_string() }),
                Cell::new(format_duration(p.uptime_secs)),
                Cell::new(p.restarts),
                status_cell,
                Cell::new(format!("{:.1}%", p.cpu_percent)),
                Cell::new(format_memory_bytes(p.memory_bytes)),
            ]);
        }

        println!("\n{}", table);
        println!(
            " {} [Daemon: ghived v{}] [Active: {}/{}] [Total Memory: {}]\n",
            "●".green(),
            gravhive::common::VERSION,
            active_count,
            processes.len(),
            format_memory_bytes(total_mem)
        );
    }
    Ok(())
}

fn print_response(resp: Response) {
    match resp {
        Response::Ok { message } => println!("{} {}", "✓".green().bold(), message),
        Response::Error { message } => eprintln!("{} {}", "✗".red().bold(), message),
        _ => {}
    }
}

fn colorize_status(status: ProcessStatus) -> ColoredString {
    match status {
        ProcessStatus::Online => "online".green().bold(),
        ProcessStatus::Stopping | ProcessStatus::Restarting => "restarting".yellow().bold(),
        ProcessStatus::Stopped => "stopped".dimmed(),
        ProcessStatus::Errored => "errored".red().bold(),
    }
}
