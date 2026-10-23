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
    // Only the one setting the re-exec needs; loaded again below with logging and notices.
    let wayland_drops = config::load(&config::config_dir(cli.config_dir.as_deref()))
        .0
        .sftp
        .wayland_drag_and_drop;
    cli::reexec_with_env(&cli, wayland_drops);
    let state_dir = state_dir();
    crash::install(state_dir.as_ref().ok().cloned());
    init_logging(&state_dir);
    let (config, notices) = load_config(&cli);
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

/// Loads `config.toml` from `--config-dir` or `$XDG_CONFIG_HOME/fogu`. A missing file is
/// normal; any other failure (bad TOML, schema mismatch) falls back to `Config::default()`.
/// Everything the user should know becomes a startup toast as well as a log line.
fn load_config(cli: &Cli) -> (fogu_core::Config, Vec<Notification>) {
    let dir = config::config_dir(cli.config_dir.as_deref());
    let (loaded, err) = config::load(&dir);
    let mut notices = Vec::new();
    if let Some(err) = err {
        tracing::error!(error = %err, "failed to load config; using defaults");
        notices.push(
            Notification::new(Level::Error, "Config not loaded; using defaults")
                .detail(err.to_string()),
        );
    }
    for issue in config::validate(&loaded) {
        tracing::warn!(%issue, "config issue");
        // The UI reports an unknown colour scheme itself, when it falls back.
        if !matches!(issue, ConfigIssue::UnknownColorScheme(_)) {
            notices.push(Notification::new(Level::Warn, "Config issue").detail(issue.to_string()));
        }
    }
    (loaded, notices)
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

#[cfg(test)]
mod tests {
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
