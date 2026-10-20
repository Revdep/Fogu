//! `run_local`: a local shell in a PTY, one task per tab.
//!
//! One `select!` loop: input (never waits behind output), pending writes in ≤ 4 KiB chunks, one
//! PTY read of ≤ 64 KiB per iteration (backpressure: we only read as fast as we parse), the
//! synchronized-update timer, and child exit.

use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;

use fogu_core::{LocalProfile, Size};
use fogu_term::TermHandle;
use pty_process::{OwnedReadPty, OwnedWritePty};
use rustix::process::{Pid, Signal};
use rustix::termios::Winsize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::Child;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::{Instant, sleep_until, timeout, timeout_at};

use crate::env::build_env;
use crate::pipeline::{Gone, Pump, WRITE_CHUNK, canonical_dir};
use crate::{ExitInfo, SessionError, SessionInput, SessionOutput};

/// PTY read buffer.
const READ_CHUNK: usize = 64 * 1024;
/// Time between SIGHUP (closing the master) and SIGKILL of the process group.
const CLOSE_GRACE: Duration = Duration::from_secs(2);
/// Upper bound for draining output after the child exited (a background job may keep the slave).
const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

/// Why the main loop stopped.
enum End {
    Exited(io::Result<ExitStatus>),
    Close,
}

/// On-demand cwd lookup (fallback when the shell reports no OSC 7): the foreground process group's
/// directory, falling back to the shell's own. `None` if neither resolves to a live directory.
async fn current_cwd(master: &OwnedFd, shell_pid: Option<u32>) -> Option<PathBuf> {
    let fg_pid = rustix::termios::tcgetpgrp(master)
        .ok()
        .and_then(|pgid| u32::try_from(pgid.as_raw_nonzero().get()).ok());
    for pid in fg_pid.into_iter().chain(shell_pid) {
        let cwd = if cfg!(target_os = "macos") {
            macos_cwd(pid).await
        } else {
            std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
        };
        if let Some(dir) = cwd.as_deref().and_then(canonical_dir) {
            return Some(dir);
        }
    }
    None
}

/// How long `lsof` / `ps` may take before the lookup gives up (the session's I/O waits on it).
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// `command`'s stdout, `None` if it fails to start or outlives `limit` (then it is killed).
async fn output_within(limit: Duration, command: &mut tokio::process::Command) -> Option<Vec<u8>> {
    // A UTF-8 locale: without one (started from Finder) `lsof` prints `è` as `\xc3\xa8` and
    // `ps` as `M-CM-(`.
    let running = command
        .env_remove("LC_ALL")
        .env("LC_CTYPE", "UTF-8")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(limit, running).await.ok()?.ok()?;
    Some(out.stdout)
}

/// macOS has no `/proc`: `lsof` names the cwd (the `n<path>` line of `-Fn`).
/// shortcut: spawns lsof (~0.2 s, the session's I/O waits meanwhile); only on demand ("new tab
/// here"). `libproc` answers in microseconds if it ever matters.
async fn macos_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;
    // -b -w: no blocking kernel calls (a dead network mount), no warnings about them.
    let stdout = output_within(
        LOOKUP_TIMEOUT,
        tokio::process::Command::new("/usr/sbin/lsof").args([
            "-b",
            "-w",
            "-a",
            "-d",
            "cwd",
            "-Fn",
            "-p",
            &pid.to_string(),
        ]),
    )
    .await?;
    let line = stdout
        .split(|b| *b == b'\n')
        .find_map(|l| l.strip_prefix(b"n"))?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(line)))
}

/// macOS: the program's name from `ps` (it prints the path, the name is its last part).
async fn macos_comm(pid: u32) -> Option<String> {
    let stdout = output_within(
        LOOKUP_TIMEOUT,
        tokio::process::Command::new("/bin/ps").args(["-o", "comm=", "-p", &pid.to_string()]),
    )
    .await?;
    let path = String::from_utf8(stdout).ok()?;
    let name = path.trim().rsplit('/').next()?;
    Some(name.to_owned())
}

/// The program in front of the shell (`/proc/<pgid>/comm`, `ps` on macOS), `None` when the shell
/// itself is.
async fn foreground(master: &OwnedFd, shell_pid: Option<u32>) -> Option<String> {
    let group = rustix::termios::tcgetpgrp(master).ok()?;
    let pid = u32::try_from(group.as_raw_nonzero().get()).ok()?;
    if Some(pid) == shell_pid {
        return None;
    }
    let name = if cfg!(target_os = "macos") {
        macos_comm(pid).await?
    } else {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?
    };
    let name = crate::sanitize_remote(name.trim(), 64, false);
    (!name.is_empty()).then_some(name)
}

