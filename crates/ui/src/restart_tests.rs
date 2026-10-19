//! Reducer tests for "press any key to restart/reconnect" and history kept across it.
#![expect(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

use std::sync::Arc;

use fogu_core::{
    Action, Config, EffectiveProfile, Profile, ProfileId, ProfileKind, SessionEndBehavior, SshOpts,
};
use fogu_session::{ConnectionId, ExitInfo, SessionInput, SessionOutput, SessionRuntime};
use fogu_term::TermMode;
use fogu_term::input::{KeyCode, KeyInput, Mods};
use iced::keyboard::key::Named;
use iced::keyboard::{Key, Location, Modifiers};

use super::{RECONNECTED, RESTARTED};
use crate::app::App;
use crate::keys;
use crate::message::Message;
use crate::metrics::{FontUnits, ResolvedFont};
use crate::tab::{PaneId, PaneState};

fn app_with(config: Config, profile: Option<Profile>) -> App {
    let runtime = Arc::new(SessionRuntime::new().unwrap());
    let resolved = ResolvedFont {
        font: iced::Font::MONOSPACE,
        units: FontUnits::FALLBACK,
    };
    let mut app = App::new(runtime, Arc::new(config), resolved);
    let profile = profile.unwrap_or_else(|| app.default_profile());
    app.open_tab(&profile, None);
    app
}

fn ssh_profile() -> Profile {
    Profile {
        id: ProfileId("box".to_owned()),
        name: "Box".to_owned(),
        kind: ProfileKind::Ssh(SshOpts {
            host: Some("box.lan".to_owned()),
            user: Some("alice".to_owned()),
            ..SshOpts::default()
        }),
        ..Profile::default_shell()
    }
}

fn send(app: &mut App, message: Message) {
    let _task = app.update(message);
}

fn pane(app: &App) -> PaneId {
    app.active_pane().unwrap().id
}

fn key(code: KeyCode, mods: Mods) -> Message {
    Message::Key(KeyInput {
        key: code,
        text: None,
        mods,
    })
}

/// The focused pane printed `output` (and set `modes`), then exited with code 3.
fn ended(app: &mut App, output: &[u8]) -> PaneId {
    let id = pane(app);
    app.active_pane_mut().unwrap().term.feed(output);
    let exit = SessionOutput::Exited(ExitInfo {
        code: Some(3),
        signal: None,
    });
    send(app, Message::Session(id, exit));
    id
}

/// Every visible row of the focused pane.
fn screen(app: &App) -> String {
    app.active_pane().unwrap().term.dump_grid()
}

#[test]
fn a_letter_restarts_an_exited_pane_keeping_its_history() {
    let mut app = app_with(Config::default(), None);
    let old = ended(&mut app, b"old output\r\n$ exit 3\r\n");
    let status = app.active_pane().unwrap().status.clone().unwrap();
    assert!(status.ends_with("Press any key to restart"), "{status}");
    send(&mut app, key(KeyCode::Char('a'), Mods::default()));
    let new = pane(&app);
    assert_ne!(new, old);
    let shown = app.active_pane().unwrap();
    assert!(shown.state.is_running() && shown.status.is_none());
    let text = screen(&app);
    assert!(text.contains("old output\n$ exit 3\n"), "{text}");
    assert!(text.contains(RESTARTED), "{text}");
    // The key that restarted is not typed into either session.
    let typed = |rx: &mut tokio::sync::mpsc::Receiver<SessionInput>| {
        std::iter::from_fn(|| rx.try_recv().ok()).any(|i| matches!(i, SessionInput::Data(_)))
    };
    assert!(!app.stub_inputs.iter_mut().any(typed));
}

#[test]
fn ime_text_and_ctrl_letters_restart_too() {
    let mut app = app_with(Config::default(), None);
    let old = ended(&mut app, b"x");
    send(&mut app, Message::Text("é".to_owned()));
    assert_ne!(pane(&app), old);
    let old = ended(&mut app, b"y");
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    send(&mut app, key(KeyCode::Char('c'), ctrl));
    assert_ne!(pane(&app), old);
}

