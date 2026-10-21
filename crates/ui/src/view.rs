//! `App::view` (tab bar, the active tab's split container with the SFTP side panel, toasts,
//! modals and the context menu) and `App::subscription` (session/SFTP events, window and
//! keyboard events).

use iced::keyboard::{self, key::Named};
use iced::widget::{column, container};
use iced::{Element, Subscription, event, window};

use crate::app::App;
use crate::context_menu;
use crate::drag::{DragMsg, Payload};
use crate::message::Message;
use crate::modal::{self, overlay};
use crate::mouse::MouseConfig;
use crate::palette::to_iced;
use crate::profile_picker;
use crate::profiles;
use crate::prompt;
use crate::sftp;
use crate::split_container::SplitContainer;
use crate::subscription::{session_events, sftp_events};
use crate::tab_bar::TabBar;
use crate::terminal_view::TerminalView;

/// Height of the header (and drag handle) above each pane of a split tab.
const GRIP_H: f32 = 28.0;

impl App {
    /// Tab bar above the active tab's split container, plus modal / menu overlays.
    #[expect(clippy::too_many_lines, reason = "one widget tree, top to bottom")]
    pub fn view(&self) -> Element<'_, Message> {
        let Some(tab) = self.active_tab() else {
            return column![].into();
        };
        if let Some(page) = self.page(tab) {
            return self.chrome(page, None, None);
        }
        let search = self.search_in(tab);
        let typing_elsewhere = self.renaming.is_some()
            || search.is_some()
            || self.menu.is_some()
            || self.picker.is_some()
            || self.pending_close.is_some()
            || self.forwards.is_some()
            || self.profiles.pending_delete.is_some()
            || (self.profiles.open && self.profiles.focused);
        let sftp_panel = tab.sftp.as_ref().filter(|p| p.visible);
        let panel_has_keys = sftp_panel.is_some_and(|p| p.focused);
        let mouse = MouseConfig {
            wheel_lines: self.config.terminal.wheel_lines,
            paste_on_middle_click: self.config.terminal.paste_on_middle_click,
            right_click: self.config.terminal.right_click,
        };
        let (backdrop, terminal_alpha) = self.backdrop(tab);
        let window_alpha = self.window_alpha();
        let layout = tab.solo.as_ref().unwrap_or(&tab.layout);
        let grips = layout.leaves().len() > 1;
        let tile_splits = use_tiles(grips, backdrop.is_some(), window_alpha);
        let panes = layout
            .leaves()
            .into_iter()
            .filter_map(|id| tab.panes.get(id))
            .map(|pane| {
                let id = pane.id;
                let focused = id == tab.focused
                    && self.focused
                    && !typing_elsewhere
                    && !panel_has_keys
                    && pane.prompt.is_none();
                let view: Element<'_, Message> = TerminalView::new(
                    id,
                    &pane.snapshot,
                    &self.colors,
                    self.metrics,
                    self.resolved.font,
                    focused,
                    mouse,
                )
                .status(pane.status.as_deref())
                .hotkeys(&self.config.hotkeys)
                .cursor_on(self.blink.on)
                .default_bg_alpha(terminal_alpha)
                .into();
                let view = view.map(move |message| message.for_pane(id));
                let view = if grips {
                    column![self.pane_grip(pane, id == tab.focused), view].into()
                } else {
                    view
                };
                // A prompt is modal over the pane that asked, not over the whole window.
                match &pane.prompt {
                    Some(state) => (id, modal::over(view, prompt::view(id, state))),
                    None => (id, view),
                }
            })
            .collect();
        let mut divider = to_iced(self.colors.foreground());
        divider.a = 0.3;
        let mut split = SplitContainer::new(
            tab.id,
            layout,
            panes,
            iced::Color {
                a: window_alpha,
                ..to_iced(self.colors.background())
            },
            divider,
        )
        .backdrop(backdrop);
        if let Some(drag) = self.drag.as_ref().filter(|d| d.started) {
            split = split.dropping(drag.target, self.look.accent);
        }
        // Focused pane border: the tab colour, else the accent once there is more than one pane;
        // split panes float as tiles.
        let border = tab
            .color
            .map(context_menu::to_iced)
            .or_else(|| (tab.panes.len() > 1).then_some(self.look.accent));
        if let Some(color) = border {
            split = split.border(tab.focused, color);
        }
        if tile_splits {
            split = split.tiles(
                iced::Color {
                    a: window_alpha,
                    ..self.look.raised
                },
                if self.look.dark {
                    crate::tokens::N900
                } else {
                    self.look.border
                },
                crate::tokens::alpha(border.unwrap_or(self.look.accent), 0.25),
            );
        }
        if let Some(panel) = sftp_panel {
            split = split.side(
                sftp::panel::view(tab.id, panel, &self.colors, self.sftp_host(panel.pane)),
                panel.width,
            );
        }
        let sftp = tab
            .panes
            .values()
            .any(|p| p.sftp.is_some())
            .then_some(sftp_panel.is_some());
        let dialog = sftp_panel.and_then(|p| sftp::dialog(tab.id, p));
        self.chrome(
            crate::search::over(split.into(), search, self.look),
            sftp,
            dialog,
        )
    }

    /// The page of an editor, settings or start page tab.
    fn page<'a>(&'a self, tab: &'a crate::tab::Tab) -> Option<Element<'a, Message>> {
        if let Some(welcome) = &tab.welcome {
            return Some(crate::welcome::page(tab.id, welcome, self));
        }
        if let Some(editor) = &tab.editor {
            let look = self.look;
            return Some(crate::profile_editor::page(
                tab.id,
                editor,
                &self.config,
                // The config.toml column needs the sidebar's room and a wide window.
                !self.profiles.open && self.window_size.width >= 1200.0,
                look,
            ));
        }
        let settings = tab.settings.as_deref()?;
        let cx = crate::settings::Ctx {
            config: &self.config,
            colors: &self.colors,
            look: self.look,
            font: self.resolved.font,
            transparent: self.window_transparent,
            path: tilde(
                &fogu_core::config::config_dir(self.config_dir.as_deref()).join("config.toml"),
            ),
        };
        Some(crate::settings::page(settings, &cx))
    }

    /// Room before the tab headers: the native window buttons on macOS (hidden in full screen).
    pub(crate) fn bar_inset(&self) -> f32 {
        if cfg!(target_os = "macos") && !self.fullscreen {
            crate::tab_bar::TRAFFIC_LIGHTS_W
        } else {
            0.0
        }
    }

    /// The tab bar above `body` (the panes or an editor page), toasts, the profile sidebar and
    /// the overlays.
    fn chrome<'a>(
        &'a self,
        body: Element<'a, Message>,
        sftp: Option<bool>,
        dialog: Option<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        let bar = TabBar {
            tabs: &self.tabs,
            active: self.active,
            hovered: self.hovered_tab,
            renaming: self.renaming.as_ref(),
            look: self.look,
            sftp,
            forwards: self.active_tab().and_then(|tab| {
                tab.panes
                    .values()
                    .any(crate::tab::Pane::is_ssh)
                    .then(|| crate::forwards_dialog::listening(tab.panes.values()))
            }),
            profiles: self.profiles.open,
            settings: self.active_tab().is_some_and(|t| t.settings.is_some()),
            inset: self.bar_inset(),
            width: self.window_size.width,
        }
        .view();
        // Toasts are layered over the panes only, never over the tab bar. The stack is always
        // there (an empty spacer without toasts): wrapping the panes only while a toast shows
        // would rebuild their widget state, and a prompt's text field would lose its focus when
        // the toast goes away.
        let toasts = self
            .toasts
            .view(self.metrics.cell_h, self.look)
            .unwrap_or_else(|| iced::widget::space().into());
        let panes = iced::widget::stack![body, toasts].into();
        let live = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.values())
            .filter(|pane| pane.state.is_running() && (!pane.is_ssh() || pane.connection.is_some()))
            .map(|pane| pane.profile.id.clone())
            .collect();
        let panes =
            profiles::view::with_sidebar(panes, &self.profiles, &self.config, &live, self.look);
        self.overlays(column![bar, panes].into(), dialog)
    }

    /// Modals, the SFTP dialog, the picker and the context menu over `base`.
    fn overlays<'a>(
        &'a self,
        base: Element<'a, Message>,
        sftp_dialog: Option<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        let modal = self
            .paste_modal()
            .or_else(|| self.close_modal())
            .or_else(|| self.delete_modal())
            .or_else(|| self.clear_sessions_modal())
            .or_else(|| self.discard_modal());
        let base = overlay(base, modal);
        let dialog = self
            .secret_dialog
            .as_ref()
            .map(crate::secret_dialog::view)
            .or(sftp_dialog)
            .or_else(|| self.forwards_view());
        let base = match dialog {
            Some(dialog) => modal::over(base, dialog),
            None => base,
        };
        let (entries, rows) = match self.picker {
            Some(_) => profile_picker::listing(self),
            None => (Vec::new(), Vec::new()),
        };
        let base = profile_picker::overlay(base, self.picker.as_ref(), &entries, &rows, self.look);
        let base = context_menu::overlay(base, self.menu.as_ref(), self.window_size, self.look);
        let base = match self.drag_label() {
            Some((at, label)) => {
                let look = self.look;
                let ghost = container(iced::widget::text(label).size(13))
                    .padding([4, 10])
                    .style(move |_theme: &iced::Theme| context_menu::panel_style(look));
                let placed = container(ghost)
                    .padding(iced::Padding::ZERO.left(at.x + 14.0).top(at.y + 14.0))
                    .width(iced::Length::Fill)
                    .height(iced::Length::Fill);
                iced::widget::stack![base, placed].into()
            }
            None => base,
        };
        // macOS: winit cannot start a resize from the app; the native frame resizes instead.
        if self.fullscreen || cfg!(target_os = "macos") {
            return base;
        }
        iced::widget::stack![base, crate::tab_bar::resize_edges()].into()
    }

    /// The header above a pane of a split tab: kind icon, host, cwd, a dot when focused and
    /// "maximize"; it is also the handle that drags the pane to another place.
    fn pane_grip(&self, pane: &crate::tab::Pane, focused: bool) -> Element<'_, Message> {
        use crate::icons::{Icon, glyph, icon};
        use crate::tokens as t;
        let look = self.look;
        let tint = match (focused, look.dark) {
            (true, true) => t::A300,
            (false, true) => t::N400,
            (true, false) => look.accent,
            (false, false) => look.dim,
        };
        let (kind, host) = match &pane.profile.kind {
            fogu_core::ProfileKind::Ssh(ssh) => (
                Icon::Ssh,
                ssh.host
                    .clone()
                    .unwrap_or_else(|| pane.profile.name.clone()),
            ),
            fogu_core::ProfileKind::Local(_) => (Icon::Local, pane.profile.name.clone()),
        };
        let cwd = pane
            .remote_cwd
            .clone()
            .or_else(|| pane.local_cwd.as_ref().map(|p| p.display().to_string()));
        let mut line = iced::widget::row![
            glyph(kind, 13.0, tint),
            iced::widget::text(host)
                .size(12)
                .font(crate::UI_MEDIUM)
                .color(tint)
                .wrapping(iced::widget::text::Wrapping::None),
            iced::widget::text(cwd.unwrap_or_default())
                .size(12)
                .color(look.dim)
                .wrapping(iced::widget::text::Wrapping::None)
                .width(iced::Length::Fill),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        if focused {
            let accent = look.accent;
            line = line.push(container(iced::widget::space()).width(6).height(6).style(
                move |_theme: &iced::Theme| container::Style {
                    background: Some(accent.into()),
                    border: iced::Border::default().rounded(3),
                    shadow: crate::style::glow(accent, 0.8, 6.0),
                    ..container::Style::default()
                },
            ));
        }
        let expand = iced::widget::button(icon(Icon::Expand, 13.0, look.dim))
            .width(22)
            .height(22)
            .padding(0)
            .style(crate::style::icon_btn(look, false))
            .on_press(Message::Action(fogu_core::Action::MaximizePane));
        let strip = container(line.push(expand))
            .width(iced::Length::Fill)
            .height(GRIP_H)
            .padding([0, 10])
            .align_y(iced::Alignment::Center)
            .clip(true);
        iced::widget::mouse_area(strip)
            .interaction(iced::mouse::Interaction::Grab)
            .on_press(Message::Drag(DragMsg::Start(Payload::Pane(pane.id))))
            .into()
    }

    /// The SFTP panel's host tag: the SSH host of its pane.
    fn sftp_host(&self, pane: crate::tab::PaneId) -> String {
        match self.pane(pane).map(|p| &p.profile.kind) {
            Some(fogu_core::ProfileKind::Ssh(ssh)) => ssh.host.clone().unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// The port forwarding dialog, while its pane exists.
    fn forwards_view(&self) -> Option<Element<'_, Message>> {
        let dialog = self.forwards.as_ref()?;
        let pane = self.pane(dialog.pane)?;
        let stored = self.config.profiles.iter().any(|p| p.id == pane.profile.id);
        let look = self.look;
        Some(crate::forwards_dialog::view(dialog, pane, stored, look))
    }

    /// Every pane's session events plus window focus/scale/size changes and Escape.
    pub fn subscription(&self) -> Subscription<Message> {
        let sessions = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.values())
            .flat_map(|pane| {
                let sftp = pane.sftp.as_ref().map(|link| sftp_events(&link.events));
                std::iter::once(session_events(&pane.events)).chain(sftp)
            });
        let window_events = event::listen_with(|event, status, _window| match event {
            iced::Event::Window(window::Event::Focused) => Some(Message::Focused(true)),
            iced::Event::Window(window::Event::CloseRequested) => Some(Message::CloseRequested),
            iced::Event::Window(window::Event::Unfocused) => Some(Message::Focused(false)),
            iced::Event::Window(window::Event::Rescaled(scale)) => {
                Some(Message::ScaleFactor(scale))
            }
            iced::Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(Named::Escape),
                ..
            }) => Some(Message::Escape),
            // Keys no widget took: SFTP panel keys, dialog defaults and picker navigation.
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modified_key,
                modifiers,
                ..
            }) if status == event::Status::Ignored => {
                Some(Message::KeyIgnored(key, modified_key, modifiers))
            }
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(mods)) => {
                Some(Message::Modifiers(mods))
            }
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_)) => Some(Message::MousePressed),
            // Always listened to: a quick click releases before a drag-only subscription starts.
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                Some(Message::Drag(DragMsg::Release))
            }
            // X11 and macOS (winit 0.30 has no Wayland drops); one event per file.
            iced::Event::Window(window::Event::FileDropped(path)) => {
                Some(Message::Desktop(sftp::DesktopMsg::Dropped(path)))
            }
            _ => None,
        });
        let frame = self
            .exit_after_first_frame
            .then(|| window::frames().map(|_| Message::Frame));
        // Pointer moves only reach `update` while a drag may be under way (releases always do).
        let drag = self.drag.is_some().then(|| {
            event::listen_with(|event, _status, _window| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::Drag(DragMsg::Move(position)))
                }
                _ => None,
            })
        });
        Subscription::batch(sessions.chain([window_events]).chain(frame).chain(drag))
    }
}

/// Tile splits need an opaque, undecorated ground; backdrops and translucent windows use the
/// classic edge-to-edge split look.
fn use_tiles(split: bool, backdrop: bool, window_alpha: f32) -> bool {
    split && !backdrop && window_alpha >= 1.0
}

/// `path` with the home directory shown as `~`.
fn tilde(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::use_tiles;

    #[test]
    fn tiles_are_only_used_on_an_opaque_tab_without_a_backdrop() {
        assert!(use_tiles(true, false, 1.0));
        assert!(!use_tiles(true, true, 1.0));
        assert!(!use_tiles(true, false, 0.8));
        assert!(!use_tiles(false, false, 1.0));
    }
}
