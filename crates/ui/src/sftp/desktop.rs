//! The SFTP panel's touch points with the desktop: files dropped onto the window are uploaded
//! into the panel's directory (X11 and macOS: winit 0.30 has no Wayland drops), downloaded files are
//! opened with `xdg-open` only when the user clicks "Open" / "Download and open" / "Show in
//! file manager" (never automatically), and the file pickers are checked for
//! before use (rfd opens nothing, silently, when neither the portal nor zenity exists).

use std::path::{Path, PathBuf};
use std::time::Duration;

use fogu_session::sftp::OverwritePolicy;
use fogu_session::{SftpCmd, display_name};
use iced::Task;

use super::transfer_label;
use super::transfers::{Direction, Origin};
use crate::app::App;
use crate::message::Message;
use crate::notifications::{Level, Notification};

/// Drop events of one gesture arrive one file at a time: they are gathered this long into one
/// upload.
const DROP_BATCH: Duration = Duration::from_millis(150);

/// Desktop messages.
#[derive(Clone, Debug)]
pub enum DesktopMsg {
    /// A file was dropped onto the window (X11).
    Dropped(PathBuf),
    /// The drop batch is complete: upload it.
    FlushDrops,
    /// Open a local file or folder with the desktop's default application.
    Open(PathBuf),
    /// The opener failed (sanitized reason).
    OpenFailed(String),
}

/// Per-window desktop state.
#[derive(Debug, Default)]
pub struct Desktop {
    /// Dropped files waiting for the batch to end.
    dropped: Vec<PathBuf>,
    /// The Wayland "drop needs X11" hint was shown.
    #[cfg_attr(windows, allow(dead_code, reason = "Wayland-only hint state"))]
    hinted: bool,
}

impl App {
    pub(crate) fn on_desktop(&mut self, message: DesktopMsg) -> Task<Message> {
        match message {
            DesktopMsg::Dropped(path) => {
                let first = self.desktop.dropped.is_empty();
                self.desktop.dropped.push(path);
                if first {
                    return self.after(DROP_BATCH, Message::Desktop(DesktopMsg::FlushDrops));
                }
                Task::none()
            }
            DesktopMsg::FlushDrops => self.upload_dropped(),
            DesktopMsg::Open(path) => self.open_local(path),
            DesktopMsg::OpenFailed(why) => {
                self.notify(Notification::new(Level::Error, "Cannot open").detail(why))
            }
        }
    }

    /// Uploads the dropped files into the active tab's visible panel directory. winit reports
    /// no drop position on X11, so the panel of the active tab is the target whenever it is
    /// open.
    fn upload_dropped(&mut self) -> Task<Message> {
        let local = std::mem::take(&mut self.desktop.dropped);
        let tab = self.active;
        let Some((panel, tx)) = self.panel_of(tab).filter(|(p, _)| p.visible) else {
            let note = Notification::new(Level::Info, "Dropped files")
                .detail("Open the SFTP panel of an SSH tab to upload dropped files.");
            return self.notify(note);
        };
        let Some(remote_dir) = panel.path.clone() else {
            return Task::none();
        };
        let cmd = SftpCmd::Upload {
            local: local.clone(),
            remote_dir: remote_dir.clone(),
            overwrite: OverwritePolicy::Ask,
            resume: false,
        };
        let label = transfer_label(&cmd);
        let origin = Origin::Upload { local, remote_dir };
        match panel.issue(&tx, cmd, Direction::Upload, label, origin) {
            Ok(_) => Task::none(),
            Err(err) => self.notify(Notification::new(Level::Error, "SFTP").detail(err)),
        }
    }

