//! The SFTP engine's public surface: the UI sends [`SftpRequest`]s and receives
//! [`SftpEvent`]s over the bounded channel pair in [`SftpHandle`] (same shape as
//! [`SessionHandle`](crate::SessionHandle)). The engine itself lives in `ssh/sftp/` and runs on
//! the tab's existing SSH connection.
//!
//! Every `String` that came from the server (names, paths, link targets, error texts) is
//! attacker-controlled. Events carry the raw names because the UI needs them to address the
//! entry again; anything shown to the user goes through [`display_name`] first. Error texts are
//! already sanitized.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc::{Receiver, Sender};

/// Longest name [`display_name`] shows before cutting it with `…`.
pub const DISPLAY_NAME_MAX_CHARS: usize = 255;
/// Linux `NAME_MAX`: longest single path component in bytes.
pub const NAME_MAX: usize = 255;

/// The UI's side of one connection's SFTP engine. Present only on SSH sessions.
#[derive(Debug)]
pub struct SftpHandle {
    /// Commands; the SFTP channel is opened on the first one.
    pub cmds: Sender<SftpRequest>,
    /// Results, progress and errors.
    pub events: Receiver<SftpEvent>,
}

/// Identifies a command and the events answering it; chosen by the UI (unique per handle).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CmdId(pub u64);

/// A transfer is identified by the id of the `Download`/`Upload` command that started it.
pub type TransferId = CmdId;

/// One command with the id its answer will carry.
#[derive(Debug)]
pub struct SftpRequest {
    /// Echoed in every event about this command.
    pub id: CmdId,
    /// What to do.
    pub cmd: SftpCmd,
}

/// What to do when a transfer's target already exists. There is no silent overwrite: `Ask`
/// skips the file and reports [`SftpEvent::Conflict`] so the UI can ask and re-issue the
/// command for the chosen files with `Overwrite` (or not).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverwritePolicy {
    /// Skip the file and report a conflict.
    #[default]
    Ask,
    /// Replace the existing target (the user confirmed).
    Overwrite,
    /// Skip the file silently (listed in the report).
    Skip,
    /// Keep the existing target and write under the first free name `name (1).ext`,
    /// `name (2).ext`, … (up to [`KEEP_BOTH_MAX`]); for downloads `<new name>.part` must be free
    /// too. A name taken meanwhile is reported as a conflict, like `Ask`.
    KeepBoth,
}

/// Highest number [`OverwritePolicy::KeepBoth`] tries before the file fails.
pub const KEEP_BOTH_MAX: u32 = 999;

/// UI → engine commands. Remote paths are SFTP paths (`/`-separated strings); relative paths
/// are relative to the login directory.
#[derive(Debug)]
pub enum SftpCmd {
    /// List a directory → [`SftpEvent::Listing`].
    List(String),
    /// `lstat` one path (symlinks resolved like in a listing) → [`SftpEvent::StatResult`].
    Stat(String),
    /// Canonical absolute path (`"."` = login directory) → [`SftpEvent::Realpath`].
    Realpath(String),
    /// Create a directory → `Done`.
    Mkdir(String),
    /// Rename/move; fails if the target exists (SFTP v3 rename) → `Done`.
    Rename(String, String),
    /// Delete a file or symlink → `Done`.
    RemoveFile(String),
    /// Delete a directory tree; symlinks inside are removed, never followed. The whole tree is
    /// walked (within the walk caps) before anything is deleted → `Done`.
    RemoveDirRecursive(String),
    /// Count what a recursive delete of this directory would remove (same walk, same caps,
    /// symlinks not followed) → [`SftpEvent::Counted`].
    CountDir(String),
    /// Set permission bits (`mode & 0o7777`) → `Done`.
    Chmod(String, u32),
    /// Download files/directories into the existing local directory `local_dir`.
    Download {
        /// Remote paths to fetch (each becomes one entry named like its last component).
        remote: Vec<String>,
        /// Destination directory (must exist).
        local_dir: PathBuf,
        /// Existing local targets.
        overwrite: OverwritePolicy,
        /// Continue from an existing `<name>.part` instead of reporting it as a conflict.
        resume: bool,
    },
    /// Upload local files/directories into the remote directory `remote_dir`.
    Upload {
        /// Local paths to send.
        local: Vec<PathBuf>,
        /// Destination directory (must exist).
        remote_dir: String,
        /// Existing remote targets.
        overwrite: OverwritePolicy,
        /// Append to an existing, shorter remote file instead of reporting it as a conflict
        /// (the remote bytes are taken as a prefix of the local file).
        resume: bool,
    },
    /// Cancel a running transfer → `TransferCancelled` (or nothing if it already ended).
    Cancel(TransferId),
    /// Replace the engine options (applies to transfers started afterwards) → `Done`.
    SetOptions(SftpOptions),
    /// The panel closed: close the SFTP channel once no transfer runs → `Closed`. The next
    /// command opens a new one.
    Shutdown,
}

