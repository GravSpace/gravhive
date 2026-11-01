use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CgroupConfig {
    pub memory_max: Option<u64>,
    pub cpu_max: Option<String>,
    pub pids_max: Option<u32>,
}

pub struct CgroupManager;

impl CgroupManager {
    pub fn is_cgroups_v2_available() -> bool {
        #[cfg(target_os = "linux")]
        {
            let mount = Path::new("/sys/fs/cgroup");
            mount.exists() && Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    pub fn apply(app_name: &str, pid: u32, config: &CgroupConfig) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            if !Self::is_cgroups_v2_available() {
                debug!("cgroups v2 not available on this host. Skipping kernel limits.");
                return Ok(());
            }

            let cgroup_path = PathBuf::from(format!("/sys/fs/cgroup/gravhive/{}", app_name));
            if !cgroup_path.exists() {
                fs::create_dir_all(&cgroup_path)?;
            }

            // 1. memory.max
            if let Some(mem_bytes) = config.memory_max {
                let mem_file = cgroup_path.join("memory.max");
                if let Err(e) = fs::write(&mem_file, mem_bytes.to_string()) {
                    warn!("Failed to write cgroup memory.max: {}", e);
                } else {
                    debug!("Applied cgroup memory.max = {} for app '{}'", mem_bytes, app_name);
                }
            }

            // 2. cpu.max (e.g. "50000 100000" for 50% quota)
            if let Some(ref cpu_str) = config.cpu_max {
                let cpu_file = cgroup_path.join("cpu.max");
                if let Err(e) = fs::write(&cpu_file, cpu_str) {
                    warn!("Failed to write cgroup cpu.max: {}", e);
                } else {
                    debug!("Applied cgroup cpu.max = '{}' for app '{}'", cpu_str, app_name);
                }
            }

            // 3. pids.max
            if let Some(pids) = config.pids_max {
                let pids_file = cgroup_path.join("pids.max");
                if let Err(e) = fs::write(&pids_file, pids.to_string()) {
                    warn!("Failed to write cgroup pids.max: {}", e);
                }
            }

            // 4. Attach PID to cgroup.procs
            let procs_file = cgroup_path.join("cgroup.procs");
            fs::write(&procs_file, pid.to_string())?;
            info!(
                "Attached PID {} for app '{}' to cgroup v2 ({})",
                pid,
                app_name,
                cgroup_path.display()
            );
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (app_name, pid, config);
            debug!("Kernel cgroups v2 is Linux-only. User-space watchdog limits active.");
        }

        Ok(())
    }
}

pub fn drop_privileges(user_name: Option<&str>, group_name: Option<&str>) -> Result<()> {
    #[cfg(unix)]
    {
        use nix::unistd::{Group, User, setgid, setuid};

        if let Some(grp) = group_name {
            if let Ok(Some(g)) = Group::from_name(grp) {
                setgid(g.gid)?;
                debug!("Set group ID to {} ({})", g.gid, grp);
            }
        }

        if let Some(usr) = user_name {
            if let Ok(Some(u)) = User::from_name(usr) {
                setuid(u.uid)?;
                debug!("Set user ID to {} ({})", u.uid, usr);
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (user_name, group_name);
    }

    Ok(())
}

