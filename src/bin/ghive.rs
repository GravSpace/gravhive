use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use colored::*;
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::{Cell, Color as TableColor, ContentArrangement, Table};
use gravhive::alerts::{
    mask_webhook_url, AlertConfig, AlertEvent, AlertManager, DiscordConfig, SlackConfig,
    TelegramConfig, WebhookConfig,
};
use gravhive::common::{ensure_dirs, socket_path};
use gravhive::config::{generate_ecosystem_template, parse_config_file};
use gravhive::detector::detect_runtime;
use gravhive::ipc::IpcClient;
use gravhive::model::{
    format_duration, format_memory_bytes, parse_memory_string, ExecMode, LogStreamType,
    ProcessConfig, ProcessStatus, TargetSelector,
};
use gravhive::process::ProcessManager;
use gravhive::protocol::{Request, Response};
use gravhive::ring_buffer::LogHub;
use gravhive::startup::{generate_startup_script, remove_startup_script};
use gravhive::tui::run_monit;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

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

        /// Load environment variables from file (.env or encrypted .env.enc)
        #[arg(long)]
        env_file: Option<PathBuf>,

        /// Run in foreground mode without connecting to background daemon (ideal for containers)
        #[arg(long)]
        no_daemon: bool,

        /// Disable automatic restart upon process exit
        #[arg(long)]
        no_autorestart: bool,

        /// Arguments passed to the target script
        #[arg(last = true)]
        args: Vec<String>,

        /// Scheduled cron expression (e.g. "0 */5 * * * *" or "0 2 * * *")
        #[arg(long)]
        cron: Option<String>,

        /// Timezone for scheduled cron expression (e.g. "Asia/Jakarta", "UTC", "America/New_York", or "local")
        #[arg(long)]
        cron_timezone: Option<String>,
    },

    /// Run an application directly in foreground mode (ideal for Docker containers as PID 1)
    Run {
        /// Target script file (e.g. app.js, main.py), executable, or ecosystem config (e.g. ghive.toml)
        target: String,

        /// Number of worker instances (e.g. 1, 4, or 'max')
        #[arg(short = 'i', long)]
        instances: Option<String>,

        /// Custom process name
        #[arg(short, long)]
        name: Option<String>,

        /// Watch source directory for file modifications and auto-reload
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

        /// Load environment variables from file (.env or encrypted .env.enc)
        #[arg(long)]
        env_file: Option<PathBuf>,

        /// Disable automatic restart upon process exit
        #[arg(long)]
        no_autorestart: bool,

        /// Arguments passed to the target script
        #[arg(last = true)]
        args: Vec<String>,

        /// Scheduled cron expression (e.g. "0 */5 * * * *" or "0 2 * * *")
        #[arg(long)]
        cron: Option<String>,

        /// Timezone for scheduled cron expression (e.g. "Asia/Jakarta", "UTC", "America/New_York", or "local")
        #[arg(long)]
        cron_timezone: Option<String>,
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

    /// Manage webhook alerts (Discord, Slack, Telegram, Custom HTTP)
    #[command(alias = "alert", alias = "alerts")]
    Webhook {
        #[command(subcommand)]
        action: Option<WebhookAction>,
    },

    /// Scale application worker instances dynamically up or down
    Scale {
        /// Target application name
        app: String,

        /// Target instances count or delta (e.g. 4, +2, or -1)
        #[arg(allow_hyphen_values = true)]
        instances: String,
    },

    /// Trigger log rotation and gzip compression for application log files
    Logrotate {
        /// Target process ID, name, or 'all' (default: all)
        #[arg(default_value = "all")]
        target: String,
    },

    /// Expose Prometheus metrics on HTTP endpoint (default localhost:9615)
    Metrics {
        /// Host address to bind the metrics server to (default 127.0.0.1; use 0.0.0.0 to expose externally)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to serve Prometheus metrics on
        #[arg(short, long, default_value = "9615")]
        port: u16,
    },

    /// Clean up dead processes (stopped/errored) and delete orphaned log files
    Prune {
        /// Preview what would be deleted without actually deleting
        #[arg(long)]
        dry_run: bool,

        /// Force cleanup without confirmation prompt
        #[arg(short, long)]
        force: bool,

        /// Only clean orphaned log files, leave stopped processes intact
        #[arg(long)]
        only_logs: bool,
    },

    /// Run comprehensive environment, system resource, and runtime diagnostics
    Doctor,

    /// Validate ecosystem configuration file (syntax, paths, cron, ports)
    #[command(alias = "check")]
    Validate {
        /// Path to configuration file (default: ghive.toml)
        #[arg(default_value = "ghive.toml")]
        config: PathBuf,
    },

    /// View chronological lifecycle events and audit timeline
    #[command(alias = "history")]
    Events {
        /// Target application name (optional, default: all)
        target: Option<String>,

        /// Number of events to display
        #[arg(short = 'n', long, default_value = "30")]
        limit: usize,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum WebhookAction {
    /// Save or update a webhook endpoint URL
    Set {
        /// Webhook URL (or bot token for Telegram)
        url: String,

        /// Webhook provider type: discord, slack, telegram, or generic (auto-detected if omitted)
        #[arg(short = 't', long)]
        r#type: Option<WebhookType>,

        /// Custom username for Discord embed
        #[arg(long)]
        username: Option<String>,

        /// Custom channel for Slack
        #[arg(long)]
        channel: Option<String>,

        /// Chat ID for Telegram
        #[arg(long)]
        chat_id: Option<String>,

        /// Custom HTTP header in 'Header: Value' format (Generic Webhook, repeatable)
        #[arg(short = 'H', long = "header", value_parser = parse_header_key_val)]
        headers: Vec<(String, String)>,

        /// Also note in local ghive.toml file
        #[arg(long)]
        save_to_config: bool,
    },

    /// Show currently configured webhook(s)
    #[command(alias = "show", alias = "ls")]
    List {
        /// Unmask full webhook URLs (security sensitive)
        #[arg(long)]
        show_full: bool,
    },

    /// Send a test alert event to verify webhook configuration
    Test {
        /// Custom message text for test alert
        #[arg(short, long, default_value = "Test notification from Gravhive")]
        message: String,
    },

    /// Remove a configured webhook
    #[command(alias = "rm")]
    Delete {
        /// Provider to remove: discord, slack, telegram, generic, or all
        #[arg(default_value = "all")]
        provider: String,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebhookType {
    Discord,
    Slack,
    Telegram,
    Generic,
}

#[derive(Subcommand, Debug, Clone)]
pub enum CronAction {
    /// List all scheduled cron jobs (default)
    #[command(alias = "ls")]
    List,

    /// View or get current cron timezone setting
    #[command(alias = "tz")]
    Timezone {
        /// Optional timezone to set (e.g. 'Asia/Jakarta', 'UTC', 'America/New_York', 'local')
        timezone: Option<String>,
    },

    /// Set timezone for cron scheduling (e.g. 'Asia/Jakarta', 'UTC', 'America/New_York', 'local')
    #[command(alias = "set-tz")]
    SetTimezone {
        /// Timezone identifier (e.g. 'Asia/Jakarta', 'UTC', 'America/New_York', 'local')
        timezone: String,
    },

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

fn parse_header_key_val(s: &str) -> std::result::Result<(String, String), String> {
    if let Some(pos) = s.find(':') {
        Ok((s[..pos].trim().to_string(), s[pos + 1..].trim().to_string()))
    } else if let Some(pos) = s.find('=') {
        Ok((s[..pos].trim().to_string(), s[pos + 1..].trim().to_string()))
    } else {
        Err(format!(
            "Invalid header format: expected 'Header: Value' or 'Header=Value', got `{}`",
            s
        ))
    }
}

fn detect_webhook_type(url: &str) -> WebhookType {
    let lower = url.to_lowercase();
    if lower.contains("discord.com/api/webhooks") {
        WebhookType::Discord
    } else if lower.contains("hooks.slack.com") {
        WebhookType::Slack
    } else if lower.contains("api.telegram.org") {
        WebhookType::Telegram
    } else {
        WebhookType::Generic
    }
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
        Commands::Webhook { action } => {
            let action = action.unwrap_or(WebhookAction::List { show_full: false });
            handle_webhook_command(action).await?;
            return Ok(());
        }
        Commands::Metrics { host, port } => {
            gravhive::metrics::start_metrics_server(&host, port).await?;
            return Ok(());
        }
        Commands::Doctor => {
            let items = gravhive::doctor::Doctor::run_diagnostics().await;
            gravhive::doctor::Doctor::print_report(&items);
            return Ok(());
        }
        Commands::Validate { config } => {
            gravhive::validator::ConfigValidator::validate_and_exit(&config)?;
            return Ok(());
        }
        Commands::Run {
            target,
            instances,
            name,
            watch,
            max_memory_restart,
            cwd,
            interpreter,
            envs,
            env_file,
            no_autorestart,
            args,
            cron,
            cron_timezone,
        } => {
            let configs = prepare_process_configs(
                &target,
                name,
                instances,
                watch,
                max_memory_restart,
                cwd,
                interpreter,
                envs,
                env_file,
                no_autorestart,
                args,
                cron,
                cron_timezone,
            )?;
            run_foreground(configs).await?;
            return Ok(());
        }
        Commands::Start {
            ref target,
            ref instances,
            ref name,
            watch,
            ref max_memory_restart,
            ref cwd,
            ref interpreter,
            ref envs,
            ref env_file,
            no_daemon,
            no_autorestart,
            ref args,
            ref cron,
            ref cron_timezone,
        } if no_daemon => {
            let configs = prepare_process_configs(
                target,
                name.clone(),
                instances.clone(),
                watch,
                max_memory_restart.clone(),
                cwd.clone(),
                interpreter.clone(),
                envs.clone(),
                env_file.clone(),
                no_autorestart,
                args.clone(),
                cron.clone(),
                cron_timezone.clone(),
            )?;
            run_foreground(configs).await?;
            return Ok(());
        }
        _ => {}
    }

    // Ensure daemon is running; auto-spawn if necessary
    ensure_daemon_running().await?;

    match cli.command {
        Commands::Cron { action } => match action.unwrap_or(CronAction::List) {
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
                                Cell::new("timezone").fg(TableColor::Cyan),
                                Cell::new("command").fg(TableColor::Cyan),
                                Cell::new("next run").fg(TableColor::Cyan),
                                Cell::new("last run").fg(TableColor::Cyan),
                                Cell::new("runs").fg(TableColor::Cyan),
                                Cell::new("status").fg(TableColor::Cyan),
                            ]);

                        for j in jobs {
                            let tz_resolved = gravhive::cron::resolve_timezone(&j.timezone).ok();
                            let next_str = if j.paused {
                                "Paused".to_string()
                            } else {
                                j.next_run
                                    .map(|d| {
                                        if let Some(tz) = tz_resolved {
                                            d.with_timezone(&tz)
                                                .format("%Y-%m-%d %H:%M:%S")
                                                .to_string()
                                        } else {
                                            d.format("%Y-%m-%d %H:%M:%S").to_string()
                                        }
                                    })
                                    .unwrap_or("-".into())
                            };
                            let last_str = j
                                .last_run
                                .map(|d| {
                                    if let Some(tz) = tz_resolved {
                                        d.with_timezone(&tz)
                                            .format("%Y-%m-%d %H:%M:%S")
                                            .to_string()
                                    } else {
                                        d.format("%Y-%m-%d %H:%M:%S").to_string()
                                    }
                                })
                                .unwrap_or("-".into());

                            let status_cell = if j.paused || j.last_status == "Stopped" {
                                Cell::new(&j.last_status).fg(TableColor::Yellow)
                            } else if j.last_status == "Success" {
                                Cell::new(&j.last_status).fg(TableColor::Green)
                            } else if j.last_status.starts_with("Failed")
                                || j.last_status.starts_with("Error")
                            {
                                Cell::new(&j.last_status).fg(TableColor::Red)
                            } else {
                                Cell::new(&j.last_status)
                            };

                            table.add_row(vec![
                                Cell::new(j.name),
                                Cell::new(j.expression),
                                Cell::new(j.timezone).fg(TableColor::DarkGrey),
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
            CronAction::Timezone { timezone } => {
                if let Some(tz) = timezone {
                    let resp = IpcClient::send_request(Request::CronSetTimezone(tz)).await?;
                    print_response(resp);
                } else {
                    let resp = IpcClient::send_request(Request::CronGetTimezone).await?;
                    match resp {
                        Response::CronTimezone { timezone, offset } => {
                            println!(
                                "\n  {} Current cron timezone: {} ({})",
                                "⏰".cyan(),
                                timezone.bold(),
                                offset
                            );
                            println!(
                                "     To change: {} {}\n",
                                "ghive cron set-timezone".cyan(),
                                "<TIMEZONE>".dimmed()
                            );
                        }
                        other => print_response(other),
                    }
                }
            }
            CronAction::SetTimezone { timezone } => {
                let resp = IpcClient::send_request(Request::CronSetTimezone(timezone)).await?;
                print_response(resp);
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
        },

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
            env_file,
            no_daemon: _,
            no_autorestart,
            args,
            cron,
            cron_timezone,
        } => {
            let is_cron = cron.is_some();
            let configs = prepare_process_configs(
                &target,
                name,
                instances,
                watch,
                max_memory_restart,
                cwd,
                interpreter,
                envs,
                env_file,
                no_autorestart,
                args,
                cron,
                cron_timezone,
            )?;

            for cfg in configs {
                let resp = IpcClient::send_request(Request::Start(cfg)).await?;
                print_response(resp);
            }

            if is_cron {
                println!(
                    "View all scheduled jobs anytime with: {}\n",
                    "ghive cron".bold()
                );
                return Ok(());
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

        Commands::Scale { app, instances } => {
            let resp = IpcClient::send_request(Request::Scale {
                app: app.clone(),
                instances,
            })
            .await?;
            match resp {
                Response::ScaleResult {
                    app,
                    previous,
                    current,
                } => {
                    let diff = if current >= previous {
                        format!("(+{} workers)", current - previous).green()
                    } else {
                        format!("(-{} workers)", previous - current).yellow()
                    };
                    println!(
                        "{} App '{}' scaled from {} to {} instances {}",
                        "✓".green().bold(),
                        app.bold(),
                        previous,
                        current.to_string().bold(),
                        diff
                    );
                    print_process_table(false).await?;
                }
                Response::Error { message } => eprintln!("{} {}", "✗".red().bold(), message),
                _ => {}
            }
        }

        Commands::Logrotate { target } => {
            let selector = TargetSelector::parse(&target);
            let resp = IpcClient::send_request(Request::LogRotate(selector)).await?;
            print_response(resp);
        }

        Commands::Prune {
            dry_run,
            force: _,
            only_logs,
        } => {
            let resp = IpcClient::send_request(Request::Prune { dry_run, only_logs }).await?;
            if let Response::PruneResult {
                removed_processes,
                deleted_log_files,
                freed_bytes,
            } = resp
            {
                println!(
                    "\n{} Prune Results {}:",
                    "●".cyan().bold(),
                    if dry_run {
                        "(Dry Run - No files or processes deleted)".yellow().bold()
                    } else {
                        "".normal()
                    }
                );
                if removed_processes.is_empty() && deleted_log_files.is_empty() {
                    println!("  Everything is clean. No stopped/errored processes or orphaned logs found.\n");
                } else {
                    if !removed_processes.is_empty() {
                        println!(
                            "  {} {} dead process(es) {}:",
                            "•".cyan(),
                            removed_processes.len(),
                            if dry_run {
                                "would be removed"
                            } else {
                                "removed"
                            }
                        );
                        for (id, name) in &removed_processes {
                            println!("    - Worker #{} ({})", id, name.bold());
                        }
                    }
                    if !deleted_log_files.is_empty() {
                        println!(
                            "  {} {} orphaned log file(s) {} (freed {}):",
                            "•".cyan(),
                            deleted_log_files.len(),
                            if dry_run {
                                "would be deleted"
                            } else {
                                "deleted"
                            },
                            format_memory_bytes(freed_bytes).green().bold()
                        );
                        for log_file in &deleted_log_files {
                            println!("    - {}", log_file.dimmed());
                        }
                    }
                    println!();
                }
            } else {
                print_response(resp);
            }
        }

        Commands::Events {
            target,
            limit,
            json,
        } => {
            let selector = target.as_deref().map(TargetSelector::parse);
            let resp = IpcClient::send_request(Request::GetEvents {
                target: selector,
                limit,
            })
            .await?;
            if let Response::EventList { events } = resp {
                if json {
                    println!("{}", serde_json::to_string_pretty(&events)?);
                    return Ok(());
                }
                if events.is_empty() {
                    println!("\nNo recorded lifecycle events found for the specified target.\n");
                    return Ok(());
                }
                let mut table = Table::new();
                table
                    .load_preset(UTF8_ROUND_CORNERS)
                    .set_content_arrangement(ContentArrangement::Dynamic)
                    .set_header(vec![
                        Cell::new("timestamp").fg(TableColor::Cyan),
                        Cell::new("app").fg(TableColor::Cyan),
                        Cell::new("worker").fg(TableColor::Cyan),
                        Cell::new("event").fg(TableColor::Cyan),
                        Cell::new("details").fg(TableColor::Cyan),
                    ]);
                for e in events {
                    let event_cell = match e.event_type {
                        gravhive::events::EventType::Spawned => {
                            Cell::new(e.event_type).fg(TableColor::Green)
                        }
                        gravhive::events::EventType::Crashed => {
                            Cell::new(e.event_type).fg(TableColor::Red)
                        }
                        gravhive::events::EventType::Restarted
                        | gravhive::events::EventType::Reloaded => {
                            Cell::new(e.event_type).fg(TableColor::Cyan)
                        }
                        gravhive::events::EventType::Exited => {
                            Cell::new(e.event_type).fg(TableColor::DarkGrey)
                        }
                        gravhive::events::EventType::Scaled => {
                            Cell::new(e.event_type).fg(TableColor::Blue)
                        }
                        _ => Cell::new(e.event_type).fg(TableColor::Yellow),
                    };
                    table.add_row(vec![
                        Cell::new(e.timestamp.format("%Y-%m-%d %H:%M:%S").to_string()),
                        Cell::new(e.app_name),
                        Cell::new(
                            e.worker_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        event_cell,
                        Cell::new(e.details),
                    ]);
                }
                println!("\n{}\n", table);
            } else {
                print_response(resp);
            }
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

#[allow(clippy::too_many_arguments)]
fn prepare_process_configs(
    target: &str,
    name: Option<String>,
    instances: Option<String>,
    watch: bool,
    max_memory_restart: Option<String>,
    cwd: Option<PathBuf>,
    interpreter: Option<String>,
    envs: Vec<(String, String)>,
    env_file: Option<PathBuf>,
    no_autorestart: bool,
    args: Vec<String>,
    cron: Option<String>,
    cron_timezone: Option<String>,
) -> Result<Vec<ProcessConfig>> {
    let target_path = Path::new(target);

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
        let mut configs = parse_config_file(target_path)?;

        let file_envs = if let Some(ref path) = env_file {
            let loaded = gravhive::env_loader::load_env_file(path)?;
            println!(
                "{} Loaded {} environment variables from {}",
                "✓".green(),
                loaded.len(),
                path.display().to_string().bold()
            );
            Some(loaded)
        } else {
            None
        };

        for cfg in configs.iter_mut() {
            if let Some(ref f_envs) = file_envs {
                for (k, v) in f_envs {
                    cfg.env.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            for (k, v) in &envs {
                cfg.env.insert(k.clone(), v.clone());
            }
            if no_autorestart {
                cfg.autorestart = false;
            }
        }
        return Ok(configs);
    }

    // Single application direct execution
    let detected = detect_runtime(target, &args);

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
    if let Some(ref path) = env_file {
        let loaded = gravhive::env_loader::load_env_file(path)?;
        println!(
            "{} Loaded {} environment variables from {}",
            "✓".green(),
            loaded.len(),
            path.display().to_string().bold()
        );
        for (k, v) in loaded {
            env_map.insert(k, v);
        }
    }
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
        autorestart: !no_autorestart,
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
        cron,
        cron_timezone,
        user: None,
        group: None,
        cgroup_memory_max: None,
        cgroup_cpu_max: None,
        cgroup_pids_max: None,
        listen_port: None,
        target_port: None,
        idle_timeout_secs: None,
        log_rotate_max_size: None,
        log_rotate_keep: None,
        log_compress: true,
        health_check: None,
    };

    Ok(vec![config])
}

async fn run_foreground(configs: Vec<ProcessConfig>) -> Result<()> {
    println!(
        "{} GravHive container runtime starting in foreground (PID: {})...",
        "●".cyan().bold(),
        std::process::id()
    );

    let log_hub = LogHub::new(1000);
    let mut rx = log_hub.subscribe();

    // Stream logs with clean colored tags to stdout / stderr
    tokio::spawn(async move {
        while let Ok(entry) = rx.recv().await {
            let tag = format!("[{}:{}]", entry.app_name, entry.worker_id);
            if entry.stream == "err" {
                eprintln!("{} {}", tag.magenta(), entry.message);
            } else {
                println!("{} {}", tag.cyan(), entry.message);
            }
        }
    });

    let manager = Arc::new(RwLock::new(ProcessManager::new(log_hub)));

    for cfg in configs {
        let app_name = cfg.name.clone();
        let instances = cfg.instances;
        let mut mgr = manager.write().await;
        match mgr.start_app(cfg).await {
            Ok(ids) => {
                println!(
                    "{} App '{}' ({} instance{}) running in foreground with IDs: {:?}",
                    "✓".green().bold(),
                    app_name.bold(),
                    instances,
                    if instances > 1 { "s" } else { "" },
                    ids
                );
            }
            Err(e) => {
                eprintln!(
                    "{} Failed to start app '{}': {}",
                    "✗".red().bold(),
                    app_name,
                    e
                );
            }
        }
    }

    // Health checker, autorestart, and zombie process reaper
    let mgr_health = manager.clone();
    let health_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1000));
        loop {
            interval.tick().await;
            let mut mgr = mgr_health.write().await;
            mgr.check_health_and_restart().await;
        }
    });

    // Handle OS shutdown signals (SIGINT, SIGTERM) and process exit monitoring
    #[cfg(unix)]
    {
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .context("Failed to install SIGINT listener")?;
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("Failed to install SIGTERM listener")?;
        let mut check_interval = tokio::time::interval(Duration::from_millis(500));

        loop {
            tokio::select! {
                _ = sigint.recv() => {
                    println!("\n{} Received SIGINT (Ctrl+C), stopping all managed processes...", "ℹ".yellow().bold());
                    break;
                }
                _ = sigterm.recv() => {
                    println!("\n{} Received SIGTERM, stopping all managed processes...", "ℹ".yellow().bold());
                    break;
                }
                _ = check_interval.tick() => {
                    let mgr = manager.read().await;
                    let procs = mgr.list();
                    let any_alive = procs.iter().any(|p| {
                        p.status == ProcessStatus::Online || p.status == ProcessStatus::Restarting
                    });
                    if !procs.is_empty() && !any_alive {
                        println!("\n{} All child processes have exited.", "ℹ".cyan().bold());
                        break;
                    }
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        println!(
            "\n{} Received Ctrl+C, stopping all managed processes...",
            "ℹ".yellow().bold()
        );
    }

    health_task.abort();
    let mut mgr = manager.write().await;
    let stopped = mgr
        .stop(&TargetSelector::All, Duration::from_millis(3000))
        .await?;
    println!(
        "{} Gracefully stopped {} worker(s). GravHive container runtime exited cleanly.",
        "✓".green().bold(),
        stopped
    );

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

async fn handle_webhook_command(action: WebhookAction) -> Result<()> {
    match action {
        WebhookAction::Set {
            url,
            r#type,
            username,
            channel,
            chat_id,
            headers,
            save_to_config,
        } => {
            let provider_type = r#type.unwrap_or_else(|| detect_webhook_type(&url));
            let mut cfg = if IpcClient::ping().await.is_ok() {
                if let Ok(Response::AlertConfig(daemon_cfg)) =
                    IpcClient::send_request(Request::GetAlertConfig).await
                {
                    daemon_cfg
                } else {
                    AlertConfig::load_global()
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                }
            } else {
                AlertConfig::load_global()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            };

            match provider_type {
                WebhookType::Discord => {
                    cfg.discord = Some(DiscordConfig {
                        webhook_url: url.clone(),
                        username,
                    });
                }
                WebhookType::Slack => {
                    cfg.slack = Some(SlackConfig {
                        webhook_url: url.clone(),
                        channel,
                    });
                }
                WebhookType::Telegram => {
                    let cid = chat_id.ok_or_else(|| {
                        anyhow::anyhow!(
                            "--chat-id is required when configuring a Telegram webhook/bot"
                        )
                    })?;
                    cfg.telegram = Some(TelegramConfig {
                        bot_token: url.clone(),
                        chat_id: cid,
                    });
                }
                WebhookType::Generic => {
                    let mut hdrs = HashMap::new();
                    for (k, v) in headers {
                        hdrs.insert(k, v);
                    }
                    cfg.webhook = Some(WebhookConfig {
                        url: url.clone(),
                        headers: hdrs,
                    });
                }
            }

            cfg.save_global()?;
            println!(
                "{} Webhook for {} saved to {}",
                "✓".green().bold(),
                format!("{:?}", provider_type).cyan().bold(),
                gravhive::common::alerts_config_path()
                    .display()
                    .to_string()
                    .bold()
            );

            if save_to_config && Path::new("ghive.toml").exists() {
                println!(
                    "{} Local ghive.toml noted for ecosystem alerts",
                    "✓".green().bold()
                );
            }

            if IpcClient::ping().await.is_ok() {
                if let Ok(Response::Ok { message }) =
                    IpcClient::send_request(Request::UpdateAlertConfig(cfg)).await
                {
                    println!(
                        "{} Live Gravhive daemon updated: {}",
                        "✓".green().bold(),
                        message
                    );
                }
            } else {
                println!(
                    "{} Daemon is not currently running. Webhook will be active when Gravhive starts.",
                    "ℹ".cyan().bold()
                );
            }

            println!("Run `ghive webhook test` to send a test alert.");
        }

        WebhookAction::List { show_full } => {
            let cfg = if IpcClient::ping().await.is_ok() {
                if let Ok(Response::AlertConfig(daemon_cfg)) =
                    IpcClient::send_request(Request::GetAlertConfig).await
                {
                    daemon_cfg
                } else {
                    AlertConfig::load_global()
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                }
            } else {
                AlertConfig::load_global()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            };

            if cfg.is_empty() {
                println!(
                    "\nNo webhooks configured.\nUse `ghive webhook set <URL>` to add an alert endpoint.\n"
                );
                return Ok(());
            }

            let mut table = Table::new();
            table
                .load_preset(UTF8_ROUND_CORNERS)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_header(vec![
                    Cell::new("provider").fg(TableColor::Cyan),
                    Cell::new("endpoint / token").fg(TableColor::Cyan),
                    Cell::new("options").fg(TableColor::Cyan),
                    Cell::new("status").fg(TableColor::Cyan),
                ]);

            if let Some(ref d) = cfg.discord {
                let ep = if show_full {
                    d.webhook_url.clone()
                } else {
                    mask_webhook_url(&d.webhook_url)
                };
                let opt = d
                    .username
                    .as_deref()
                    .map(|u| format!("username: {}", u))
                    .unwrap_or_else(|| "-".into());
                table.add_row(vec![
                    Cell::new("Discord"),
                    Cell::new(ep),
                    Cell::new(opt),
                    Cell::new("active").fg(TableColor::Green),
                ]);
            }

            if let Some(ref s) = cfg.slack {
                let ep = if show_full {
                    s.webhook_url.clone()
                } else {
                    mask_webhook_url(&s.webhook_url)
                };
                let opt = s
                    .channel
                    .as_deref()
                    .map(|c| format!("channel: {}", c))
                    .unwrap_or_else(|| "-".into());
                table.add_row(vec![
                    Cell::new("Slack"),
                    Cell::new(ep),
                    Cell::new(opt),
                    Cell::new("active").fg(TableColor::Green),
                ]);
            }

            if let Some(ref tg) = cfg.telegram {
                let ep = if show_full {
                    tg.bot_token.clone()
                } else {
                    mask_webhook_url(&tg.bot_token)
                };
                let opt = format!("chat_id: {}", tg.chat_id);
                table.add_row(vec![
                    Cell::new("Telegram"),
                    Cell::new(ep),
                    Cell::new(opt),
                    Cell::new("active").fg(TableColor::Green),
                ]);
            }

            if let Some(ref w) = cfg.webhook {
                let ep = if show_full {
                    w.url.clone()
                } else {
                    mask_webhook_url(&w.url)
                };
                let opt = if w.headers.is_empty() {
                    "-".to_string()
                } else {
                    format!("{} custom header(s)", w.headers.len())
                };
                table.add_row(vec![
                    Cell::new("Generic Webhook"),
                    Cell::new(ep),
                    Cell::new(opt),
                    Cell::new("active").fg(TableColor::Green),
                ]);
            }

            println!("\n{}\n", table);
        }

        WebhookAction::Test { message } => {
            println!("{} Dispatching test alert event...", "●".cyan().bold());
            let reports = if IpcClient::ping().await.is_ok() {
                match IpcClient::send_request(Request::TestAlert {
                    message: message.clone(),
                })
                .await
                {
                    Ok(Response::TestAlertResult { results }) => results,
                    Ok(Response::Error { message: err }) => {
                        eprintln!("{} Failed to test alerts: {}", "✗".red().bold(), err);
                        return Ok(());
                    }
                    _ => Vec::new(),
                }
            } else {
                let cfg = AlertConfig::load_global()
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if cfg.is_empty() {
                    eprintln!(
                        "{} No webhooks configured. Run `ghive webhook set <URL>` first.",
                        "✗".red().bold()
                    );
                    return Ok(());
                }
                let alert_mgr = AlertManager::new(cfg);
                alert_mgr
                    .dispatch_and_report(AlertEvent::Test { message })
                    .await
            };

            if reports.is_empty() {
                eprintln!("{} No webhook endpoints were reached.", "✗".red().bold());
            } else {
                for (provider, success, msg) in reports {
                    if success {
                        println!("  {} {}: {}", "✓".green().bold(), provider.bold(), msg);
                    } else {
                        eprintln!("  {} {}: {}", "✗".red().bold(), provider.bold(), msg);
                    }
                }
            }
        }

        WebhookAction::Delete { provider } => {
            let mut cfg = if IpcClient::ping().await.is_ok() {
                if let Ok(Response::AlertConfig(daemon_cfg)) =
                    IpcClient::send_request(Request::GetAlertConfig).await
                {
                    daemon_cfg
                } else {
                    AlertConfig::load_global()
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                }
            } else {
                AlertConfig::load_global()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            };

            match provider.to_lowercase().as_str() {
                "discord" => {
                    cfg.discord = None;
                    println!("{} Removed Discord webhook", "✓".green().bold());
                }
                "slack" => {
                    cfg.slack = None;
                    println!("{} Removed Slack webhook", "✓".green().bold());
                }
                "telegram" => {
                    cfg.telegram = None;
                    println!("{} Removed Telegram webhook", "✓".green().bold());
                }
                "generic" | "webhook" => {
                    cfg.webhook = None;
                    println!("{} Removed Generic Webhook", "✓".green().bold());
                }
                "all" => {
                    cfg.discord = None;
                    cfg.slack = None;
                    cfg.telegram = None;
                    cfg.webhook = None;
                    println!("{} Cleared all webhook configurations", "✓".green().bold());
                }
                other => {
                    eprintln!(
                        "{} Unknown provider '{}'. Valid options: discord, slack, telegram, generic, all",
                        "✗".red().bold(),
                        other
                    );
                    return Ok(());
                }
            }

            cfg.save_global()?;
            if IpcClient::ping().await.is_ok() {
                let _ = IpcClient::send_request(Request::UpdateAlertConfig(cfg)).await;
            }
        }
    }

    Ok(())
}
