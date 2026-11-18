//! File operations of the panel: inline rename and new folder, delete with a confirmation that
//! counts a directory's contents first, and the permissions dialog. Pure reducer over the command
//! channel; the App wiring is in `menu.rs`, the dialogs in `dialogs.rs`.
//!
//! Remote names are attacker data: shown through `display_name` only. Destructive operations
//! always ask; a symlink is deleted as a link (`RemoveFile`), never followed, and the recursive
//! delete refuses a symlink root.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use fogu_session::sftp::{CmdId, EntryKind};
use fogu_session::{SftpCmd, SftpRequest, display_name, validate_name};
use tokio::sync::mpsc::Sender;

use super::breadcrumb::join;
use super::chmod::ChmodDialog;
use super::{Pending, SftpPanelState};

/// The inline name field: rename `old` in `dir`, or (`old` = `None`) create a folder there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameEdit {
    pub dir: String,
    pub old: Option<String>,
    pub value: String,
}

/// What a delete target is (decides the command and the dialog text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    /// File, device, …: `RemoveFile`.
    File,
    /// A symbolic link (to anything): `RemoveFile` removes the link itself.
    Link,
    /// A real directory: counted, then `RemoveDirRecursive`.
    Tree,
}

/// The delete confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteDialog {
    pub dir: String,
    /// (name, kind), raw names.
    pub targets: Vec<(String, TargetKind)>,
    /// `CountDir` answers still missing.
    pub waiting: BTreeSet<CmdId>,
    /// Entries inside the directories counted so far.
    pub items: u64,
    /// Why a count failed (sanitized), if one did.
    pub count_error: Option<String>,
}

/// Inline edits, dialogs and the context-menu target of one panel.
#[derive(Clone, Debug, Default)]
pub struct Ops {
    pub edit: Option<NameEdit>,
    pub delete: Option<DeleteDialog>,
    pub chmod: Option<ChmodDialog>,
    /// The "Follow shell" setup dialog is open.
    pub cwd_setup: bool,
    /// The entry the next `MenuAt` opens its menu for (set by the entry's right-press).
    pub menu_target: Option<String>,
}

impl SftpPanelState {
    /// F2 / "Rename…": an inline field in `name`'s row.
    pub fn start_rename(&mut self, name: &str) -> bool {
        let Some(dir) = self.path.clone() else {
            return false;
        };
        if !self.listing.entries.iter().any(|e| e.name == name) {
            return false;
        }
        self.ops.edit = Some(NameEdit {
            dir,
            old: Some(name.to_owned()),
            value: name.to_owned(),
        });
        true
    }

    /// "New folder": an empty inline field at the top of the list.
    pub fn start_new_folder(&mut self) -> bool {
        let Some(dir) = self.path.clone() else {
            return false;
        };
        self.ops.edit = Some(NameEdit {
            dir,
            old: None,
            value: String::new(),
        });
        true
    }

    /// Enter in the inline field: validates the name and sends `Rename` / `Mkdir`. An invalid
    /// name keeps the field open and returns why.
    pub fn submit_edit(&mut self, tx: &Sender<SftpRequest>) -> Result<(), String> {
        let Some(edit) = self.ops.edit.take() else {
            return Ok(());
        };
        if edit.old.as_deref() == Some(edit.value.as_str()) {
            return Ok(());
        }
        let checked = match validate_name(&edit.value) {
            Err(err) => Err(format!("Invalid name: {err}")),
            Ok(_) if self.listing.entries.iter().any(|e| e.name == edit.value) => Err(format!(
                "\u{201c}{}\u{201d} already exists",
                display_name(&edit.value)
            )),
            Ok(_) => Ok(()),
        };
        if let Err(err) = checked {
            self.ops.edit = Some(edit);
            return Err(err);
        }
        let target = join(&edit.dir, &edit.value);
        let (what, cmd, name) = match &edit.old {
            Some(old) => ("Rename", SftpCmd::Rename(join(&edit.dir, old), target), old),
            None => ("New folder", SftpCmd::Mkdir(target), &edit.value),
        };
        let name = name.clone();
        let id = self.send(tx, cmd)?;
        self.pending.insert(
            id,
            Pending::Op {
                what,
                dir: edit.dir,
                name,
            },
        );
        // Selected once the refreshed listing shows it (`Listing::replace` keeps it).
        self.listing.selected = BTreeSet::from([edit.value.clone()]);
        self.listing.cursor = Some(edit.value);
        Ok(())
    }

