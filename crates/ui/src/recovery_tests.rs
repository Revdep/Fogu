#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "tests"
)]

use std::path::PathBuf;
use std::sync::Arc;

use fogu_core::recovery::{self, SCROLLBACK_DIR};
use fogu_core::{Config, Profile, RgbColor, Side};
use fogu_session::SessionRuntime;

use super::{InstanceClaim, Recovery};
use crate::RunOptions;
use crate::app::App;
use crate::metrics::{FontUnits, ResolvedFont};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fogu-ui-recovery-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn app(config: Config) -> App {
    let runtime = Arc::new(SessionRuntime::new().unwrap());
    let resolved = ResolvedFont {
        font: iced::Font::MONOSPACE,
        units: FontUnits::FALLBACK,
    };
    App::new(runtime, Arc::new(config), resolved)
}

/// Two tabs: the first split in two, renamed and coloured, with output in its first pane.
fn two_tabs(app: &mut App) {
    let profile = app.default_profile();
    app.open_tab(&profile, None);
    let first = app.active_pane().unwrap().id;
    app.active_pane_mut()
        .unwrap()
        .term
        .feed(b"\x1b[32mhello from before\x1b[0m\r\nline two\r\n");
    app.split_now(first, Side::Right, None, None);
    let tab = app.tabs.iter_mut().find(|t| t.id == app.active).unwrap();
    tab.title_override = Some("logs".to_owned());
    tab.color = Some(RgbColor::from_hex(0x00e5_484d));
    app.open_tab(&profile, None);
}

fn save_now(app: &mut App, dir: &std::path::Path) {
    let mut rec = Recovery::new(dir.to_path_buf());
    app.save_recovery(&mut rec, true, true);
    app.recovery = Some(rec);
}

#[test]
fn a_restart_rebuilds_tabs_splits_titles_and_scrollback() {
    let dir = tmp("restart");
    let mut before = app(Config::default());
    two_tabs(&mut before);
    save_now(&mut before, &dir);
    assert_eq!(
        std::fs::read_dir(dir.join(SCROLLBACK_DIR)).unwrap().count(),
        3
    );

    let mut after = app(Config::default());
    assert!(after.restore_session(&dir, true));
    assert_eq!(after.tabs.len(), 2);
    let tab = &after.tabs[0];
    assert_eq!(tab.layout.leaves().len(), 2);
    assert_eq!(tab.title_override.as_deref(), Some("logs"));
    assert_eq!(tab.color, Some(RgbColor::from_hex(0x00e5_484d)));
    assert_eq!(after.active, after.tabs[1].id, "the active tab too");
    let first = tab.panes[tab.layout.leaves()[0]].term.dump_grid();
    assert!(first.contains("hello from before"), "{first}");
    assert!(first.contains("restored from"), "{first}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_scrollback_file_keeps_the_tab_and_says_so() {
    let dir = tmp("corrupt");
    let mut before = app(Config::default());
    two_tabs(&mut before);
    save_now(&mut before, &dir);
    for entry in std::fs::read_dir(dir.join(SCROLLBACK_DIR)).unwrap() {
        std::fs::write(entry.unwrap().path(), "{ not json").unwrap();
    }
    let mut after = app(Config::default());
    assert!(after.restore_session(&dir, true));
    assert_eq!(after.tabs.len(), 2);
    assert!(!after.toasts.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opted_out_profiles_write_no_scrollback_and_disabling_clears() {
    let dir = tmp("optout");
    let mut config = Config::default();
    config.recovery.restore_scrollback = false;
    let mut before = app(config);
    two_tabs(&mut before);
    save_now(&mut before, &dir);
    assert!(dir.join(recovery::SESSION_FILE).exists());
    let files = std::fs::read_dir(dir.join(SCROLLBACK_DIR)).map_or(0, Iterator::count);
    assert_eq!(files, 0);
    // A profile can still opt in.
    let mut config = Config::default();
    config.recovery.restore_scrollback = false;
    let mut mine = Profile::default_shell();
    mine.restore_scrollback = Some(true);
    let mut app2 = app(config);
    app2.open_tab(&mine, None);
    save_now(&mut app2, &dir);
    assert_eq!(
        std::fs::read_dir(dir.join(SCROLLBACK_DIR)).unwrap().count(),
        1
    );
    // `recovery.enabled = false` deletes everything at startup.
    let mut off = app(Config::default());
    assert!(!off.restore_session(&dir, false));
    assert!(!dir.join(recovery::SESSION_FILE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unchanged_layout_is_not_written_again() {
    let dir = tmp("debounce");
    let mut a = app(Config::default());
    two_tabs(&mut a);
    let mut rec = Recovery::new(dir.clone());
    a.save_recovery(&mut rec, false, true);
    std::fs::remove_file(dir.join(recovery::SESSION_FILE)).unwrap();
    a.save_recovery(&mut rec, false, true);
    assert!(
        !dir.join(recovery::SESSION_FILE).exists(),
        "nothing changed"
    );
    let active = a.active;
    a.tabs
        .iter_mut()
        .find(|t| t.id == active)
        .unwrap()
        .title_override = Some("new name".to_owned());
    a.save_recovery(&mut rec, false, true);
    assert!(dir.join(recovery::SESSION_FILE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_first_instance_claims_the_saved_tabs() {
    let dir = tmp("instance-lock");
    std::fs::create_dir_all(&dir).unwrap();
    let InstanceClaim::Owner(lock) = super::claim_first(&dir) else {
        panic!("the first window owns the lock");
    };
    assert!(matches!(super::claim_first(&dir), InstanceClaim::Other));
    drop(lock);
    assert!(
        matches!(super::claim_first(&dir), InstanceClaim::Owner(_)),
        "the lock goes with the first window"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unusable_instance_lock_never_claims_ownership() {
    let dir = tmp("instance-lock-error");
    assert!(matches!(
        super::claim_first(&dir),
        InstanceClaim::Unconfirmed
    ));
}

#[test]
fn no_recovery_skips_an_existing_lock_and_starts_a_default_tab() {
    let dir = tmp("no-recovery-lock");
    std::fs::create_dir_all(&dir).unwrap();
    let InstanceClaim::Owner(lock) = super::claim_first(&dir) else {
        panic!("test owns the lock");
    };
    let runtime = Arc::new(SessionRuntime::new().unwrap());
    let (app, _task) = App::boot(
        runtime,
        Arc::new(Config::default()),
        RunOptions {
            state_dir: Some(dir.clone()),
            no_recovery: true,
            ..RunOptions::default()
        },
    );
    assert!(app.instance_lock.is_none() && app.recovery.is_none());
    assert_eq!(app.tabs.len(), 1);
    assert!(app.tabs[0].welcome.is_none() && !app.tabs[0].panes.is_empty());
    drop(lock);
    let _ = std::fs::remove_dir_all(&dir);
}
