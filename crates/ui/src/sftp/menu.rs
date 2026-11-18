//! The entry / empty-area context menu of the SFTP panel (the generic `ContextMenu` overlay, items
//! carrying panel messages) and the App wiring of the file operations.

use fogu_session::sftp::EntryKind;
use fogu_session::{display_name, validate_name};
use iced::widget::operation;
use iced::{Point, Task, clipboard};

use super::breadcrumb::join;
use super::list_view::EDIT_INPUT;
use super::listing::is_dir;
use super::panel::msg;
use super::transfers::{Direction, Origin};
use super::{PanelMsg, SftpPanelState, download_cmd, download_dir, transfer_label};
use crate::app::App;
use crate::context_menu::{ContextMenu, MenuAction, MenuItem};
use crate::message::Message;
use crate::notifications::{Level, Notification};
use crate::tab::TabId;

impl App {
    /// Operation, menu and follow input of tab `tab`'s panel.
    pub(super) fn on_panel_ops(&mut self, tab: TabId, message: PanelMsg) -> Task<Message> {
        match message {
            PanelMsg::MenuAt(at) => return self.open_panel_menu(tab, at),
            PanelMsg::CopyPath(name) => return self.copy_path(tab, &name),
            PanelMsg::DownloadOpen(name) => return self.download_entry(tab, &name, true),
            PanelMsg::CdHere => return self.cd_here(tab),
            PanelMsg::CwdSetupDone(copy) => return self.cwd_setup_done(tab, copy),
            _ => {}
        }
        let Some((panel, tx)) = self.panel_of(tab) else {
            return Task::none();
        };
        let focus_edit = || {
            Task::batch([
                operation::focus(EDIT_INPUT),
                operation::select_all(EDIT_INPUT),
            ])
        };
        let result = match message {
            PanelMsg::EntryMenu(name) => {
                panel.focused = true;
                panel.listing.right_click(&name);
                panel.ops.menu_target = Some(name);
                Ok(())
            }
            PanelMsg::StartRename(name) => {
                if panel.start_rename(&name) {
                    return focus_edit();
                }
                Ok(())
            }
            PanelMsg::NewFolder => {
                if panel.start_new_folder() {
                    return focus_edit();
                }
                Ok(())
            }
            PanelMsg::NameInput(value) => {
                if let Some(edit) = panel.ops.edit.as_mut() {
                    edit.value = value;
                }
                Ok(())
            }
            PanelMsg::NameSubmit => match panel.submit_edit(&tx) {
                // Invalid name: the field stays open; give it the keyboard back.
                Err(err) => {
                    let note = Notification::new(Level::Warn, "SFTP").detail(err);
                    return Task::batch([self.notify(note), focus_edit()]);
                }
                Ok(()) => Ok(()),
            },
            PanelMsg::CwdSetup => {
                panel.ops.cwd_setup = true;
                Ok(())
            }
            PanelMsg::Delete => panel.start_delete(&tx),
            PanelMsg::DeleteConfirm(true) => panel.confirm_delete(&tx),
            PanelMsg::DeleteConfirm(false) => {
                panel.ops.delete = None;
                Ok(())
            }
            PanelMsg::Permissions(name) => panel.start_chmod(&name),
            PanelMsg::ChmodBit(bit) => {
                if let Some(dialog) = panel.ops.chmod.as_mut() {
                    dialog.toggle(bit);
                }
                Ok(())
            }
            PanelMsg::ChmodOctal(value) => {
                if let Some(dialog) = panel.ops.chmod.as_mut() {
                    dialog.type_octal(value);
                }
                Ok(())
            }
            PanelMsg::ChmodDone(true) => panel.apply_chmod(&tx),
            PanelMsg::ChmodDone(false) => {
                panel.ops.chmod = None;
                Ok(())
            }
            PanelMsg::Follow(on) => panel.set_follow(on, &tx),
            _ => Ok(()),
        };
        match result {
            Ok(()) => Task::none(),
            Err(err) => self.notify(Notification::new(Level::Warn, "SFTP").detail(err)),
        }
    }

    /// Right-press in the panel at `at` (window coordinates): the entry menu when the press
    /// was on an entry (`EntryMenu` came first), else the empty-area menu.
    fn open_panel_menu(&mut self, tab: TabId, at: Point) -> Task<Message> {
        let Some((panel, _)) = self.panel_of(tab) else {
            return Task::none();
        };
        let target = panel.ops.menu_target.take();
        let items = match target.and_then(|n| panel.listing.entries.iter().find(|e| e.name == n)) {
            Some(entry) => entry_menu(tab, panel, entry),
            None => area_menu(tab, panel),
        };
        self.menu = Some(ContextMenu::new(at, items));
        Task::none()
    }