#[test]
fn modifiers_unencodable_keys_and_app_shortcuts_do_not_restart() {
    let mut app = app_with(Config::default(), None);
    let old = ended(&mut app, b"x");
    // A bare modifier never becomes a key for the terminal.
    let shift = keys::key_input(
        &Key::Named(Named::Shift),
        Location::Left,
        Modifiers::SHIFT,
        None,
    );
    assert_eq!(shift, None);
    // A key without an encoding (F13) is ignored.
    send(&mut app, key(KeyCode::F(13), Mods::default()));
    assert_eq!(pane(&app), old);
    // Ctrl+Shift+T / Ctrl+Shift+W (Cmd+T / Cmd+W on macOS) are actions (the widget never sends
    // them as keys).
    let hotkeys = &app.config.hotkeys;
    let mods = if cfg!(target_os = "macos") {
        Modifiers::LOGO
    } else {
        Modifiers::CTRL | Modifiers::SHIFT
    };
    let chord = |c: &str| {
        let k = Key::Character(c.into());
        keys::hotkey(hotkeys, &k, &k, mods)
    };
    assert_eq!(chord("t"), Some(Action::NewTab));
    assert_eq!(chord("w"), Some(Action::CloseTab));
    send(&mut app, Message::Action(Action::NewTab));
    assert_eq!(app.tabs.len(), 2);
    let first = &app.tabs[0];
    assert_eq!(first.focused, old, "the ended pane was not restarted");
    assert!(matches!(first.panes[&old].state, PaneState::Exited(_)));
}

#[test]
fn history_is_dropped_when_the_option_is_off() {
    let mut config = Config::default();
    config.terminal.keep_history_on_restart = false;
    let mut app = app_with(config, None);
    ended(&mut app, b"old output\r\n");
    send(&mut app, key(KeyCode::Enter, Mods::default()));
    let text = screen(&app);
    assert!(
        !text.contains("old output") && !text.contains(RESTARTED),
        "{text}"
    );
}

