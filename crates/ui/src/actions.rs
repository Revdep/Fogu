//! Tab lifecycle and keymap actions: new / close / switch / rename / colour / restart / new tab
//! here, and session events routed to their pane. Pane-level operations live in `panes.rs`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fogu_core::{Action, Dir, EffectiveProfile, Profile, ProfileId, Side, Size, resolve};
use fogu_session::{SessionHandle, SessionInput};
use fogu_term::{Scroll, TermHandle, input, sanitize_title};
use iced::widget::operation;
use iced::{Task, clipboard, window};

use crate::app::App;
use crate::appearance::cursor_shape;
use crate::context_menu::{ContextMenu, MENU_INPUT};
use crate::message::{Message, PasteSource};
use crate::notifications::{Level, Notification};
use crate::panes::{CloseTarget, CwdFor, CwdQuery, SPLIT_CWD_WAIT};
use crate::tab::{Pane, PaneId, Tab, TabId};
use crate::tab_bar::{RENAME_INPUT, Rename, TAB_STRIDE, TAB_STRIP, TAB_W};

/// Logical px the pointer must move with the title bar pressed before the window moves.
const DRAG_THRESHOLD: f32 = 4.0;

/// Grid of the first tab, before the first layout resizes it.
pub(crate) const INITIAL_SIZE: Size = Size {
    cols: 80,
    rows: 24,
    px_w: 0,
    px_h: 0,
};
/// How long a cwd query stays valid.
pub(crate) const CWD_QUERY_TIMEOUT: Duration = Duration::from_secs(2);