/// Engine settings (the UI maps the user's config onto these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpOptions {
    /// Files transferred at the same time per connection (≥ 1).
    pub max_parallel_files: usize,
    /// Keep `<name>.part` when a download is cancelled or fails (for a later resume).
    pub keep_partial: bool,
    /// How long one SFTP request may wait for its reply.
    pub request_timeout: Duration,
    /// Entries per listing; the rest are dropped and the listing is marked `truncated`.
    pub max_listing_entries: usize,
    /// Entries one recursive transfer/delete may walk before it fails.
    pub max_walk_entries: usize,
    /// Directory depth a recursive transfer/delete may walk before it fails.
    pub max_walk_depth: usize,
    /// Disk-fill protection: by default a download is capped at the size the server reported
    /// when the transfer was planned (no size reported = 0); a server sending more fails the
    /// file before any byte past the cap is written. `true` lifts that cap for files that grow
    /// while being fetched (live logs); the free-space reserve and `max_download_bytes` still
    /// apply.
    pub allow_growing_files: bool,
    /// Hard limit per downloaded file in bytes, applied even with `allow_growing_files`
    /// (`None` = no limit beyond the reported size).
    pub max_download_bytes: Option<u64>,
    /// Local free space a download must leave on the destination filesystem, in bytes. A fixed
    /// margin, not a share of the disk: the reported-size cap and the "does it fit" check are
    /// what stop a hostile server; this only keeps the system usable.
    pub free_space_reserve_bytes: u64,
    /// Give a downloaded file mode `0755` when the remote mode had an `x` bit. Off by default:
    /// a server-controlled executable bit on a `.desktop` file, script or `AppImage` is
    /// what lets a file manager run it on a double-click; downloads are `0644` like a browser's.
    pub keep_executable_bit: bool,
}

impl Default for SftpOptions {
    fn default() -> Self {
        Self {
            max_parallel_files: 4,
            keep_partial: false,
            request_timeout: Duration::from_secs(60),
            max_listing_entries: 100_000,
            max_walk_entries: 200_000,
            max_walk_depth: 64,
            allow_growing_files: false,
            max_download_bytes: None,
            free_space_reserve_bytes: 1 << 30,
            keep_executable_bit: false,
        }
    }
}

/// Type of a directory entry (from the mode bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Regular file.
    File,
    /// Directory.
    Dir,
    /// Symbolic link (see [`Entry::link_target`] / [`Entry::link_kind`]).
    Symlink,
    /// Device, FIFO, socket, or no type reported.
    Other,
}

/// One remote entry. `name` is raw server data: show it with [`display_name`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// File name (no `/`).
    pub name: String,
    /// What the entry itself is (`lstat`).
    pub kind: EntryKind,
    /// Size in bytes, if reported.
    pub size: Option<u64>,
    /// Modification time (seconds since the epoch), if reported.
    pub mtime: Option<u32>,
    /// Permission bits (`& 0o7777`), if reported.
    pub mode: Option<u32>,
    /// Owner uid, if reported.
    pub uid: Option<u32>,
    /// Group gid, if reported.
    pub gid: Option<u32>,
    /// Symlinks only: where the link points (raw server data), if it could be read.
    pub link_target: Option<String>,
    /// Symlinks only: what the target is (`stat`), `None` if dangling or not resolved.
    pub link_kind: Option<EntryKind>,
}

/// Why a conflict was reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// The target exists.
    Exists,
    /// A `<name>.part` exists (a kept partial download, or an unrelated file of that name);
    /// re-issue with `resume` or `Overwrite`.
    PartialExists,
}

/// Why a remote/local name cannot become a local/remote path component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// Empty name.
    #[error("empty name")]
    Empty,
    /// `.` or `..`.
    #[error("`.` and `..` are not file names")]
    Dot,
    /// Contains `/`.
    #[error("name contains '/'")]
    Slash,
    /// Contains NUL.
    #[error("name contains NUL")]
    Nul,
    /// Contains a control character.
    #[error("name contains a control character")]
    Control,
    /// Contains a bidi override/isolate (it would display differently than it is).
    #[error("name contains a bidi control character")]
    Bidi,
    /// Longer than 255 bytes (Linux `NAME_MAX`), or not valid UTF-8 (uploads).
    #[error("name is too long or not UTF-8")]
    Invalid,
}

/// Why an entry was not transferred (listed in the [`TransferReport`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// A symlink to a directory: never descended (loop/escape protection).
    SymlinkToDir,
    /// A symlink whose target is not a regular file (dangling, device, …).
    SymlinkNotFile,
    /// Not a regular file or directory (device, FIFO, socket, unknown).
    Special,
    /// The name cannot be used locally/remotely.
    InvalidName(NameError),
    /// The target exists (policy `Ask` → a `Conflict` event was sent, or `Skip`).
    Exists(ConflictKind),
}