/// How often a watched foreground program is checked.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// A spawned shell and our ends of its PTY.
struct Local {
    reader: OwnedReadPty,
    writer: OwnedWritePty,
    /// Dup of the master, kept for `tcsetwinsize` (owned halves don't implement `AsFd`).
    master: OwnedFd,
    child: Child,
    /// The PTY read side hit EOF/EIO.
    eof: bool,
}

/// Session task for a local shell. Errors are reported as `SessionOutput::Error`.
pub(crate) async fn run_local(
    profile: LocalProfile,
    size: Size,
    term: TermHandle,
    mut in_rx: Receiver<SessionInput>,
    out_tx: Sender<SessionOutput>,
) {
    let mut pump = Pump::new(term, out_tx, false);
    let mut local = match spawn(&profile, size) {
        Ok(local) => local,
        Err(err) => {
            tracing::warn!(error = %err, "local shell failed to start");
            let _gone = pump.emit(SessionOutput::Error(err)).await;
            return;
        }
    };
    let mut buf = vec![0_u8; READ_CHUNK];
    let status = match run_loop(&mut local, &mut pump, &mut in_rx, &mut buf).await {
        Ok(End::Exited(status)) => {
            if !local.eof && drain(&mut local.reader, &mut pump, &mut buf).await.is_err() {
                return;
            }
            status
        }
        Ok(End::Close) => close(local).await,
        Err(Gone) => {
            close(local).await.ok();
            return;
        }
    };
    let out = match status {
        Ok(status) => SessionOutput::Exited(ExitInfo {
            code: status.code(),
            signal: status.signal(),
        }),
        Err(err) => SessionOutput::Error(SessionError::Io(Arc::new(err))),
    };
    let _gone = pump.emit(out).await;
}

async fn run_loop(
    local: &mut Local,
    pump: &mut Pump,
    in_rx: &mut Receiver<SessionInput>,
    buf: &mut [u8],
) -> Result<End, Gone> {
    // `WatchForeground`: the program seen in front (`Some(None)` = armed, none seen yet).
    let mut watching: Option<Option<String>> = None;
    let mut watch_tick = tokio::time::interval(WATCH_EVERY);
    loop {
        let sync_deadline = pump.term.sync_timeout();
        let chunk = pump.pending.len().min(WRITE_CHUNK);
        tokio::select! {
            biased;
            input = in_rx.recv() => match input {
                Some(SessionInput::Data(data)) => {
                    if pump.queue_input(&data) {
                        tracing::warn!(len = data.len(), "input backlog full; dropping input");
                        pump.emit(SessionOutput::InputDropped).await?;
                    }
                }
                Some(SessionInput::Resize(size)) => resize(&local.master, size),
                Some(SessionInput::PromptReply(id, _)) => {
                    tracing::debug!(?id, "prompt reply for a local session ignored");
                }
                Some(SessionInput::Forward(_)) => tracing::debug!("forward for a local session ignored"),
                Some(SessionInput::QueryCwd) => {
                    if let Some(path) = current_cwd(&local.master, local.child.id()).await {
                        pump.emit(SessionOutput::Cwd { path, remote: false }).await?;
                    }
                    let name = foreground(&local.master, local.child.id()).await;
                    pump.emit(SessionOutput::Foreground(name)).await?;
                }
                Some(SessionInput::WatchForeground(on)) => {
                    watching = if on {
                        Some(foreground(&local.master, local.child.id()).await)
                    } else {
                        None
                    };
                }
                Some(SessionInput::Close) | None => return Ok(End::Close),
            },
            written = local.writer.write(pump.pending.get(..chunk).unwrap_or_default()),
                if chunk > 0 => match written {
                Ok(n) => pump.written(n),
                Err(err) => {
                    tracing::debug!(error = %err, "PTY write failed; dropping pending input");
                    pump.write_failed();
                }
            },
            read = local.reader.read(buf), if !local.eof => match read {
                Ok(0) => local.eof = true,
                Ok(n) => pump.feed(buf.get(..n).unwrap_or_default()).await?,
                Err(err) => {
                    if !is_eof(&err) {
                        tracing::warn!(error = %err, "PTY read failed");
                    }
                    local.eof = true;
                }
            },
            () = sleep_until(sync_deadline.map_or_else(Instant::now, Instant::from_std)),
                if sync_deadline.is_some() => pump.feed(&[]).await?,
            _ = watch_tick.tick(), if watching.is_some() => {
                let now = foreground(&local.master, local.child.id()).await;
                match (watching.take().flatten(), now) {
                    (Some(name), None) => pump.emit(SessionOutput::ForegroundDone(name)).await?,
                    (_, now) => watching = Some(now),
                }
            }
            status = local.child.wait() => return Ok(End::Exited(status)),
        }
    }
}

