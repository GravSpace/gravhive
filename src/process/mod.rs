pub mod manager;
pub mod worker;

pub use manager::ProcessManager;
pub use worker::Worker;

/// Non-blocking reaper function to reap orphaned zombie child processes.
/// Returns the number of reaped zombie processes.
#[cfg(unix)]
pub fn reap_zombies() -> usize {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;

    let mut count = 0;
    loop {
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, status)) => {
                count += 1;
                tracing::debug!(
                    "[REAPER] Reaped orphaned child {} with status {}",
                    pid,
                    status
                );
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                count += 1;
                tracing::debug!(
                    "[REAPER] Reaped orphaned child {} terminated by signal {:?}",
                    pid,
                    sig
                );
            }
            _ => break,
        }
    }
    count
}

#[cfg(not(unix))]
pub fn reap_zombies() -> usize {
    0
}

/// Registers the current process as a subreaper on Linux systems.
/// This ensures any orphaned descendants of child processes will be reparented
/// to this process rather than PID 1 (or if running as PID 1, allows reaping them).
pub fn setup_subreaper() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
}
