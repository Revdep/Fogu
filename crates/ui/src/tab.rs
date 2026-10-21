//! Tabs and their panes: a tab lays its panes out with a [`SplitTree<PaneId>`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fogu_core::{Profile, ProfileKind, RgbColor, Size, SplitTree, TabBackground};
use fogu_session::{
    ConnectionId, ExitInfo, ForwardStatus, SessionHandle, SessionInput, SessionOutput,
    SessionWarning, SftpEvent, SftpRequest,
};
use fogu_term::{RenderSnapshot, Scroll, TermHandle};
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

use crate::profile_editor::ProfileEditor;
use crate::prompt::PromptState;
use crate::sftp::SftpPanelState;
use crate::subscription::EventSlot;

/// Identifies a tab; monotonic, never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TabId(pub u64);

/// Identifies a pane (and its session subscription); monotonic, never reused, so events from a
/// restarted pane's old session can't reach the new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneId(pub u64);

/// Longest crash detail shown in a pane's status line, in characters.
const STATUS_DETAIL_CHARS: usize = 80;
/// Same for a disconnected SSH pane (its line also says "Press any key to reconnect").
const REMOTE_DETAIL_CHARS: usize = 40;

/// Lifecycle of a pane's session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneState {
    /// The session is running.
    Running,
    /// The process ended.
    Exited(ExitInfo),
    /// The session failed or its task panicked (spawn error, I/O error, panic); the text is the
    /// sanitized reason. A restart reuses the grid only after
    /// `TermHandle::reset_for_new_session` (`terminal.keep_history_on_restart`).
    Crashed(String),
}

impl PaneState {
    /// `true` while the session is alive.
    pub fn is_running(&self) -> bool {
        *self == Self::Running
    }

    /// `true` for an exit with code 0 (not killed by a signal): what `CloseIfClean` waits for.
    pub fn is_clean_exit(&self) -> bool {
        matches!(
            self,
            Self::Exited(ExitInfo {
                code: Some(0),
                signal: None
            })
        )
    }

    /// No exit status: the connection dropped (transport error, server gone) or the session
    /// failed. What `behavior_on_session_end = "auto"` reconnects after.
    pub fn dropped(&self) -> bool {
        matches!(
            self,
            Self::Crashed(_)
                | Self::Exited(ExitInfo {
                    code: None,
                    signal: None
                })
        )
    }

    /// The overlay during an SSH pane's automatic reconnect countdown.
    pub fn reconnect_line(&self, secs: u8) -> Option<String> {
        self.remote_reason().map(|why| {
            format!(
                "[Disconnected ({why})] \u{2014} Reconnecting in {secs} s \u{00b7} any key now \u{00b7} Esc cancels"
            )
        })
    }

    /// Why an SSH pane is disconnected, short; `None` while running.
    fn remote_reason(&self) -> Option<String> {
        let why = match self {
            Self::Running => return None,
            Self::Exited(ExitInfo {
                signal: Some(signal),
                ..
            }) => format!("signal {signal}"),
            Self::Exited(ExitInfo {
                code: Some(code), ..
            }) => format!("exit code {code}"),
            // No exit status: the server or network closed the connection.
            Self::Exited(_) => "connection closed".to_owned(),
            // Short: "Press any key to reconnect" must stay visible; the full reason is in the
            // terminal and the error toast.
            Self::Crashed(why) if why.chars().count() > REMOTE_DETAIL_CHARS => {
                let mut short: String = why.chars().take(REMOTE_DETAIL_CHARS - 1).collect();
                short.push('…');
                short
            }
            Self::Crashed(why) => why.clone(),
        };
        Some(why)
    }

    /// The overlay drawn over an ended pane; `None` while running. `remote` panes (SSH) say
    /// "Disconnected" and offer a reconnect instead of a restart.
    pub fn status_line(&self, remote: bool) -> Option<String> {
        if remote {
            return self
                .remote_reason()
                .map(|why| format!("[Disconnected ({why})] \u{2014} Press any key to reconnect"));
        }
        let what = match self {
            Self::Running => return None,
            Self::Exited(ExitInfo {
                signal: Some(signal),
                ..
            }) => format!("process killed by signal {signal}"),
            Self::Exited(ExitInfo { code, .. }) => {
                format!("process exited with code {}", code.unwrap_or(0))
            }
            Self::Crashed(why) => {
                let why: String = why.chars().take(STATUS_DETAIL_CHARS).collect();
                format!("crashed: {why}")
            }
        };
        Some(format!("[{what}] \u{2014} Press any key to restart"))
    }
}

