//! Reducer tests for the file operations: rename / new folder / delete / permissions,
//! follow-the-terminal, the conflict choices, and the App wiring of the entry menu, remote cwd and
//! "cd terminal here".

use std::time::Instant;

use fogu_core::Action;
use fogu_session::sftp::{ConflictKind, Entry, EntryKind, OverwritePolicy};
use fogu_session::{SessionInput, SessionOutput, SftpCmd, SftpEvent};

use super::ops::TargetKind;
use super::tests::{at_home, channel, drain, file, listing, ssh_app};
use super::transfers::{ConflictChoice, Direction, Origin};
use super::*;
use crate::message::Message;

fn link_to_dir(name: &str) -> Entry {
    Entry {
        kind: EntryKind::Symlink,
        link_kind: Some(EntryKind::Dir),
        ..file(name)
    }
}

fn dir(name: &str) -> Entry {
    Entry {
        kind: EntryKind::Dir,
        mode: Some(0o755),
        ..file(name)
    }
}

#[test]
fn rename_validates_then_renames_and_refreshes() {
    let (tx, mut rx) = channel();
    let mut panel = at_home(&tx, &mut rx);
    assert!(panel.start_rename("a"));
    panel.ops.edit.as_mut().unwrap().value = "x/y".into();
    assert!(panel.submit_edit(&tx).unwrap_err().contains("'/'"));
    assert!(panel.ops.edit.is_some(), "the field stays open");
    panel.ops.edit.as_mut().unwrap().value = "b".into();
    assert!(
        panel
            .submit_edit(&tx)
            .unwrap_err()
            .contains("already exists")
    );
    panel.ops.edit.as_mut().unwrap().value = "c".into();
    panel.submit_edit(&tx).unwrap();
    let sent = drain(&mut rx);
    assert!(
        matches!(&sent[0].cmd, SftpCmd::Rename(from, to) if from == "/home/u/a" && to == "/home/u/c")
    );
    let now = Instant::now();
    panel.on_event(SftpEvent::Done(sent[0].id), &tx, now);
    let list = drain(&mut rx);
    assert!(matches!(&list[0].cmd, SftpCmd::List(p) if p == "/home/u"));
    panel.on_event(listing(list[0].id, "/home/u", &["b", "c"]), &tx, now);
    assert!(
        panel.listing.selected.contains("c"),
        "renamed entry selected"
    );
    // A failed operation is a toast, and the listing is refreshed anyway.
    assert!(panel.start_new_folder());
    panel.ops.edit.as_mut().unwrap().value = "new".into();
    panel.submit_edit(&tx).unwrap();
    let sent = drain(&mut rx);
    assert!(matches!(&sent[0].cmd, SftpCmd::Mkdir(p) if p == "/home/u/new"));
    let error = SftpEvent::Error {
        id: sent[0].id,
        message: "permission denied".into(),
    };
    let r = panel.on_event(error, &tx, now);
    assert_eq!(r.notes[0].title, "New folder failed");
    assert!(matches!(&drain(&mut rx)[0].cmd, SftpCmd::List(_)));
}

#[test]
fn delete_counts_directories_and_never_follows_links() {
    let (tx, mut rx) = channel();
    let mut panel = at_home(&tx, &mut rx);
    let entries = vec![file("f"), dir("d"), link_to_dir("l")];
    panel.listing.replace(entries, false);
    for name in ["f", "d", "l"] {
        panel.listing.selected.insert(name.into());
    }
    panel.start_delete(&tx).unwrap();
    let sent = drain(&mut rx);
    assert_eq!(sent.len(), 1, "only the real directory is counted");
    assert!(matches!(&sent[0].cmd, SftpCmd::CountDir(p) if p == "/home/u/d"));
    let dialog = panel.ops.delete.clone().unwrap();
    assert!(ops::delete_text(&dialog).starts_with("Counting"));
    assert!(panel.dialog_open());
    let counted = SftpEvent::Counted {
        id: sent[0].id,
        path: "/home/u/d".into(),
        entries: 42,
    };
    panel.on_event(counted, &tx, Instant::now());
    let dialog = panel.ops.delete.clone().unwrap();
    assert_eq!(
        ops::delete_text(&dialog),
        "Delete 3 selected entries and the 42 items inside? This cannot be undone."
    );
    assert!(dialog.targets.contains(&("l".into(), TargetKind::Link)));
    panel.confirm_delete(&tx).unwrap();
    let cmds: Vec<SftpCmd> = drain(&mut rx).into_iter().map(|r| r.cmd).collect();
    assert_eq!(cmds.len(), 3);
    assert!(
        cmds.iter()
            .any(|c| matches!(c, SftpCmd::RemoveDirRecursive(p) if p == "/home/u/d"))
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, SftpCmd::RemoveFile(p) if p == "/home/u/l"))
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, SftpCmd::RemoveFile(p) if p == "/home/u/f"))
    );
    assert!(panel.ops.delete.is_none());
}

