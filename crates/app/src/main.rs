//! `fogu`: entry point. Hardens the process, parses the CLI, installs the panic hook,
//! initialises logging (stderr + daily file), loads the config file and runs the GUI.

mod cli;
mod crash;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fogu_core::config::{self, ConfigIssue};
use fogu_core::fsutil;
use fogu_ui::{Level, Notification, RunOptions};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::cli::Cli;

/// Own crates at `info`, dependencies at `warn`; `RUST_LOG` overrides.
const DEFAULT_FILTER: &str = "warn,fogu=info,fogu_core=info,fogu_term=info,\
                              fogu_session=info,fogu_ui=info";
/// The log file in the state directory.
const LOG_FILE: &str = "fogu.log";
/// Past this size the log is moved to `fogu.log.1` at start-up (one older file kept).
const LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;

fn main() -> fogu_ui::Result {
    harden_process();
    let cli = Cli::parse();
    if let Some(pid) = cli.after_update_of
        && let Err(error) = wait_update_parent(pid)
    {
        eprintln!("fogu: {error}");
        std::process::exit(1);
    }
    #[cfg(all(debug_assertions, target_os = "linux"))]
    if let Some((dir, version)) = &cli.updater_verify {
        match fogu_ui::updater::debug_verify_release(dir, version) {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                eprintln!("fogu: {error}");
                std::process::exit(1);
            }
        }
    }
    #[cfg(debug_assertions)]
    if cli.updater_test {
        match fogu_ui::updater::debug_install() {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                eprintln!("fogu: {error}");
                std::process::exit(1);
            }
        }
    }
    // Read once, before the re-exec that one setting decides; reported once logging is up.
    let (config, config_err) = config::load(&config::config_dir(cli.config_dir.as_deref()));
    cli::reexec_with_env(&cli, config.sftp.wayland_drag_and_drop);
    let state_dir = state_dir();
    crash::install(state_dir.as_ref().ok().cloned());
    init_logging(&state_dir);
    let notices = config_notices(&config, config_err);
    let options = RunOptions {
        notices,
        exit_after_first_frame: cli.exit_after_first_frame,
        config_dir: cli.config_dir.clone(),
        state_dir: state_dir.ok(),
        // Benchmarks measure a cold start with one tab.
        no_recovery: cli.no_recovery || cli.exit_after_first_frame,
    };
    fogu_ui::run(Arc::new(config), options)
}

/// Release builds: no core dumps because SSH secrets will live in memory. On Linux
/// `PR_SET_DUMPABLE=0` also blocks ptrace attach by same-uid processes and makes
/// `/proc/<pid>/mem` unreadable; macOS only gets core dumps off (anti-attach needs FFI, which the
/// workspace's `unsafe_code = "deny"` keeps out of this crate). Debug builds skip it so debuggers
/// work. Runs first, and again after the `--software-render` re-exec (exec resets the flag).
fn harden_process() {
    #[cfg(unix)]
    {
        if cfg!(debug_assertions) {
            return;
        }
        if let Err(err) = make_undumpable() {
            // Logging is not up yet; this is a hardening failure the user should see.
            eprintln!("fogu: could not make the process non-dumpable: {err}");
        }
    }
    #[cfg(windows)]
    {
        // Windows has no `PR_SET_DUMPABLE`; a process mitigation policy is the platform
        // equivalent and is deliberately deferred to the Windows hardening pass.
    }
}

#[cfg(target_os = "linux")]
fn make_undumpable() -> rustix::io::Result<()> {
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn make_undumpable() -> rustix::io::Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    // The soft limit only: child shells inherit it and may raise it again.
    let maximum = getrlimit(Resource::Core).maximum;
    setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum,
        },
    )
}

/// What the user should know about `config.toml` (from `--config-dir` or `$XDG_CONFIG_HOME/fogu`),
/// as startup toasts and log lines. A missing file is normal; any other failure (bad TOML, schema
/// mismatch) already fell back to `Config::default()` in [`config::load`].
fn config_notices(
    loaded: &fogu_core::Config,
    err: Option<config::ConfigError>,
) -> Vec<Notification> {
    let mut notices = Vec::new();
    if let Some(err) = err {
        tracing::error!(error = %err, "failed to load config; using defaults");
        notices.push(
            Notification::new(Level::Error, "Config not loaded; using defaults")
                .detail(err.to_string()),
        );
    }
    for issue in config::validate(loaded) {
        tracing::warn!(%issue, "config issue");
        // The UI reports an unknown colour scheme itself, when it falls back.
        if !matches!(issue, ConfigIssue::UnknownColorScheme(_)) {
            notices.push(Notification::new(Level::Warn, "Config issue").detail(issue.to_string()));
        }
    }
    notices
}

