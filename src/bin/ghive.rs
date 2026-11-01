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

        /// Scheduled cron expression (e.g. "0 */5 * * * *" or "0 2 * * *")
        #[arg(long)]
        cron: Option<String>,
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

        /// Readiness stabilization delay between worker replacements in milliseconds
        #[arg(long, default_value = "80")]
        delay: u64,
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

    /// Manage scheduled cron jobs (list, stop, restart, delete)
    Cron {
        #[command(subcommand)]
        action: Option<CronAction>,
    },

    /// List active WebAssembly (WASM) plugins
    Plugins,

    /// Manage encrypted secrets and environment variables (AES-256-GCM)
    Secrets {
        #[command(subcommand)]
        action: SecretAction,
    },

    /// Manage multi-server cluster mesh
    Mesh {
        #[command(subcommand)]
        action: MeshAction,
    },

    /// Profile CPU or memory of a process and generate an interactive SVG Flamegraph
    Profile {
        /// Target application name or process ID
        target: String,

        /// Duration of profiling in seconds (default: 10)
        #[arg(short, long, default_value = "10")]
        duration: u64,

        /// Output path for interactive SVG Flamegraph
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Profiling mode: cpu or memory (default: cpu)
        #[arg(short, long, default_value = "cpu")]
        mode: String,
    },

    /// Stop the Gravhive background daemon
    Kill,

    /// Check daemon status and connection
    Ping,

    /// Upgrade Gravhive binary to the latest or a specified release
    #[command(alias = "update")]
    Upgrade {
        /// Target version to install (e.g. 0.2.0, default: latest)
        #[arg(long)]
        to: Option<String>,

        /// Force reinstallation even if already on the latest version
        #[arg(short, long)]
        force: bool,

        /// Only check for updates without downloading or installing
        #[arg(short, long)]
        check: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum CronAction {
    /// List all scheduled cron jobs (default)
    #[command(alias = "ls")]
    List,

    /// Stop/pause a scheduled cron job
    Stop {
        /// Name of the cron job or 'all'
        name: String,
    },

    /// Restart/resume a paused cron job
    Restart {
        /// Name of the cron job or 'all'
        name: String,
    },

    /// Delete/remove a scheduled cron job
    #[command(alias = "rm")]
    Delete {
        /// Name of the cron job or 'all'
        name: String,
    },
}

#[derive(Subcommand)]
pub enum SecretAction {
    /// Store an encrypted secret key-value pair
    Set { key: String, value: String },

    /// Retrieve and decrypt a secret by key
    Get { key: String },

    /// List all stored secret keys (with masked values)
    List,

    /// Encrypt a plaintext .env file into .env.enc using AES-256-GCM
    Encrypt {
        /// Path to input .env file
        input: PathBuf,

        /// Path to output encrypted file (default: input with .env.enc)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Decrypt a .env.enc file to stdout
    Decrypt {
        /// Path to encrypted .env.enc file
        input: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum MeshAction {
    /// Start a Mesh Coordinator on this server to connect multiple nodes
    Init {
        /// Listening port (default: 9443)
        #[arg(short, long, default_value = "9443")]
        port: u16,

        /// Cluster secret token
        #[arg(short, long)]
        token: Option<String>,
    },

    /// Join this Gravhive daemon to a remote Mesh Coordinator
    Join {
        /// Coordinator address (e.g. 192.168.1.10:9443)
        coordinator: String,

        /// Cluster secret token
        #[arg(short, long)]
        token: String,

        /// Node name identifier (default: hostname)
        #[arg(short, long)]
        name: Option<String>,
    },

    /// List all connected server nodes and their health in the mesh
    Nodes {
        /// Coordinator address (default: 127.0.0.1:9443)
        #[arg(short, long, default_value = "127.0.0.1:9443")]
        coordinator: String,

        /// Cluster secret token
        #[arg(short, long)]
        token: Option<String>,
    },

    /// View unified process list across all servers in the mesh
    List {
        /// Coordinator address (default: 127.0.0.1:9443)
        #[arg(short, long, default_value = "127.0.0.1:9443")]
        coordinator: String,

        /// Cluster secret token
        #[arg(short, long)]
        token: Option<String>,
    },
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
        Commands::Deploy {
            environment,
            config,
        } => {
            let (_, alerts, targets) = gravhive::config::load_ecosystem(&config)?;
            let alert_mgr = alerts.map(gravhive::alerts::AlertManager::new);
            if let Some(target) = targets.get(&environment) {
                gravhive::deploy::DeployManager::execute_deploy(
                    &environment,
                    target,
                    alert_mgr.as_ref(),
                )?;
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
        Commands::Secrets { action } => {
            match action {
                SecretAction::Set { key, value } => {
                    gravhive::vault::SecretVault::set_secret(&key, &value)?;
                    println!(
                        "{} Secret '{}' stored securely in vault (AES-256-GCM)",
                        "✓".green().bold(),
                        key.bold()
                    );
                }
                SecretAction::Get { key } => {
                    match gravhive::vault::SecretVault::get_secret(&key)? {
                        Some(val) => println!("{}", val),
                        None => {
                            eprintln!("{} Secret '{}' not found in vault", "✗".red().bold(), key)
                        }
                    }
                }
                SecretAction::List => {
                    let secrets = gravhive::vault::SecretVault::list_secrets()?;
                    if secrets.is_empty() {
                        println!("\nNo secrets stored in vault.\nUse `ghive secrets set <KEY> <VALUE>` to store one.\n");
                    } else {
                        let mut table = Table::new();
                        table
                            .load_preset(UTF8_ROUND_CORNERS)
                            .set_content_arrangement(ContentArrangement::Dynamic)
                            .set_header(vec![
                                Cell::new("secret key").fg(TableColor::Cyan),
                                Cell::new("masked value").fg(TableColor::Cyan),
                                Cell::new("status").fg(TableColor::Cyan),
                            ]);
                        for (k, masked) in secrets {
                            table.add_row(vec![
                                Cell::new(k),
                                Cell::new(masked),
                                Cell::new("encrypted (aes-256-gcm)").fg(TableColor::Green),
                            ]);
                        }
                        println!("\n{}\n", table);
                    }
                }
                SecretAction::Encrypt { input, output } => {
                    let out_path = output.unwrap_or_else(|| {
                        let mut path = input.clone();
                        path.set_extension("env.enc");
                        path
                    });
                    gravhive::vault::SecretVault::encrypt_env_file(&input, &out_path)?;
                    println!(
                        "{} Encrypted {} to {}",
                        "✓".green().bold(),
                        input.display(),
                        out_path.display()
                    );
                }
                SecretAction::Decrypt { input } => {
                    let text = gravhive::vault::SecretVault::decrypt_env_file(&input)?;
                    print!("{}", text);
                }
            }
            return Ok(());
        }
        Commands::Mesh {
            action: MeshAction::Init { port, token },
        } => {
            let secret_token = token.unwrap_or_else(|| "ghive-mesh-token".to_string());
            println!(
                "{} Starting Gravhive Mesh Coordinator on port {}...",
                "●".cyan().bold(),
                port
            );
            println!("  Cluster Token: {}", secret_token.bold());
            println!(
                "  Worker nodes can join with: ghive mesh join <COORDINATOR_IP>:{} --token {}",
                port, secret_token
            );
            let coordinator = gravhive::mesh::MeshCoordinator::new(port, secret_token);
            coordinator.run().await?;
            return Ok(());
        }
        Commands::Mesh {
            action: MeshAction::Nodes { coordinator, token },
        } => {
            let secret_token = token.unwrap_or_else(|| "ghive-mesh-token".to_string());
            let nodes = gravhive::mesh::MeshAgent::query_nodes(&coordinator, &secret_token).await?;
            if nodes.is_empty() {
                println!(
                    "\nNo worker nodes currently registered on coordinator {}\n",
                    coordinator
                );
            } else {
                let mut table = Table::new();
                table
                    .load_preset(UTF8_ROUND_CORNERS)
                    .set_content_arrangement(ContentArrangement::Dynamic)
                    .set_header(vec![
                        Cell::new("node id").fg(TableColor::Cyan),
                        Cell::new("hostname").fg(TableColor::Cyan),
                        Cell::new("address").fg(TableColor::Cyan),
                        Cell::new("os").fg(TableColor::Cyan),
                        Cell::new("cpu").fg(TableColor::Cyan),
                        Cell::new("memory").fg(TableColor::Cyan),
                        Cell::new("procs").fg(TableColor::Cyan),
                        Cell::new("last seen").fg(TableColor::Cyan),
                        Cell::new("status").fg(TableColor::Cyan),
                    ]);
                for n in nodes {
                    let status_cell = if n.status == "online" {
                        Cell::new("online").fg(TableColor::Green)
                    } else {
                        Cell::new("offline").fg(TableColor::Red)
                    };
                    table.add_row(vec![
                        Cell::new(n.id),
                        Cell::new(n.name),
                        Cell::new(n.address),
                        Cell::new(n.os),
                        Cell::new(format!("{:.1}%", n.cpu_percent)),
                        Cell::new(format_memory_bytes(n.memory_bytes)),
                        Cell::new(n.processes_count),
                        Cell::new(n.last_seen.format("%H:%M:%S").to_string()),
                        status_cell,
                    ]);
                }
                println!("\n{}\n", table);
            }
            return Ok(());
        }
        Commands::Mesh {
            action: MeshAction::List { coordinator, token },
        } => {
            let secret_token = token.unwrap_or_else(|| "ghive-mesh-token".to_string());
            let map =
                gravhive::mesh::MeshAgent::query_all_processes(&coordinator, &secret_token).await?;
            if map.is_empty() {
                println!("\nNo active processes reported across cluster mesh.\n");
            } else {
                for (node_name, processes) in map {
                    println!("\n{} Node: {}", "●".cyan().bold(), node_name.bold());
                    if processes.is_empty() {
                        println!("  (No active processes)");
                    } else {
                        let mut table = Table::new();
                        table
                            .load_preset(UTF8_ROUND_CORNERS)
                            .set_content_arrangement(ContentArrangement::Dynamic)
                            .set_header(vec![
                                Cell::new("id").fg(TableColor::Cyan),
                                Cell::new("name").fg(TableColor::Cyan),
                                Cell::new("pid").fg(TableColor::Cyan),
                                Cell::new("status").fg(TableColor::Cyan),
                                Cell::new("cpu").fg(TableColor::Cyan),
                                Cell::new("memory").fg(TableColor::Cyan),
                                Cell::new("uptime").fg(TableColor::Cyan),
                            ]);
                        for p in processes {
                            let status_cell = match p.status {
                                ProcessStatus::Online => Cell::new("online").fg(TableColor::Green),
                                ProcessStatus::Stopping | ProcessStatus::Restarting => {
                                    Cell::new(format!("{:?}", p.status)).fg(TableColor::Yellow)
                                }
                                ProcessStatus::Stopped => {
                                    Cell::new("stopped").fg(TableColor::DarkGrey)
                                }
                                ProcessStatus::Errored => Cell::new("errored").fg(TableColor::Red),
                            };
                            table.add_row(vec![
                                Cell::new(p.id),
                                Cell::new(p.name),
                                Cell::new(p.pid),
                                status_cell,
                                Cell::new(format!("{:.1}%", p.cpu_percent)),
                                Cell::new(format_memory_bytes(p.memory_bytes)),
                                Cell::new(format_duration(p.uptime_secs)),
                            ]);
                        }
                        println!("{}", table);
                    }
                }
                println!();
            }
            return Ok(());
        }
        Commands::Upgrade { to, force, check } => {
            gravhive::upgrade::Upgrader::upgrade(to, force, check).await?;
            return Ok(());
        }
        _ => {}
    }

    // Ensure daemon is running; auto-spawn if necessary
    ensure_daemon_running().await?;

    match cli.command {
        Commands::Cron { action } => {
            match action.unwrap_or(CronAction::List) {
                CronAction::List => {
                    let resp = IpcClient::send_request(Request::CronList).await?;
                    if let Response::CronList { jobs } = resp {
                        if jobs.is_empty() {
                            println!("\nNo scheduled cron jobs found.\nAdd one in ghive.toml using `cron = \"...\"` or `ghive start --cron \"...\"`\n");
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
                                let next_str = if j.paused {
                                    "Paused".to_string()
                                } else {
                                    j.next_run
                                        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                                        .unwrap_or("-".into())
                                };
                                let last_str = j
                                    .last_run
                                    .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                                    .unwrap_or("-".into());

                                let status_cell = if j.paused || j.last_status == "Stopped" {
                                    Cell::new(&j.last_status).fg(TableColor::Yellow)
                                } else if j.last_status == "Success" {
                                    Cell::new(&j.last_status).fg(TableColor::Green)
                                } else if j.last_status.starts_with("Failed") || j.last_status.starts_with("Error") {
                                    Cell::new(&j.last_status).fg(TableColor::Red)
                                } else {
                                    Cell::new(&j.last_status)
                                };

                                table.add_row(vec![
                                    Cell::new(j.name),
                                    Cell::new(j.expression),
                                    Cell::new(j.command),
                                    Cell::new(next_str),
                                    Cell::new(last_str),
                                    Cell::new(j.runs_count),
                                    status_cell,
                                ]);
                            }
                            println!("\n{}\n", table);
                        }
                    }
                }
                CronAction::Stop { name } => {
                    let resp = IpcClient::send_request(Request::CronStop(name)).await?;
                    print_response(resp);
                }
                CronAction::Restart { name } => {
                    let resp = IpcClient::send_request(Request::CronRestart(name)).await?;
                    print_response(resp);
                }
                CronAction::Delete { name } => {
                    let resp = IpcClient::send_request(Request::CronDelete(name)).await?;
                    print_response(resp);
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
            cron,
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

                let mem_limit = max_memory_restart.as_deref().and_then(parse_memory_string);

                let mut env_map = HashMap::new();
                for (k, v) in envs {
                    env_map.insert(k, v);
                }

                let final_cwd = cwd.or_else(|| std::env::current_dir().ok());
                let final_interpreter = interpreter.or(detected.interpreter);

                let script_path = Path::new(&detected.script);
                let final_script = if script_path.exists() {
                    if let Ok(abs) = script_path.canonicalize() {
                        abs.to_string_lossy().to_string()
                    } else {
                        detected.script
                    }
                } else {
                    detected.script
                };

                let config = ProcessConfig {
                    name: app_name,
                    script: final_script,
                    args: detected.args,
                    cwd: final_cwd,
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
                    cron: cron.clone(),
                    user: None,
                    group: None,
                    cgroup_memory_max: None,
                    cgroup_cpu_max: None,
                    cgroup_pids_max: None,
                    listen_port: None,
                    target_port: None,
                    idle_timeout_secs: None,
                };

                let is_cron = cron.is_some();
                let resp = IpcClient::send_request(Request::Start(config)).await?;
                print_response(resp);

                if is_cron {
                    println!("View all scheduled jobs anytime with: {}\n", "ghive cron".bold());
                    return Ok(());
                }
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

        Commands::Reload {
            target,
            timeout,
            delay,
        } => {
            let selector = TargetSelector::parse(&target);
            let resp = IpcClient::send_request(Request::Reload {
                target: selector,
                graceful_timeout_ms: Some(timeout),
                readiness_delay_ms: Some(delay),
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
                        entry
                            .timestamp
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string()
                            .dimmed(),
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
                    let secrets_vault =
                        gravhive::vault::SecretVault::load_vault().unwrap_or_default();
                    let secret_values: Vec<String> =
                        secrets_vault.secrets.values().cloned().collect();

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
                    println!(
                        "  Memory:         {}",
                        format_memory_bytes(info.memory_bytes)
                    );
                    println!("  Script:         {}", config.script);
                    println!("  Interpreter:    {:?}", config.interpreter);
                    println!("  Working Dir:    {:?}", config.cwd);
                    println!("  Auto Restart:   {}", config.autorestart);
                    println!(
                        "  Max Memory:     {:?}",
                        config.max_memory_restart.map(format_memory_bytes)
                    );
                    println!("  Watch:          {}", config.watch);

                    if !config.env.is_empty() {
                        println!("\n  Environment Variables:");
                        for (k, v) in &config.env {
                            let masked_val =
                                gravhive::vault::mask_sensitive_text(v, &secret_values);
                            println!("    {} = {}", k.dimmed(), masked_val);
                        }
                    }

                    if !logs_preview.is_empty() {
                        println!("\n  Recent Logs Preview:");
                        for line in logs_preview {
                            let masked_line =
                                gravhive::vault::mask_sensitive_text(&line, &secret_values);
                            println!("    {}", masked_line.dimmed());
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

        Commands::Mesh {
            action:
                MeshAction::Join {
                    coordinator,
                    token,
                    name,
                },
        } => {
            let node_name = name.unwrap_or_else(|| {
                std::env::var("HOSTNAME").unwrap_or_else(|_| "node-worker".to_string())
            });
            println!(
                "{} Joining Gravhive cluster coordinator at {} as '{}'...",
                "●".cyan().bold(),
                coordinator,
                node_name
            );
            gravhive::mesh::MeshAgent::run_agent(coordinator, node_name, token, 3).await?;
        }

        Commands::Profile {
            target,
            duration,
            output,
            mode,
        } => {
            let selector = TargetSelector::parse(&target);
            let resp = IpcClient::send_request(Request::Describe(selector)).await?;
            let (pid, app_name) = match resp {
                Response::ProcessDetails { info, .. } => {
                    if info.pid == 0 || info.status != ProcessStatus::Online {
                        anyhow::bail!(
                            "Process '{}' is not online (PID: {}). Only running processes can be profiled.",
                            info.name,
                            info.pid
                        );
                    }
                    (info.pid, info.name)
                }
                Response::Error { message } => anyhow::bail!("{}", message),
                _ => anyhow::bail!("Could not retrieve process info for target '{}'", target),
            };

            let out_path = output.unwrap_or_else(|| {
                let ts = chrono::Utc::now().timestamp();
                PathBuf::from(format!("profile-{}-{}.svg", app_name, ts))
            });

            let prof_mode = match mode.to_lowercase().as_str() {
                "memory" | "mem" => gravhive::profile::ProfileMode::Memory,
                _ => gravhive::profile::ProfileMode::Cpu,
            };

            println!(
                "{} Profiling [{:?}] for '{}' (PID: {}) for {} seconds...",
                "●".cyan().bold(),
                prof_mode,
                app_name.bold(),
                pid,
                duration
            );

            gravhive::profile::Profiler::profile_process(
                pid, &app_name, duration, &out_path, prof_mode,
            )
            .await?;

            println!(
                "\n{} Profiling completed! Flamegraph SVG generated at:\n  {}",
                "✓".green().bold(),
                out_path.display().to_string().bold()
            );
        }

        Commands::Kill => {
            let resp = IpcClient::send_request(Request::Shutdown).await?;
            print_response(resp);
        }

        Commands::Ping => {
            IpcClient::ping().await?;
            println!(
                "{} Gravhive daemon is alive and responsive.",
                "✓".green().bold()
            );
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
            println!(
                "{} Gravhive daemon spawned successfully.",
                "✓".green().bold()
            );
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
                Cell::new(if p.pid == 0 {
                    "-".to_string()
                } else {
                    p.pid.to_string()
                }),
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