impl App {
    pub(crate) fn active_index(&self) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == self.active)
    }

    pub(crate) fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active)
    }

    /// The active tab's focused pane.
    pub(crate) fn active_pane(&self) -> Option<&Pane> {
        self.active_tab().and_then(Tab::focused_pane)
    }

    pub(crate) fn active_pane_mut(&mut self) -> Option<&mut Pane> {
        let active = self.active;
        self.tabs
            .iter_mut()
            .find(|t| t.id == active)
            .and_then(Tab::focused_pane_mut)
    }

    /// Index of the tab holding pane `id`.
    pub(crate) fn tab_of(&self, id: PaneId) -> Option<usize> {
        self.tabs.iter().position(|t| t.panes.contains_key(&id))
    }

    pub(crate) fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.tabs.iter().find_map(|t| t.panes.get(&id))
    }

    pub(crate) fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.tabs.iter_mut().find_map(|t| t.panes.get_mut(&id))
    }

    pub(crate) fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// `config.default_profile`, or the built-in default shell.
    pub(crate) fn default_profile(&self) -> Profile {
        let config = &self.config;
        config
            .default_profile
            .as_ref()
            .and_then(|id| config.profiles.iter().find(|p| &p.id == id))
            .cloned()
            .unwrap_or_else(Profile::default_shell)
    }

    /// Opens `profile` (local `cwd` override for "new tab here") in a new last tab and
    /// activates it.
    pub(crate) fn open_tab(&mut self, profile: &Profile, cwd: Option<PathBuf>) -> TabId {
        let pane = self.open_pane(profile, cwd);
        let id = TabId(self.take_id());
        self.tabs.push(Tab::new(id, pane));
        self.activate(id);
        id
    }

    /// A new pane running `profile`, sized like the active pane.
    pub(crate) fn open_pane(&mut self, profile: &Profile, cwd: Option<PathBuf>) -> Pane {
        let size = self.active_pane().map_or(INITIAL_SIZE, Pane::size);
        let term = self.new_term(size);
        self.start_pane(profile, cwd, (term, false), size)
    }

    /// A new pane running `profile` on `term` of `size` cells: blank, or (`kept_history`) a
    /// restarted pane's grid already reset with [`TermHandle::reset_for_new_session`], which an
    /// SSH session's `clear_service_messages_on_connect` must not wipe.
    pub(crate) fn start_pane(
        &mut self,
        profile: &Profile,
        cwd: Option<PathBuf>,
        (mut term, kept_history): (TermHandle, bool),
        size: Size,
    ) -> Pane {
        term.set_palette(self.colors.palette());
        term.set_default_cursor_shape(cursor_shape(self.config.appearance.cursor_shape));
        term.set_cell_size_px(self.metrics.cell_w_px, self.metrics.cell_h_px);
        let mut effective = resolve(profile, &self.config.profile_defaults);
        match (cwd, &mut effective) {
            (Some(cwd), EffectiveProfile::Local(local)) => local.cwd = Some(cwd),
            // An SSH pane's last remote cwd (reconnect, split) with `remember_cwd`.
            (Some(cwd), EffectiveProfile::Ssh(ssh)) if ssh.remember_cwd => {
                ssh.cwd = Some(cwd.to_string_lossy().into_owned());
            }
            _ => {}
        }
        if let EffectiveProfile::Ssh(ssh) = &mut effective {
            ssh.clear_service_messages_on_connect &= !kept_history;
            self.resolve_jumps(&profile.id, ssh);
        }
        let size = Size {
            px_w: size.cols.saturating_mul(self.metrics.cell_w_px),
            px_h: size.rows.saturating_mul(self.metrics.cell_h_px),
            ..size
        };
        let default_title = match &effective {
            EffectiveProfile::Ssh(ssh) => sanitize_title(&format!("{}@{}", ssh.user, ssh.host)),
            EffectiveProfile::Local(_) => profile.name.clone(),
        };
        let handle = self.open_session(&profile.id, &effective, size, term.share());
        let id = PaneId(self.take_id());
        Pane::new(id, profile.clone(), default_title, term, handle)
    }

    /// SSH sessions keep their `known_hosts` next to the config (`--config-dir`) and reuse a
    /// live connection of the same profile (`reuse_session`).
    #[cfg(not(test))]
    fn open_session(
        &mut self,
        id: &ProfileId,
        profile: &EffectiveProfile,
        size: Size,
        term: TermHandle,
    ) -> SessionHandle {
        #[cfg(feature = "ssh")]
        if let EffectiveProfile::Ssh(ssh) = profile {
            let paths = self.config_dir.as_deref().map_or_else(
                fogu_session::SshPaths::default_for_user,
                fogu_session::SshPaths::in_config_dir,
            );
            return self.runtime.open_ssh_shared(id, ssh, paths, size, term);
        }
        #[cfg(not(feature = "ssh"))]
        let _ = id;
        fogu_session::open(self.runtime.handle(), profile, size, term)
    }

    /// Test builds: an inert session (no process). Its input end is kept in `stub_inputs`; its
    /// event sender is dropped, so the event stream is empty. SSH stubs get an SFTP handle whose
    /// command end is kept in `stub_sftp`.
    #[cfg(test)]
    fn open_session(
        &mut self,
        _: &ProfileId,
        p: &EffectiveProfile,
        _: Size,
        _: TermHandle,
    ) -> SessionHandle {
        self.stub_profiles.push(p.clone());
        let (input, input_rx) = tokio::sync::mpsc::channel(8);
        let (_, events) = tokio::sync::mpsc::channel(8);
        self.stub_inputs.push(input_rx);
        let sftp = matches!(p, EffectiveProfile::Ssh(_)).then(|| {
            let (cmds, cmds_rx) = tokio::sync::mpsc::channel(64);
            let (_, events) = tokio::sync::mpsc::channel(8);
            self.stub_sftp.push(cmds_rx);
            fogu_session::SftpHandle { cmds, events }
        });
        SessionHandle {
            input,
            events,
            sftp,
        }
    }

    /// Makes `id` the visible tab: focus out/in reports, fresh snapshots (inactive panes skip
    /// wakeup refreshes).
    pub(crate) fn activate(&mut self, id: TabId) {
        if id == self.active && self.active_pane().is_some() {
            return;
        }
        self.focus_report(false);
        self.active = id;
        self.focus_report(true);
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.panes.values_mut().for_each(Pane::refresh);
        }
    }

    /// Sends a focus in/out report to the active pane, if the app asked for them and the
    /// window is focused.
    pub(crate) fn focus_report(&self, focused: bool) {
        if let Some(pane) = self.active_pane().filter(|_| self.focused)
            && let Some(bytes) = input::encode_focus(focused, pane.term.mode())
        {
            pane.write_input(bytes);
        }
    }

    /// Handles a keymap / menu action.
    pub(crate) fn on_action(&mut self, action: Action) -> Task<Message> {
        self.menu = None;
        match action {
            Action::NewTab => {
                let profile = self.default_profile();
                self.open_tab(&profile, None);
            }
            Action::NewTabHere => {
                if let Some(pane) = self.active_pane().map(|p| p.id) {
                    self.ask_cwd(pane, CwdFor::Tab);
                }
            }
            Action::CloseTab => return self.request_close(CloseTarget::Tab(self.active)),
            Action::NextTab | Action::PrevTab | Action::GoToTab(_) => self.switch(action),
            Action::RenameTab => return self.start_rename(self.active),
            Action::RestartTab => {
                if let Some(pane) = self.active_pane().map(|p| p.id) {
                    return self.restart_pane(pane);
                }
            }
            Action::Copy => {
                if let Some(text) = self.active_pane().and_then(|p| p.term.selection_text()) {
                    return clipboard::write(text);
                }
            }
            Action::Paste => return self.update(Message::Paste(PasteSource::Clipboard)),
            Action::PastePrimary => return self.update(Message::Paste(PasteSource::Primary)),
            Action::ZoomIn => self.set_font_size(self.font.size_pt + 1.0),
            Action::ZoomOut => self.set_font_size(self.font.size_pt - 1.0),
            Action::ZoomReset => self.zoom_reset(),
            Action::Clear => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.term.clear();
                    pane.refresh();
                }
            }
            Action::ScrollPageUp => return self.update(Message::Scroll(Scroll::PageUp)),
            Action::ScrollPageDown => return self.update(Message::Scroll(Scroll::PageDown)),
            Action::ScrollTop => return self.update(Message::Scroll(Scroll::Top)),
            Action::ScrollBottom => return self.update(Message::Scroll(Scroll::Bottom)),
            Action::ToggleFullscreen => {
                self.fullscreen = !self.fullscreen;
                let mode = if self.fullscreen {
                    window::Mode::Fullscreen
                } else {
                    window::Mode::Windowed
                };
                return window::oldest().and_then(move |id| window::set_mode(id, mode));
            }
            Action::SplitRight => return self.split_focused(Side::Right),
            Action::SplitDown => return self.split_focused(Side::Bottom),
            Action::ClosePane => {
                if let Some(pane) = self.active_pane().map(|p| p.id) {
                    return self.request_close(CloseTarget::Pane(pane));
                }
            }
            Action::PaneNav(dir) => self.pane_nav(dir),
            Action::Reconnect => {
                // A new session for an SSH pane or any ended pane (history: `restart_pane`).
                if let Some(pane) = self
                    .active_pane()
                    .filter(|p| p.is_ssh() || !p.state.is_running())
                {
                    return self.restart_pane(pane.id);
                }
            }
            Action::OpenProfilePicker => return self.open_picker(),
            Action::ToggleSftp => return self.toggle_sftp(),
            Action::ToggleProfiles => return self.toggle_profiles(),
            Action::OpenSettings => return self.open_settings(),
            Action::ClearSavedSessions => match &mut self.recovery {
                Some(recovery) => recovery.pending_clear = true,
                None => return self.notify(Notification::new(Level::Info, "Tab recovery is off")),
            },
            Action::SelectAll => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.term.select_all();
                    pane.refresh();
                }
            }
            Action::Find => return self.start_search(),
            Action::DuplicateTab => return self.duplicate_tab(),
            Action::CloseOtherTabs => {
                return self.request_close(CloseTarget::OthersOf(self.active));
            }
            Action::CloseTabsRight => return self.request_close(CloseTarget::RightOf(self.active)),
            Action::CopyPath => return self.copy_pane_path(),
            Action::SwapPanes => self.swap_focused(),
            Action::EqualizePanes => return self.update(Message::Equalize(self.active)),
            Action::MaximizePane => self.toggle_maximize(),
            Action::ResizePane(dir) => {
                let step = self.config.terminal.pane_resize_step.clamp(0.01, 0.5);
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == self.active) {
                    let focused = tab.focused;
                    tab.layout.resize_step(&focused, dir, step);
                }
            }
        }
        Task::none()
    }

    /// Shows only the focused pane of the active tab, or all of them again.
    fn toggle_maximize(&mut self) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == self.active) else {
            return;
        };
        tab.solo = (tab.solo.is_none() && tab.panes.len() > 1)
            .then(|| fogu_core::SplitTree::new(tab.focused));
    }

    /// `NextTab` / `PrevTab` (wrapping) / `GoToTab(n)` (1-based; out of range is ignored).
    fn switch(&mut self, action: Action) {
        let (Some(index), len) = (self.active_index(), self.tabs.len()) else {
            return;
        };
        let target = match action {
            Action::NextTab => (index + 1) % len,
            Action::PrevTab => (index + len - 1) % len,
            Action::GoToTab(n) => usize::from(n).saturating_sub(1),
            _ => return,
        };
        if let Some(id) = self.tabs.get(target).map(|t| t.id) {
            self.activate(id);
        }
    }

    /// The active pane's profile again in a new tab, in the same directory (a local shell is
    /// asked first; an SSH pane uses its last reported cwd under `remember_cwd`).
    fn duplicate_tab(&mut self) -> Task<Message> {
        let Some(pane) = self.active_pane() else {
            return Task::none();
        };
        let (id, profile) = (pane.id, pane.profile.clone());
        let local_running = !pane.is_ssh() && pane.state.is_running();
        let cwd = pane.remote_cwd.clone().map(PathBuf::from);
        if local_running && self.ask_cwd(id, CwdFor::Duplicate) {
            return self.after(SPLIT_CWD_WAIT, Message::CwdTimeout(id));
        }
        self.open_tab(&profile, cwd);
        Task::none()
    }

    /// The focused pane's current directory to the clipboard.
    fn copy_pane_path(&mut self) -> Task<Message> {
        let Some(pane) = self.active_pane() else {
            return Task::none();
        };
        if let Some(path) = pane.remote_cwd.clone().filter(|_| pane.is_ssh()) {
            return clipboard::write(path);
        }
        let id = pane.id;
        if !pane.is_ssh() && pane.state.is_running() && self.ask_cwd(id, CwdFor::Copy) {
            return Task::none();
        }
        self.notify(
            Notification::new(Level::Info, "Current directory unknown")
                .detail("The shell has not reported its directory (OSC 7)."),
        )
    }

    /// Swaps the focused pane with a neighbour (right, down, left, then up).
    fn swap_focused(&mut self) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == self.active) else {
            return;
        };
        let focused = tab.focused;
        let other = [Dir::Right, Dir::Down, Dir::Left, Dir::Up]
            .into_iter()
            .find_map(|dir| tab.layout.neighbor(&focused, dir).copied());
        if let Some(other) = other {
            tab.layout.swap(&focused, &other);
        }
    }

    /// "Save to profile": the active tab's colour and background become its stored profile's.
    pub(crate) fn save_tab_to_profile(&mut self) -> Task<Message> {
        self.menu = None;
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let Some(old) = self
            .config
            .profiles
            .iter()
            .find(|p| p.id == tab.profile.id)
            .cloned()
        else {
            return Task::none();
        };
        let mut new = old.clone();
        new.color = tab.color;
        match &tab.background {
            fogu_core::TabBackground::Profile => {}
            fogu_core::TabBackground::None => new.background = None,
            fogu_core::TabBackground::Set(bg) => new.background = Some(bg.clone()),
        }
        self.save_edits(&[fogu_core::config::ProfileEdit::Update {
            old: Box::new(old),
            new: Box::new(new),
        }])
    }

    /// Asks every running local pane for its cwd (tab recovery keeps the answers).
    pub(crate) fn query_local_cwds(&self) {
        for pane in self.tabs.iter().flat_map(|t| t.panes.values()) {
            if !pane.is_ssh() && pane.state.is_running() {
                let _sent = pane.control(SessionInput::QueryCwd);
            }
        }
    }

    /// Asks pane `pane` for its cwd on behalf of `purpose`; `false` when the session is gone.
    pub(crate) fn ask_cwd(&mut self, pane: PaneId, purpose: CwdFor) -> bool {
        let asked = self
            .pane(pane)
            .is_some_and(|p| p.control(SessionInput::QueryCwd));
        if asked {
            self.pending_cwd = Some(CwdQuery {
                pane,
                asked: Instant::now(),
                purpose,
            });
        }
        asked
    }

    /// Closes every pane of tab `id`; the tab goes with its last pane.
    pub(crate) fn close_tab(&mut self, id: TabId) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return Task::none();
        };
        if tab.panes.is_empty() {
            // A profile editor tab: unsaved changes are confirmed first.
            if tab.editor.as_ref().is_some_and(|e| e.dirty()) {
                self.pending_discard = Some(id);
                return Task::none();
            }
            return self.remove_tab(id);
        }
        let panes: Vec<PaneId> = tab.panes.keys().copied().collect();
        Task::batch(panes.into_iter().map(|pane| self.close_pane(pane)))
    }

    /// Drops tab `id` (its panes close their sessions); exits the app after the last tab.
    pub(crate) fn remove_tab(&mut self, id: TabId) -> Task<Message> {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return Task::none();
        };
        self.tabs.remove(index);
        if self.renaming.as_ref().is_some_and(|r| r.tab == id) {
            self.renaming = None;
        }
        if self.hovered_tab == Some(id) {
            self.hovered_tab = None;
        }
        if self.tabs.is_empty() {
            self.forget_session();
            return iced::exit();
        }
        if self.active == id {
            self.menu = None;
            let next = index.min(self.tabs.len() - 1);
            if let Some(next) = self.tabs.get(next).map(|t| t.id) {
                self.activate(next);
            }
        }
        Task::none()
    }

    /// Scrolls the tab strip just enough to show the active tab's header.
    pub(crate) fn reveal_active_tab(&self) -> Task<Message> {
        let Some(index) = self.tabs.iter().position(|t| t.id == self.active) else {
            return Task::none();
        };
        let left = crate::metrics::cells_f32(index) * TAB_STRIDE;
        let Some(viewport) = self.tab_strip else {
            // Not overflowing yet as far as we know: a new last tab may be the first that does.
            return if index + 1 == self.tabs.len() {
                operation::snap_to_end(TAB_STRIP)
            } else {
                Task::none()
            };
        };
        let (x, width) = (viewport.absolute_offset().x, viewport.bounds().width);
        let to = if left < x {
            left
        } else if left + TAB_W > x + width {
            left + TAB_W - width
        } else {
            return Task::none();
        };
        operation::scroll_to(
            TAB_STRIP,
            operation::AbsoluteOffset {
                x: Some(to),
                y: None,
            },
        )
    }

    fn start_rename(&mut self, id: TabId) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return Task::none();
        };
        let value = tab.title().to_owned();
        self.activate(id);
        self.menu = None;
        self.renaming = Some(Rename { tab: id, value });
        Task::batch([
            operation::focus(RENAME_INPUT),
            operation::select_all(RENAME_INPUT),
        ])
    }

    /// Applies the inline rename: sanitized, empty clears the override.
    pub(crate) fn commit_rename(&mut self) {
        let Some(rename) = self.renaming.take() else {
            return;
        };
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == rename.tab) {
            let title = sanitize_title(rename.value.trim());
            tab.title_override = (!title.is_empty()).then_some(title);
        }
    }

    /// Tab bar, rename and context-menu messages.
    pub(crate) fn on_tab_ui(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::CloseTab(id) => return self.request_close(CloseTarget::Tab(id)),
            Message::HoverTab(id) => self.hovered_tab = id,
            Message::LeaveTab(id) => {
                if self.hovered_tab == Some(id) {
                    self.hovered_tab = None;
                }
            }
            Message::TabBarCursor(at) => {
                // The bar spans the whole window (the profile sidebar sits below it).
                self.tab_bar_cursor = at;
                if self
                    .bar_press
                    .is_some_and(|from| from.distance(at) >= DRAG_THRESHOLD)
                {
                    self.bar_press = None;
                    return iced::window::latest().and_then(iced::window::drag);
                }
            }
            Message::TabStripViewport(viewport) => self.tab_strip = Some(viewport),
            Message::TabBarWheel(delta) => {
                let dx = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => -y * 60.0,
                    iced::mouse::ScrollDelta::Pixels { y, .. } => -y,
                };
                return operation::scroll_by(
                    TAB_STRIP,
                    operation::AbsoluteOffset { x: dx, y: 0.0 },
                );
            }
            Message::StartRename(id) => return self.start_rename(id),
            Message::RenameInput(value) => {
                if let Some(rename) = &mut self.renaming {
                    rename.value = value;
                }
            }
            Message::CommitRename => self.commit_rename(),
            Message::OpenTabMenu(id) => {
                self.commit_rename();
                self.activate(id);
                let ssh = self
                    .active_tab()
                    .is_some_and(|t| t.panes.values().any(|p| p.sftp.is_some()));
                let stored = self
                    .active_tab()
                    .is_some_and(|t| self.config.profiles.iter().any(|p| p.id == t.profile.id));
                self.menu = Some(ContextMenu::tab(self.tab_bar_cursor, ssh, stored));
            }
            Message::MenuColors => {
                let anchor = self.menu.as_ref().map_or(self.tab_bar_cursor, |m| m.anchor);
                self.menu = Some(ContextMenu::tab_colors(anchor));
                return operation::focus(MENU_INPUT);
            }
            Message::MenuInput(value) => {
                if let Some(input) = self.menu.as_mut().and_then(|m| m.input.as_mut()) {
                    input.value = value;
                    input.invalid = false;
                }
            }
            Message::MenuSubmit => return self.submit_menu_input(),
            Message::SetTabColor(color) => {
                self.menu = None;
                let active = self.active;
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == active) {
                    tab.color = color;
                }
            }
            Message::CloseMenu => self.menu = None,
            Message::Escape => return self.on_escape(),
            _ => {}
        }
        Task::none()
    }

    /// Enter in the menu's field: a valid colour / non-empty name is applied, anything else
    /// marks the field.
    fn submit_menu_input(&mut self) -> Task<Message> {
        let Some(input) = self.menu.as_mut().and_then(|m| m.input.as_mut()) else {
            return Task::none();
        };
        if let Some(message) = input.submit.message(&input.value) {
            return self.update(message);
        }
        input.invalid = true;
        Task::none()
    }
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
