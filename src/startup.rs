use anyhow::{Context, Result};
use colored::*;
use std::fs;

pub fn generate_startup_script() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        generate_systemd_service()?;
    }
    #[cfg(target_os = "macos")]
    {
        generate_launchd_plist()?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        println!("{}", "Automated startup script generation is not supported on this OS.".yellow());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn generate_systemd_service() -> Result<()> {
    let current_exe = std::env::current_exe()?;
    let ghived_exe = current_exe.with_file_name("ghived");

    let service_content = format!(
        r#"[Unit]
Description=Gravhive Process Manager Daemon
After=network.target

[Service]
Type=simple
ExecStart={}
Restart=always
RestartSec=3
Environment=PATH=/usr/local/bin:/usr/bin:/bin:$PATH

[Install]
WantedBy=multi-user.target
"#,
        ghived_exe.display()
    );

    let target_path = PathBuf::from("/etc/systemd/system/gravhive.service");
    println!("Generated Systemd service:\n{}", service_content);
    println!("To install system-wide, run as root:\n  sudo cp <file> {}", target_path.display());
    println!("  sudo systemctl daemon-reload");
    println!("  sudo systemctl enable --now gravhive");
    Ok(())
}

#[cfg(target_os = "macos")]
fn generate_launchd_plist() -> Result<()> {
    let current_exe = std::env::current_exe()?;
    let ghived_exe = current_exe.with_file_name("ghived");

    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let plist_dir = home.join("Library/LaunchAgents");
    fs::create_dir_all(&plist_dir)?;
    let plist_path = plist_dir.join("com.gravhive.ghived.plist");

    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.gravhive.ghived</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>{}/.gravhive/ghived.err.log</string>
    <key>StandardOutPath</key>
    <string>{}/.gravhive/ghived.out.log</string>
</dict>
</plist>
"#,
        ghived_exe.display(),
        home.display(),
        home.display()
    );

    fs::write(&plist_path, plist_content)?;
    println!("{} Created launchd service at {}", "✓".green().bold(), plist_path.display());
    println!("To start service now, run:");
    println!("  launchctl load {}", plist_path.display());
    Ok(())
}

pub fn remove_startup_script() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            let plist_path = home.join("Library/LaunchAgents/com.gravhive.ghived.plist");
            if plist_path.exists() {
                let _ = fs::remove_file(&plist_path);
                println!("{} Removed launchd service at {}", "✓".green().bold(), plist_path.display());
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        println!("To remove systemd service, run:");
        println!("  sudo systemctl disable --now gravhive");
        println!("  sudo rm /etc/systemd/system/gravhive.service");
    }
    Ok(())
}