/// A skipped entry: `path` is relative to the transfer root (raw; display with
/// [`display_name`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// Path relative to the transfer root.
    pub path: String,
    /// Why.
    pub reason: SkipReason,
}

/// A file that failed: `message` is sanitized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    /// Path relative to the transfer root.
    pub path: String,
    /// What went wrong.
    pub message: String,
}

/// Outcome of a transfer, per job. Lists are capped at [`REPORT_MAX_ITEMS`] each; `omitted`
/// counts the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransferReport {
    /// Files completed.
    pub files_done: u64,
    /// Directories created.
    pub dirs_created: u64,
    /// Bytes transferred (this run; a resume counts only the new bytes).
    pub bytes: u64,
    /// Entries not transferred, with the reason.
    pub skipped: Vec<Skipped>,
    /// Files that failed.
    pub failed: Vec<Failed>,
    /// Skipped/failed entries not listed because the lists were full.
    pub omitted: u64,
}

/// Cap of [`TransferReport::skipped`] and [`TransferReport::failed`].
pub const REPORT_MAX_ITEMS: usize = 1000;

/// Engine → UI events.
#[derive(Clone, Debug)]
pub enum SftpEvent {
    /// A directory listing: dirs first, then by name. `truncated` = more than
    /// `max_listing_entries`; `path` is as requested.
    Listing {
        /// The command.
        id: CmdId,
        /// The listed directory.
        path: String,
        /// The entries (no `.`/`..`).
        entries: Vec<Entry>,
        /// Entries were dropped at the cap.
        truncated: bool,
    },
    /// Answer to [`SftpCmd::Stat`]; `entry.name` is the last path component.
    StatResult {
        /// The command.
        id: CmdId,
        /// The path as requested.
        path: String,
        /// Its attributes.
        entry: Entry,
    },
    /// Answer to [`SftpCmd::CountDir`]: entries below `path` (files, links and directories;
    /// the directory itself not included).
    Counted {
        /// The command.
        id: CmdId,
        /// The directory as requested.
        path: String,
        /// Entries a recursive delete would remove besides `path`.
        entries: u64,
    },
    /// Answer to [`SftpCmd::Realpath`].
    Realpath {
        /// The command.
        id: CmdId,
        /// The canonical path (raw server data).
        path: String,
    },
    /// The command succeeded (no data).
    Done(CmdId),
    /// The command failed; `message` is sanitized and capped.
    Error {
        /// The command.
        id: CmdId,
        /// What went wrong.
        message: String,
    },
    /// The transfer's job list is built: `files` to copy, `total` bytes (as reported).
    TransferQueued {
        /// The transfer.
        id: TransferId,
        /// Files to copy.
        files: u64,
        /// Their total size.
        total: u64,
    },
    /// Progress, at most ~10 per second per transfer (lossy when the UI lags).
    TransferProgress {
        /// The transfer.
        id: TransferId,
        /// Bytes done so far (resumed bytes included).
        bytes: u64,
        /// Total bytes.
        total: u64,
        /// Average bytes per second since the transfer started.
        rate: u64,
    },
    /// The transfer finished (some files may have failed or been skipped: see the report).
    TransferDone {
        /// The transfer.
        id: TransferId,
        /// Per-job outcome.
        report: TransferReport,
    },
    /// The transfer could not run at all (e.g. the destination is missing, a walk cap hit).
    TransferFailed {
        /// The transfer.
        id: TransferId,
        /// What went wrong (sanitized).
        message: String,
        /// What happened before it failed.
        report: TransferReport,
    },
    /// The transfer was cancelled; partial files were removed unless `keep_partial`.
    TransferCancelled {
        /// The transfer.
        id: TransferId,
        /// What happened before the cancel.
        report: TransferReport,
    },
    /// A target exists and the policy is `Ask`: the file was skipped. `path` is the target
    /// relative to the transfer root (raw).
    Conflict {
        /// The transfer.
        id: TransferId,
        /// The existing target.
        path: String,
        /// What exists.
        kind: ConflictKind,
        /// Re-issuing with `resume` is plausible: the existing partial file (`<name>.part`
        /// locally, or the remote target of an upload) is a regular file shorter than the
        /// source and was written after the source last changed (when both times are known).
        resumable: bool,
    },
    /// The SFTP channel closed (after `Shutdown`, or because the connection or the server's
    /// SFTP process went away). The next command opens a new one.
    Closed,
}

