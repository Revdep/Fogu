//! The profile row menu: Connect, Connect in split, Open SFTP, Edit… (the profile editor),
//! Duplicate, Use for new tabs, Copy ssh command, Move to group ▸, Delete. Editing items are enabled for stored
//! profiles only; a `~/.ssh/config` host offers "Copy to my profiles" instead of Duplicate.

use fogu_core::config::ProfileEdit;
use fogu_core::{Config, ProfileKind};
use iced::Task;
use iced::widget::operation;

use super::handlers::{Launch, ProfilesMsg};
use super::{Entry, Origin, ssh_command};
use crate::app::App;
use crate::context_menu::{ContextMenu, MENU_INPUT, MenuAction, MenuInput, MenuItem, MenuSubmit};
use crate::message::Message;

fn ui(message: ProfilesMsg) -> MenuAction {
    MenuAction::Ui(Message::Profiles(message))
}

/// The row menu for `entry`.
pub(super) fn row_menu(anchor: iced::Point, entry: &Entry, config: &Config) -> ContextMenu {
    let id = entry.profile.id.clone();
    let stored = entry.origin == Origin::Stored;
    let ssh = matches!(entry.profile.kind, ProfileKind::Ssh(_));
    let item = |label: &str, message: ProfilesMsg, enabled: bool| {
        let mut item = MenuItem::new(label, ui(message));
        item.enabled = enabled;
        item
    };
    let mut items = vec![
        item("Connect", ProfilesMsg::Open(id.clone(), Launch::Tab), true),
        item(
            "Connect in split",
            ProfilesMsg::Open(id.clone(), Launch::Split),
            true,
        ),
    ];
    if ssh {
        items.push(item(
            "Open SFTP",
            ProfilesMsg::Open(id.clone(), Launch::Sftp),
            true,
        ));
    }
    let edit_label = match entry.origin {
        Origin::Stored => "Edit…",
        Origin::SshConfig => "View…",
        Origin::Builtin | Origin::Quick => "Save as profile…",
    };
    let mut edit = item(edit_label, ProfilesMsg::Edit(id.clone()), true);
    edit.separator = true;
    items.push(edit);
    let duplicate = if entry.origin == Origin::SshConfig {
        "Copy to my profiles"
    } else {
        "Duplicate"
    };
    items.push(item(duplicate, ProfilesMsg::Duplicate(id.clone()), true));
    // Stored and built-in profiles: the ones `default_profile` can name.
    if matches!(entry.origin, Origin::Stored | Origin::Builtin) {
        let default = super::is_default(config, &id);
        let label = if default {
            "✓ Used for new tabs"
        } else {
            "Use for new tabs"
        };
        items.push(item(label, ProfilesMsg::SetDefault(id.clone()), !default));
    }
    if ssh {
        let mut copy = item(
            "Copy ssh command",
            ProfilesMsg::CopySsh(id.clone()),
            ssh_command(&entry.profile, config).is_some(),
        );
        if !copy.enabled {
            copy.tooltip = Some("User or host has characters a shell could misread");
        }
        items.push(copy);
    }
    items.push(item("Move to group ▸", ProfilesMsg::MoveMenu, stored));
    let mut delete = item("Delete", ProfilesMsg::AskDelete(id), stored);
    delete.separator = true;
    items.push(delete);
    ContextMenu::new(anchor, items)
}

/// "Move to group ▸": every group (✓ = current), Ungrouped, and a "New group…" field.
fn group_menu(anchor: iced::Point, entry: &Entry, config: &Config) -> ContextMenu {
    let current = entry.profile.group.as_ref();
    let check = |on: bool, name: &str| {
        if on {
            format!("✓ {name}")
        } else {
            format!("   {name}")
        }
    };
    let mut items: Vec<MenuItem> = config
        .groups
        .iter()
        .map(|g| {
            let label = check(current == Some(&g.id), &fogu_term::sanitize_title(&g.name));
            MenuItem::new(label, ui(ProfilesMsg::SetGroup(Some(g.id.clone()))))
        })
        .collect();
    let known = current.is_some_and(|c| config.groups.iter().any(|g| &g.id == c));
    let mut none = MenuItem::new(check(!known, "Ungrouped"), ui(ProfilesMsg::SetGroup(None)));
    none.separator = !items.is_empty();
    items.push(none);
    let mut menu = ContextMenu::new(anchor, items);
    menu.input = Some(MenuInput {
        placeholder: "New group…",
        value: String::new(),
        invalid: false,
        submit: MenuSubmit::Text(|name| Message::Profiles(ProfilesMsg::NewGroup(name))),
    });
    menu
}

impl App {
    /// Row-menu messages.
    pub(super) fn on_profile_menu(&mut self, message: ProfilesMsg) -> Task<Message> {
        let target = match &message {
            ProfilesMsg::Menu(id, _)
            | ProfilesMsg::Duplicate(id)
            | ProfilesMsg::SetDefault(id)
            | ProfilesMsg::Edit(id)
            | ProfilesMsg::CopySsh(id)
            | ProfilesMsg::AskDelete(id) => Some(id.clone()),
            _ => self.profiles.menu_target.clone(),
        };
        let Some(entry) =
            target.and_then(|id| self.all_entries().into_iter().find(|e| e.profile.id == id))
        else {
            self.menu = None;
            return Task::none();
        };
        match message {
            ProfilesMsg::Menu(_, pos) => {
                self.commit_rename();
                self.profiles.selected = pos;
                self.profiles.menu_target = Some(entry.profile.id.clone());
                self.menu = Some(row_menu(self.profiles.cursor, &entry, &self.config));
            }
            ProfilesMsg::MoveMenu => {
                let anchor = self
                    .menu
                    .as_ref()
                    .map_or(self.profiles.cursor, |m| m.anchor);
                self.menu = Some(group_menu(anchor, &entry, &self.config));
                return operation::focus(MENU_INPUT);
            }
            ProfilesMsg::SetGroup(group) if entry.origin == Origin::Stored => {
                let id = entry.profile.id;
                return self.save_edits(&[ProfileEdit::SetGroup { id, group }]);
            }
            ProfilesMsg::NewGroup(name) if entry.origin == Origin::Stored => {
                return self.new_group(&name);
            }
            ProfilesMsg::Duplicate(_) => return self.duplicate(&entry),
            ProfilesMsg::SetDefault(id)
                if matches!(entry.origin, Origin::Stored | Origin::Builtin) =>
            {
                // The built-in default shell is what an unset key means: keep the file clean.
                let id = (id != fogu_core::Profile::default_shell().id).then_some(id);
                return self.save_edits(&[ProfileEdit::SetDefault(id)]);
            }
            ProfilesMsg::Edit(_) => return self.edit_entry(&entry),
            ProfilesMsg::CopySsh(_) => return self.copy_ssh(&entry.profile),
            ProfilesMsg::AskDelete(_) if entry.origin == Origin::Stored => {
                self.menu = None;
                self.profiles.pending_delete = Some(entry.profile.id);
            }
            _ => self.menu = None,
        }
        Task::none()
    }
}