/// A pane's notification toggles (pane context menu).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneNotify {
    /// Toast when the program running in the shell ends (local panes).
    pub done: bool,
    /// Toast when output arrives while the pane's tab is in the background.
    pub activity: bool,
}

/// One terminal pane: its grid, the session's input side and the last drawn snapshot.
#[derive(Debug)]
pub struct Pane {
    pub id: PaneId,
    /// The profile this pane runs (restart, reconnect, exit policy, OSC 52 policy).
    pub profile: Profile,
    /// Title until an OSC title arrives: the profile name, `user@host` for SSH.
    pub default_title: String,
    pub term: TermHandle,
    dirty: Arc<AtomicBool>,
    session_input: Sender<SessionInput>,
    pub events: EventSlot<SessionOutput>,
    /// SSH panes: the connection's SFTP engine (used by the tab's SFTP panel).
    pub sftp: Option<SftpLink>,
    pub state: PaneState,
    pub snapshot: RenderSnapshot,
    /// Bumped per resize; older debounce timers are stale.
    pub resize_generation: u64,
    /// PTY size waiting for the debounce timer.
    pub pending_resize: Option<Size>,
    /// Last OSC 0/2 title (sanitized by the session; `None` after a reset).
    pub title_from_osc: Option<String>,
    /// Status line drawn over an ended pane ("[process exited with code 0] …").
    pub status: Option<String>,
    /// The session's open question (password, host key…), drawn as a modal over the pane.
    pub prompt: Option<PromptState>,
    /// Warnings the session reported (tab badge), without duplicates.
    pub warnings: Vec<SessionWarning>,
    /// SSH panes: the last cwd the remote shell reported (OSC 7, raw remote path).
    pub remote_cwd: Option<String>,
    /// Local panes: the last known working directory (OSC 7 or a cwd query), for tab recovery.
    pub local_cwd: Option<std::path::PathBuf>,
    /// The link under the pointer (Ctrl held) or under the last right-click.
    pub link: Option<fogu_term::links::Link>,
    /// Local panes: the program in front of the shell at the last cwd query (`None`: the shell).
    pub foreground: Option<String>,
    /// The pane menu's "Notify when done" / "Notify on activity" toggles.
    pub notify: PaneNotify,
    /// SSH panes: the session's port forwards (latest status of each; stopped ones dropped).
    pub forwards: Vec<ForwardStatus>,
    /// SSH panes: the connection the session runs on (shared with `reuse_session`).
    pub connection: Option<ConnectionId>,
    /// Seconds left before an ended SSH pane reconnects by itself (`behavior_on_session_end`
    /// `reconnect` / `auto`); `None` = no countdown (never started or cancelled with Esc).
    pub reconnect_in: Option<u8>,
}

impl Pane {
    /// Wraps a freshly opened session `handle` of `profile` feeding `term`.
    pub fn new(
        id: PaneId,
        profile: Profile,
        default_title: String,
        term: TermHandle,
        handle: SessionHandle,
    ) -> Self {
        Self {
            id,
            profile,
            default_title,
            dirty: term.dirty(),
            snapshot: term.snapshot(),
            term,
            session_input: handle.input,
            events: EventSlot::new(id.0, handle.events),
            sftp: handle.sftp.map(|h| SftpLink {
                cmds: h.cmds,
                events: EventSlot::new(id.0, h.events),
            }),
            state: PaneState::Running,
            resize_generation: 0,
            pending_resize: None,
            title_from_osc: None,
            status: None,
            prompt: None,
            warnings: Vec::new(),
            remote_cwd: None,
            local_cwd: None,
            link: None,
            foreground: None,
            notify: PaneNotify::default(),
            forwards: Vec::new(),
            connection: None,
            reconnect_in: None,
        }
    }

    /// `true` for an SSH pane.
    pub fn is_ssh(&self) -> bool {
        matches!(self.profile.kind, ProfileKind::Ssh(_))
    }

    /// Clears `dirty` *before* snapshotting, so output arriving meanwhile sends a new wakeup.
    pub fn refresh(&mut self) {
        self.dirty.store(false, Ordering::SeqCst);
        self.snapshot = self.term.snapshot();
    }

    /// Sends a control message without blocking the UI. `false` when the session is gone
    /// (closed channel) or busy (full; logged).
    pub fn control(&self, input: SessionInput) -> bool {
        match self.session_input.try_send(input) {
            Ok(()) => true,
            Err(TrySendError::Closed(_)) => false,
            Err(TrySendError::Full(_)) => {
                tracing::warn!("session input full; control message dropped");
                false
            }
        }
    }

    /// Writes `bytes` to a running session without blocking the UI (drops them when full).
    pub fn write_input(&self, bytes: Vec<u8>) {
        if self.state.is_running() {
            self.control(SessionInput::Data(bytes.into()));
        }
    }

