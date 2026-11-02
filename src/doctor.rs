use colored::*;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct DiagnosticItem {
    pub category: String,
    pub name: String,
    pub status: DiagnosticStatus,
    pub message: String,
    pub recommendation: Option<String>,
}

pub struct Doctor;

impl Doctor {
    pub async fn run_diagnostics() -> Vec<DiagnosticItem> {
        let mut items = Vec::new();

        // 1. Daemon & IPC Checks
        items.push(Self::check_daemon().await);
        items.push(Self::check_socket_path());

        // 2. Security & Vault Checks
        items.push(Self::check_master_key());
        items.push(Self::check_alerts_config());

        // 3. System Resources & Kernel Checks
        items.extend(Self::check_system_resources());

        // 4. Runtime & Interpreter Checks
        items.extend(Self::check_runtimes());

        // 5. Storage & Logs Checks
        items.push(Self::check_logs_storage());

        // 6. Metrics Port Check
        items.push(Self::check_metrics_port());

        items
    }

    async fn check_daemon() -> DiagnosticItem {
        let ping_start = std::time::Instant::now();
        match crate::ipc::IpcClient::ping().await {
            Ok(_) => DiagnosticItem {
                category: "Daemon".to_string(),
                name: "Daemon Connectivity".to_string(),
                status: DiagnosticStatus::Pass,
                message: format!(
                    "Daemon ghived is responsive (ping: {}ms)",
                    ping_start.elapsed().as_millis()
                ),
                recommendation: None,
            },
            Err(_) => DiagnosticItem {
                category: "Daemon".to_string(),
                name: "Daemon Connectivity".to_string(),
                status: DiagnosticStatus::Warn,
                message: "Daemon is currently not running in background".to_string(),
                recommendation: Some(
                    "Start an application with `ghive start` to automatically spawn the daemon."
                        .to_string(),
                ),
            },
        }
    }

    fn check_socket_path() -> DiagnosticItem {
        let sock = crate::common::socket_path();
        if sock.exists() {
            DiagnosticItem {
                category: "Daemon".to_string(),
                name: "IPC Socket File".to_string(),
                status: DiagnosticStatus::Pass,
                message: format!("Socket exists at {}", sock.display()),
                recommendation: None,
            }
        } else {
            DiagnosticItem {
                category: "Daemon".to_string(),
                name: "IPC Socket File".to_string(),
                status: DiagnosticStatus::Pass,
                message: "Socket file ready to be created upon daemon startup".to_string(),
                recommendation: None,
            }
        }
    }

