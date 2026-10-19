//! The per-tab output pipeline shared by every transport: bytes read from the PTY or SSH channel go
//! through [`Pump::feed`] (OSC side scanner → `TermHandle::feed` → terminal events →
//! `SessionOutput`s, `Wakeup` on dirty false→true), and everything the transport must write (user
//! input, terminal replies) queues in [`Pump::pending`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::BytesMut;
#[cfg(feature = "ssh")]
use fogu_core::LoginScript;
use fogu_term::login_script::LoginScriptMatcher;
use fogu_term::{OscEvent, OscScanner, TermEvent, TermHandle};
use tokio::sync::mpsc::Sender;

use crate::{ClipboardRequest, SessionOutput, SessionWarning};

/// Largest single transport write.
pub(crate) const WRITE_CHUNK: usize = 4 * 1024;

/// Terminal replies (`PtyWrite`) are dropped while this much is already waiting to be written,
/// so a program flooding queries without reading its input cannot grow our memory.
pub(crate) const MAX_PENDING_FOR_REPLIES: usize = 1024 * 1024;
/// User input (`SessionInput::Data`) is dropped while this much is already waiting to be
/// written (the program stopped reading its tty); resizes, close and cwd queries keep flowing.
pub(crate) const MAX_PENDING_INPUT: usize = 4 * 1024 * 1024;
/// Login scripts that have not all fired by then are dropped.
const LOGIN_SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
/// Appended to every login-script reply: what the Enter key sends.
#[cfg(feature = "ssh")]
const LOGIN_SCRIPT_NEWLINE: &[u8] = b"\r";

/// The UI dropped its event receiver; stop quietly.
pub(crate) struct Gone;

/// Feeds bytes into the terminal and routes the resulting events.
pub(crate) struct Pump {
    pub(crate) term: TermHandle,
    dirty: Arc<AtomicBool>,
    out_tx: Sender<SessionOutput>,
    /// Bytes waiting to be written to the PTY (user input + terminal replies).
    pub(crate) pending: BytesMut,
    /// Side-channel scanner for OSC codes `Term`/`vte` drop.
    osc: OscScanner,
    /// This machine's hostname, used to tell an OSC 7 report of a local path from one made on a
    /// remote host reached by a plain `ssh` inside this shell.
    /// `None` for SSH sessions: every path they report is remote.
    local_hostname: Option<String>,
    /// `InputDropped` was already reported for the current backlog.
    pub(crate) overflow_reported: bool,
    /// Login scripts still running on the shell's output, and when they started.
    login: Option<(LoginScriptMatcher, Instant)>,
    /// Typed when the shell's first output arrives (the profile's `cwd`).
    startup: Option<Vec<u8>>,
}

impl Pump {
    /// A pump for `term`; `remote` = the transport is a network session (SSH), so OSC 7 paths
    /// are never treated as local.
    pub(crate) fn new(term: TermHandle, out_tx: Sender<SessionOutput>, remote: bool) -> Self {
        Self {
            dirty: term.dirty(),
            term,
            out_tx,
            pending: BytesMut::new(),
            osc: OscScanner::new(),
            local_hostname: (!remote).then(local_hostname),
            overflow_reported: false,
            login: None,
            startup: None,
        }
    }

    // Only SSH profiles have `cwd`/`scripts` to run in the shell (local shells start in theirs).
    #[cfg(feature = "ssh")]
    /// Arms the shell's start-up work: `cd` into `cwd` and the login `scripts`, both run on the
    /// shell's output from now on (not on the "Connecting…" lines fed before).
    pub(crate) async fn start_shell(
        &mut self,
        cwd: Option<&str>,
        scripts: &[LoginScript],
    ) -> Result<(), Gone> {
        if let Some(cwd) = cwd {
            match fogu_core::shellquote::cd_command(cwd) {
                Ok(line) => self.startup = Some(line),
                Err(why) => self.setup_warning(format!("cwd not used: {why}")).await?,
            }
        }
        if !scripts.is_empty() {
            let (matcher, problems) = LoginScriptMatcher::new(scripts, LOGIN_SCRIPT_NEWLINE);
            for problem in problems {
                self.setup_warning(problem).await?;
            }
            self.login = Some((matcher, Instant::now()));
        }
        Ok(())
    }

    async fn setup_warning(&self, why: String) -> Result<(), Gone> {
        tracing::warn!(%why, "shell setup");
        self.emit(SessionOutput::Warning(SessionWarning::ShellSetup(why)))
            .await
    }