    /// Delete key / "Delete…": the selection (else the cursor entry) goes into a confirmation;
    /// each real directory is counted first.
    pub fn start_delete(&mut self, tx: &Sender<SftpRequest>) -> Result<(), String> {
        let Some(dir) = self.path.clone() else {
            return Ok(());
        };
        let mut targets: Vec<(String, TargetKind)> = self
            .listing
            .selected_entries()
            .iter()
            .map(|e| (e.name.clone(), target_kind(e.kind)))
            .collect();
        if targets.is_empty()
            && let Some(entry) = self.listing.cursor_entry()
        {
            targets.push((entry.name.clone(), target_kind(entry.kind)));
        }
        if targets.is_empty() {
            return Ok(());
        }
        let mut waiting = BTreeSet::new();
        for (name, _) in targets.iter().filter(|(_, k)| *k == TargetKind::Tree) {
            let id = self.send(tx, SftpCmd::CountDir(join(&dir, name)))?;
            self.pending.insert(id, Pending::Count);
            waiting.insert(id);
        }
        self.ops.delete = Some(DeleteDialog {
            dir,
            targets,
            waiting,
            items: 0,
            count_error: None,
        });
        Ok(())
    }

    /// A `CountDir` answer (`Err` = sanitized failure).
    pub(super) fn on_counted(&mut self, id: CmdId, entries: Result<u64, String>) {
        let Some(dialog) = self.ops.delete.as_mut() else {
            return;
        };
        if !dialog.waiting.remove(&id) {
            return;
        }
        match entries {
            Ok(n) => dialog.items = dialog.items.saturating_add(n),
            Err(err) => dialog.count_error = Some(err),
        }
    }

    /// "Delete" in the confirmation: one command per target (links and files as files).
    pub fn confirm_delete(&mut self, tx: &Sender<SftpRequest>) -> Result<(), String> {
        let Some(dialog) = self.ops.delete.take() else {
            return Ok(());
        };
        for (name, kind) in dialog.targets {
            let path = join(&dialog.dir, &name);
            let cmd = match kind {
                TargetKind::Tree => SftpCmd::RemoveDirRecursive(path),
                TargetKind::File | TargetKind::Link => SftpCmd::RemoveFile(path),
            };
            let id = self.send(tx, cmd)?;
            let dir = dialog.dir.clone();
            self.pending.insert(
                id,
                Pending::Op {
                    what: "Delete",
                    dir,
                    name,
                },
            );
        }
        Ok(())
    }

    /// "Permissions…" for `name` (not for symlinks: `setstat` would change the target).
    pub fn start_chmod(&mut self, name: &str) -> Result<(), String> {
        let (Some(dir), Some(entry)) = (
            self.path.clone(),
            self.listing.entries.iter().find(|e| e.name == name),
        ) else {
            return Ok(());
        };
        if entry.kind == EntryKind::Symlink {
            return Err("Permissions of a symbolic link cannot be changed over SFTP".into());
        }
        let Some(mode) = entry.mode else {
            return Err("The server did not report this entry's permissions".into());
        };
        self.ops.chmod = Some(ChmodDialog::new(join(&dir, name), name.to_owned(), mode));
        Ok(())
    }

    /// "Apply" in the permissions dialog.
    pub fn apply_chmod(&mut self, tx: &Sender<SftpRequest>) -> Result<(), String> {
        let Some(dialog) = self.ops.chmod.take() else {
            return Ok(());
        };
        let Some(mode) = dialog.result() else {
            self.ops.chmod = Some(dialog);
            return Err("Not an octal mode (e.g. 644)".into());
        };
        let id = self.send(tx, SftpCmd::Chmod(dialog.path.clone(), mode))?;
        let dir = super::breadcrumb::parent(&dialog.path).unwrap_or_default();
        self.pending.insert(
            id,
            Pending::Op {
                what: "Permissions",
                dir,
                name: dialog.name,
            },
        );
        Ok(())
    }