/// Reads what the child left in the PTY before it exited, until EOF/EIO or [`DRAIN_TIMEOUT`].
async fn drain(reader: &mut OwnedReadPty, pump: &mut Pump, buf: &mut [u8]) -> Result<(), Gone> {
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    while let Ok(Ok(n)) = timeout_at(deadline, reader.read(buf)).await {
        if n == 0 {
            break;
        }
        pump.feed(buf.get(..n).unwrap_or_default()).await?;
    }
    Ok(())
}

/// Close: drop every master fd (kernel sends SIGHUP), wait ≤ 2 s, SIGKILL the process group,
/// always reap.
async fn close(local: Local) -> io::Result<ExitStatus> {
    let Local {
        reader,
        writer,
        master,
        mut child,
        ..
    } = local;
    drop((reader, writer, master));
    if let Ok(status) = timeout(CLOSE_GRACE, child.wait()).await {
        return status;
    }
    // The child is a session leader (setsid), so its pid is also its process-group id.
    if let Some(pid) = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(Pid::from_raw)
        && let Err(err) = rustix::process::kill_process_group(pid, Signal::KILL)
    {
        tracing::warn!(error = %err, "SIGKILL of the shell's process group failed");
    }
    child.wait().await
}

/// On Linux, reading the master after the last slave fd closed returns `EIO`: that is EOF.
fn is_eof(err: &io::Error) -> bool {
    err.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error())
}

fn winsize(size: Size) -> Winsize {
    // Never hand the kernel a 0×0 size (hidden/minimized panes): clamp to ≥ 2×1.
    Winsize {
        ws_row: size.rows.max(1),
        ws_col: size.cols.max(2),
        ws_xpixel: size.px_w,
        ws_ypixel: size.px_h,
    }
}

fn resize(master: &OwnedFd, size: Size) {
    if let Err(err) = rustix::termios::tcsetwinsize(master, winsize(size)) {
        tracing::warn!(error = %err, "PTY resize failed");
    }
}

fn spawn(profile: &LocalProfile, size: Size) -> Result<Local, SessionError> {
    let program = profile.program.clone().unwrap_or_else(default_shell);
    let spawn_err = |source: io::Error| SessionError::Spawn {
        program: program.clone(),
        source: Arc::new(source),
    };
    let (pty, pts) = pty_process::open().map_err(|e| spawn_err(pty_io(e)))?;
    let ws = winsize(size);
    pty.resize(pty_process::Size::new_with_pixel(
        ws.ws_row,
        ws.ws_col,
        ws.ws_xpixel,
        ws.ws_ypixel,
    ))
    .map_err(|e| spawn_err(pty_io(e)))?;
    let cwd = profile
        .cwd
        .clone()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut cmd = pty_process::Command::new(&program)
        .args(&profile.args)
        .current_dir(cwd)
        .env_clear()
        .envs(build_env(std::env::vars_os(), &profile.env));
    if profile.login {
        cmd = cmd.arg0(login_argv0(&program));
    }
    // `spawn` consumes `pts`, so our copy of the slave is gone and EOF/EIO arrives on exit.
    let child = cmd.spawn(pts).map_err(|e| spawn_err(pty_io(e)))?;
    let master = rustix::io::dup(&pty).map_err(|e| spawn_err(e.into()))?;
    let (reader, writer) = pty.into_split();
    Ok(Local {
        reader,
        writer,
        master,
        child,
        eof: false,
    })
}

fn pty_io(err: pty_process::Error) -> io::Error {
    match err {
        pty_process::Error::Io(err) => err,
        pty_process::Error::Rustix(errno) => errno.into(),
        err @ pty_process::Error::Unsplit(..) => io::Error::other(err.to_string()),
    }
}

/// `$SHELL` → the passwd entry of the current uid → `/bin/sh`.
fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .or_else(|| {
            let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
            shell_from_passwd(&passwd, rustix::process::getuid().as_raw())
        })
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

/// Login shell field of `uid`'s line in an `/etc/passwd`-format file.
fn shell_from_passwd(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let line_uid = fields.nth(2)?.parse::<u32>().ok()?;
        let shell = fields.nth(3)?;
        (line_uid == uid && !shell.is_empty()).then(|| shell.to_owned())
    })
}