    /// Writes encoded keys, then scrolls to the bottom when `scroll_on_input` is set
    /// (`terminal.scroll_on_input`).
    pub fn send_input(&mut self, bytes: Vec<u8>, scroll_on_input: bool) {
        if !self.state.is_running() {
            return;
        }
        self.write_input(bytes);
        if scroll_on_input && self.snapshot.display_offset != 0 {
            self.term.scroll_display(Scroll::Bottom);
            self.refresh();
        }
    }

    /// Current grid size (for opening a sibling tab at the same size).
    pub fn size(&self) -> Size {
        Size {
            cols: u16::try_from(self.snapshot.cols).unwrap_or(u16::MAX),
            rows: u16::try_from(self.snapshot.rows).unwrap_or(u16::MAX),
            ..Size::default()
        }
    }
}

/// The UI side of a pane's SFTP engine: commands out, events in (subscribed for every SSH
/// pane, panel open or not).
#[derive(Debug)]
pub struct SftpLink {
    /// Commands to the engine.
    pub cmds: Sender<SftpRequest>,
    /// Engine events until the subscription takes them.
    pub events: EventSlot<SftpEvent>,
}

/// One tab: its title override, colour and its panes laid out by a split tree. Each pane
/// carries its own profile (a split may connect elsewhere).
#[derive(Debug)]
pub struct Tab {
    pub id: TabId,
    /// The user's rename; wins over everything until cleared.
    pub title_override: Option<String>,
    /// Header tint + focused pane border; defaults to the profile's colour.
    pub color: Option<RgbColor>,
    /// Pane arrangement; never empty while the tab exists.
    pub layout: SplitTree<PaneId>,
    /// Every pane of `layout`.
    pub panes: BTreeMap<PaneId, Pane>,
    /// The pane receiving keys; always a leaf of `layout`.
    pub focused: PaneId,
    /// The SFTP side panel (SSH tabs), kept while hidden so transfers stay listed.
    pub sftp: Option<SftpPanelState>,
    /// The profile the tab was opened with (background, "Save to profile").
    pub profile: Profile,
    /// The tab's own background choice (wins over `profile.background`).
    pub background: TabBackground,
    /// A profile editor tab: no panes, the editor page instead of the split container.
    pub editor: Option<Box<ProfileEditor>>,
    /// The settings tab: no panes, the settings page instead.
    pub settings: Option<Box<crate::settings::Settings>>,
    /// A start page tab: no panes, the start page instead.
    pub welcome: Option<Box<crate::welcome::Welcome>>,
    /// "Maximize pane": the one-pane tree shown instead of `layout` until the layout or the
    /// focus changes.
    pub solo: Option<SplitTree<PaneId>>,
}

impl Tab {
    /// A new tab with the single pane `pane`, coloured like its profile.
    pub fn new(id: TabId, pane: Pane) -> Self {
        Self {
            id,
            color: pane.profile.color,
            profile: pane.profile.clone(),
            background: TabBackground::default(),
            title_override: None,
            layout: SplitTree::new(pane.id),
            focused: pane.id,
            panes: BTreeMap::from([(pane.id, pane)]),
            sftp: None,
            editor: None,
            settings: None,
            welcome: None,
            solo: None,
        }
    }

    /// A "Profile: …" tab showing `editor`. It has no pane (`layout` holds a placeholder id no
    /// pane ever gets) and the built-in profile, so the tab menu's "Save to profile" is off.
    pub fn editor(id: TabId, editor: ProfileEditor) -> Self {
        Self {
            editor: Some(Box::new(editor)),
            ..Self::page(id)
        }
    }

    /// The "Settings" tab: no pane, like an editor tab.
    pub fn settings(id: TabId, settings: crate::settings::Settings) -> Self {
        Self {
            settings: Some(Box::new(settings)),
            ..Self::page(id)
        }
    }

    /// A "New tab" start page: no pane, like a settings tab.
    pub fn welcome(id: TabId) -> Self {
        Self {
            welcome: Some(Box::default()),
            ..Self::page(id)
        }
    }

    /// A tab without panes, for a page.
    fn page(id: TabId) -> Self {
        let none = PaneId(0);
        Self {
            id,
            title_override: None,
            color: None,
            layout: SplitTree::new(none),
            panes: BTreeMap::new(),
            focused: none,
            sftp: None,
            profile: Profile::default_shell(),
            background: TabBackground::None,
            editor: None,
            settings: None,
            welcome: None,
            solo: None,
        }
    }