    /// Escape: closes an inline field or an operation dialog; `false` when none was open.
    pub fn cancel_op(&mut self) -> bool {
        self.ops.edit.take().is_some()
            || self.ops.delete.take().is_some()
            || self.ops.chmod.take().is_some()
            || std::mem::take(&mut self.ops.cwd_setup)
    }

    /// An operation ended (`Done` or `Error`): refresh the directory once the last one of
    /// its batch is answered.
    pub(super) fn op_ended(&mut self, dir: &str, tx: &Sender<SftpRequest>) -> Result<(), String> {
        let more = self
            .pending
            .values()
            .any(|p| matches!(p, Pending::Op { .. }));
        if more || self.path.as_deref() != Some(dir) || self.loading.is_some() {
            return Ok(());
        }
        self.list(tx, dir.to_owned(), false)
    }
}

fn target_kind(kind: EntryKind) -> TargetKind {
    match kind {
        EntryKind::Dir => TargetKind::Tree,
        EntryKind::Symlink => TargetKind::Link,
        EntryKind::File | EntryKind::Other => TargetKind::File,
    }
}

/// The confirmation text (names sanitized).
pub fn delete_text(d: &DeleteDialog) -> String {
    if !d.waiting.is_empty() {
        return "Counting what would be deleted\u{2026}".to_owned();
    }
    let inside = match d.items {
        0 => String::new(),
        1 => " and the 1 item inside".to_owned(),
        n => format!(" and the {n} items inside"),
    };
    let mut text = match d.targets.as_slice() {
        [(name, kind)] => {
            let name = display_name(name);
            match kind {
                TargetKind::File => format!("Delete \u{201c}{name}\u{201d}?"),
                TargetKind::Link => {
                    format!("Delete the link \u{201c}{name}\u{201d}? What it points to is kept.")
                }
                TargetKind::Tree if d.items == 0 && d.count_error.is_none() => {
                    format!("Delete the empty folder \u{201c}{name}\u{201d}?")
                }
                TargetKind::Tree => format!("Delete the folder \u{201c}{name}\u{201d}{inside}?"),
            }
        }
        targets => format!("Delete {} selected entries{inside}?", targets.len()),
    };
    if let Some(err) = &d.count_error {
        let _infallible = write!(text, " (The contents could not be counted: {err})");
    }
    text.push_str(" This cannot be undone.");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(targets: &[(&str, TargetKind)], items: u64) -> DeleteDialog {
        DeleteDialog {
            dir: "/d".into(),
            targets: targets.iter().map(|(n, k)| ((*n).to_owned(), *k)).collect(),
            waiting: BTreeSet::new(),
            items,
            count_error: None,
        }
    }

    #[test]
    fn delete_texts() {
        let rows = [
            (
                dialog(&[("a.txt", TargetKind::File)], 0),
                "Delete \u{201c}a.txt\u{201d}? This cannot be undone.",
            ),
            (
                dialog(&[("logs", TargetKind::Tree)], 42),
                "Delete the folder \u{201c}logs\u{201d} and the 42 items inside? This cannot be \
                 undone.",
            ),
            (
                dialog(&[("e", TargetKind::Tree)], 0),
                "Delete the empty folder \u{201c}e\u{201d}? This cannot be undone.",
            ),
            (
                dialog(&[("l", TargetKind::Link)], 0),
                "Delete the link \u{201c}l\u{201d}? What it points to is kept. This cannot be \
                 undone.",
            ),
            (
                dialog(&[("a", TargetKind::File), ("b", TargetKind::Tree)], 1),
                "Delete 2 selected entries and the 1 item inside? This cannot be undone.",
            ),
            (
                dialog(&[("x\u{1b}[2J", TargetKind::File)], 0),
                "Delete \u{201c}x\\x1b[2J\u{201d}? This cannot be undone.",
            ),
        ];
        for (d, want) in rows {
            assert_eq!(delete_text(&d), want);
        }
        let mut counting = dialog(&[("logs", TargetKind::Tree)], 0);
        counting.waiting.insert(CmdId(1));
        assert!(delete_text(&counting).starts_with("Counting"));
    }
}