/// Accepts `name` as one local (or remote) path component, or says why not: empty, `.`, `..`,
/// `/`, NUL, control characters (C0, DEL, C1), bidi overrides/isolates, or longer than
/// [`NAME_MAX`] bytes. Pure: the fuzz target `sftp_name_sanitize` runs it on arbitrary input.
pub fn validate_name(name: &str) -> Result<&str, NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name == "." || name == ".." {
        return Err(NameError::Dot);
    }
    if name.len() > NAME_MAX {
        return Err(NameError::Invalid);
    }
    for c in name.chars() {
        match c {
            '/' => return Err(NameError::Slash),
            '\0' => return Err(NameError::Nul),
            c if c.is_control() => return Err(NameError::Control),
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => return Err(NameError::Bidi),
            _ => {}
        }
    }
    Ok(name)
}

/// `name` made safe to show as plain text: control characters (C0, DEL, C1) and bidi
/// overrides/isolates are escaped as `\xNN` / `\u{NNNN}`, a backslash is doubled (so an
/// escape can't be faked), and the result is cut at [`DISPLAY_NAME_MAX_CHARS`] with `…`.
#[must_use]
pub fn display_name(name: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(name.len().min(4 * DISPLAY_NAME_MAX_CHARS));
    // the limit counts shown chars: an escape is several of them (`\u{202e}` is 8)
    let mut shown = 0;
    let mut piece = String::new();
    for c in name.chars() {
        piece.clear();
        match c {
            '\\' => piece.push_str("\\\\"),
            c if u32::from(c) < 0x100 && c.is_control() => {
                let _infallible = write!(piece, "\\x{:02x}", u32::from(c));
            }
            c if c.is_control()
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                let _infallible = write!(piece, "\\u{{{:04x}}}", u32::from(c));
            }
            c => piece.push(c),
        }
        let len = piece.chars().count();
        if shown + len > DISPLAY_NAME_MAX_CHARS {
            out.push('…');
            break;
        }
        out.push_str(&piece);
        shown += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_escapes_controls_bidi_and_backslash() {
        let table = [
            ("plain.txt", "plain.txt"),
            ("a\nb", "a\\x0ab"),
            ("\x1b[31mred", "\\x1b[31mred"),
            ("del\x7f", "del\\x7f"),
            ("c1\u{9b}", "c1\\x9b"),
            ("evil\u{202e}txt.exe", "evil\\u{202e}txt.exe"),
            ("iso\u{2066}x", "iso\\u{2066}x"),
            ("back\\slash", "back\\\\slash"),
            ("日本語", "日本語"),
        ];
        for (raw, shown) in table {
            assert_eq!(display_name(raw), shown, "{raw:?}");
        }
        let long = "x".repeat(DISPLAY_NAME_MAX_CHARS + 10);
        let shown = display_name(&long);
        assert_eq!(shown.chars().count(), DISPLAY_NAME_MAX_CHARS + 1);
        assert!(shown.ends_with('…'));
        // escapes count at their shown width (nightly fuzz find): never cut inside one
        for raw in [
            "\u{202e}".repeat(DISPLAY_NAME_MAX_CHARS),
            "\\".repeat(DISPLAY_NAME_MAX_CHARS),
        ] {
            let shown = display_name(&raw);
            assert!(
                shown.chars().count() <= DISPLAY_NAME_MAX_CHARS + 1,
                "{}",
                shown.len()
            );
            assert!(shown.ends_with('…'));
        }
    }

    #[test]
    fn validate_name_table() {
        let long = "a".repeat(NAME_MAX);
        let too_long = "a".repeat(NAME_MAX + 1);
        let table: &[(&str, Result<(), NameError>)] = &[
            ("file.txt", Ok(())),
            (".hidden", Ok(())),
            ("...", Ok(())),
            ("with space", Ok(())),
            ("日本語.txt", Ok(())),
            ("back\\slash", Ok(())),
            ("-rf", Ok(())),
            (&long, Ok(())),
            ("", Err(NameError::Empty)),
            (".", Err(NameError::Dot)),
            ("..", Err(NameError::Dot)),
            ("a/b", Err(NameError::Slash)),
            ("/etc", Err(NameError::Slash)),
            ("../../x", Err(NameError::Slash)),
            ("nul\0x", Err(NameError::Nul)),
            ("new\nline", Err(NameError::Control)),
            ("esc\x1b[31m", Err(NameError::Control)),
            ("del\x7f", Err(NameError::Control)),
            ("c1\u{9b}", Err(NameError::Control)),
            ("evil\u{202e}gpj.exe", Err(NameError::Bidi)),
            ("iso\u{2068}x", Err(NameError::Bidi)),
            (&too_long, Err(NameError::Invalid)),
        ];
        for (name, want) in table {
            assert_eq!(validate_name(name).map(|_| ()), *want, "{name:?}");
        }
    }
}
