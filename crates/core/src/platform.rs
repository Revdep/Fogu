//! Platform paths and host conventions.
//!
//! Everything Linux-specific that used to be spread across the config, recovery and session
//! code lives here, so the rest of the workspace can use one vocabulary for "the user's
//! config directory", "the state directory", "home" and "`known_hosts`".
//!
//! On Linux these are the XDG directories (and still `$XDG_*`-aware); on Windows they are the
//! roaming (`%APPDATA%`) and local (`%LOCALAPPDATA%`) app-data directories.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf, Prefix};

/// The application's directory name under the platform's config/state root.
pub const APP_DIR: &str = "fogu";

/// Makes winit and most toolkits use Wayland: the compositor's display name.
pub const WAYLAND_DISPLAY: &str = "WAYLAND_DISPLAY";
/// Makes them use Wayland too: a connection inherited from the launcher (a file descriptor).
pub const WAYLAND_SOCKET: &str = "WAYLAND_SOCKET";

/// Where Fogu keeps the session's `WAYLAND_DISPLAY` after restarting itself on `XWayland` (file
/// drops need X11): the programs it starts get it back, so they still reach the compositor.
pub const KEPT_WAYLAND_DISPLAY: &str = "FOGU_WAYLAND_DISPLAY";

/// Whether this process talks to a Wayland compositor: either variable set and not empty.
#[must_use]
pub fn on_wayland() -> bool {
    [WAYLAND_DISPLAY, WAYLAND_SOCKET]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
}

/// The Wayland display Fogu set aside when it restarted on `XWayland`, for the programs it starts
/// (`None` when it did not).
#[must_use]
pub fn kept_wayland_display() -> Option<OsString> {
    std::env::var_os(KEPT_WAYLAND_DISPLAY).filter(|v| !v.is_empty())
}

/// Gives a program Fogu starts the session's Wayland display back (`kept`, from
/// [`kept_wayland_display`]), so it is not moved to `XWayland` with Fogu; a restarted Fogu then
/// decides again from its settings.
pub fn give_back_wayland(command: &mut std::process::Command, kept: Option<&OsStr>) {
    if let Some(display) = kept {
        command
            .env(WAYLAND_DISPLAY, display)
            .env_remove(KEPT_WAYLAND_DISPLAY);
    }
}

/// Library overrides that belong to Fogu when it runs as an `AppImage`: its `AppRun` puts its
/// bundled X11/xkb libraries on `LD_LIBRARY_PATH` when the host lacks some.
pub const APPIMAGE_LIB_VARS: &[&str] = &["LD_LIBRARY_PATH", "LD_PRELOAD"];

/// Whether Fogu runs as an `AppImage` (its runtime sets `APPIMAGE`).
#[must_use]
pub fn in_appimage() -> bool {
    std::env::var_os("APPIMAGE").is_some()
}

/// Keeps [`APPIMAGE_LIB_VARS`] from a program Fogu starts when `appimage` ([`in_appimage`]): a
/// desktop app opened from Fogu loads the host's libraries, not the `AppImage`'s older ones.
pub fn drop_appimage_libs(command: &mut std::process::Command, appimage: bool) {
    if appimage {
        for key in APPIMAGE_LIB_VARS {
            command.env_remove(key);
        }
    }
}

/// The user's home directory.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// `$XDG_CONFIG_HOME/fogu` on Unix, `%APPDATA%\fogu` on Windows.
#[must_use]
pub fn config_dir() -> PathBuf {
    #[cfg(unix)]
    {
        xdg::BaseDirectories::with_prefix(APP_DIR)
            .get_config_home()
            .unwrap_or_else(|| PathBuf::from(".config/fogu"))
    }
    #[cfg(windows)]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_DIR)
    }
}

/// `$XDG_STATE_HOME/fogu` on Unix (`~/.local/state/fogu` by default), `%LOCALAPPDATA%\fogu`
/// on Windows. `None` when no base directory is known: never a path relative to the working
/// directory.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local/state")));
    #[cfg(windows)]
    let base = dirs::data_local_dir();
    base.map(|base| base.join(APP_DIR))
}

/// The computer's name, without a domain (`P14S` for `P14S.lan`): the built-in default shell's
/// name. `None` when the system reports none.
#[must_use]
pub fn host_name() -> Option<String> {
    #[cfg(unix)]
    let name = rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned();
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok()?;
    short_host(&name)
}

/// `name` up to its first dot, trimmed; `None` when that leaves nothing.
fn short_host(name: &str) -> Option<String> {
    let short = name.split('.').next().unwrap_or_default().trim();
    (!short.is_empty()).then(|| short.to_owned())
}

/// The user's download directory, used by SFTP double-click downloads.
#[must_use]
pub fn download_dir() -> Option<PathBuf> {
    dirs::download_dir()
}

/// The per-user OpenSSH client configuration.
#[must_use]
pub fn ssh_config_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".ssh").join("config"))
}

/// `known_hosts` files to consult, most specific first: Fogu's own file under [`config_dir`],
/// then the user's `~/.ssh/known_hosts`, then the system-wide file.
#[must_use]
pub fn known_hosts_files() -> Vec<PathBuf> {
    let mut out = vec![config_dir().join("known_hosts")];
    if let Some(home) = home_dir() {
        out.push(home.join(".ssh").join("known_hosts"));
    }
    if let Some(system) = system_known_hosts() {
        out.push(system);
    }
    out
}