    /// Start-up `cd` and login-script replies for a chunk of shell output.
    async fn shell_setup(&mut self, bytes: &[u8]) -> Result<(), Gone> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Some(line) = self.startup.take() {
            self.pending.extend_from_slice(&line);
        }
        let Some((matcher, started)) = &mut self.login else {
            return Ok(());
        };
        if started.elapsed() > LOGIN_SCRIPT_TIMEOUT {
            self.login = None;
            return self
                .setup_warning("login scripts timed out".to_owned())
                .await;
        }
        for reply in matcher.process(bytes) {
            self.pending.extend_from_slice(&reply);
        }
        if matcher.done() {
            self.login = None;
        }
        Ok(())
    }

    /// Queues a terminal reply (`PtyWrite`) unless the backlog is over
    /// [`MAX_PENDING_FOR_REPLIES`].
    fn queue_reply(&mut self, reply: &[u8]) {
        if self.pending.len() < MAX_PENDING_FOR_REPLIES {
            self.pending.extend_from_slice(reply);
        }
    }

    /// Marks `n` pending bytes as written.
    pub(crate) fn written(&mut self, n: usize) {
        bytes::Buf::advance(&mut self.pending, n);
        if self.pending.is_empty() {
            self.overflow_reported = false;
        }
    }

    /// Drops everything pending (the transport can no longer be written).
    pub(crate) fn write_failed(&mut self) {
        self.pending.clear();
        self.overflow_reported = false;
    }

    /// Queues user input unless the backlog is over [`MAX_PENDING_INPUT`]. Returns `true` the
    /// first time input is dropped for the current backlog (to report it once).
    pub(crate) fn queue_input(&mut self, data: &[u8]) -> bool {
        if self.pending.len().saturating_add(data.len()) <= MAX_PENDING_INPUT {
            self.pending.extend_from_slice(data);
            return false;
        }
        !std::mem::replace(&mut self.overflow_reported, true)
    }

    pub(crate) async fn feed(&mut self, bytes: &[u8]) -> Result<(), Gone> {
        self.shell_setup(bytes).await?;
        // Non-consuming: `bytes` is fed to the terminal unchanged below.
        let osc_events = self.osc.scan(bytes);
        self.term.feed(bytes);
        for event in osc_events {
            if let Some(out) = self.map_osc(event) {
                self.emit(out).await?;
            }
        }
        for event in self.term.drain_events() {
            let out = match event {
                TermEvent::PtyWrite(reply) => {
                    self.queue_reply(&reply);
                    continue;
                }
                TermEvent::Title(title) => SessionOutput::Title(title),
                TermEvent::ResetTitle => SessionOutput::Title(String::new()),
                TermEvent::Bell => SessionOutput::Bell,
                TermEvent::ClipboardStore {
                    target,
                    payload,
                    len,
                } => SessionOutput::ClipboardStore(ClipboardRequest {
                    target,
                    payload,
                    len,
                }),
                TermEvent::CursorBlinkingChange => continue,
            };
            self.emit(out).await?;
        }
        if !self.dirty.swap(true, Ordering::AcqRel) {
            self.emit(SessionOutput::Wakeup).await?;
        }
        Ok(())
    }

    /// A sender for tasks that report on their own (port forwards).
    #[cfg(feature = "ssh")]
    pub(crate) fn sender(&self) -> Sender<SessionOutput> {
        self.out_tx.clone()
    }

    pub(crate) async fn emit(&self, out: SessionOutput) -> Result<(), Gone> {
        self.out_tx.send(out).await.map_err(|_| Gone)
    }

    /// Maps a scanner event to a `SessionOutput`, logging the ones that have no owner yet.
    fn map_osc(&self, event: OscEvent) -> Option<SessionOutput> {
        Some(match event {
            OscEvent::Cwd { host, path } => {
                let remote = self
                    .local_hostname
                    .as_deref()
                    .is_none_or(|local| !is_local_host(&host, local));
                let path = if remote {
                    Some(path)
                } else {
                    canonical_dir(&path)
                }?;
                tracing::debug!(path = %path.display(), remote, "cwd");
                SessionOutput::Cwd { path, remote }
            }
            OscEvent::Progress(pct) => {
                tracing::debug!(?pct, "progress");
                SessionOutput::Progress(pct)
            }
            OscEvent::Notify { title, body } => {
                tracing::debug!(?title, %body, "notify (no UI yet)");
                SessionOutput::Notify { title, body }
            }
            OscEvent::Prompt(mark) => {
                tracing::debug!(?mark, "prompt mark (unused)");
                SessionOutput::PromptMark(mark)
            }
        })
    }
}

/// `true` when `host` (an OSC 7 host field) names this machine: empty (no host given) or
/// matching `local_hostname` case-insensitively.
fn is_local_host(host: &str, local_hostname: &str) -> bool {
    host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || (!local_hostname.is_empty() && host.eq_ignore_ascii_case(local_hostname))
}

/// Resolves and validates a reported local path (realpath-resolved, unusable cwds ignored); `None`
/// if it doesn't canonicalize to an existing directory.
pub(crate) fn canonical_dir(path: &Path) -> Option<PathBuf> {
    let canon = path.canonicalize().ok()?;
    canon.is_dir().then_some(canon)
}

/// This machine's hostname (`/proc/sys/kernel/hostname`, `uname` on macOS), for
/// [`is_local_host`]. Empty on failure, which only ever widens what counts as "local" by one
/// fewer alias, never narrows it.
pub(crate) fn local_hostname() -> String {
    if cfg!(target_os = "macos") {
        return rustix::system::uname()
            .nodename()
            .to_string_lossy()
            .into_owned();
    }
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}
