use crate::config::load_ecosystem;
use crate::model::parse_memory_string;
use anyhow::Result;
use colored::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    pub file_path: PathBuf,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub apps_checked: usize,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn print_report(&self) {
        println!(
            "\n{} Validating ecosystem configuration at {}\n",
            "●".cyan().bold(),
            self.file_path.display().to_string().bold()
        );

        if self.is_valid() {
            println!(
                "  {} Configuration syntax and schema are valid! ({} application(s) verified)",
                "✓".green().bold(),
                self.apps_checked
            );
        } else {
            println!(
                "  {} Found {} error(s) in configuration:",
                "✗".red().bold(),
                self.errors.len()
            );
            for err in &self.errors {
                println!("    {} {}", "•".red().bold(), err);
            }
        }

        if !self.warnings.is_empty() {
            println!("\n  {} Warnings (recommendations):", "!".yellow().bold());
            for warn in &self.warnings {
                println!("    {} {}", "•".yellow().bold(), warn);
            }
        }

        println!();
    }
}

pub struct ConfigValidator;

impl ConfigValidator {
    pub fn validate_file(path: &Path) -> ValidationReport {
        let mut report = ValidationReport {
            file_path: path.to_path_buf(),
            errors: Vec::new(),
            warnings: Vec::new(),
            apps_checked: 0,
        };

        if !path.exists() {
            report.errors.push(format!(
                "Configuration file does not exist: {}",
                path.display()
            ));
            return report;
        }

        let configs = match load_ecosystem(path) {
            Ok((apps, _, _)) => apps,
            Err(e) => {
                report
                    .errors
                    .push(format!("Failed to parse configuration file: {}", e));
                return report;
            }
        };

        report.apps_checked = configs.len();
        if configs.is_empty() {
            report
                .warnings
                .push("No application blocks ([app.<name>]) defined in configuration.".to_string());
            return report;
        }

        if let Some(eco_tz) = crate::config::load_cron_config_from_ecosystem(path) {
            if let Err(e) = crate::cron::resolve_timezone(&eco_tz) {
                report.errors.push(format!(
                    "[cron] Invalid global cron timezone '{}': {}",
                    eco_tz, e
                ));
            }
        }

        let mut bound_ports: HashMap<u16, String> = HashMap::new();

        for config in configs {
            let app_name = &config.name;

            // 1. Script existence check
            let script_path = if let Some(ref cwd) = config.cwd {
                cwd.join(&config.script)
            } else {
                PathBuf::from(&config.script)
            };

            if !script_path.exists() {
                // Check if script is a command available in $PATH
                let first_token = config
                    .script
                    .split_whitespace()
                    .next()
                    .unwrap_or(&config.script);
                let in_path = Command::new("which")
                    .arg(first_token)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);

                if !in_path {
                    report.errors.push(format!(
                        "[app.{}] Script file or command '{}' does not exist in filesystem or $PATH.",
                        app_name, config.script
                    ));
                }
            }

            // 2. Interpreter check
            if let Some(ref interp) = config.interpreter {
                let interp_in_path = Command::new("which")
                    .arg(interp)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);

                if !interp_in_path {
                    report.warnings.push(format!(
                        "[app.{}] Custom interpreter '{}' is not found in system $PATH.",
                        app_name, interp
                    ));
                }
            }

            // 3. Cron expression check
            if let Some(ref cron_expr) = config.cron {
                let normalized = if cron_expr.split_whitespace().count() == 5 {
                    format!("0 {}", cron_expr)
                } else {
                    cron_expr.to_string()
                };

                match cron::Schedule::from_str(&normalized) {
                    Ok(_) => {}
                    Err(e) => {
                        report.errors.push(format!(
                            "[app.{}] Invalid cron expression '{}': {}",
                            app_name, cron_expr, e
                        ));
                    }
                }
            }

            // 3b. Cron timezone check
            if let Some(ref tz_str) = config.cron_timezone {
                if let Err(e) = crate::cron::resolve_timezone(tz_str) {
                    report.errors.push(format!(
                        "[app.{}] Invalid cron timezone '{}': {}",
                        app_name, tz_str, e
                    ));
                }
            }

            // 4. Memory limit format check
            if let Some(ref mem_str) = config.max_memory_restart {
                let s = mem_str.to_string();
                if parse_memory_string(&s).is_none() {
                    report.errors.push(format!(
                        "[app.{}] Invalid memory limit format '{}' (expected e.g. '300MB', '1GB').",
                        app_name, mem_str
                    ));
                }
            }

            // 5. Port collision check
            if let Some(port) = config.listen_port {
                if let Some(existing_app) = bound_ports.get(&port) {
                    report.errors.push(format!(
                        "Port collision: [app.{}] and [app.{}] both define listen_port = {}.",
                        existing_app, app_name, port
                    ));
                } else {
                    bound_ports.insert(port, app_name.clone());
                }
            }

            // 6. Health check format check
            if let Some(ref hc) = config.health_check {
                if let Some(ref url) = hc.http_path {
                    if !url.starts_with("http://")
                        && !url.starts_with("https://")
                        && !url.starts_with('/')
                    {
                        report.warnings.push(format!(
                            "[app.{}] Health check http_path '{}' should start with 'http://', 'https://', or '/'.",
                            app_name, url
                        ));
                    }
                }
            }
        }

        report
    }

    pub fn validate_and_exit(path: &Path) -> Result<()> {
        let report = Self::validate_file(path);
        report.print_report();
        if !report.is_valid() {
            std::process::exit(1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_nonexistent_file() {
        let report = ConfigValidator::validate_file(Path::new("non_existent_file.toml"));
        assert!(!report.is_valid());
        assert!(!report.errors.is_empty());
    }

    #[test]
    fn test_validate_valid_config() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join(format!("test_validate_{}.toml", std::process::id()));
        let content = r#"
[app.my-service]
script = "echo"
args = ["hello"]
instances = 2
cron = "0 */5 * * * *"
max_memory_restart = "256MB"
listen_port = 8888
"#;
        std::fs::write(&path, content).unwrap();

        let report = ConfigValidator::validate_file(&path);
        let _ = std::fs::remove_file(path);

        assert!(report.is_valid(), "Errors: {:?}", report.errors);
        assert_eq!(report.apps_checked, 1);
    }

    #[test]
    fn test_validate_port_collision_and_bad_cron() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join(format!("test_collision_{}.toml", std::process::id()));
        let content = r#"
[app.app1]
script = "echo"
listen_port = 9000

[app.app2]
script = "echo"
listen_port = 9000
cron = "invalid cron expression"
"#;
        std::fs::write(&path, content).unwrap();

        let report = ConfigValidator::validate_file(&path);
        let _ = std::fs::remove_file(path);

        assert!(!report.is_valid());
        assert!(report.errors.iter().any(|e| e.contains("Port collision")));
        assert!(report
            .errors
            .iter()
            .any(|e| e.contains("Invalid cron expression")));
    }

    #[test]
    fn test_validate_bad_cron_timezone() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join(format!("test_bad_tz_{}.toml", std::process::id()));
        let content = r#"
[app.app1]
script = "echo"
cron = "0 2 * * *"
cron_timezone = "Invalid/Timezone"
"#;
        std::fs::write(&path, content).unwrap();

        let report = ConfigValidator::validate_file(&path);
        let _ = std::fs::remove_file(path);

        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|e| e.contains("Invalid cron timezone 'Invalid/Timezone'")));
    }
}
