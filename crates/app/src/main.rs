//! `fogu`: entry point. Hardens the process, parses the CLI, installs the panic hook,
//! initialises logging (stderr + daily file), loads the config file and runs the GUI.

mod cli;
mod crash;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser as _;
use fogu_core::config::{self, ConfigIssue};
use fogu_core::fsutil;
use fogu_ui::{Level, Notification, RunOptions};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::cli::Cli;

/// Own crates at `info`, dependencies at `warn`; `RUST_LOG` overrides.
const DEFAULT_FILTER: &str = "warn,fogu=info,fogu_core=info,fogu_term=info,\
                              fogu_session=info,fogu_ui=info";
/// Log file name prefix (`fogu.log.YYYY-MM-DD`).
const LOG_PREFIX: &str = "fogu.log";
/// Daily log files kept.
const MAX_LOG_FILES: usize = 7;

fn main() -> fogu_ui::Result {
    harden_process();
    let cli = Cli::parse();
    cli::reexec_for_software_render(&cli);
    let state_dir = state_dir();
    crash::install(state_dir.as_ref().ok().cloned());
    let _log_guard = init_logging(&state_dir);
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
/// `/proc/<pid>/mem` unreadable; macOS only gets core dumps off (anti-attach needs FFI, which
/// `unsafe_code = "forbid"` rules out). Debug builds skip it so debuggers work. Runs first, and
/// again after the `--software-render` re-exec (exec resets the flag).
fn harden_process() {
    if cfg!(debug_assertions) {
        return;
    }
    if let Err(err) = make_undumpable() {
        // Logging is not up yet; this is a hardening failure the user should see.
        eprintln!("fogu: could not make the process non-dumpable: {err}");
    }
}

#[cfg(target_os = "linux")]
fn make_undumpable() -> rustix::io::Result<()> {
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
}

#[cfg(not(target_os = "linux"))]
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

/// stderr + non-blocking daily file in `$XDG_STATE_HOME/fogu/`; `log` records are bridged
/// by tracing-subscriber's `tracing-log` feature. The guard flushes the file on drop.
fn init_logging(state_dir: &Result<PathBuf, String>) -> Option<WorkerGuard> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let file = state_dir.clone().and_then(|dir| {
        RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(LOG_PREFIX)
            .max_log_files(MAX_LOG_FILES)
            .build(&dir)
            .map_err(|err| format!("{}: {err}", dir.display()))
    });
    let (file_layer, guard, file_err) = match file {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer);
            (Some(layer), Some(guard), None)
        }
        Err(err) => (None, None, Some(err)),
    };
    if let Ok(dir) = state_dir
        && let Err(err) = fsutil::restrict_files(dir, LOG_PREFIX)
    {
        eprintln!("fogu: could not restrict log file permissions: {err}");
    }
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .init();
    if let Some(err) = file_err {
        tracing::warn!("file logging disabled: {err}");
    }
    guard
}

/// `$XDG_STATE_HOME/fogu` (default `~/.local/state/fogu`), made 0700 even if it already existed:
/// the daily log files rotated in later are created with the umask, so the directory keeps them
/// private.
fn state_dir() -> Result<PathBuf, String> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or("neither XDG_STATE_HOME nor HOME is set")?;
    let dir = state.join("fogu");
    fsutil::ensure_private_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    /// Core dumps off for Fogu, but only the soft limit: shells started in Fogu inherit it and
    /// can still raise it (`ulimit -c unlimited`), as in any other terminal.
    #[cfg(not(target_os = "linux"))]
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
