//! What happens when something the user must know about happens: a session ends or crashes
//! (pane overlay + `behavior_on_session_end`), OSC 52 clipboard writes (policy + ask toast) and
//! the toast plumbing. Nothing here is silent.

use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fogu_core::{Osc52Policy, Profile, ProfileKind, effective_osc52_write};
use fogu_session::{ClipboardRequest, SessionError, SessionOutput, SessionWarning};
use fogu_term::{ClipboardType, sanitize_title};
use iced::{Task, clipboard};

use crate::actions::CWD_QUERY_TIMEOUT;
use crate::app::App;
use crate::message::Message;
use crate::notifications::{AUTO_DISMISS, Level, Notification, ToastAction};
use crate::panes::CwdFor;
use crate::tab::{PaneId, PaneState};

/// OSC 52 writes waiting for the user's decision at once; older ones are dropped.
const MAX_PENDING_CLIPBOARD: usize = 4;
/// Countdown before an SSH pane reconnects by itself (`behavior_on_session_end`).
const RECONNECT_SECS: u8 = 2;

/// An OSC 52 write behind an "ask" toast. `Debug` never shows the payload.
pub(crate) struct PendingClipboard {
    pub toast: u64,
    pub request: ClipboardRequest,
}

impl fmt::Debug for PendingClipboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingClipboard")
            .field("toast", &self.toast)
            .field("len", &self.request.len)
            .finish_non_exhaustive()
    }
}

/// One line of "what went wrong": the error plus its source, control characters removed.
pub(crate) fn describe(err: &SessionError) -> String {
    let mut text = err.to_string();
    if let Some(source) = std::error::Error::source(err) {
        text.push_str(": ");
        text.push_str(&source.to_string());
    }
    sanitize_title(&text)
}

fn write_clipboard(request: ClipboardRequest) -> Task<Message> {
    match request.target {
        ClipboardType::Clipboard => clipboard::write(request.payload),
        ClipboardType::Selection => write_selection(request.payload),
    }
}

/// Writes the primary selection; macOS has none, so the clipboard there.
pub(crate) fn write_selection<T>(text: String) -> Task<T> {
    if cfg!(target_os = "macos") {
        clipboard::write(text)
    } else {
        clipboard::write_primary(text)
    }
}

/// Reads the primary selection (the clipboard on macOS, see [`write_selection`]).
pub(crate) fn read_selection() -> Task<Option<String>> {
    if cfg!(target_os = "macos") {
        clipboard::read()
    } else {
        clipboard::read_primary()
    }
}

impl App {
    /// Shows `note`; the returned task removes it after [`AUTO_DISMISS`] when it is the
    /// self-dismissing kind.
    pub(crate) fn notify(&mut self, note: Notification) -> Task<Message> {
        let auto = note.auto_dismisses();
        self.toasts.push(note);
        self.sync_pending_clipboard();
        if auto {
            self.after(AUTO_DISMISS, Message::ToastExpire)
        } else {
            Task::none()
        }
    }

    /// Forgets OSC 52 writes whose toast is gone (dismissed, expired or evicted).
    fn sync_pending_clipboard(&mut self) {
        let toasts = &self.toasts;
        self.pending_clipboard
            .retain(|p| toasts.get(p.toast).is_some());
    }

    pub(crate) fn dismiss_toast(&mut self, id: u64) {
        self.toasts.dismiss(id);
        self.sync_pending_clipboard();
    }

    pub(crate) fn expire_toasts(&mut self) {
        self.toasts.expire(Instant::now());
        self.sync_pending_clipboard();
    }

    /// "Copy" on an OSC 52 toast: applies the write.
    pub(crate) fn accept_toast(&mut self, id: u64) -> Task<Message> {
        // "Save as profile…" after a quick connect: the toast goes, the editor opens.
        if let Some(crate::notifications::ToastAction::SaveAsProfile(profile)) =
            self.toasts.get(id).and_then(|n| n.action.clone())
        {
            self.dismiss_toast(id);
            return self.update(Message::Profiles(crate::profiles::ProfilesMsg::Edit(
                profile,
            )));
        }
        let pending = self
            .pending_clipboard
            .iter()
            .position(|p| p.toast == id)
            .map(|i| self.pending_clipboard.remove(i));
        self.dismiss_toast(id);
        pending.map_or_else(Task::none, |p| write_clipboard(p.request))
    }

