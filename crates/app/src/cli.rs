//! Command-line flags.

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::Command;

use std::ffi::OsString;

use fogu_core::platform;

/// Environment variable iced's renderer selection reads (`tiny-skia` = CPU rendering).
const BACKEND_ENV: &str = "ICED_BACKEND";
/// Mesa option that lists 8-bit BGRA first among a window's surface formats. iced 0.14 takes the
/// first one, which on RADV is `Rgb10a2Unorm`: 2 alpha bits, so window opacity snaps to 0, ⅓,
/// ⅔ or 1. Ignored by non-Mesa drivers.
/// TODO(iced 0.15): drop it; iced master already skips `Rgb10a2` formats.
#[cfg(target_os = "linux")]
const MESA_BGRA8_FIRST: &str = "vk_wsi_force_bgra8_unorm_first";

const HELP: &str = "\
Fogu: a native terminal emulator

Usage: fogu [OPTIONS]

Options:
      --config-dir <PATH>  Use this directory instead of the platform one for config.toml
      --software-render    Render on the CPU instead of the GPU; for broken drivers and VMs
      --no-recovery        Start with one new tab: don't restore or save the open tabs
      --licenses           Print the licences of every crate built into Fogu
  -h, --help               Print help
  -V, --version            Print version
";

/// Command-line flags.
#[derive(Debug, Default)]
#[cfg_attr(
    debug_assertions,
    expect(
        clippy::struct_excessive_bools,
        reason = "independent command-line flags"
    )
)]
pub struct Cli {
    /// `--config-dir PATH`: use this directory instead of the platform one for `config.toml`.
    pub config_dir: Option<PathBuf>,
    /// `--software-render`: render on the CPU (tiny-skia) instead of the GPU.
    pub software_render: bool,
    /// `--exit-after-first-frame` (hidden): exit after the first frame (`scripts/metrics.sh`).
    pub exit_after_first_frame: bool,
    /// `--no-recovery`: start with one new tab; don't restore or save the session.
    pub no_recovery: bool,
    /// Internal restart barrier, consumed before bootstrap.
    pub after_update_of: Option<u32>,
    /// Headless updater exercise; absent from release builds.
    #[cfg(debug_assertions)]
    pub updater_test: bool,
    /// `--updater-test-verify DIR VERSION`: the release gate (CI, debug builds).
    #[cfg(all(debug_assertions, target_os = "linux"))]
    pub updater_verify: Option<(PathBuf, String)>,
}

/// Why parsing stopped before a [`Cli`].
#[derive(Debug, PartialEq, Eq)]
enum Stop {
    Help,
    Version,
    Licenses,
    Error(String),
}

impl Cli {
    /// The process arguments. `--help`, `--version` and errors print and exit (0, 0, 2).
    #[expect(clippy::print_stdout, reason = "--help and --version go to stdout")]
    pub fn parse() -> Self {
        match Self::parse_from(std::env::args_os().skip(1)) {
            Ok(cli) => cli,
            Err(Stop::Help) => {
                print!("{HELP}");
                std::process::exit(0)
            }
            Err(Stop::Licenses) => {
                println!("{}", fogu_ui::licenses());
                std::process::exit(0)
            }
            Err(Stop::Version) => {
                println!("fogu {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0)
            }
            Err(Stop::Error(why)) => {
                eprintln!("error: {why}\n\nFor more information, try '--help'.");
                std::process::exit(2)
            }
        }
    }

    fn parse_from(args: impl IntoIterator<Item = OsString>) -> Result<Self, Stop> {
        let mut cli = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let text = arg.to_string_lossy();
            match text.as_ref() {
                "--licenses" => return Err(Stop::Licenses),
                "-h" | "--help" => return Err(Stop::Help),
                "-V" | "--version" => return Err(Stop::Version),
                "--after-update-of" => {
                    let pid = args
                        .next()
                        .and_then(|p| p.to_str().and_then(|p| p.parse::<u32>().ok()))
                        .filter(|p| *p > 1 && *p != std::process::id())
                        .ok_or_else(|| Stop::Error("invalid update parent pid".into()))?;
                    cli.after_update_of = Some(pid);
                }
                #[cfg(all(debug_assertions, target_os = "linux"))]
                "--updater-test-verify" => {
                    let dir = args
                        .next()
                        .map(PathBuf::from)
                        .ok_or_else(|| Stop::Error("missing release directory".into()))?;
                    let version = args
                        .next()
                        .and_then(|v| v.into_string().ok())
                        .ok_or_else(|| Stop::Error("missing release version".into()))?;
                    cli.updater_verify = Some((dir, version));
                }
                #[cfg(debug_assertions)]
                "--updater-test-install" => cli.updater_test = true,
                "--software-render" => cli.software_render = true,
                "--exit-after-first-frame" => cli.exit_after_first_frame = true,
                "--no-recovery" => cli.no_recovery = true,
                "--config-dir" => {
                    let path = args.next().ok_or_else(|| {
                        Stop::Error("a value is required for '--config-dir <PATH>'".to_owned())
                    })?;
                    cli.config_dir = Some(PathBuf::from(path));
                }
                // `--config-dir=PATH` goes through text; `--config-dir PATH` keeps any OS path.
                other => match other.strip_prefix("--config-dir=") {
                    Some(path) => cli.config_dir = Some(PathBuf::from(path)),
                    None => return Err(Stop::Error(format!("unexpected argument '{other}'"))),
                },
            }
        }
        Ok(cli)
    }
}

