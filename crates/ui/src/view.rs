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

/// Height of the drag strip above each pane of a split tab.
const GRIP_H: f32 = 6.0;

impl App {
    /// Tab bar above the active tab's split container, plus modal / menu overlays.
    pub fn view(&self) -> Element<'_, Message> {
        let Some(tab) = self.active_tab() else {
            return column![].into();
        };
        if let Some(editor) = &tab.editor {
            let look = self.look;
            let page = crate::profile_editor::page(tab.id, editor, &self.config, look);
            return self.chrome(page, None, None);
        }
        let typing_elsewhere = self.renaming.is_some()
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
        let layout = tab.solo.as_ref().unwrap_or(&tab.layout);
        let grips = layout.leaves().len() > 1;
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
                    column![self.pane_grip(id), view].into()
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
            to_iced(self.colors.background()),
            divider,
        )
        .backdrop(backdrop);
        if let Some(drag) = self.drag.as_ref().filter(|d| d.started) {
            split = split.dropping(drag.target, self.look.accent);
        }
        // Focused pane border: the tab colour, else the accent once there is more than one pane.
        let border = tab
            .color
            .map(context_menu::to_iced)
            .or_else(|| (tab.panes.len() > 1).then(|| to_iced(self.colors.cursor())));
        if let Some(color) = border {
            split = split.border(tab.focused, color);
        }
        if let Some(panel) = sftp_panel {
            split = split.side(sftp::panel::view(tab.id, panel, &self.colors), panel.width);
        }
        let sftp = tab
            .panes
            .values()
            .any(|p| p.sftp.is_some())
            .then_some(sftp_panel.is_some());
        let dialog = sftp_panel.and_then(|p| sftp::dialog(tab.id, p));
        self.chrome(split.into(), sftp, dialog)
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
        let panes = profiles::view::with_sidebar(panes, &self.profiles, &self.config, self.look);
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
        let picker_rows = self
            .picker
            .as_ref()
            .map(|_| profile_picker::listing(self).1);
        let base = profile_picker::overlay(
            base,
            self.picker.as_ref(),
            picker_rows.as_deref().unwrap_or_default(),
            self.look,
        );
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

    /// The strip above a pane (tabs with several panes) that drags it to another place.
    fn pane_grip(&self, pane: crate::tab::PaneId) -> Element<'_, Message> {
        let look = self.look;
        let strip = container(iced::widget::space())
            .width(iced::Length::Fill)
            .height(GRIP_H)
            .style(move |_theme: &iced::Theme| container::background(look.raised));
        iced::widget::mouse_area(strip)
            .interaction(iced::mouse::Interaction::Grab)
            .on_press(Message::Drag(DragMsg::Start(Payload::Pane(pane))))
            .into()
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
            // X11 only (winit 0.30); one event per file.
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