#[test]
fn modes_set_by_the_old_session_do_not_reach_the_new_one() {
    let mut app = app_with(Config::default(), None);
    let fresh = app.active_pane().unwrap().term.mode();
    // Mouse reporting, bracketed paste, app cursor keys + keypad, then vim's alternate screen.
    ended(
        &mut app,
        b"kept\r\n\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?1h\x1b=\x1b[?1049hVIM",
    );
    send(&mut app, key(KeyCode::Char('q'), Mods::default()));
    let shown = app.active_pane().unwrap();
    assert_eq!(shown.term.mode(), fresh);
    assert!(!shown.term.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(shown.snapshot.mode, fresh, "the drawn snapshot too");
    let text = screen(&app);
    assert!(text.contains("kept") && !text.contains("VIM"), "{text}");
}

#[test]
fn a_disconnected_ssh_pane_reconnects_on_any_key_with_its_history() {
    let mut app = app_with(Config::default(), Some(ssh_profile()));
    let old = ended(&mut app, b"alice@box:~$ ls\r\nnotes.txt\r\n");
    let status = app.pane(old).unwrap().status.clone().unwrap();
    assert!(status.ends_with("Press any key to reconnect"), "{status}");
    send(&mut app, key(KeyCode::Char('r'), Mods::default()));
    assert_ne!(pane(&app), old);
    let text = screen(&app);
    assert!(
        text.contains("notes.txt") && text.contains(RECONNECTED),
        "{text}"
    ); // The kept history must survive `clear_service_messages_on_connect` (default on).
    let clears = |p: &EffectiveProfile| match p {
        EffectiveProfile::Ssh(ssh) => ssh.clear_service_messages_on_connect,
        EffectiveProfile::Local(_) => false,
    };
    let opened: Vec<bool> = app.stub_profiles.iter().map(clears).collect();
    assert_eq!(opened, [true, false]);
}

#[test]
fn restarting_a_running_pane_starts_blank() {
    let mut app = app_with(Config::default(), None);
    app.active_pane_mut()
        .unwrap()
        .term
        .feed(b"still running\r\n");
    send(&mut app, Message::Action(Action::RestartTab));
    assert!(!screen(&app).contains("still running"));
}

fn ssh_with(end: SessionEndBehavior, remember_cwd: bool) -> Profile {
    let mut profile = ssh_profile();
    profile.behavior_on_session_end = end;
    if let ProfileKind::Ssh(ssh) = &mut profile.kind {
        ssh.remember_cwd = Some(remember_cwd);
    }
    profile
}

/// The active SSH pane got connected (with `cwd` reported), then its session ended with `info`.
fn connected_then(app: &mut App, info: ExitInfo, cwd: Option<&str>) -> PaneId {
    let id = pane(app);
    send(
        app,
        Message::Session(id, SessionOutput::Connection(ConnectionId(1))),
    );
    app.active_pane_mut().unwrap().remote_cwd = cwd.map(str::to_owned);
    send(app, Message::Session(id, SessionOutput::Exited(info)));
    id
}

const KILLED: ExitInfo = ExitInfo {
    code: None,
    signal: Some(9),
};
const DROPPED: ExitInfo = ExitInfo {
    code: None,
    signal: None,
};

#[test]
fn reconnect_counts_down_then_reconnects_in_the_remembered_cwd() {
    let profile = ssh_with(SessionEndBehavior::Reconnect, true);
    let mut app = app_with(Config::default(), Some(profile));
    let old = connected_then(&mut app, KILLED, Some("/srv/app"));
    let status = app.pane(old).unwrap().status.clone().unwrap();
    assert!(status.contains("Reconnecting in 2 s"), "{status}");
    send(&mut app, Message::ReconnectTick(old));
    let status = app.pane(old).unwrap().status.clone().unwrap();
    assert!(status.contains("Reconnecting in 1 s"), "{status}");
    send(&mut app, Message::ReconnectTick(old));
    assert_ne!(pane(&app), old, "reconnected in place");
    let cwd = match app.stub_profiles.last().unwrap() {
        EffectiveProfile::Ssh(ssh) => ssh.cwd.clone(),
        EffectiveProfile::Local(_) => None,
    };
    assert_eq!(cwd.as_deref(), Some("/srv/app"));
}

#[test]
fn esc_cancels_the_countdown_and_a_failed_login_never_counts_down() {
    let mut app = app_with(
        Config::default(),
        Some(ssh_with(SessionEndBehavior::Reconnect, false)),
    );
    let old = connected_then(&mut app, KILLED, None);
    send(&mut app, key(KeyCode::Escape, Mods::default()));
    assert_eq!(pane(&app), old, "Esc does not reconnect");
    let status = app.pane(old).unwrap().status.clone().unwrap();
    assert!(status.ends_with("Press any key to reconnect"), "{status}");
    send(&mut app, Message::ReconnectTick(old));
    assert_eq!(pane(&app), old, "a cancelled countdown stays cancelled");
    // Never connected (login failed): no automatic retry loop.
    let mut app = app_with(
        Config::default(),
        Some(ssh_with(SessionEndBehavior::Reconnect, false)),
    );
    let id = pane(&app);
    send(
        &mut app,
        Message::Session(id, SessionOutput::Exited(DROPPED)),
    );
    assert_eq!(app.pane(id).unwrap().reconnect_in, None);
}

#[test]
fn auto_reconnects_only_a_dropped_connection() {
    let auto = || Some(ssh_with(SessionEndBehavior::Auto, false));
    let mut app = app_with(Config::default(), auto());
    let id = connected_then(&mut app, DROPPED, None);
    assert_eq!(app.pane(id).unwrap().reconnect_in, Some(2));
    let mut app = app_with(Config::default(), auto());
    let exit3 = ExitInfo {
        code: Some(3),
        signal: None,
    };
    let id = connected_then(&mut app, exit3, None);
    assert_eq!(
        app.pane(id).unwrap().reconnect_in,
        None,
        "a real exit is kept"
    );
    let mut app = app_with(Config::default(), auto());
    let clean = ExitInfo {
        code: Some(0),
        signal: None,
    };
    let id = connected_then(&mut app, clean, None);
    assert!(app.pane(id).is_none(), "`exit` closes the tab");
}

#[test]
fn a_split_of_the_same_profile_starts_in_the_remembered_cwd() {
    let profile = ssh_with(SessionEndBehavior::Keep, true);
    let mut app = app_with(Config::default(), Some(profile.clone()));
    let target = pane(&app);
    app.active_pane_mut().unwrap().remote_cwd = Some("/srv".to_owned());
    let last_cwd = |app: &App| match app.stub_profiles.last().unwrap() {
        EffectiveProfile::Ssh(ssh) => ssh.cwd.clone(),
        EffectiveProfile::Local(_) => None,
    };
    // Sidebar middle-click passes the profile explicitly.
    app.split_now(target, fogu_core::Side::Right, None, Some(profile));
    assert_eq!(last_cwd(&app).as_deref(), Some("/srv"));
    let mut other = ssh_with(SessionEndBehavior::Keep, true);
    other.id = ProfileId("other".to_owned());
    app.split_now(target, fogu_core::Side::Right, None, Some(other));
    assert_eq!(last_cwd(&app), None, "another profile's host");
}