    /// A page (profile editor, settings) instead of terminals.
    pub fn is_page(&self) -> bool {
        self.editor.is_some() || self.settings.is_some() || self.welcome.is_some()
    }

    /// The focused pane.
    pub fn focused_pane(&self) -> Option<&Pane> {
        self.panes.get(&self.focused)
    }

    /// The focused pane, mutably.
    pub fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        self.panes.get_mut(&self.focused)
    }

    /// The title shown in the header and the window title (the focused pane's OSC title).
    pub fn title(&self) -> &str {
        if let Some(editor) = &self.editor {
            return self.title_override.as_deref().unwrap_or(&editor.title);
        }
        if self.settings.is_some() {
            return self.title_override.as_deref().unwrap_or("Settings");
        }
        if self.welcome.is_some() {
            return self.title_override.as_deref().unwrap_or("New tab");
        }
        let Some(pane) = self.focused_pane() else {
            return self.title_override.as_deref().unwrap_or_default();
        };
        let profile = &pane.profile;
        // An SSH pane's default title is `user@host`; "keep the profile name" means the name.
        let fallback = if profile.disable_dynamic_title {
            &profile.name
        } else {
            &pane.default_title
        };
        resolve_title(
            self.title_override.as_deref(),
            pane.title_from_osc.as_deref(),
            profile.disable_dynamic_title,
            fallback,
        )
    }

    /// Every warning any of the tab's panes reported (tab badge).
    pub fn warnings(&self) -> impl Iterator<Item = &SessionWarning> {
        self.panes.values().flat_map(|p| p.warnings.iter())
    }
}

/// Title precedence: user override > OSC title (unless the profile disables dynamic titles) >
/// profile name.
pub fn resolve_title<'a>(
    title_override: Option<&'a str>,
    from_osc: Option<&'a str>,
    disable_dynamic_title: bool,
    profile_name: &'a str,
) -> &'a str {
    title_override
        .or(from_osc.filter(|_| !disable_dynamic_title))
        .unwrap_or(profile_name)
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn status_lines_and_clean_exit() {
        let exit = |code, signal| PaneState::Exited(ExitInfo { code, signal });
        // (state, clean, status text)
        let rows = [
            (PaneState::Running, false, None),
            (
                exit(Some(0), None),
                true,
                Some("[process exited with code 0]"),
            ),
            (
                exit(Some(3), None),
                false,
                Some("[process exited with code 3]"),
            ),
            (
                exit(None, Some(9)),
                false,
                Some("[process killed by signal 9]"),
            ),
            (
                exit(Some(0), Some(15)),
                false,
                Some("[process killed by signal 15]"),
            ),
            (
                PaneState::Crashed("boom".into()),
                false,
                Some("[crashed: boom]"),
            ),
        ];
        for (state, clean, want) in rows {
            assert_eq!(state.is_clean_exit(), clean, "{state:?}");
            let line = state.status_line(false);
            match want {
                None => assert_eq!(line, None),
                Some(want) => {
                    let line = line.unwrap();
                    assert!(line.starts_with(want), "{line}");
                    assert!(line.ends_with("Press any key to restart"));
                }
            }
        }
    }

    #[test]
    fn remote_status_lines_offer_reconnect() {
        let rows = [
            (PaneState::Running, None),
            (
                PaneState::Exited(ExitInfo {
                    code: Some(3),
                    signal: None,
                }),
                Some("[Disconnected (exit code 3)]"),
            ),
            (
                PaneState::Exited(ExitInfo {
                    code: None,
                    signal: None,
                }),
                Some("[Disconnected (connection closed)]"),
            ),
            (
                PaneState::Crashed("authentication failed".into()),
                Some("[Disconnected (authentication failed)]"),
            ),
        ];
        for (state, want) in rows {
            let line = state.status_line(true);
            match want {
                None => assert_eq!(line, None),
                Some(want) => {
                    let line = line.unwrap();
                    assert!(line.starts_with(want), "{line}");
                    assert!(line.ends_with("Press any key to reconnect"), "{line}");
                }
            }
        }
    }

    #[test]
    fn title_precedence_table() {
        let rows = [
            // (override, osc, disable_dynamic, want)
            (Some("work"), Some("vim"), false, "work"),
            (Some("work"), None, true, "work"),
            (None, Some("vim"), false, "vim"),
            (None, Some("vim"), true, "Profile"),
            (None, None, false, "Profile"),
            (None, None, true, "Profile"),
        ];
        for (ovr, osc, disable, want) in rows {
            assert_eq!(
                resolve_title(ovr, osc, disable, "Profile"),
                want,
                "{ovr:?} {osc:?} {disable}"
            );
        }
    }
}