#[test]
fn permissions_dialog_sends_the_new_mode_but_not_for_links() {
    let (tx, mut rx) = channel();
    let mut panel = at_home(&tx, &mut rx);
    panel
        .listing
        .replace(vec![file("a"), link_to_dir("l")], false);
    assert!(panel.start_chmod("l").is_err());
    panel.start_chmod("a").unwrap();
    panel.ops.chmod.as_mut().unwrap().toggle(0o020);
    assert_eq!(panel.ops.chmod.as_ref().unwrap().octal, "664");
    panel.apply_chmod(&tx).unwrap();
    let sent = drain(&mut rx);
    assert!(matches!(&sent[0].cmd, SftpCmd::Chmod(p, 0o664) if p == "/home/u/a"));
}

#[test]
fn follow_reducer_tracks_the_shell_until_manual_navigation() {
    let (tx, mut rx) = channel();
    let mut panel = at_home(&tx, &mut rx);
    assert!(panel.follow, "on by default");
    panel.on_shell_cwd("/tmp", &tx).unwrap();
    let sent = drain(&mut rx);
    assert!(matches!(&sent[0].cmd, SftpCmd::List(p) if p == "/tmp"));
    let now = Instant::now();
    panel.on_event(listing(sent[0].id, "/tmp", &[]), &tx, now);
    // Same directory again: nothing to do; a relative path is ignored.
    panel.on_shell_cwd("/tmp", &tx).unwrap();
    panel.on_shell_cwd("relative", &tx).unwrap();
    assert!(drain(&mut rx).is_empty());
    // Manual navigation turns following off (handlers set it); reports are only remembered.
    panel.follow = false;
    panel.on_shell_cwd("/var", &tx).unwrap();
    assert!(drain(&mut rx).is_empty());
    assert_eq!(panel.shell_cwd.as_deref(), Some("/var"));
    // "Resume following" goes there.
    panel.set_follow(true, &tx).unwrap();
    let sent = drain(&mut rx);
    assert!(matches!(&sent[0].cmd, SftpCmd::List(p) if p == "/var"));
    // A followed directory that can't be listed stops following, with one warning.
    let error = SftpEvent::Error {
        id: sent[0].id,
        message: "no such file".into(),
    };
    let r = panel.on_event(error, &tx, now);
    assert!(!panel.follow);
    assert_eq!(r.notes[0].title, "Stopped following the terminal");
    assert_eq!(panel.path.as_deref(), Some("/tmp"));
}

#[test]
fn keep_both_and_resume_reissue_with_their_policy() {
    let (tx, mut rx) = channel();
    let mut panel = at_home(&tx, &mut rx);
    let local = std::env::temp_dir();
    let origin = Origin::Download {
        remote_dir: "/home/u".into(),
        local_dir: local.clone(),
    };
    let cmd = download_cmd(vec!["/home/u/a".into(), "/home/u/b".into()], local);
    panel
        .issue(&tx, cmd, Direction::Download, "a".into(), origin)
        .unwrap();
    let id = drain(&mut rx)[0].id;
    for (name, resumable) in [("a", true), ("b", false)] {
        let conflict = SftpEvent::Conflict {
            id,
            path: name.into(),
            kind: ConflictKind::PartialExists,
            resumable,
        };
        panel.on_event(conflict, &tx, Instant::now());
    }
    assert!(
        panel
            .answer_conflict(&tx, ConflictChoice::Resume)
            .is_empty()
    );
    assert!(
        panel
            .answer_conflict(&tx, ConflictChoice::KeepBoth)
            .is_empty()
    );
    let cmds: Vec<SftpCmd> = drain(&mut rx).into_iter().map(|r| r.cmd).collect();
    assert!(
        matches!(&cmds[0], SftpCmd::Download { remote, overwrite: OverwritePolicy::Ask, resume: true, .. } if remote == &["/home/u/a"])
    );
    assert!(
        matches!(&cmds[1], SftpCmd::Download { remote, overwrite: OverwritePolicy::KeepBoth, resume: false, .. } if remote == &["/home/u/b"])
    );
    // "keep both" picks the name in the engine: no "Open" target for it.
    let last = panel.transfers.items.last().unwrap();
    assert!(last.open_file.is_none());
    assert!(panel.transfers.items[1].open_file.is_some());
}