/// Environment variables iced and the GPU driver read at start-up: `ICED_BACKEND=tiny-skia` for
/// `--software-render`, and on Linux [`MESA_BGRA8_FIRST`]; plus, when `wayland_drops` (the
/// `[sftp] wayland_drag_and_drop` setting), the move to X11 of [`x11_restart`]. Setting one
/// in-process is `unsafe` (denied outside `fogu_core::winsec`), so the process re-executes itself
/// once with them set. Returns only if no re-exec was needed (or it failed, which is logged and
/// ignored).
pub fn reexec_with_env(cli: &Cli, wayland_drops: bool) {
    let mut wanted: Vec<(&str, OsString)> = Vec::new();
    if cli.software_render {
        wanted.push((BACKEND_ENV, "tiny-skia".into()));
    }
    #[cfg(target_os = "linux")]
    wanted.push((MESA_BGRA8_FIRST, "true".into()));
    let x11 = x11_restart(wayland_drops);
    if x11
        && let Some(display) = std::env::var_os(platform::WAYLAND_DISPLAY).filter(|v| !v.is_empty())
    {
        wanted.push((platform::KEPT_WAYLAND_DISPLAY, display));
    }
    if !x11
        && wanted
            .iter()
            .all(|(key, value)| std::env::var_os(key).is_some_and(|v| v == *value))
    {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("fogu: cannot find own executable to set {wanted:?}; starting without");
        return;
    };
    let mut command = Command::new(exe);
    command.args(std::env::args_os().skip(1)).envs(wanted);
    if x11 {
        command
            .env_remove(platform::WAYLAND_DISPLAY)
            .env_remove(platform::WAYLAND_SOCKET);
    }
    #[cfg(unix)]
    {
        let err = command.exec();
        eprintln!("fogu: re-exec failed ({err}); starting without the environment variables");
    }
    #[cfg(windows)]
    {
        // Windows has no `exec`: start the child (which sees the environment variables and
        // skips this branch), wait for it and relay its exit code.
        match command.spawn() {
            Ok(mut child) => match child.wait() {
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(err) => {
                    eprintln!("fogu: cannot wait for the re-exec ({err}); starting here");
                }
            },
            Err(err) => {
                eprintln!(
                    "fogu: re-exec failed ({err}); starting without the environment variables"
                );
            }
        }
    }
}

/// Whether to restart on `XWayland` (see [`x11_switch`]); the Wayland display, if any, is then
/// kept in [`platform::KEPT_WAYLAND_DISPLAY`] for the programs Fogu starts.
fn x11_restart(enabled: bool) -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let display = std::env::var("DISPLAY").ok();
        x11_switch(enabled, platform::on_wayland(), display.as_deref(), answers)
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = enabled;
        false
    }
}

/// winit 0.30 has no Wayland drops, so under Wayland the SFTP panel cannot take files dropped
/// from a file manager, while X11 can. Restart on `XWayland` when the user `enabled` it, the
/// session is `wayland` and the X server named by `display` `answers` (probed only then).
#[cfg(all(unix, not(target_os = "macos")))]
fn x11_switch(
    enabled: bool,
    wayland: bool,
    display: Option<&str>,
    answers: impl Fn(&std::path::Path) -> bool,
) -> bool {
    enabled
        && wayland
        && display
            .and_then(x_socket)
            .is_some_and(|socket| answers(&socket))
}