/// `"/usr/bin/zsh"` → `"-zsh"`.
fn login_argv0(program: &str) -> String {
    let name = Path::new(program)
        .file_name()
        .map_or_else(|| program.into(), |name| name.to_string_lossy());
    format!("-{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_cwd_and_program_of_a_live_process() {
        // Accented, as Italian paths are: `lsof` escapes them (`\xc3\xa8`) without a locale.
        let base = std::env::temp_dir().canonicalize().unwrap_or_default();
        let dir = base.join(format!("fogu-cwd caffè {}", std::process::id()));
        let _made = std::fs::create_dir_all(&dir);
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .current_dir(&dir)
            .spawn()
            .ok();
        let pid = child.as_ref().map_or(0, std::process::Child::id);
        let cwd = macos_cwd(pid).await.and_then(|p| p.canonicalize().ok());
        let name = macos_comm(pid).await;
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _removed = std::fs::remove_dir(&dir);
        assert_eq!(cwd, Some(dir));
        assert_eq!(name.as_deref(), Some("sleep"));
        assert_eq!(macos_cwd(u32::MAX).await, None, "no such process");
    }

    /// A program with an accented name keeps it (`ps` without a locale prints `dormM-CM-,`).
    /// Needs `cc` (Xcode command line tools) to build that program; skips without it.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_program_names_keep_their_accents() {
        let dir = std::env::temp_dir().join(format!("fogu-comm-{}", std::process::id()));
        let _made = std::fs::create_dir_all(&dir);
        let program = dir.join("dormì");
        let built = std::process::Command::new("cc")
            .args(["-x", "c", "-", "-o"])
            .arg(&program)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut cc| {
                use std::io::Write as _;
                if let Some(stdin) = cc.stdin.as_mut() {
                    stdin
                        .write_all(b"#include <unistd.h>\nint main(void){sleep(5);return 0;}\n")?;
                }
                drop(cc.stdin.take());
                cc.wait()
            })
            .is_ok_and(|status| status.success());
        if !built {
            let _removed = std::fs::remove_dir_all(&dir);
            return;
        }
        let mut child = std::process::Command::new(&program).spawn().ok();
        let pid = child.as_ref().map_or(0, std::process::Child::id);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let name = macos_comm(pid).await;
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _removed = std::fs::remove_dir_all(&dir);
        assert_eq!(name.as_deref(), Some("dormì"));
    }

    /// `lsof` takes ~0.2 s and `ps` runs every second while watching: other tasks must keep
    /// running meanwhile (one thread, so a blocking call would starve the spawned task).
    #[cfg(target_os = "macos")]
    #[test]
    #[expect(clippy::unwrap_used, reason = "test")]
    fn macos_lookups_do_not_block_the_runtime() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let pid = std::process::id();
            for lookup in ["cwd", "comm"] {
                let ran = Arc::new(AtomicBool::new(false));
                let flag = Arc::clone(&ran);
                tokio::spawn(async move { flag.store(true, Ordering::SeqCst) });
                let found = if lookup == "cwd" {
                    macos_cwd(pid).await.is_some()
                } else {
                    macos_comm(pid).await.is_some()
                };
                assert!(found, "{lookup}");
                assert!(ran.load(Ordering::SeqCst), "{lookup} blocked the runtime");
            }
        });
    }

    /// A hung `lsof` (dead network mount) must not hold the session: it is killed at the limit.
    #[tokio::test]
    async fn lookups_give_up_and_kill_the_command_at_the_limit() {
        let started = std::time::Instant::now();
        let mut sleeper = tokio::process::Command::new("/bin/sleep");
        sleeper.arg("7.913");
        let out = output_within(Duration::from_millis(200), &mut sleeper).await;
        assert_eq!(out, None);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let left = std::process::Command::new("pgrep")
            .args(["-f", "sleep 7.913"])
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default();
        assert!(left.is_empty(), "the command still runs");
    }

    #[test]
    fn passwd_lookup_by_uid() {
        let passwd = "root:x:0:0::/root:/bin/bash\n\
                      broken line\n\
                      me:x:1000:1000:Me:/home/me:/usr/bin/zsh\n\
                      noshell:x:1001:1001::/home/n:\n";
        assert_eq!(
            shell_from_passwd(passwd, 1000).as_deref(),
            Some("/usr/bin/zsh")
        );
        assert_eq!(shell_from_passwd(passwd, 0).as_deref(), Some("/bin/bash"));
        assert_eq!(shell_from_passwd(passwd, 1001), None);
        assert_eq!(shell_from_passwd(passwd, 4242), None);
    }

    #[test]
    fn login_argv0_uses_basename() {
        assert_eq!(login_argv0("/usr/bin/zsh"), "-zsh");
        assert_eq!(login_argv0("bash"), "-bash");
    }

    #[test]
    fn winsize_never_zero() {
        let ws = winsize(Size::default());
        assert_eq!((ws.ws_col, ws.ws_row), (2, 1));
    }
}