    /// Once per run on Wayland, when a panel opens: drops need X11 for now. Windows and X11
    /// deliver drops directly, so no hint is needed there.
    #[cfg_attr(
        windows,
        allow(clippy::unused_self, reason = "the Wayland hint is a no-op on Windows")
    )]
    pub(super) fn wayland_drop_hint(&mut self) -> Task<Message> {
        #[cfg(unix)]
        {
            let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
            if !wayland || self.desktop.hinted {
                return Task::none();
            }
            self.desktop.hinted = true;
            // On but still on Wayland: not restarted yet, or no XWayland answered at start-up.
            let detail = if self.config.sftp.wayland_drag_and_drop {
                "Dropping files onto the panel needs X11: restart Fogu (XWayland must be running), \
                 or use Upload\u{2026}"
            } else {
                "Dropping files onto the panel needs X11 for now: turn on Settings \u{2192} SFTP \u{2192} \
                 Enable drag & drop on Wayland, or use Upload\u{2026}"
            };
            let note = Notification::new(Level::Info, "Drag and drop").detail(detail);
            self.notify(note)
        }
        #[cfg(windows)]
        {
            Task::none()
        }
    }

    /// Opens `path` with the desktop's default application (detached; the exit status is
    /// collected on the session runtime). Only for local paths the app wrote or chose, only on
    /// an explicit click, and never an executable file (see [`refuse_open`]).
    pub(crate) fn open_local(&self, path: PathBuf) -> Task<Message> {
        if let Err(why) = refuse_open(&path) {
            return Task::done(Message::Desktop(DesktopMsg::OpenFailed(why)));
        }
        self.xdg_open(path.into_os_string())
    }

    /// Opens a web or mail link (already checked by `open_link::resolve`) with `xdg-open`.
    pub(crate) fn open_url(&self, url: String) -> Task<Message> {
        self.xdg_open(url.into())
    }

    fn xdg_open(&self, target: std::ffi::OsString) -> Task<Message> {
        let run = self
            .runtime
            .handle()
            .spawn(async move { open_target(&target).await });
        Task::future(run).then(|result| {
            let why = match result {
                Ok(Ok(())) => return Task::none(),
                Ok(Err(why)) => why,
                Err(_) => "the opener task failed".to_owned(),
            };
            Task::done(Message::Desktop(DesktopMsg::OpenFailed(why)))
        })
    }
}

/// Extensions whose default "opener" may run them as code (`java -jar`, desktop launchers,
/// interpreters, Wine, package installers): never opened from here, even without an
/// executable bit. Web pages are left out: the browser sandboxes them.
const LAUNCHABLE: &[&str] = &[
    "jar",
    "desktop",
    "appimage",
    "run",
    "bin",
    "sh",
    "bash",
    "zsh",
    "fish",
    "csh",
    "py",
    "pl",
    "rb",
    "php",
    "exe",
    "msi",
    "bat",
    "cmd",
    "com",
    "ps1",
    "vbs",
    "deb",
    "rpm",
    "flatpakref",
    "flatpakrepo",
    "kwinscript",
    "plasmoid",
];

/// macOS: `open` also runs or installs these, or (`fileloc`, `inetloc`, `webloc`) opens what
/// they point at; `.app`/`.workflow` are bundles (directories), so refused whatever their type.
const LAUNCHABLE_MACOS: &[&str] = &[
    "app",
    "pkg",
    "mpkg",
    "dmg",
    "command",
    "tool",
    "terminal",
    "workflow",
    "scpt",
    "fileloc",
    "inetloc",
    "webloc",
    "prefpane",
    "saver",
    "mobileconfig",
    "shortcut",
];

/// Why `path` must not be handed to the desktop opener: not absolute, missing, a regular file with an
/// executable bit, or one of the [`LAUNCHABLE`] types. Downloads are `0644` unless
/// `keep_executable_bit` is on, but the opener picks by type: a downloaded `.jar` would be run
/// by `java -jar`, a `.desktop` file by the launcher. Such files are only shown in the file
/// manager.
fn refuse_open(path: &Path) -> Result<(), String> {
    let shown = || display_name(&path.to_string_lossy());
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) if path.is_absolute() => meta,
        _ => return Err(format!("{} does not exist", shown())),
    };
    let ext = opened_extension(path);
    let listed = |list: &[&str]| ext.as_deref().is_some_and(|e| list.contains(&e));
    // Windows: a stream name (`x.exe::$DATA`, `notes.txt:x.exe`) hides what actually runs.
    let stream = cfg!(windows)
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().contains(':'));
    let launchable =
        stream || listed(LAUNCHABLE) || ext.as_deref().is_some_and(is_windows_executable);
    let bundle = cfg!(target_os = "macos") && listed(LAUNCHABLE_MACOS);
    if bundle || meta.is_file() && (is_executable(&meta) || launchable) {
        return Err(format!(
            "{} could run code and is not opened from here; use \u{201c}Show in file \
             manager\u{201d}",
            shown()
        ));
    }
    Ok(())
}