    /// "Copy path": the raw remote path, unless it holds control characters (pasted into a
    /// shell, a newline would run a command).
    fn copy_path(&mut self, tab: TabId, name: &str) -> Task<Message> {
        let Some(path) = self
            .panel_of(tab)
            .and_then(|(panel, _)| panel.path.clone())
            .map(|dir| join(&dir, name))
        else {
            return Task::none();
        };
        if path.chars().any(char::is_control) {
            let note = Notification::new(Level::Warn, "Not copied").detail(format!(
                "{} contains control characters.",
                display_name(&path)
            ));
            return self.notify(note);
        }
        clipboard::write(path)
    }

    /// Downloads entry `name` of the shown directory to the download directory; `open` =
    /// "Download and open" (opened with the desktop's default app once complete).
    pub(super) fn download_entry(&mut self, tab: TabId, name: &str, open: bool) -> Task<Message> {
        let target = download_dir(&self.config.sftp);
        let Some((panel, tx)) = self.panel_of(tab) else {
            return Task::none();
        };
        let Some(dir) = panel.path.clone() else {
            return Task::none();
        };
        let result = target
            .ok_or_else(|| "no download directory (set [sftp] download_dir)".to_owned())
            .and_then(|local_dir| {
                std::fs::create_dir_all(&local_dir).map_err(|err| {
                    let shown = display_name(&local_dir.to_string_lossy());
                    format!("cannot create {shown}: {err}")
                })?;
                let cmd = download_cmd(vec![join(&dir, name)], local_dir.clone());
                let label = transfer_label(&cmd);
                let origin = Origin::Download {
                    remote_dir: dir,
                    local_dir,
                };
                panel
                    .issue(&tx, cmd, Direction::Download, label, origin)
                    .map(|row| row.open_when_done = open)
            });
        match result {
            Ok(()) => Task::none(),
            Err(err) => self.notify(Notification::new(Level::Error, "SFTP").detail(err)),
        }
    }
}

fn item(label: &str, message: Option<Message>) -> MenuItem {
    let enabled = message.is_some();
    let mut item = MenuItem::new(label, MenuAction::Ui(message.unwrap_or(Message::CloseMenu)));
    item.enabled = enabled;
    item
}

fn separated(mut item: MenuItem) -> MenuItem {
    item.separator = true;
    item
}

/// Download…, Download and open, Rename…, Delete…, Permissions…, Copy path, Refresh.
fn entry_menu(
    tab: TabId,
    panel: &SftpPanelState,
    entry: &fogu_session::sftp::Entry,
) -> Vec<MenuItem> {
    let m = |message| Some(msg(tab, message));
    let single = panel.listing.selected.len() <= 1;
    let name = entry.name.clone();
    let openable = single && !is_dir(entry) && entry.kind != EntryKind::Other;
    let chmod = single && entry.kind != EntryKind::Symlink && entry.mode.is_some();
    // A name that cannot be written locally can't be downloaded either (the engine skips it).
    let local_ok = validate_name(&name).is_ok();
    vec![
        item("Download\u{2026}", m(PanelMsg::Download)),
        item(
            "Download and open",
            m(PanelMsg::DownloadOpen(name.clone())).filter(|_| openable && local_ok),
        ),
        separated(item(
            "Rename\u{2026}  (F2)",
            m(PanelMsg::StartRename(name.clone())).filter(|_| single),
        )),
        item("Delete\u{2026}  (Del)", m(PanelMsg::Delete)),
        item(
            "Permissions\u{2026}",
            m(PanelMsg::Permissions(name.clone())).filter(|_| chmod),
        ),
        item("Copy path", m(PanelMsg::CopyPath(name))),
        separated(item("Refresh", m(PanelMsg::Refresh))),
    ]
}

/// Upload…, New folder…, Refresh, Show/Hide hidden files.
fn area_menu(tab: TabId, panel: &SftpPanelState) -> Vec<MenuItem> {
    let m = |message| Some(msg(tab, message));
    let has_dir = panel.path.is_some();
    let hidden = panel.listing.show_hidden;
    vec![
        item("Upload\u{2026}", m(PanelMsg::Upload).filter(|_| has_dir)),
        item(
            "New folder\u{2026}",
            m(PanelMsg::NewFolder).filter(|_| has_dir),
        ),
        separated(item("Refresh", m(PanelMsg::Refresh))),
        item(
            if hidden {
                "Hide hidden files"
            } else {
                "Show hidden files"
            },
            m(PanelMsg::ShowHidden(!hidden)),
        ),
    ]
}