/// The SSH app with its panel open on `/home/alice` holding `a.txt` and `dir/`.
fn app_with_panel() -> (crate::app::App, crate::tab::TabId, crate::tab::PaneId) {
    let mut app = ssh_app();
    let _ = app.update(Message::Action(Action::ToggleSftp));
    let (tab, pane) = (app.active, app.active_tab().unwrap().focused);
    let sent = drain(&mut app.stub_sftp[0]);
    let realpath = SftpEvent::Realpath {
        id: sent[1].id,
        path: "/home/alice".into(),
    };
    let _ = app.update(Message::Sftp(SftpMsg::Event(pane, Box::new(realpath))));
    let sent = drain(&mut app.stub_sftp[0]);
    let list = SftpEvent::Listing {
        id: sent[0].id,
        path: "/home/alice".into(),
        entries: vec![file("a.txt"), dir("dir")],
        truncated: false,
    };
    let _ = app.update(Message::Sftp(SftpMsg::Event(pane, Box::new(list))));
    (app, tab, pane)
}

#[test]
fn right_press_opens_the_entry_or_the_area_menu() {
    let (mut app, tab, _) = app_with_panel();
    let panel_msg = |m| Message::Sftp(SftpMsg::Panel(tab, m));
    let at = iced::Point::new(700.0, 300.0);
    // Entry first (the row's own right-press), then the container's `MenuAt`.
    let _ = app.update(panel_msg(PanelMsg::EntryMenu("a.txt".into())));
    let _ = app.update(panel_msg(PanelMsg::MenuAt(at)));
    let menu = app.menu.as_ref().unwrap();
    let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels[0], "Download\u{2026}");
    assert!(labels.contains(&"Permissions\u{2026}"));
    assert_eq!(menu.anchor, at);
    let panel = app.active_tab().unwrap().sftp.as_ref().unwrap();
    assert!(panel.listing.selected.contains("a.txt"));
    // A menu item closes the menu.
    let _ = app.update(panel_msg(PanelMsg::Refresh));
    assert!(app.menu.is_none());
    // Empty area.
    let _ = app.update(panel_msg(PanelMsg::MenuAt(at)));
    let labels: Vec<String> = app
        .menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|i| i.label.clone())
        .collect();
    assert_eq!(labels[0], "Upload\u{2026}");
    assert!(labels.iter().any(|l| l == "New folder\u{2026}"));
}

#[test]
fn remote_cwd_moves_the_panel_and_cd_here_types_a_quoted_cd() {
    let (mut app, tab, pane) = app_with_panel();
    drain(&mut app.stub_sftp[0]);
    let cwd = SessionOutput::Cwd {
        path: "/srv/it's here".into(),
        remote: true,
    };
    let _ = app.update(Message::Session(pane, cwd));
    let sent = drain(&mut app.stub_sftp[0]);
    assert!(matches!(&sent[0].cmd, SftpCmd::List(p) if p == "/srv/it's here"));
    let _ = app.update(Message::Sftp(SftpMsg::Event(
        pane,
        Box::new(listing(sent[0].id, "/srv/it's here", &[])),
    )));
    let _ = app.update(Message::Sftp(SftpMsg::Panel(tab, PanelMsg::CdHere)));
    let mut typed = Vec::new();
    while let Ok(input) = app.stub_inputs[0].try_recv() {
        if let SessionInput::Data(bytes) = input {
            typed.extend_from_slice(&bytes);
        }
    }
    assert_eq!(typed, b"cd -- '/srv/it'\\''s here'\r");
    // Manual navigation stops following.
    let _ = app.update(Message::Sftp(SftpMsg::Panel(tab, PanelMsg::Up)));
    assert!(!app.active_tab().unwrap().sftp.as_ref().unwrap().follow);
}

#[test]
fn follow_setup_asks_before_copying() {
    fn panel(app: &mut crate::app::App, tab: crate::tab::TabId) -> &mut SftpPanelState {
        app.panel_of(tab).unwrap().0
    }
    let (mut app, tab, _pane) = app_with_panel();
    let _ = app.update(Message::Sftp(SftpMsg::Panel(tab, PanelMsg::CwdSetup)));
    assert!(panel(&mut app, tab).dialog_open());
    assert!(dialog(tab, panel(&mut app, tab)).is_some());
    let _ = app.update(Message::Sftp(SftpMsg::Panel(
        tab,
        PanelMsg::CwdSetupDone(false),
    )));
    assert!(!panel(&mut app, tab).dialog_open());
    // Escape closes it too.
    let _ = app.update(Message::Sftp(SftpMsg::Panel(tab, PanelMsg::CwdSetup)));
    assert!(panel(&mut app, tab).cancel_op());
    assert!(!panel(&mut app, tab).dialog_open());
}
