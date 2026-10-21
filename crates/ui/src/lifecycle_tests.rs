//! Reducer tests for pane lifecycle (exit policy, crashes), OSC 52 policy and toasts.
#![expect(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

use std::io;
use std::sync::Arc;

use fogu_core::{Action, Config, Osc52Policy, SessionEndBehavior};
use fogu_session::{ClipboardRequest, ExitInfo, SessionError, SessionOutput, SessionRuntime};
use fogu_term::ClipboardType;
use fogu_term::input::{KeyCode, KeyInput, Mods};

use crate::app::App;
use crate::message::Message;
use crate::metrics::{FontUnits, ResolvedFont};
use crate::notifications::Level;
use crate::tab::{PaneId, PaneState};

fn app_with(config: Config) -> App {
    let runtime = Arc::new(SessionRuntime::new().unwrap());
    let resolved = ResolvedFont {
        font: iced::Font::MONOSPACE,
        units: FontUnits::FALLBACK,
    };
    let mut app = App::new(runtime, Arc::new(config), resolved);
    let profile = app.default_profile();
    app.open_tab(&profile, None);
    app
}

fn send(app: &mut App, message: Message) {
    let _task = app.update(message);
}

fn focused(app: &App) -> PaneId {
    app.tabs[0].focused
}

fn leaves(app: &App) -> Vec<PaneId> {
    app.tabs[0].layout.leaves().into_iter().copied().collect()
}

fn exit(code: Option<i32>, signal: Option<i32>) -> SessionOutput {
    SessionOutput::Exited(ExitInfo { code, signal })
}

fn enter() -> Message {
    Message::Key(KeyInput {
        key: KeyCode::Enter,
        text: None,
        mods: Mods::default(),
    })
}

/// Splits the focused pane (the cwd query is answered right away) and returns the new pane.
fn split(app: &mut App) -> PaneId {
    let target = focused(app);
    send(app, Message::Action(Action::SplitRight));
    let cwd = SessionOutput::Cwd {
        path: "/tmp".into(),
        remote: false,
    };
    send(app, Message::Session(target, cwd));
    focused(app)
}

fn spawn_failure() -> SessionOutput {
    SessionOutput::Error(SessionError::Spawn {
        program: "/nonexistent".to_owned(),
        source: Arc::new(io::Error::from(io::ErrorKind::NotFound)),
    })
}

fn clip(len: usize) -> SessionOutput {
    SessionOutput::ClipboardStore(ClipboardRequest {
        target: ClipboardType::Clipboard,
        payload: "x".repeat(len),
        len,
    })
}

#[test]
fn default_policy_closes_clean_exits_and_keeps_failures() {
    let mut app = app_with(Config::default());
    let keep = focused(&app);
    let second = split(&mut app);
    assert_eq!(
        app.pane(second).unwrap().profile.behavior_on_session_end,
        SessionEndBehavior::CloseIfClean
    );
    // exit 1 keeps the pane with the code shown.
    send(&mut app, Message::Session(second, exit(Some(1), None)));
    let status = app.pane(second).unwrap().status.clone().unwrap();
    assert!(status.contains("exited with code 1"), "{status}");
    // exit 0 closes just that pane; the tab stays.
    send(&mut app, Message::Session(keep, exit(Some(0), None)));
    assert_eq!(leaves(&app), [second]);
    // A signal death is not clean.
    send(&mut app, Message::Session(second, exit(None, Some(9))));
    assert!(matches!(
        app.pane(second).unwrap().state,
        PaneState::Exited(_)
    ));
}

#[test]
fn spawn_failure_crashes_the_pane_with_an_error_toast_and_restarts() {
    let mut app = app_with(Config::default());
    // Even `Close` must not hide a crash.
    app.active_pane_mut()
        .unwrap()
        .profile
        .behavior_on_session_end = SessionEndBehavior::Close;
    let pane = focused(&app);
    send(&mut app, Message::Session(pane, spawn_failure()));
    let shown = app.pane(pane).unwrap();
    assert!(matches!(&shown.state, PaneState::Crashed(why) if why.contains("/nonexistent")));
    assert!(
        shown
            .status
            .as_deref()
            .unwrap()
            .contains("Press any key to restart")
    );
    let toast = app.toasts.iter().next().unwrap();
    assert_eq!(toast.level, Level::Error);
    assert!(!toast.auto_dismisses(), "errors are sticky");
    assert!(toast.detail.contains("failed to start /nonexistent"));
    // Enter (any key) replaces the pane with a new one (new id, new session, running).
    send(&mut app, enter());
    let fresh = focused(&app);
    assert_ne!(fresh, pane);
    assert!(app.pane(pane).is_none());
    assert!(app.pane(fresh).unwrap().state.is_running());
    assert!(app.pane(fresh).unwrap().status.is_none());
}