    /// A session failed: a sticky error toast plus the pane's crashed overlay.
    pub(crate) fn on_session_error(&mut self, pane: PaneId, err: &SessionError) -> Task<Message> {
        let detail = describe(err);
        tracing::error!(error = %detail, "session failed");
        let toast = self.notify(Notification::new(Level::Error, "Session failed").detail(&detail));
        Task::batch([toast, self.on_pane_ended(pane, PaneState::Crashed(detail))])
    }

    /// Marks the pane ended and applies the profile's `behavior_on_session_end`: a pane being
    /// closed, or whose exit the policy says to close, goes away now; otherwise it shows its
    /// status and restarts on any key (`restart.rs`). A crash is never closed by policy.
    pub(crate) fn on_pane_ended(&mut self, pane_id: PaneId, state: PaneState) -> Task<Message> {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|t| t.panes.contains_key(&pane_id))
        else {
            return Task::none();
        };
        let Some(pane) = tab.panes.get_mut(&pane_id) else {
            return Task::none();
        };
        let by_policy = matches!(state, PaneState::Exited(_))
            && pane
                .profile
                .behavior_on_session_end
                .closes(state.is_clean_exit());
        // Only a pane that got connected reconnects by itself: a failing login must not loop.
        let reconnect = pane.is_ssh()
            && pane.connection.is_some()
            && pane
                .profile
                .behavior_on_session_end
                .reconnects(state.dropped());
        // A question the session can no longer take an answer to.
        pane.prompt = None;
        // The session stopped its forwards (or left them to the tabs still on the connection)
        // and its agent / X11 forwarding ended.
        pane.forwards.clear();
        pane.connection = None;
        pane.warnings.retain(|w| {
            !matches!(
                w,
                SessionWarning::AgentForwarded | SessionWarning::X11Forwarded
            )
        });
        pane.status = state.status_line(pane.is_ssh());
        pane.state = state;
        pane.refresh();
        if reconnect {
            pane.reconnect_in = Some(RECONNECT_SECS);
            pane.status = pane.state.reconnect_line(RECONNECT_SECS);
            pane.refresh();
            return self.after(Duration::from_secs(1), Message::ReconnectTick(pane_id));
        }
        if by_policy {
            return self.remove_pane(pane_id);
        }
        Task::none()
    }

    /// One second of a reconnect countdown: show the time left, or reconnect at zero. A pane
    /// that was restarted, closed or cancelled (Esc) meanwhile is left alone.
    pub(crate) fn reconnect_tick(&mut self, pane_id: PaneId) -> Task<Message> {
        let Some(pane) = self.pane_mut(pane_id) else {
            return Task::none();
        };
        let Some(left) = pane.reconnect_in.filter(|_| !pane.state.is_running()) else {
            return Task::none();
        };
        if left <= 1 {
            return self.restart_pane(pane_id);
        }
        pane.reconnect_in = Some(left - 1);
        pane.status = pane.state.reconnect_line(left - 1);
        pane.refresh();
        self.after(Duration::from_secs(1), Message::ReconnectTick(pane_id))
    }

    /// A non-fatal session condition: a toast now (the tab badge comes from `Pane::warnings`).
    pub(crate) fn on_session_warning(&mut self, warning: &SessionWarning) -> Task<Message> {
        let note =
            Notification::new(Level::Warn, warning_title(warning)).detail(warning_detail(warning));
        self.notify(note)
    }

    /// OSC 52 write from pane `pane_id`, per `terminal.osc52_write` (profile override first):
    /// `Allow` copies, `Deny` says so, `Ask` shows a sticky toast with Copy / Deny. Only the
    /// length is ever shown or logged.
    pub(crate) fn on_clipboard_store(
        &mut self,
        pane_id: PaneId,
        request: ClipboardRequest,
    ) -> Task<Message> {
        let profile = self.pane(pane_id).map(|p| &p.profile);
        let default = self.config.terminal.osc52_write;
        let policy = profile.map_or(default, |p| effective_osc52_write(p, default));
        match policy {
            Osc52Policy::Allow => write_clipboard(request),
            Osc52Policy::Deny => self.notify(
                Notification::new(Level::Info, "Clipboard write denied").detail(format!(
                    "A program tried to set the clipboard ({} bytes)",
                    request.len
                )),
            ),
            Osc52Policy::Ask => {
                let note = Notification::new(Level::Warn, "Remote wants to set the clipboard")
                    .detail(format!("{} bytes", request.len))
                    .action(ToastAction::ClipboardWrite);
                let toast = self.toasts.push(note);
                self.pending_clipboard
                    .push(PendingClipboard { toast, request });
                if self.pending_clipboard.len() > MAX_PENDING_CLIPBOARD {
                    let oldest = self.pending_clipboard.remove(0).toast;
                    self.toasts.dismiss(oldest);
                }
                self.sync_pending_clipboard();
                Task::none()
            }
        }
    }
}

