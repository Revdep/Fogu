//! Keyboard and pointer focus of the SFTP panel: after a click inside it, keys no widget took
//! are its hotkeys and navigation (Up/Down, Enter, Backspace, type-to-filter).

use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::operation;

use super::PanelMsg;
use super::panel::{FILTER_INPUT, LIST_ID, ROW_H, rows_px};
use crate::app::App;
use crate::message::Message;

impl App {
    /// Whether keys nobody else took go to the active tab's panel.
    pub(crate) fn panel_has_keys(&self) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let panel_focused = tab
            .sftp
            .as_ref()
            .is_some_and(|p| p.visible && p.focused && !p.dialog_open());
        panel_focused
            && self.picker.is_none()
            && self.menu.is_none()
            && self.renaming.is_none()
            && self.pending_close.is_none()
            && self.pending_paste.is_none()
            && tab.panes.values().all(|p| p.prompt.is_none())
    }

    /// A mouse press anywhere: the SFTP panel takes the keyboard when the pointer is over it,
    /// and so does the profile sidebar. The sidebar gives it back on a click into a pane
    /// (`focus_pane`) or into the panel, not here: this event can arrive after the click that
    /// just opened the sidebar under the pointer.
    pub(crate) fn on_mouse_pressed(&mut self) {
        let active = self.active;
        let mut in_panel = false;
        if let Some(panel) = self
            .tabs
            .iter_mut()
            .find(|t| t.id == active)
            .and_then(|t| t.sftp.as_mut())
        {
            panel.focused = panel.visible && panel.hovered;
            in_panel = panel.focused;
        }
        let sidebar = &mut self.profiles;
        if std::mem::take(&mut sidebar.skip_press) {
            // That press opened a profile: the keyboard stays with the new terminal.
        } else if sidebar.open && sidebar.hovered {
            sidebar.focused = true;
        } else if in_panel || self.tabs.iter().any(|t| t.id == active && t.is_page()) {
            // A click into the SFTP panel or a page (editor, settings) takes the keyboard.
            sidebar.focused = false;
        }
    }

    /// A key no widget took: the panel's hotkeys/navigation when it has the keyboard, else
    /// dialog defaults and picker navigation.
    pub(crate) fn on_key_ignored(
        &mut self,
        key: &Key,
        modified: &Key,
        mods: Modifiers,
    ) -> Task<Message> {
        if let Some(menu) = &mut self.menu
            && menu.input.is_none()
        {
            match key {
                Key::Named(Named::ArrowUp) => menu.move_selection(false),
                Key::Named(Named::ArrowDown) => menu.move_selection(true),
                Key::Named(Named::Enter) => {
                    if let Some(message) = menu.chosen() {
                        return self.update(message);
                    }
                }
                _ => {}
            }
            return Task::none();
        }
        if self.panel_has_keys() {
            if let Some(action) = crate::keys::hotkey(&self.config.hotkeys, key, modified, mods) {
                return self.on_action(action);
            }
            return self.on_panel_key(modified, mods);
        }
        if self.sidebar_has_keys() {
            use crate::profiles::ProfilesMsg;
            if let Some(action) = crate::keys::hotkey(&self.config.hotkeys, key, modified, mods) {
                return self.on_action(action);
            }
            return match key {
                Key::Named(Named::ArrowUp) => self.on_profiles(ProfilesMsg::Move(-1)),
                Key::Named(Named::ArrowDown) => self.on_profiles(ProfilesMsg::Move(1)),
                Key::Named(Named::Enter) => self.on_profiles(ProfilesMsg::Submit),
                _ => Task::none(),
            };
        }
        if self.editor_has_keys() {
            if let Some(action) = crate::keys::hotkey(&self.config.hotkeys, key, modified, mods) {
                return self.on_action(action);
            }
            // Tab / Shift+Tab move between the editor's fields.
            return match key {
                Key::Named(Named::Tab) if mods.shift() => iced::widget::operation::focus_previous(),
                Key::Named(Named::Tab) => iced::widget::operation::focus_next(),
                _ => Task::none(),
            };
        }
        // Tab / Shift+Tab in a pane's login prompt: next/previous field, or back into the only one.
        if let Key::Named(Named::Tab) = key
            && let Some(pane) = self.active_pane().filter(|p| p.prompt.is_some())
        {
            let fields = pane
                .prompt
                .as_ref()
                .map_or(0, crate::prompt::PromptState::fields);
            return match fields {
                0 => Task::none(),
                1 => iced::widget::operation::focus(crate::prompt::field_id(pane.id, 0)),
                _ if mods.shift() => iced::widget::operation::focus_previous(),
                _ => iced::widget::operation::focus_next(),
            };
        }
        match key {
            Key::Named(Named::Enter) => self.on_enter(),
            Key::Named(Named::ArrowUp) => {
                self.on_picker(crate::profile_picker::PickerMsg::Move(-1))
            }
            Key::Named(Named::ArrowDown) => {
                self.on_picker(crate::profile_picker::PickerMsg::Move(1))
            }
            _ => Task::none(),
        }
    }

    /// ↑/↓ move, Enter opens, Backspace goes up, F2 renames, Delete deletes, printable text
    /// starts filtering.
    fn on_panel_key(&mut self, key: &Key, mods: Modifiers) -> Task<Message> {
        let tab = self.active;
        let Some((panel, _)) = self.panel_of(tab) else {
            return Task::none();
        };
        match key {
            Key::Named(Named::ArrowUp | Named::ArrowDown) => {
                let delta = if *key == Key::Named(Named::ArrowUp) {
                    -1
                } else {
                    1
                };
                let Some(row) = panel.listing.move_cursor(delta) else {
                    return Task::none();
                };
                let top = rows_px(row);
                let view = panel.view_h.max(ROW_H);
                let target = if top < panel.scroll_y {
                    top
                } else if top + ROW_H > panel.scroll_y + view {
                    top + ROW_H - view
                } else {
                    return Task::none();
                };
                operation::scroll_to(
                    LIST_ID,
                    iced::widget::scrollable::AbsoluteOffset {
                        x: None,
                        y: Some(target),
                    },
                )
            }
            Key::Named(Named::Enter) => match panel.listing.cursor.clone() {
                Some(name) => self.on_panel(tab, PanelMsg::Open(name)),
                None => Task::none(),
            },
            Key::Named(Named::Backspace) => self.on_panel(tab, PanelMsg::Up),
            Key::Named(Named::F2) => {
                let single = match panel.listing.selected.len() {
                    0 => panel.listing.cursor.clone(),
                    1 => panel.listing.selected.first().cloned(),
                    _ => None,
                };
                match single {
                    Some(name) => self.on_panel(tab, PanelMsg::StartRename(name)),
                    None => Task::none(),
                }
            }
            Key::Named(Named::Delete) => self.on_panel(tab, PanelMsg::Delete),
            Key::Character(c) if !mods.control() && !mods.alt() && !mods.logo() => {
                panel.listing.filter.push_str(c);
                Task::batch([
                    operation::focus(FILTER_INPUT),
                    operation::move_cursor_to_end(FILTER_INPUT),
                ])
            }
            _ => Task::none(),
        }
    }
}