/// The system-wide `known_hosts`, if the platform has one.
#[must_use]
pub fn system_known_hosts() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/ssh/ssh_known_hosts"))
    }
    #[cfg(windows)]
    {
        std::env::var_os("ProgramData").map(|program_data| {
            PathBuf::from(program_data)
                .join("ssh")
                .join("ssh_known_hosts")
        })
    }
}

/// Whether looking at `path` stays on this machine: `false` for a Windows UNC or device path
/// (`\\host\share`, `//host/share`, `\\?\UNC\…`, `\??\UNC\…`, `\\.\pipe\…`), where even a metadata
/// lookup connects to the host and sends the user's NTLM credentials. Always `true` on Unix.
#[must_use]
pub fn is_local_path(path: &Path) -> bool {
    // `\??\…` names the NT object namespace: Windows passes it through as is (so
    // `\??\UNC\host\share` is a share), while Rust parses it as a plain root.
    let bytes = path.as_os_str().as_encoded_bytes();
    let nt = matches!(bytes, [b'\\' | b'/', b'?', b'?', b'\\' | b'/', ..]);
    if cfg!(windows) && nt {
        return false;
    }
    match path.components().next() {
        Some(Component::Prefix(prefix)) => {
            matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        }
        _ => true,
    }
}

/// The first `name` in an absolute `PATH` directory. Relative entries (`.`, an empty entry) are
/// skipped: they name the current directory, where anyone could drop a `pwsh.exe`.
#[must_use]
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    find_in(&std::env::var_os("PATH")?, name)
}

fn find_in(paths: &OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn host_names_lose_their_domain() {
        assert_eq!(short_host("P14S.lan").as_deref(), Some("P14S"));
        assert_eq!(short_host("mac-mini.local").as_deref(), Some("mac-mini"));
        assert_eq!(short_host("DESKTOP-1").as_deref(), Some("DESKTOP-1"));
        assert_eq!(short_host(" "), None);
        assert_eq!(short_host(".lan"), None);
    }

    #[test]
    fn app_directories_end_with_the_app_name() {
        assert!(config_dir().ends_with(APP_DIR));
        assert!(state_dir().is_none_or(|dir| dir.ends_with(APP_DIR)));
    }

    #[test]
    fn path_lookup_skips_relative_dirs() {
        // Tests run in the crate directory, so `src/lib.rs` exists relative to it.
        let abs = std::env::current_dir().unwrap().join("src");
        assert_eq!(find_in("src".as_ref(), "lib.rs"), None);
        assert_eq!(find_in(abs.as_os_str(), "lib.rs"), Some(abs.join("lib.rs")));
    }

    #[test]
    fn network_and_device_paths_are_not_local() {
        let remote = [
            r"\\host\share\x",
            "//host/share/x",
            r"/\host\share",
            r"\\?\UNC\host\share",
            r"\\.\pipe\x",
            r"\\?\GLOBALROOT\x",
            r"\??\UNC\host\share",
            "/??/UNC/host/share",
        ];
        for p in remote {
            assert_eq!(is_local_path(p.as_ref()), cfg!(unix), "{p}");
        }
        for p in [r"C:\x", r"\\?\C:\x", r"rel\x", r"\x", "/home/x"] {
            assert!(is_local_path(p.as_ref()), "{p}");
        }
    }

    #[test]
    fn known_hosts_starts_with_fogus_own_file() {
        let files = known_hosts_files();
        assert_eq!(files.first(), Some(&config_dir().join("known_hosts")));
    }

    #[test]
    fn programs_lose_the_appimage_libraries_only_under_an_appimage() {
        let mut cmd = std::process::Command::new("true");
        drop_appimage_libs(&mut cmd, true);
        let removed: Vec<_> = cmd.get_envs().collect();
        assert_eq!(
            removed,
            [
                (OsStr::new("LD_LIBRARY_PATH"), None),
                (OsStr::new("LD_PRELOAD"), None)
            ]
        );
        let mut untouched = std::process::Command::new("true");
        drop_appimage_libs(&mut untouched, false);
        assert_eq!(untouched.get_envs().count(), 0, "the user's own LD_* stay");
    }

    #[test]
    fn programs_get_the_wayland_display_back_and_not_the_kept_copy() {
        let envs = |cmd: &std::process::Command| {
            cmd.get_envs()
                .map(|(k, v)| (k.to_owned(), v.map(OsStr::to_owned)))
                .collect::<Vec<_>>()
        };
        let mut kept = std::process::Command::new("true");
        give_back_wayland(&mut kept, Some(OsStr::new("wayland-1")));
        let set = envs(&kept);
        assert!(set.contains(&(WAYLAND_DISPLAY.into(), Some("wayland-1".into()))));
        assert!(
            set.contains(&(KEPT_WAYLAND_DISPLAY.into(), None)),
            "removed for the child"
        );
        let mut untouched = std::process::Command::new("true");
        give_back_wayland(&mut untouched, None);
        assert_eq!(envs(&untouched), Vec::new(), "no kept display: nothing set");
    }
}
