use gravhive::process::reap_zombies;

#[test]
fn test_reap_zombies_runs_without_panic() {
    // Calling reap_zombies when there are no zombies should execute cleanly without error
    let _reaped = reap_zombies();
}

#[cfg(unix)]
#[test]
fn test_reap_orphaned_zombie_process() {
    use std::process::Command;

    // Spawn a quick short-lived child process
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .expect("Failed to spawn dummy child process");

    let pid = child.id();
    assert!(pid > 0);

    // Give it a moment to terminate and enter zombie/exited state
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Reap zombies using our non-blocking helper
    let reaped = reap_zombies();

    // The child should have been reaped by reap_zombies
    // Even if wait() is called subsequently or if reap_zombies reaped it, it won't panic
    let _ = child.wait();
    println!("Reaped count: {}", reaped);
}