/// A config whose OSC 52 policy is "ask" (the default is "allow").
fn asking() -> Config {
    let mut config = Config::default();
    config.terminal.osc52_write = Osc52Policy::Ask;
    config
}

#[test]
fn osc52_ask_shows_a_sticky_toast_and_copy_or_deny_resolves_it() {
    let mut app = app_with(asking());
    let pane = focused(&app);
    send(&mut app, Message::Session(pane, clip(42)));
    let toast = app.toasts.iter().next().unwrap().clone();
    assert!(toast.detail.contains("42 bytes"));
    assert!(!toast.auto_dismisses());
    assert_eq!(app.pending_clipboard.len(), 1);
    send(&mut app, Message::DismissToast(toast.id));
    assert!(
        app.toasts.is_empty() && app.pending_clipboard.is_empty(),
        "deny drops it"
    );

    send(&mut app, Message::Session(pane, clip(7)));
    let id = app.toasts.iter().next().unwrap().id;
    send(&mut app, Message::ToastAccept(id));
    assert!(
        app.toasts.is_empty() && app.pending_clipboard.is_empty(),
        "copy consumed it"
    );
}

#[test]
fn enter_copies_a_pending_osc52_write_instead_of_typing() {
    let mut app = app_with(asking());
    let pane = focused(&app);
    send(&mut app, Message::Session(pane, clip(5)));
    send(&mut app, enter());
    assert!(app.toasts.is_empty() && app.pending_clipboard.is_empty());
}

#[test]
fn enter_confirms_and_escape_cancels_a_pending_paste() {
    let mut app = app_with(Config::default());
    let pane = focused(&app);
    let paste = |text: &str| Message::Pane(pane, Box::new(Message::PasteReady(Some(text.into()))));
    send(&mut app, paste("ls\nrm -rf build"));
    assert!(app.pending_paste.is_some(), "multi-line paste asks first");
    send(&mut app, enter());
    assert!(app.pending_paste.is_none(), "Enter pastes");
    send(&mut app, paste("a\nb"));
    let _ = app.on_escape();
    assert!(app.pending_paste.is_none(), "Escape cancels");
}

#[test]
fn osc52_allow_is_silent_and_deny_is_an_info_toast() {
    let mut config = Config::default();
    config.terminal.osc52_write = Osc52Policy::Allow;
    let mut app = app_with(config);
    let pane = focused(&app);
    send(&mut app, Message::Session(pane, clip(5)));
    assert!(app.toasts.is_empty() && app.pending_clipboard.is_empty());

    let mut config = Config::default();
    config.terminal.osc52_write = Osc52Policy::Deny;
    let mut app = app_with(config);
    let pane = focused(&app);
    send(&mut app, Message::Session(pane, clip(5)));
    let toast = app.toasts.iter().next().unwrap();
    assert_eq!(toast.level, Level::Info);
    assert!(app.pending_clipboard.is_empty());
    assert!(!format!("{:?}", app.pending_clipboard).contains('x'));
}

#[test]
fn dropped_input_and_unknown_scheme_become_toasts() {
    let mut config = Config::default();
    config.appearance.color_scheme = "No Such Scheme".to_owned();
    let mut app = app_with(config);
    let note = app.toasts.iter().next().unwrap();
    assert_eq!(note.level, Level::Warn);
    assert!(note.detail.contains("No Such Scheme"));
    let pane = focused(&app);
    send(
        &mut app,
        Message::Session(pane, SessionOutput::InputDropped),
    );
    assert_eq!(app.toasts.iter().count(), 2);
}

#[test]
fn expire_message_drops_only_due_toasts() {
    let mut app = app_with(Config::default());
    let pane = focused(&app);
    send(
        &mut app,
        Message::Session(pane, SessionOutput::InputDropped),
    );
    send(&mut app, Message::Session(pane, spawn_failure()));
    assert_eq!(app.toasts.iter().count(), 2);
    send(&mut app, Message::ToastExpire);
    assert_eq!(app.toasts.iter().count(), 2, "nothing is due yet");
}