/// The socket of a local X display (`:1` or `:1.0` is `/tmp/.X11-unix/X1`); `None` for a
/// remote or malformed `DISPLAY`.
#[cfg(all(unix, not(target_os = "macos")))]
fn x_socket(display: &str) -> Option<PathBuf> {
    let number = display.strip_prefix(':')?.split('.').next()?;
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .then(|| PathBuf::from(format!("/tmp/.X11-unix/X{number}")))
}

/// Whether something listens on `socket`. A file left behind by a crashed `XWayland` exists but
/// refuses connections; starting on X11 there would make winit panic before the first frame.
/// shortcut: a blocking connect; an X server hung with a full backlog delays start-up, as it
/// would delay any X client.
#[cfg(all(unix, not(target_os = "macos")))]
fn answers(socket: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(test)]
mod tests {

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, Stop> {
        Cli::parse_from(args.iter().map(OsString::from))
    }

    #[test]
    fn flags_parse() {
        let cli = parse(&[
            "--config-dir",
            "/tmp/x",
            "--software-render",
            "--exit-after-first-frame",
            "--no-recovery",
        ])
        .unwrap();
        assert_eq!(cli.config_dir, Some(PathBuf::from("/tmp/x")));
        assert!(cli.software_render && cli.exit_after_first_frame && cli.no_recovery);
        let eq = parse(&["--config-dir=/tmp/y"]).unwrap();
        assert_eq!(eq.config_dir, Some(PathBuf::from("/tmp/y")));
        let none = parse(&[]).unwrap();
        assert!(none.config_dir.is_none() && !none.software_render && !none.no_recovery);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn x11_switch_table() {
        let yes = |_: &std::path::Path| true;
        let no = |_: &std::path::Path| false;
        let unasked = |_: &std::path::Path| -> bool { panic!("probed the X server") };
        assert!(!x11_switch(false, true, Some(":1"), unasked), "setting off");
        assert!(!x11_switch(true, false, Some(":1"), unasked), "already X11");
        assert!(
            !x11_switch(true, true, Some(":1"), no),
            "no X server answering"
        );
        assert!(!x11_switch(true, true, None, yes), "no DISPLAY");
        assert!(
            !x11_switch(true, true, Some("host:0"), yes),
            "remote DISPLAY"
        );
        assert!(x11_switch(true, true, Some(":1"), yes), "switch");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn x_socket_from_display() {
        let sock = |d| x_socket(d).map(|p| p.display().to_string());
        assert_eq!(sock(":1").as_deref(), Some("/tmp/.X11-unix/X1"));
        assert_eq!(sock(":0.0").as_deref(), Some("/tmp/.X11-unix/X0"));
        assert_eq!(sock("host:0"), None);
        assert_eq!(sock(":"), None);
        assert_eq!(sock(":a"), None);
        assert_eq!(sock(""), None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_stale_x_socket_does_not_answer() {
        let dir = std::env::temp_dir().join(format!("fogu-x-socket-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("X9");
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(answers(&socket), "a live server answers");
        drop(listener);
        assert!(socket.exists(), "the file stays behind, as after a crash");
        assert!(!answers(&socket), "but nothing answers on it");
        assert!(!answers(&dir.join("missing")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn help_version_and_errors_stop() {
        assert_eq!(parse(&["--help"]).unwrap_err(), Stop::Help);
        assert_eq!(parse(&["--licenses"]).unwrap_err(), Stop::Licenses);
        assert!(parse(&["--after-update-of", "1"]).is_err());
        assert!(parse(&["--after-update-of", "invalid"]).is_err());
        assert_eq!(
            parse(&["--after-update-of", "123456"])
                .unwrap()
                .after_update_of,
            Some(123_456)
        );
        assert_eq!(parse(&["-V"]).unwrap_err(), Stop::Version);
        assert!(matches!(parse(&["--bogus"]), Err(Stop::Error(_))));
        assert!(matches!(parse(&["--config-dir"]), Err(Stop::Error(_))));
    }
}