/// The lower-case extension the desktop opens `path` as. Windows ignores trailing dots and
/// spaces in a name (`setup.exe.` opens `setup.exe`), so they are dropped there first. A name
/// that is only an extension counts too: `ShellExecute` runs a file called `.exe`, and the
/// `*.jar` glob matches `.jar`.
fn opened_extension(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    #[cfg(windows)]
    let name = name.trim_end_matches(['.', ' ']);
    let (_, ext) = name.rsplit_once('.')?;
    (!ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// Windows `ShellExecute` runs, installs or follows these extensions (programs, scripts,
/// installers, shortcuts, registry and control-panel files), plus whatever `PATHEXT` adds; a
/// downloaded file must not start just because the user double-clicked it in Fogu. `ext` is
/// lower case.
fn is_windows_executable(ext: &str) -> bool {
    #[cfg(windows)]
    {
        const BLOCKED: &[&str] = &[
            "exe",
            "com",
            "bat",
            "cmd",
            "ps1",
            "psm1",
            "psd1",
            "vbs",
            "vbe",
            "vb",
            "js",
            "jse",
            "wsf",
            "wsh",
            "ws",
            "msi",
            "msp",
            "msc",
            "msix",
            "msixbundle",
            "appx",
            "appxbundle",
            "appinstaller",
            "lnk",
            "url",
            "scf",
            "pif",
            "scr",
            "hta",
            "reg",
            "cpl",
            "inf",
            "sct",
            "chm",
            "application",
            "appref-ms",
            "settingcontent-ms",
            "gadget",
        ];
        BLOCKED.contains(&ext)
            || std::env::var("PATHEXT").is_ok_and(|list| {
                list.split(';')
                    .map(|e| e.trim().trim_start_matches('.'))
                    .any(|e| !e.is_empty() && e.eq_ignore_ascii_case(ext))
            })
    }
    #[cfg(not(windows))]
    {
        let _ = ext;
        false
    }
}

/// Whether opening this file could execute it. Unix looks at the execute bits; Windows looks
/// at the extension, because `ShellExecute` runs `.exe`, scripts and installers.
fn is_executable(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    {
        let _ = meta;
        false
    }
}

/// Opens a path or URL with the desktop's default application.
async fn open_target(target: &std::ffi::OsStr) -> Result<(), String> {
    let target = target.to_owned();
    let shown = display_name(&target.to_string_lossy());
    tokio::task::spawn_blocking(move || open::that(&target))
        .await
        .map_err(|err| format!("open: {err}"))?
        .map_err(|err| format!("could not open {shown}: {err}"))
}

/// Whether a file picker can open: always on Windows and macOS (native dialog), else `zenity`
/// on `PATH`, or a session bus with the desktop portal service installed.
pub(crate) fn picker_available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(unix)]
    {
        cfg!(target_os = "macos") || in_path("zenity") || portal_installed()
    }
}

/// The toast shown instead of a picker that cannot open.
pub(crate) fn no_picker_note() -> Notification {
    #[cfg(windows)]
    {
        Notification::new(Level::Error, "No file picker available")
            .detail("The Windows file dialog could not be opened.")
    }
    #[cfg(unix)]
    {
        Notification::new(Level::Error, "No file picker available").detail(
            "Install xdg-desktop-portal (with a backend such as xdg-desktop-portal-gtk) or zenity. \
             Double-click downloads still work, and on X11 files can be dropped onto the panel.",
        )
    }
}

#[cfg(unix)]
fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

