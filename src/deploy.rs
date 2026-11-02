use crate::alerts::{AlertEvent, AlertManager};
use anyhow::{bail, Context, Result};
use colored::*;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployTarget {
    pub user: Option<String>,
    pub host: Option<String>,
    pub repo: Option<String>,
    pub path: String,
    #[serde(rename = "ref", default = "default_ref")]
    pub git_ref: String,
    pub pre_deploy: Option<String>,
    pub post_deploy: Option<String>,
}

fn default_ref() -> String {
    "origin/main".to_string()
}

pub struct DeployManager;

impl DeployManager {
    pub fn execute_deploy(
        env_name: &str,
        target: &DeployTarget,
        alert_mgr: Option<&AlertManager>,
    ) -> Result<()> {
        println!("\n{}", "─".repeat(60).cyan());
        println!(
            " {} Initiating Deployment: Environment '{}'",
            "🚀".bold(),
            env_name.bold()
        );
        println!("{}", "─".repeat(60).cyan());

        let is_local = target
            .host
            .as_deref()
            .unwrap_or("local")
            .eq_ignore_ascii_case("local")
            || target.host.as_deref().unwrap_or("") == "127.0.0.1";

        let result = if is_local {
            Self::execute_local(env_name, target)
        } else {
            Self::execute_remote(env_name, target)
        };

        match &result {
            Ok(_) => {
                println!(
                    "\n{} Deployment to '{}' succeeded!",
                    "✓".green().bold(),
                    env_name
                );
                if let Some(alerts) = alert_mgr {
                    alerts.send(AlertEvent::DeployResult {
                        environment: env_name.to_string(),
                        success: true,
                        message: format!("Target path: {}", target.path),
                    });
                }
            }
            Err(e) => {
                eprintln!(
                    "\n{} Deployment to '{}' failed: {}",
                    "✗".red().bold(),
                    env_name,
                    e
                );
                if let Some(alerts) = alert_mgr {
                    alerts.send(AlertEvent::DeployResult {
                        environment: env_name.to_string(),
                        success: false,
                        message: e.to_string(),
                    });
                }
            }
        }

        result
    }

    fn execute_local(_env_name: &str, target: &DeployTarget) -> Result<()> {
        validate_safe_arg(&target.path, "path")?;
        validate_safe_arg(&target.git_ref, "git_ref")?;

        let work_dir = &target.path;

        // 1. Pre-deploy hook
        if let Some(ref pre) = target.pre_deploy {
            println!(
                "{} Running pre-deploy hook: {}",
                "⚡".yellow(),
                pre.dimmed()
            );
            let status = Command::new("sh")
                .arg("-c")
                .arg(pre)
                .current_dir(work_dir)
                .status()
                .with_context(|| "Failed to run pre-deploy hook".to_string())?;
            if !status.success() {
                bail!("Pre-deploy hook exited with status: {}", status);
            }
        }

        // 2. Git Pull / Checkout
        if target.repo.is_some() {
            println!(
                "{} Fetching and checking out git ref: {}",
                "📦".cyan(),
                target.git_ref.bold()
            );
            let git_cmd = format!(
                "git fetch --all && git checkout {} && git pull",
                target.git_ref
            );
            let status = Command::new("sh")
                .arg("-c")
                .arg(&git_cmd)
                .current_dir(work_dir)
                .status()
                .with_context(|| "Failed to execute git fetch/pull")?;
            if !status.success() {
                bail!("Git pull failed with status: {}", status);
            }
        }

        // 3. Post-deploy hook
        if let Some(ref post) = target.post_deploy {
            println!(
                "{} Running post-deploy hook: {}",
                "⚡".yellow(),
                post.dimmed()
            );
            let status = Command::new("sh")
                .arg("-c")
                .arg(post)
                .current_dir(work_dir)
                .status()
                .with_context(|| "Failed to run post-deploy hook".to_string())?;
            if !status.success() {
                bail!("Post-deploy hook exited with status: {}", status);
            }
        }

        Ok(())
    }

    fn execute_remote(_env_name: &str, target: &DeployTarget) -> Result<()> {
        validate_safe_arg(&target.path, "path")?;
        validate_safe_arg(&target.git_ref, "git_ref")?;
        if let Some(ref h) = target.host {
            validate_safe_arg(h, "host")?;
        }
        if let Some(ref u) = target.user {
            validate_safe_arg(u, "user")?;
        }

        let host = target.host.as_deref().unwrap_or("localhost");
        let user_prefix = if let Some(ref u) = target.user {
            format!("{}@", u)
        } else {
            "".to_string()
        };
        let ssh_target = format!("{}{}", user_prefix, host);

        let mut remote_script = format!("cd {}", target.path);

        if let Some(ref pre) = target.pre_deploy {
            remote_script.push_str(&format!(" && {}", pre));
        }

        if target.repo.is_some() {
            remote_script.push_str(&format!(
                " && git fetch --all && git checkout {} && git pull",
                target.git_ref
            ));
        }

        if let Some(ref post) = target.post_deploy {
            remote_script.push_str(&format!(" && {}", post));
        }

        println!(
            "{} Connecting via SSH to {}: {}",
            "🔗".cyan(),
            ssh_target.bold(),
            remote_script.dimmed()
        );

        let status = Command::new("ssh")
            .arg(&ssh_target)
            .arg(&remote_script)
            .status()
            .with_context(|| format!("SSH deployment failed for host {}", ssh_target))?;

        if !status.success() {
            bail!("Remote SSH execution exited with status: {}", status);
        }

        Ok(())
    }
}

fn validate_safe_arg(val: &str, field_name: &str) -> Result<()> {
    if val.chars().any(|c| ";|&$`\n\r><\"'\\".contains(c)) {
        bail!(
            "Security validation error: deployment field '{}' contains prohibited shell metacharacters: '{}'",
            field_name,
            val
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_safe_arg() {
        assert!(validate_safe_arg("main", "git_ref").is_ok());
        assert!(validate_safe_arg("/var/www/my-app", "path").is_ok());
        assert!(validate_safe_arg("v1.2.3", "git_ref").is_ok());
        assert!(validate_safe_arg("main; rm -rf /", "git_ref").is_err());
        assert!(validate_safe_arg("app && whoami", "path").is_err());
        assert!(validate_safe_arg("`touch /tmp/pwn`", "git_ref").is_err());
    }
}