impl App {
    /// Routes a session event to its pane; events from panes that no longer exist are dropped.
    pub(crate) fn on_session(&mut self, pane_id: PaneId, event: SessionOutput) -> Task<Message> {
        let active = self.active;
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|t| t.panes.contains_key(&pane_id))
        else {
            return Task::none();
        };
        let is_active = tab.id == active;
        let Some(pane) = tab.panes.get_mut(&pane_id) else {
            return Task::none();
        };
        match event {
            // Inactive panes keep `dirty` set (no further wakeups) until activated, so this
            // toasts once per stretch in the background.
            SessionOutput::Wakeup if !is_active && pane.notify.activity => {
                let title = tab.title().to_owned();
                return self.notify(
                    Notification::new(Level::Info, "Activity")
                        .detail(format!("New output in \"{title}\"")),
                );
            }
            SessionOutput::Wakeup if is_active => pane.refresh(),
            SessionOutput::Foreground(name) => {
                pane.foreground = name;
                self.refresh_pane_menu(pane_id);
            }
            SessionOutput::ForegroundDone(name) => {
                pane.notify.done = false;
                let title = tab.title().to_owned();
                return self.notify(
                    Notification::new(Level::Info, format!("{name} finished"))
                        .detail(format!("in \"{title}\"")),
                );
            }
            SessionOutput::Title(title) => {
                pane.title_from_osc = (!title.is_empty()).then_some(title);
            }
            SessionOutput::Exited(info) => {
                tracing::info!(?info, "session exited");
                return self.on_pane_ended(pane_id, PaneState::Exited(info));
            }
            SessionOutput::Error(err) => return self.on_session_error(pane_id, &err),
            SessionOutput::ClipboardStore(request) => {
                return self.on_clipboard_store(pane_id, request);
            }
            SessionOutput::InputDropped => {
                let note = Notification::new(Level::Warn, "Input dropped").detail(
                    "The program stopped reading its input; further typing or paste was discarded.",
                );
                return self.notify(note);
            }
            SessionOutput::Cwd { path, remote: true } => {
                return self.on_remote_cwd(pane_id, &path);
            }
            SessionOutput::Cwd { path, remote } => return self.on_cwd(pane_id, path, remote),
            SessionOutput::Prompt(id, kind, via) => return self.on_prompt(pane_id, id, kind, via),
            SessionOutput::ForwardStatus(status) => {
                crate::forwards_dialog::apply_status(&mut pane.forwards, status);
            }
            SessionOutput::Connection(id) => pane.connection = Some(id),
            SessionOutput::Warning(warning) if warning != SessionWarning::AgentForwarded => {
                if !pane.warnings.contains(&warning) {
                    pane.warnings.push(warning.clone());
                }
                return self.on_session_warning(&warning);
            }
            // No UI for these yet; Progress/Notify/PromptMark are logged by the session. Agent
            // forwarding is what the profile asked for, not a problem: no badge, no toast.
            SessionOutput::Warning(_)
            | SessionOutput::Wakeup
            | SessionOutput::Bell
            | SessionOutput::Progress(_)
            | SessionOutput::Notify { .. }
            | SessionOutput::PromptMark(_) => {}
        }
        Task::none()
    }

    /// A cwd report. Attacker-controlled text: only logged sanitized, and only a local
    /// (`remote: false`) path answering a fresh query opens a tab or a split.
    fn on_cwd(&mut self, pane_id: PaneId, path: PathBuf, remote: bool) -> Task<Message> {
        tracing::debug!(
            cwd = %sanitize_title(&path.to_string_lossy()),
            remote,
            "cwd reported"
        );
        if !remote && let Some(pane) = self.pane_mut(pane_id) {
            pane.local_cwd = Some(path.clone());
        }
        let Some(query) = self.pending_cwd else {
            return Task::none();
        };
        if query.pane != pane_id || remote || query.asked.elapsed() > CWD_QUERY_TIMEOUT {
            return Task::none();
        }
        self.pending_cwd = None;
        match query.purpose {
            CwdFor::Tab => {
                let profile = self
                    .pane(pane_id)
                    .map(|p| p.profile.clone())
                    .filter(|p| matches!(p.kind, ProfileKind::Local(_)))
                    .unwrap_or_else(Profile::default_shell);
                self.open_tab(&profile, Some(path));
            }
            CwdFor::Split(side) => self.split_now(pane_id, side, Some(path), None),
            CwdFor::Duplicate => {
                if let Some(profile) = self.pane(pane_id).map(|p| p.profile.clone()) {
                    self.open_tab(&profile, Some(path));
                }
            }
            CwdFor::Copy => return clipboard::write(path.to_string_lossy().into_owned()),
        }
        Task::none()
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

/// Short name of a session warning (toast title).
pub(crate) fn warning_title(warning: &SessionWarning) -> &'static str {
    match warning {
        SessionWarning::HostKeyNotVerified => "Host key not verified",
        SessionWarning::Unsupported(_) => "SSH options ignored",
        SessionWarning::ForwardExposed(_) => "Port forward reachable from other machines",
        SessionWarning::AgentForwarded => "Agent forwarded",
        SessionWarning::X11Forwarded => "X11 forwarded",
        SessionWarning::UnknownAlgorithms(_) => "Unknown SSH algorithms ignored",
        SessionWarning::WeakAlgorithms(_) => "Weaker SSH algorithms enabled",
        SessionWarning::SecretStore(_) => "Stored secrets",
        SessionWarning::ShellSetup(_) => "Shell start-up",
    }
}

/// One-line explanation of a session warning (toast detail, tab badge tooltip).
pub(crate) fn warning_detail(warning: &SessionWarning) -> String {
    match warning {
        SessionWarning::HostKeyNotVerified => {
            "verify_host_keys = false: the server's identity was not checked.".to_owned()
        }
        SessionWarning::Unsupported(options) => {
            format!("Not supported yet: {}", options.join(", "))
        }
        SessionWarning::ForwardExposed(label) => {
            format!("{label} listens on a non-loopback address.")
        }
        SessionWarning::AgentForwarded => {
            "The server can use your SSH agent's keys while this session is open.".to_owned()
        }
        SessionWarning::X11Forwarded => "Remote X programs show here and can read your \
             keyboard and screen (the real X cookie stays on this computer)."
            .to_owned(),
        SessionWarning::UnknownAlgorithms(names) => {
            format!("Not known to this build: {}", names.join(", "))
        }
        SessionWarning::WeakAlgorithms(names) => {
            format!("Outside the secure defaults: {}", names.join(", "))
        }
        SessionWarning::SecretStore(text) => format!("{text}. You are asked instead."),
        SessionWarning::ShellSetup(text) => text.clone(),
    }
}