#[cfg(unix)]
fn portal_installed() -> bool {
    let runtime_bus =
        std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|dir| Path::new(&dir).join("bus").exists());
    let bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() || runtime_bus;
    let service = "dbus-1/services/org.freedesktop.portal.Desktop.service";
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    bus && data_home
        .into_iter()
        .chain(std::env::split_paths(&data_dirs))
        .any(|dir| dir.join(service).is_file())
}

#[cfg(all(test, windows))]
mod windows_tests {

    use super::refuse_open;

    #[test]
    fn windows_launchers_are_not_opened_even_behind_a_trailing_dot() {
        let dir = std::env::temp_dir().join(format!("fogu-open-win-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "setup.exe",
            "saver.SCR",
            "x.hta",
            "x.reg",
            "x.url",
            "x.lnk",
            "x.application",
            "x.appref-ms",
            ".exe",
            ".cmd",
        ] {
            std::fs::write(dir.join(name), "x").unwrap();
            let why = refuse_open(&dir.join(name)).unwrap_err();
            assert!(why.contains("file manager"), "{name}: {why}");
        }
        // Windows drops trailing dots and spaces: this opens setup.exe.
        assert!(refuse_open(&dir.join("setup.exe.")).is_err());
        assert!(refuse_open(&dir.join("setup.exe .")).is_err());
        // A stream name opens the same file: `setup.exe::$DATA` is setup.exe.
        assert!(refuse_open(&dir.join("setup.exe::$DATA")).is_err());
        for name in ["photo.png", "notes.txt", "doc.pdf"] {
            std::fs::write(dir.join(name), "x").unwrap();
            assert_eq!(refuse_open(&dir.join(name)), Ok(()), "{name}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(all(test, unix))]
mod tests {

    use std::os::unix::fs::PermissionsExt as _;

    use super::refuse_open;

    #[test]
    fn executables_and_missing_paths_are_not_opened() {
        let dir = std::env::temp_dir().join(format!("fogu-open-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("report.txt");
        std::fs::write(&plain, "x").unwrap();
        let script = dir.join("run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        assert_eq!(refuse_open(&plain), Ok(()));
        assert_eq!(refuse_open(&dir), Ok(()), "directories open (file manager)");
        assert!(refuse_open(&script).unwrap_err().contains("could run code"));
        assert!(
            refuse_open(&dir.join("missing"))
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(refuse_open(std::path::Path::new("report.txt")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn macos_bundles_and_installers_are_not_opened() {
        let dir = std::env::temp_dir().join(format!("fogu-open-mac-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&dir);
        let app = dir.join("Evil.app");
        std::fs::create_dir_all(&app).unwrap();
        let dmg = dir.join("setup.DMG");
        std::fs::write(&dmg, "x").unwrap();
        let mut paths = vec![app, dmg];
        // Bundles `open` installs, and files that point it at something to run.
        for name in [
            "x.fileloc",
            "x.inetloc",
            "x.webloc",
            "x.prefPane",
            "x.saver",
            "x.mobileconfig",
            "x.shortcut",
        ] {
            let path = dir.join(name);
            std::fs::write(&path, "x").unwrap();
            paths.push(path);
        }
        for path in &paths {
            assert_eq!(
                refuse_open(path).is_err(),
                cfg!(target_os = "macos"),
                "{}",
                path.display()
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn launchable_types_are_not_opened_even_without_exec_bit() {
        let dir = std::env::temp_dir().join(format!("fogu-open-types-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "report.jar",
            "x.desktop",
            "a.AppImage",
            "tool.EXE",
            "s.sh",
            "x.py",
            ".jar",
            ".desktop",
        ] {
            let path = dir.join(name);
            std::fs::write(&path, "x").unwrap();
            let why = refuse_open(&path).unwrap_err();
            assert!(why.contains("file manager"), "{name}: {why}");
        }
        for name in [
            "photo.png",
            "notes.txt",
            "data.tar.gz",
            "doc.pdf",
            "page.html",
        ] {
            let path = dir.join(name);
            std::fs::write(&path, "x").unwrap();
            assert_eq!(refuse_open(&path), Ok(()), "{name}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
