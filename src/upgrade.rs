use anyhow::{bail, Context, Result};
use colored::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_REPO: &str = "GravSpace/GravHive";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

pub struct Upgrader;

impl Upgrader {
    /// Detects current system target triple corresponding to release asset names
    pub fn detect_target() -> Result<&'static str> {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        match (os, arch) {
            ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
            ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
            ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
            ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
            _ => bail!(
                "Self-upgrade is currently not supported for OS '{}' and architecture '{}'",
                os,
                arch
            ),
        }
    }

    /// Compare semver-like version strings
    pub fn is_newer(current: &str, target: &str) -> bool {
        let current_parts = Self::parse_semver(current);
        let target_parts = Self::parse_semver(target);
        target_parts > current_parts
    }

    fn parse_semver(v: &str) -> (u32, u32, u32) {
        let clean = v.trim().trim_start_matches('v');
        let mut parts = clean.split('.');
        let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts
            .next()
            .and_then(|s| s.split('-').next()) // ignore pre-release suffix
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (major, minor, patch)
    }

    /// Fetch latest or specific release metadata from GitHub API
    pub async fn fetch_release(repo: &str, target_tag: Option<&str>) -> Result<GitHubRelease> {
        let url = match target_tag {
            Some(tag) => {
                let clean_tag = if tag.starts_with('v') {
                    tag.to_string()
                } else {
                    format!("v{}", tag)
                };
                format!(
                    "https://api.github.com/repos/{}/releases/tags/{}",
                    repo, clean_tag
                )
            }
            None => format!("https://api.github.com/repos/{}/releases/latest", repo),
        };

        let client = reqwest::Client::builder()
            .user_agent(format!("gravhive-updater/{}", CURRENT_VERSION))
            .build()?;

        let resp = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to connect to GitHub Releases API at {}", url))?;

        if !resp.status().is_success() {
            if resp.status().as_u16() == 404 {
                bail!("Release not found on repository '{}' ({})", repo, url);
            }
            bail!(
                "GitHub API returned error {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }

        let release = resp
            .json::<GitHubRelease>()
            .await
            .context("Failed to parse release response from GitHub")?;

        Ok(release)
    }

    /// Execute the complete upgrade workflow
    pub async fn upgrade(
        target_version: Option<String>,
        force: bool,
        check_only: bool,
    ) -> Result<()> {
        let repo = std::env::var("GHIVE_UPDATE_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string());
        let current_target = Self::detect_target()?;

        println!(
            "{} Checking for Gravhive updates on {}...",
            "ℹ".cyan(),
            repo.bold()
        );

        let release = Self::fetch_release(&repo, target_version.as_deref()).await?;
        let latest_tag = release.tag_name.trim_start_matches('v');

        println!("  Current version: {}", CURRENT_VERSION.bold());
        println!("  Latest version:  {}", latest_tag.bold());

        let has_new_version = Self::is_newer(CURRENT_VERSION, latest_tag);

        if check_only {
            if has_new_version {
                println!(
                    "\n{} A new version is available: {} -> {}\nRun {} to update.\n",
                    "🚀".green(),
                    CURRENT_VERSION.yellow(),
                    latest_tag.green().bold(),
                    "ghive upgrade".bold()
                );
            } else {
                println!(
                    "\n{} Gravhive is up to date (v{}).\n",
                    "✓".green().bold(),
                    CURRENT_VERSION
                );
            }
            return Ok(());
        }

        if !has_new_version && !force && target_version.is_none() {
            println!(
                "\n{} Gravhive is already at the latest version (v{}). Use {} to force reinstall.\n",
                "✓".green().bold(),
                CURRENT_VERSION,
                "--force".bold()
            );
            return Ok(());
        }

        // Find appropriate tarball asset (matches musl first for static compatibility, falls back to gnu)
        let expected_asset_suffix = format!("{}.tar.gz", current_target);
        let fallback_suffix = if current_target.contains("musl") {
            Some(format!("{}.tar.gz", current_target.replace("musl", "gnu")))
        } else if current_target.contains("gnu") {
            Some(format!("{}.tar.gz", current_target.replace("gnu", "musl")))
        } else {
            None
        };

        let asset = release
            .assets
            .iter()
            .find(|a| a.name.ends_with(&expected_asset_suffix))
            .or_else(|| {
                fallback_suffix
                    .as_ref()
                    .and_then(|suffix| release.assets.iter().find(|a| a.name.ends_with(suffix)))
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No matching release asset found for target '{}' in release '{}'. Available assets: {:?}",
                    current_target,
                    release.tag_name,
                    release.assets.iter().map(|a| &a.name).collect::<Vec<_>>()
                )
            })?;

        println!(
            "\n{} Downloading {} ({:.2} MB)...",
            "⬇".cyan().bold(),
            asset.name.bold(),
            asset.size as f64 / (1024.0 * 1024.0)
        );

        let client = reqwest::Client::builder()
            .user_agent(format!("gravhive-updater/{}", CURRENT_VERSION))
            .build()?;

        let bytes = client
            .get(&asset.browser_download_url)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to download asset from {}",
                    asset.browser_download_url
                )
            })?
            .bytes()
            .await
            .context("Failed to read asset payload")?;

        // Cryptographic integrity check: verify SHA-256 hash if checksum asset exists
        let checksum_asset = release.assets.iter().find(|a| {
            a.name.eq_ignore_ascii_case("checksums.txt")
                || a.name.eq_ignore_ascii_case("sha256sums")
                || a.name == format!("{}.sha256", asset.name)
        });

        if let Some(c_asset) = checksum_asset {
            println!(
                "{} Verifying SHA-256 integrity against {}...",
                "🔒".cyan(),
                c_asset.name.bold()
            );
            if let Ok(c_resp) = client.get(&c_asset.browser_download_url).send().await {
                if let Ok(c_text) = c_resp.text().await {
                    let mut hasher = Sha256::new();
                    hasher.update(&bytes);
                    let computed_hash = format!("{:x}", hasher.finalize());

                    let mut matched = false;
                    for line in c_text.lines() {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if let Some(expected_hash) = parts.first() {
                            if expected_hash.eq_ignore_ascii_case(&computed_hash) {
                                matched = true;
                                break;
                            }
                        }
                    }

                    if matched {
                        println!(
                            "{} SHA-256 checksum verified: {}",
                            "✓".green().bold(),
                            &computed_hash[..16]
                        );
                    } else {
                        bail!(
                            "Security integrity failure: SHA-256 hash {} did not match release checksums in {}",
                            computed_hash,
                            c_asset.name
                        );
                    }
                }
            }
        }

        // Prepare private temporary extraction directory in ~/.gravhive/tmp_upgrade (mode 0700)
        // to protect against local TOCTOU / shared-temp-dir binary tampering attacks.
        let upgrade_base = crate::common::gravhive_dir().join("tmp_upgrade");
        if !upgrade_base.exists() {
            fs::create_dir_all(&upgrade_base)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&upgrade_base, fs::Permissions::from_mode(0o700));
            }
        }
        let temp_dir = upgrade_base.join(format!("update_{}", std::process::id()));
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        fs::create_dir_all(&temp_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&temp_dir, fs::Permissions::from_mode(0o700));
        }

        let archive_path = temp_dir.join("release.tar.gz");
        fs::write(&archive_path, &bytes)?;

        // Extract tar.gz using system tar
        println!("{} Extracting release archive...", "📦".cyan());
        let tar_status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive_path)
            .arg("-C")
            .arg(&temp_dir)
            .status()
            .context("Failed to execute 'tar' utility to unpack release archive")?;

        if !tar_status.success() {
            bail!(
                "Failed to unpack release archive: exit status {}",
                tar_status
            );
        }

        // Locate newly extracted binaries
        let (new_ghive, new_ghived) = Self::find_binaries_in_dir(&temp_dir)?;

        // Determine destination paths based on current running binary
        let current_ghive = std::env::current_exe()
            .context("Failed to determine current 'ghive' executable path")?;
        let current_ghived = current_ghive.with_file_name("ghived");

        println!(
            "{} Installing updated binaries to {} and {}...",
            "⚙".cyan(),
            current_ghive.display(),
            current_ghived.display()
        );

        // Atomic replacement
        Self::replace_binary(&new_ghive, &current_ghive)
            .with_context(|| format!("Failed to update binary at {}", current_ghive.display()))?;

        if current_ghived.exists() || new_ghived.exists() {
            Self::replace_binary(&new_ghived, &current_ghived).with_context(|| {
                format!(
                    "Failed to update daemon binary at {}",
                    current_ghived.display()
                )
            })?;
        }

        // Cleanup temp folder
        let _ = fs::remove_dir_all(&temp_dir);

        println!(
            "\n{} Successfully upgraded Gravhive to {} ({})!",
            "✓".green().bold(),
            release.tag_name.bold(),
            current_target
        );

        // Check if daemon is running and restart it gracefully with state resurrection
        if crate::ipc::IpcClient::ping().await.is_ok() {
            println!(
                "{} Preserving running state and restarting background daemon with updated binary...",
                "↺".cyan()
            );

            // 1. Explicitly save process snapshot and cron jobs to disk
            let _ = crate::ipc::IpcClient::send_request(crate::protocol::Request::Save).await;

            // 2. Request graceful daemon shutdown
            let _ = crate::ipc::IpcClient::send_request(crate::protocol::Request::Shutdown).await;

            // 3. Wait for previous daemon process to terminate
            let start_exit = std::time::Instant::now();
            while start_exit.elapsed() < std::time::Duration::from_secs(4) {
                if crate::ipc::IpcClient::ping().await.is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            // 4. Ensure new daemon is running (either auto-restarted by systemd/launchd or spawn directly)
            let mut daemon_ready = false;
            let check_service_start = std::time::Instant::now();
            while check_service_start.elapsed() < std::time::Duration::from_millis(1500) {
                if crate::ipc::IpcClient::ping().await.is_ok() {
                    daemon_ready = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }

            if !daemon_ready && current_ghived.exists() {
                let _ = std::process::Command::new(&current_ghived)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();

                let start_spawn_wait = std::time::Instant::now();
                while start_spawn_wait.elapsed() < std::time::Duration::from_secs(5) {
                    if crate::ipc::IpcClient::ping().await.is_ok() {
                        daemon_ready = true;
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
            }

            // 5. Trigger resurrection to restore running apps and cron jobs
            if daemon_ready {
                match crate::ipc::IpcClient::send_request(crate::protocol::Request::Resurrect).await
                {
                    Ok(crate::protocol::Response::Ok { message }) => {
                        println!(
                            "{} Daemon restarted successfully. {}",
                            "✓".green().bold(),
                            message
                        );
                    }
                    _ => {
                        println!(
                            "{} Daemon restarted with updated binary.",
                            "✓".green().bold()
                        );
                    }
                }
            } else {
                println!(
                    "{} Daemon stopped for binary update. Run `ghive resurrect` or start an app to resume with the new version.",
                    "ℹ".cyan()
                );
            }
        }

        if let Some(ref notes) = release.body {
            if !notes.trim().is_empty() {
                println!("\nRelease Notes:\n{}", notes.trim().dimmed());
            }
        }

        Ok(())
    }

    /// Search for ghive and ghived in extracted directory
    fn find_binaries_in_dir(root: &Path) -> Result<(PathBuf, PathBuf)> {
        let mut ghive_opt = None;
        let mut ghived_opt = None;

        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
                        if file_name == "ghive" {
                            ghive_opt = Some(path.clone());
                        } else if file_name == "ghived" {
                            ghived_opt = Some(path.clone());
                        }
                    }
                }
            }
        }

        let ghive_path = ghive_opt
            .ok_or_else(|| anyhow::anyhow!("Could not find 'ghive' binary in unpacked archive"))?;
        let ghived_path = ghived_opt
            .ok_or_else(|| anyhow::anyhow!("Could not find 'ghived' binary in unpacked archive"))?;

        Ok((ghive_path, ghived_path))
    }

    /// Safely replace an existing binary in a Unix-atomic manner
    fn replace_binary(src: &Path, dest: &Path) -> Result<()> {
        let parent = dest.parent().unwrap_or_else(|| Path::new("."));
        let temp_dest = parent.join(format!(
            ".{}.new.{}",
            dest.file_name().and_then(|f| f.to_str()).unwrap_or("bin"),
            std::process::id()
        ));

        // Copy source to temporary file in the destination directory
        if let Err(e) = fs::copy(src, &temp_dest) {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                bail!(
                    "Permission denied to write to '{}'.\nPlease re-run the upgrade command with root privileges: 'sudo ghive upgrade'",
                    dest.display()
                );
            }
            return Err(e).context("Failed to write temporary binary");
        }

        // Set executable permissions (0755)
        let _ = fs::set_permissions(&temp_dest, fs::Permissions::from_mode(0o755));

        // Atomic rename into target location
        if let Err(e) = fs::rename(&temp_dest, dest) {
            let _ = fs::remove_file(&temp_dest);
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                bail!(
                    "Permission denied to replace '{}'.\nPlease re-run with sudo: 'sudo ghive upgrade'",
                    dest.display()
                );
            }
            return Err(e).context("Failed to replace destination binary");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_parsing() {
        assert_eq!(Upgrader::parse_semver("0.1.0"), (0, 1, 0));
        assert_eq!(Upgrader::parse_semver("v1.2.3"), (1, 2, 3));
        assert_eq!(Upgrader::parse_semver("v2.10.5-beta.1"), (2, 10, 5));
    }

    #[test]
    fn test_is_newer_version() {
        assert!(Upgrader::is_newer("0.1.0", "0.1.1"));
        assert!(Upgrader::is_newer("0.1.0", "0.2.0"));
        assert!(Upgrader::is_newer("0.1.0", "1.0.0"));
        assert!(!Upgrader::is_newer("0.2.0", "0.1.9"));
        assert!(!Upgrader::is_newer("1.0.0", "1.0.0"));
    }

    #[test]
    fn test_detect_target() {
        let target = Upgrader::detect_target();
        assert!(target.is_ok());
    }
}