/// stderr + `fogu.log` in the state directory ([`open_log`]); `log` records are bridged by
/// tracing-subscriber's `tracing-log` feature. Writes are unbuffered, so nothing is lost on exit.
fn init_logging(state_dir: &Result<PathBuf, String>) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let file = state_dir
        .clone()
        .and_then(|dir| open_log(&dir).map_err(|err| format!("{}: {err}", dir.display())));
    let (file_layer, file_err) = match file {
        Ok(file) => {
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file));
            (Some(layer), None)
        }
        Err(err) => (None, Some(err)),
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .init();
    if let Some(err) = file_err {
        tracing::warn!("file logging disabled: {err}");
    }
}

/// `fogu.log` in `dir`, private and opened for appending, after moving it to `fogu.log.1` once it
/// passed [`LOG_MAX_BYTES`].
fn open_log(dir: &Path) -> std::io::Result<std::fs::File> {
    let path = dir.join(LOG_FILE);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_MAX_BYTES) {
        std::fs::rename(&path, dir.join(format!("{LOG_FILE}.1")))?;
    }
    // shortcut: drops the daily files 0.1.6 and older wrote (`fogu.log.2026-10-22`); delete this
    // loop once nobody upgrades from those versions.
    for entry in std::fs::read_dir(dir)?.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{LOG_FILE}.20"))
        {
            let _gone = std::fs::remove_file(entry.path());
        }
    }
    fsutil::open_private_append(&path)
}

/// The platform state directory (see [`fogu_core::platform::state_dir`]), created private on
/// Unix.
fn state_dir() -> Result<PathBuf, String> {
    let dir = fogu_core::platform::state_dir().ok_or(if cfg!(windows) {
        "the local app-data folder (%LOCALAPPDATA%) is not known"
    } else {
        "neither XDG_STATE_HOME nor HOME is set"
    })?;
    fsutil::ensure_private_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    Ok(dir)
}

/// A restarted child must never race the parent's recovery writers or instance lock.
fn wait_update_parent(pid: u32) -> Result<(), String> {
    let start = std::time::Instant::now();
    while parent_alive(pid)? {
        if start.elapsed() >= std::time::Duration::from_secs(30) {
            return Err("update parent did not exit; reopen Fogu manually".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(unix)]
fn parent_alive(pid: u32) -> Result<bool, String> {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).map_err(|e| e.to_string())?)
        .ok_or("invalid parent pid")?;
    match rustix::process::test_kill_process(pid) {
        Ok(()) | Err(rustix::io::Errno::PERM) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(windows)]
fn parent_alive(pid: u32) -> Result<bool, String> {
    // shortcut: tasklist is the safe process query, replace with a safe handle API when available.
    use std::os::windows::process::CommandExt as _;
    let tool =
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("missing Windows system directory")?)
            .join("System32/tasklist.exe");
    let mut child = std::process::Command::new(tool)
        .creation_flags(0x0800_0000)
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < std::time::Duration::from_secs(3) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("could not query update parent in time".into());
            }
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("could not query update parent".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn update_barrier_waits_until_the_parent_is_reaped() {
        assert!(super::parent_alive(std::process::id()).unwrap());
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 0.1"])
            .spawn()
            .unwrap();
        let pid = child.id();
        let reap = std::thread::spawn(move || child.wait().unwrap());
        super::wait_update_parent(pid).unwrap();
        assert!(!super::parent_alive(pid).unwrap());
        assert!(reap.join().unwrap().success());
    }

    #[test]
    fn the_log_rotates_past_its_limit_and_old_daily_files_go() {
        use std::io::Write as _;

        let dir = std::env::temp_dir().join(format!("fogu-log-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let big = vec![b'x'; usize::try_from(super::LOG_MAX_BYTES).unwrap() + 1];
        std::fs::write(dir.join("fogu.log"), &big).unwrap();
        std::fs::write(dir.join("fogu.log.2026-10-22"), "old").unwrap();
        std::fs::write(dir.join("crash.log"), "keep").unwrap();
        super::open_log(&dir).unwrap().write_all(b"new\n").unwrap();
        assert_eq!(
            std::fs::read(dir.join("fogu.log.1")).unwrap().len(),
            big.len()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("fogu.log")).unwrap(),
            "new\n"
        );
        assert!(!dir.join("fogu.log.2026-10-22").exists());
        assert!(dir.join("crash.log").exists());
        // Under the limit: appended to, not rotated.
        super::open_log(&dir).unwrap().write_all(b"more\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("fogu.log")).unwrap(),
            "new\nmore\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Core dumps off for Fogu, but only the soft limit: shells started in Fogu inherit it and
    /// can still raise it (`ulimit -c unlimited`), as in any other terminal.
    #[cfg(all(unix, not(target_os = "linux")))]
    #[test]
    fn undumpable_leaves_the_hard_core_limit_alone() {
        use rustix::process::{Resource, getrlimit};
        let before = getrlimit(Resource::Core).maximum;
        assert!(super::make_undumpable().is_ok());
        let after = getrlimit(Resource::Core);
        assert_eq!(after.current, Some(0));
        assert_eq!(after.maximum, before);
    }
}