    fn check_master_key() -> DiagnosticItem {
        let key_file = crate::common::gravhive_dir().join("master.key");
        if key_file.exists() {
            if let Ok(metadata) = std::fs::metadata(&key_file) {
                let len = metadata.len();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = metadata.permissions().mode() & 0o777;
                    if mode > 0o600 {
                        return DiagnosticItem {
                            category: "Security".to_string(),
                            name: "Vault Master Key".to_string(),
                            status: DiagnosticStatus::Warn,
                            message: format!(
                                "Master key permissions are overly permissive (mode {:o}, recommended 0600)",
                                mode
                            ),
                            recommendation: Some(
                                "Run `chmod 600 ~/.gravhive/master.key` to restrict access."
                                    .to_string(),
                            ),
                        };
                    }
                }
                if len == 32 || len == 64 {
                    return DiagnosticItem {
                        category: "Security".to_string(),
                        name: "Vault Master Key".to_string(),
                        status: DiagnosticStatus::Pass,
                        message: format!("Master encryption key valid ({} bytes AES-256)", len),
                        recommendation: None,
                    };
                }
            }
            DiagnosticItem {
                category: "Security".to_string(),
                name: "Vault Master Key".to_string(),
                status: DiagnosticStatus::Warn,
                message: "Master key exists but has non-standard byte length".to_string(),
                recommendation: Some("Verify ~/.gravhive/master.key or remove it to regenerate a secure 32-byte key.".to_string()),
            }
        } else {
            DiagnosticItem {
                category: "Security".to_string(),
                name: "Vault Master Key".to_string(),
                status: DiagnosticStatus::Pass,
                message: "Master key will be generated automatically on first vault operation"
                    .to_string(),
                recommendation: None,
            }
        }
    }

    fn check_alerts_config() -> DiagnosticItem {
        let path = crate::common::alerts_config_path();
        if path.exists() {
            DiagnosticItem {
                category: "Alerting".to_string(),
                name: "Webhook Configuration".to_string(),
                status: DiagnosticStatus::Pass,
                message: format!("Webhook configuration file present at {}", path.display()),
                recommendation: None,
            }
        } else {
            DiagnosticItem {
                category: "Alerting".to_string(),
                name: "Webhook Configuration".to_string(),
                status: DiagnosticStatus::Pass,
                message: "No global webhooks configured (optional). Run `ghive webhook set <URL>` to add one.".to_string(),
                recommendation: None,
            }
        }
    }

    fn check_system_resources() -> Vec<DiagnosticItem> {
        let mut list = Vec::new();
        use sysinfo::System;
        let mut sys = System::new_all();
        sys.refresh_memory();

        let total_ram_mb = sys.total_memory() / (1024 * 1024);
        let avail_ram_mb = sys.available_memory() / (1024 * 1024);
        let cpu_count = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(1);

        list.push(DiagnosticItem {
            category: "System".to_string(),
            name: "CPU Cores".to_string(),
            status: DiagnosticStatus::Pass,
            message: format!("{} logical CPU cores available for cluster mode", cpu_count),
            recommendation: None,
        });

        if avail_ram_mb < 200 {
            list.push(DiagnosticItem {
                category: "System".to_string(),
                name: "RAM Capacity".to_string(),
                status: DiagnosticStatus::Warn,
                message: format!("Total: {} MB, Available: {} MB (Low Memory)", total_ram_mb, avail_ram_mb),
                recommendation: Some("Server memory is running low. Monitor with `ghive monit` and set memory limits.".to_string()),
            });
        } else {
            list.push(DiagnosticItem {
                category: "System".to_string(),
                name: "RAM Capacity".to_string(),
                status: DiagnosticStatus::Pass,
                message: format!("Total: {} MB, Available: {} MB", total_ram_mb, avail_ram_mb),
                recommendation: None,
            });
        }

        // File descriptor limit check on Unix
        #[cfg(unix)]
        {
            if let Ok((soft, _hard)) =
                nix::sys::resource::getrlimit(nix::sys::resource::Resource::RLIMIT_NOFILE)
            {
                if soft < 1024 {
                    list.push(DiagnosticItem {
                        category: "System".to_string(),
                        name: "File Descriptor Limit".to_string(),
                        status: DiagnosticStatus::Warn,
                        message: format!("ulimit -n soft limit is currently {} (recommended >= 1024)", soft),
                        recommendation: Some("Increase file descriptor limit via `ulimit -n 65535` for high-concurrency clusters.".to_string()),
                    });
                } else {
                    list.push(DiagnosticItem {
                        category: "System".to_string(),
                        name: "File Descriptor Limit".to_string(),
                        status: DiagnosticStatus::Pass,
                        message: format!("ulimit -n is adequate ({} open files allowed)", soft),
                        recommendation: None,
                    });
                }
            }
        }

        // Linux cgroups v2 check
        #[cfg(target_os = "linux")]
        {
            let cgroups_available = crate::isolation::CgroupManager::is_cgroups_v2_available();
            if cgroups_available {
                list.push(DiagnosticItem {
                    category: "Kernel".to_string(),
                    name: "Linux cgroups v2".to_string(),
                    status: DiagnosticStatus::Pass,
                    message: "cgroups v2 controller active for kernel memory & CPU throttling"
                        .to_string(),
                    recommendation: None,
                });
            } else {
                list.push(DiagnosticItem {
                    category: "Kernel".to_string(),
                    name: "Linux cgroups v2".to_string(),
                    status: DiagnosticStatus::Warn,
                    message: "cgroups v2 not detected; user-space memory watchdog will be used"
                        .to_string(),
                    recommendation: Some(
                        "Mount cgroup2 on systemd hosts if kernel hard limits are desired."
                            .to_string(),
                    ),
                });
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            list.push(DiagnosticItem {
                category: "Kernel".to_string(),
                name: "Process Isolation".to_string(),
                status: DiagnosticStatus::Pass,
                message:
                    "User-space resource watchdog active (standard for macOS / non-Linux hosts)"
                        .to_string(),
                recommendation: None,
            });
        }

        list
    }

    fn check_runtimes() -> Vec<DiagnosticItem> {
        let runtimes = [
            ("Node.js", "node", "--version"),
            ("Bun", "bun", "--version"),
            ("Deno", "deno", "--version"),
            ("Python 3", "python3", "--version"),
            ("Rust / Cargo", "cargo", "--version"),
            ("Docker", "docker", "--version"),
            ("PHP", "php", "-v"),
        ];

        let mut items = Vec::new();
        for (label, bin, arg) in runtimes {
            match Command::new(bin).arg(arg).output() {
                Ok(output) if output.status.success() => {
                    let version_str = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    items.push(DiagnosticItem {
                        category: "Runtimes".to_string(),
                        name: label.to_string(),
                        status: DiagnosticStatus::Pass,
                        message: if version_str.is_empty() {
                            format!("Installed ({})", bin)
                        } else {
                            format!("Installed ({})", version_str)
                        },
                        recommendation: None,
                    });
                }
                _ => {
                    items.push(DiagnosticItem {
                        category: "Runtimes".to_string(),
                        name: label.to_string(),
                        status: DiagnosticStatus::Pass,
                        message: "Not found in $PATH (optional)".to_string(),
                        recommendation: None,
                    });
                }
            }
        }
        items
    }

    fn check_logs_storage() -> DiagnosticItem {
        let logs_dir = crate::common::logs_dir();
        if logs_dir.exists() {
            let mut total_size = 0u64;
            let mut file_count = 0usize;
            if let Ok(entries) = std::fs::read_dir(&logs_dir) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_file() {
                            total_size += meta.len();
                            file_count += 1;
                        }
                    }
                }
            }

            let size_mb = total_size / (1024 * 1024);
            if size_mb > 1000 {
                DiagnosticItem {
                    category: "Storage".to_string(),
                    name: "Log Storage Capacity".to_string(),
                    status: DiagnosticStatus::Warn,
                    message: format!("Log directory contains {} files ({} MB)", file_count, size_mb),
                    recommendation: Some("Run `ghive prune` or `ghive logrotate all` to compress or remove stale logs.".to_string()),
                }
            } else {
                DiagnosticItem {
                    category: "Storage".to_string(),
                    name: "Log Storage Capacity".to_string(),
                    status: DiagnosticStatus::Pass,
                    message: format!(
                        "{} files using {} MB in {}",
                        file_count,
                        size_mb,
                        logs_dir.display()
                    ),
                    recommendation: None,
                }
            }
        } else {
            DiagnosticItem {
                category: "Storage".to_string(),
                name: "Log Storage Capacity".to_string(),
                status: DiagnosticStatus::Pass,
                message: "Log storage directory initialized".to_string(),
                recommendation: None,
            }
        }
    }

    fn check_metrics_port() -> DiagnosticItem {
        match std::net::TcpListener::bind("127.0.0.1:9615") {
            Ok(_) => DiagnosticItem {
                category: "Networking".to_string(),
                name: "Prometheus Port (9615)".to_string(),
                status: DiagnosticStatus::Pass,
                message: "Port 9615 is available for `ghive metrics`".to_string(),
                recommendation: None,
            },
            Err(_) => DiagnosticItem {
                category: "Networking".to_string(),
                name: "Prometheus Port (9615)".to_string(),
                status: DiagnosticStatus::Pass,
                message: "Port 9615 is currently in use or serving metrics".to_string(),
                recommendation: None,
            },
        }
    }

    pub fn print_report(items: &[DiagnosticItem]) {
        println!(
            "\n{}",
            "=== GravHive System & Runtime Diagnostics ==="
                .bold()
                .cyan()
        );

        let mut current_cat = String::new();
        let mut pass_count = 0;
        let mut warn_count = 0;
        let mut fail_count = 0;

        for item in items {
            if item.category != current_cat {
                current_cat = item.category.clone();
                println!("\n  {}", current_cat.bold());
            }

            let icon = match item.status {
                DiagnosticStatus::Pass => {
                    pass_count += 1;
                    "✓".green().bold()
                }
                DiagnosticStatus::Warn => {
                    warn_count += 1;
                    "!".yellow().bold()
                }
                DiagnosticStatus::Fail => {
                    fail_count += 1;
                    "✗".red().bold()
                }
            };

            println!("    {} {}: {}", icon, item.name.bold(), item.message);
            if let Some(ref rec) = item.recommendation {
                println!("      {} {}", "↳ Recommendation:".dimmed(), rec.cyan());
            }
        }

        println!(
            "\n{}",
            "-------------------------------------------------".dimmed()
        );
        print!("  Diagnostic Summary: ");
        print!("{} passed", pass_count.to_string().green().bold());
        if warn_count > 0 {
            print!(", {}", format!("{} warnings", warn_count).yellow().bold());
        }
        if fail_count > 0 {
            print!(", {}", format!("{} failures", fail_count).red().bold());
        }
        println!("\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_diagnostics_produces_items() {
        let items = Doctor::run_diagnostics().await;
        assert!(!items.is_empty());
        assert!(items.iter().any(|i| i.category == "System"));
        assert!(items.iter().any(|i| i.category == "Runtimes"));
    }
}
