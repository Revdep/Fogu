#![expect(clippy::unwrap_used, reason = "tests")]

use super::*;

#[test]
fn candidates_order_env_systemd_gcr_then_dir() {
    let got = candidates(
        Some("/env.sock".into()),
        Some(Path::new("/run/user/1000")),
        vec![PathBuf::from("/h/.ssh/agent/s.1")],
    );
    let want: Vec<PathBuf> = [
        "/env.sock",
        "/run/user/1000/ssh-agent.socket",
        "/run/user/1000/gcr/ssh",
        "/h/.ssh/agent/s.1",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    assert_eq!(got, want);
    assert_eq!(
        candidates(Some(OsString::new()), None, Vec::new()),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn owned_sockets_needs_a_private_dir_of_ours() {
    let dir = std::env::temp_dir().join(format!("fogu-agent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let sock = dir.join("s.1");
    let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    std::fs::write(dir.join("not-a-socket"), b"x").unwrap();
    let uid = std::fs::metadata(&dir).unwrap().uid();
    assert_eq!(owned_sockets(&dir, uid), vec![sock]);
    assert!(
        owned_sockets(&dir, uid + 1).is_empty(),
        "someone else's dir"
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(owned_sockets(&dir, uid).is_empty(), "world-writable dir");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wait_for_pid_lives_with_the_pid() {
    let wait = |pid: u32| {
        std::process::Command::new("sh")
            .args(["-c", WAIT_FOR_PID, "_", &pid.to_string()])
            // the killed loop's `sleep` outlives the test; keep it off the test's output pipes
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let gone_pid = gone.id();
    gone.wait().unwrap();
    assert!(
        wait(gone_pid).wait().unwrap().success(),
        "exits once the pid is gone"
    );

    let mut alive = wait(std::process::id());
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        alive.try_wait().unwrap().is_none(),
        "keeps running while we do"
    );
    alive.kill().unwrap();
    alive.wait().unwrap();
}
