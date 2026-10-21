//! Reducer tests on a headless `App` whose panes use stub sessions (no process, no window).
#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "tests"
)]

use std::sync::Arc;

use fogu_core::{Action, Config, Dir, Rect, RgbColor, SessionEndBehavior};
use fogu_session::{ExitInfo, SessionInput, SessionOutput, SessionRuntime};
use fogu_term::input::{KeyCode, KeyInput, Mods};

use crate::app::App;
use crate::message::Message;
use crate::metrics::{FontUnits, ResolvedFont};
use crate::tab::{PaneId, TabId};

fn headless() -> App {
    let runtime = Arc::new(SessionRuntime::new().unwrap());
    let resolved = ResolvedFont {
        font: iced::Font::MONOSPACE,
        units: FontUnits::FALLBACK,
    };
    let mut app = App::new(runtime, Arc::new(Config::default()), resolved);
    let profile = app.default_profile();
    app.open_tab(&profile, None);
    app
}

fn act(app: &mut App, action: Action) {
    let _task = app.update(Message::Action(action));
}

fn send(app: &mut App, message: Message) {
    let _task = app.update(message);
}

fn ids(app: &App) -> Vec<TabId> {
    app.tabs.iter().map(|t| t.id).collect()
}

/// Tab `index`'s focused pane.
fn pane_of(app: &App, index: usize) -> PaneId {
    app.tabs[index].focused
}

/// The active tab's panes in layout order.
fn leaves(app: &App) -> Vec<PaneId> {
    let tab = app.active_tab().unwrap();
    tab.layout.leaves().into_iter().copied().collect()
}

/// Splits the focused pane and answers the session's cwd query; returns the new pane.
fn split(app: &mut App, action: Action) -> PaneId {
    let target = pane_of(app, app.active_index().unwrap());
    act(app, action);
    let cwd = SessionOutput::Cwd {
        path: "/tmp".into(),
        remote: false,
    };
    send(app, Message::Session(target, cwd));
    let new = pane_of(app, app.active_index().unwrap());
    assert_ne!(new, target, "the new pane is focused");
    new
}

fn exited(code: i32) -> SessionOutput {
    SessionOutput::Exited(ExitInfo {
        code: Some(code),
        signal: None,
    })
}

/// What the UI sent to stub session `n` (in opening order).
fn sent(app: &mut App, n: usize) -> Vec<SessionInput> {
    let rx = &mut app.stub_inputs[n];
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

const EXITED: SessionOutput = SessionOutput::Exited(ExitInfo {
    code: Some(0),
    signal: None,
});

#[test]
fn new_tab_appends_and_activates() {
    let mut app = headless();
    act(&mut app, Action::NewTab);
    act(&mut app, Action::NewTab);
    let tabs = ids(&app);
    assert_eq!(tabs.len(), 3);
    assert_eq!(app.active, tabs[2]);
    assert!(
        tabs[0].0 < tabs[1].0 && tabs[1].0 < tabs[2].0,
        "monotonic ids"
    );
    assert_eq!(app.title(), "Default shell");
}

#[test]
fn next_prev_and_go_to_tab() {
    let mut app = headless();
    act(&mut app, Action::NewTab);
    act(&mut app, Action::NewTab);
    let tabs = ids(&app);
    act(&mut app, Action::NextTab);
    assert_eq!(app.active, tabs[0], "next wraps around");
    act(&mut app, Action::PrevTab);
    assert_eq!(app.active, tabs[2], "prev wraps around");
    act(&mut app, Action::GoToTab(2));
    assert_eq!(app.active, tabs[1]);
    act(&mut app, Action::GoToTab(9));
    assert_eq!(app.active, tabs[1], "out of range is ignored");
    act(&mut app, Action::GoToTab(1));
    assert_eq!(app.active, tabs[0]);
}

#[test]
fn close_tab_hangs_up_and_goes_at_once_then_activates_a_neighbour() {
    let mut app = headless();
    act(&mut app, Action::NewTab);
    act(&mut app, Action::NewTab);
    let tabs = ids(&app);
    act(&mut app, Action::GoToTab(2));
    act(&mut app, Action::CloseTab);
    assert!(matches!(
        sent(&mut app, 1).as_slice(),
        [SessionInput::Close]
    ));
    // Gone at once: the session hangs up and reaps the shell in the background.
    assert_eq!(ids(&app), [tabs[0], tabs[2]]);
    assert_eq!(app.active, tabs[2]);
}

#[test]
fn close_tab_times_out_and_closed_sessions_go_at_once() {
    let mut app = headless();
    act(&mut app, Action::NewTab);
    let tabs = ids(&app);
    send(&mut app, Message::CloseTab(tabs[0]));
    assert_eq!(ids(&app), [tabs[1]]);

    // Session already gone (input channel closed): removed without waiting. Last tab → empty
    // (the app exits).
    app.stub_inputs.clear();
    act(&mut app, Action::CloseTab);
    assert!(app.tabs.is_empty());
}

#[test]
fn rename_overrides_osc_title_and_empty_clears() {
    let mut app = headless();
    let pane = pane_of(&app, 0);
    // Dynamic titles on (profiles keep their name by default).
    app.pane_mut(pane).unwrap().profile.disable_dynamic_title = false;
    send(
        &mut app,
        Message::Session(pane, SessionOutput::Title("vim".into())),
    );
    assert_eq!(app.title(), "vim");
    act(&mut app, Action::RenameTab);
    send(&mut app, Message::RenameInput("work\u{7}".into()));
    send(&mut app, Message::CommitRename);
    assert_eq!(app.title(), "work");
    send(
        &mut app,
        Message::Session(pane, SessionOutput::Title("zsh".into())),
    );
    assert_eq!(
        app.title(),
        "work",
        "the override wins over later OSC titles"
    );
    let tab = app.active;
    send(&mut app, Message::StartRename(tab));
    send(&mut app, Message::RenameInput("  ".into()));
    send(&mut app, Message::CommitRename);
    assert_eq!(app.title(), "zsh");
    send(&mut app, Message::StartRename(tab));
    send(&mut app, Message::Escape);
    assert!(app.renaming.is_none());
}

#[test]
fn tab_menu_sets_and_clears_colour() {
    let mut app = headless();
    let tab = app.active;
    send(&mut app, Message::OpenTabMenu(tab));
    send(&mut app, Message::MenuColors);
    send(&mut app, Message::MenuInput("nope".into()));
    send(&mut app, Message::MenuSubmit);
    assert!(app.menu.as_ref().unwrap().input.as_ref().unwrap().invalid);
    send(&mut app, Message::MenuInput("#ff0000".into()));
    send(&mut app, Message::MenuSubmit);
    assert!(app.menu.is_none());
    assert_eq!(app.tabs[0].color, Some(RgbColor { r: 255, g: 0, b: 0 }));
    send(&mut app, Message::OpenTabMenu(tab));
    send(&mut app, Message::Escape);
    assert!(app.menu.is_none());
    send(&mut app, Message::SetTabColor(None));
    assert_eq!(app.tabs[0].color, None);
}

#[test]
fn background_menu_sets_edits_and_clears_the_tab_background() {
    use crate::background_menu::BgMsg;
    use fogu_core::{Background, Fit, TabBackground};

    let mut app = headless();
    let tab = app.active;
    send(&mut app, Message::OpenTabMenu(tab));
    send(&mut app, BgMsg::Menu.into());
    let menu = app.menu.as_ref().unwrap();
    let save = menu
        .items
        .iter()
        .find(|i| i.label == "Save to profile")
        .unwrap();
    assert!(!save.enabled, "the built-in profile is not stored");
    assert!(save.tooltip.is_some());
    assert!(menu.slider.is_none(), "no image: no opacity slider");
    send(&mut app, BgMsg::Colors.into());
    send(&mut app, Message::MenuInput("#102030".into()));
    send(&mut app, Message::MenuSubmit);
    let color = Background::Color(RgbColor::from_hex(0x10_2030));
    assert_eq!(app.tabs[0].background, TabBackground::Set(color));
    assert!(app.menu.is_none());

    send(
        &mut app,
        BgMsg::ImagePicked(Some("/no/such/wall.png".into())).into(),
    );
    send(&mut app, Message::OpenTabMenu(tab));
    send(&mut app, BgMsg::Menu.into());
    send(&mut app, BgMsg::Opacity(40.0).into());
    send(&mut app, BgMsg::Fit(Fit::Contain).into());
    let TabBackground::Set(Background::Image { opacity, fit, .. }) = &app.tabs[0].background else {
        panic!("expected an image");
    };
    assert!((opacity - 0.4).abs() < 1e-6);
    assert_eq!(*fit, Fit::Contain);
    assert!(
        app.menu.as_ref().unwrap().slider.is_some(),
        "menu stays open"
    );
    // Not decoded yet (or failed): nothing drawn, terminal opaque.
    let (backdrop, alpha) = app.backdrop(&app.tabs[0]);
    assert!(backdrop.is_none());
    assert!((alpha - 1.0).abs() < f32::EPSILON);

    send(&mut app, BgMsg::Clear.into());
    assert_eq!(app.tabs[0].background, TabBackground::None);
}

#[test]
fn new_tab_here_uses_only_a_local_cwd_reply() {
    let mut app = headless();
    let pane = pane_of(&app, 0);
    act(&mut app, Action::NewTabHere);
    assert!(matches!(
        sent(&mut app, 0).as_slice(),
        [SessionInput::QueryCwd]
    ));
    let cwd = |remote| SessionOutput::Cwd {
        path: "/tmp".into(),
        remote,
    };
    send(&mut app, Message::Session(pane, cwd(true)));
    assert_eq!(app.tabs.len(), 1, "remote paths are informational");
    send(&mut app, Message::Session(pane, cwd(false)));
    assert_eq!(app.tabs.len(), 2);
    send(&mut app, Message::Session(pane, cwd(false)));
    assert_eq!(app.tabs.len(), 2, "only one tab per query");
}

#[test]
fn events_from_unknown_panes_are_ignored() {
    let mut app = headless();
    send(&mut app, Message::Session(PaneId(999), EXITED));
    assert_eq!(app.tabs.len(), 1);
    assert!(app.active_pane().unwrap().status.is_none());
    let pane = pane_of(&app, 0);
    act(&mut app, Action::RestartTab);
    assert_ne!(pane_of(&app, 0), pane, "restart opens a new pane");
    send(
        &mut app,
        Message::Session(pane, SessionOutput::Title("old".into())),
    );
    assert_eq!(app.title(), "Default shell");
}

#[test]
fn split_twice_navigate_and_close_the_middle_pane() {
    let mut app = headless();
    let p0 = pane_of(&app, 0);
    let p1 = split(&mut app, Action::SplitRight);
    assert!(matches!(
        sent(&mut app, 0).as_slice(),
        [SessionInput::QueryCwd]
    ));
    let p2 = split(&mut app, Action::SplitDown);
    assert_eq!(leaves(&app), [p0, p1, p2]);
    assert_eq!(app.tabs.len(), 1, "splits stay in the tab");

    act(&mut app, Action::PaneNav(Dir::Up));
    assert_eq!(pane_of(&app, 0), p1);
    act(&mut app, Action::PaneNav(Dir::Left));
    assert_eq!(pane_of(&app, 0), p0);
    act(&mut app, Action::PaneNav(Dir::Left));
    assert_eq!(pane_of(&app, 0), p0, "no neighbour: focus stays");
    send(&mut app, Message::FocusPane(p1));
    assert_eq!(pane_of(&app, 0), p1);

    act(&mut app, Action::ClosePane);
    assert!(matches!(
        sent(&mut app, 1).as_slice(),
        [SessionInput::QueryCwd, SessionInput::Close]
    ));
    assert_eq!(leaves(&app), [p0, p2]);
    assert_eq!(pane_of(&app, 0), p0, "focus moves to a remaining pane");
    act(&mut app, Action::PaneNav(Dir::Right));
    assert_eq!(pane_of(&app, 0), p2);

    // Closing the last panes closes the tab (and, here, the app).
    act(&mut app, Action::ClosePane);
    act(&mut app, Action::ClosePane);
    assert!(app.tabs.is_empty());
}

#[test]
fn split_without_a_local_cwd_reply_opens_after_the_timeout() {
    let mut app = headless();
    let p0 = pane_of(&app, 0);
    act(&mut app, Action::SplitRight);
    let remote = SessionOutput::Cwd {
        path: "/srv".into(),
        remote: true,
    };
    send(&mut app, Message::Session(p0, remote));
    assert_eq!(leaves(&app), [p0], "remote paths are ignored");
    send(&mut app, Message::CwdTimeout(p0));
    assert_eq!(leaves(&app).len(), 2);
    send(&mut app, Message::CwdTimeout(p0));
    assert_eq!(leaves(&app).len(), 2, "one pane per query");
}

#[test]
fn divider_ratio_is_clamped_and_equalized() {
    let mut app = headless();
    split(&mut app, Action::SplitRight);
    let tab = app.active;
    let first_width = |app: &App| {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
        };
        app.active_tab().unwrap().layout.layout(area, 0.0)[0].1.w
    };
    send(&mut app, Message::SplitRatio(tab, Vec::new(), 5.0));
    assert!((first_width(&app) - 90.0).abs() < 1e-3);
    send(&mut app, Message::SplitRatio(tab, Vec::new(), -1.0));
    assert!((first_width(&app) - 10.0).abs() < 1e-3);
    send(&mut app, Message::Equalize(tab));
    assert!((first_width(&app) - 50.0).abs() < 1e-3);
}

#[test]
fn exited_pane_keeps_and_restarts_on_enter_or_closes() {
    let mut app = headless();
    let p0 = pane_of(&app, 0);
    send(&mut app, Message::Session(p0, exited(3)));
    let status = app.active_pane().unwrap().status.clone().unwrap();
    assert!(status.contains("exited with code 3") && status.contains("Press any key to restart"));
    let enter = KeyInput {
        key: KeyCode::Enter,
        text: None,
        mods: Mods::default(),
    };
    send(&mut app, Message::Key(enter));
    let restarted = pane_of(&app, 0);
    assert_ne!(restarted, p0, "Enter restarts the pane in place");
    assert_eq!(leaves(&app), [restarted]);
    assert!(app.active_pane().unwrap().status.is_none());

    // `Close`: the exited pane goes away, its sibling and the tab stay.
    app.active_pane_mut()
        .unwrap()
        .profile
        .behavior_on_session_end = SessionEndBehavior::Close;
    let p1 = split(&mut app, Action::SplitDown);
    send(&mut app, Message::Session(p1, exited(0)));
    assert_eq!(leaves(&app), [restarted]);
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn grid_resize_targets_its_pane_and_never_goes_below_two_by_one() {
    let mut app = headless();
    let p0 = pane_of(&app, 0);
    let p1 = split(&mut app, Action::SplitRight);
    send(
        &mut app,
        Message::GridResized {
            pane: p0,
            cols: 0,
            rows: 0,
        },
    );
    let size = |app: &App, id| app.pane(id).unwrap().size();
    assert_eq!((size(&app, p0).cols, size(&app, p0).rows), (2, 1));
    assert_eq!(size(&app, p1).cols, 80, "the sibling is untouched");
}

#[test]
fn the_title_bar_moves_the_window_only_once_the_pointer_moves() {
    use crate::message::WindowCmd;
    use iced::Point;
    let mut app = headless();
    send(&mut app, Message::TabBarCursor(Point::new(500.0, 10.0)));
    send(&mut app, Message::Window(WindowCmd::Press));
    send(&mut app, Message::TabBarCursor(Point::new(502.0, 11.0)));
    assert!(app.bar_press.is_some(), "a jitter is still a click");
    // A double-click maximizes without a pending move.
    send(&mut app, Message::Window(WindowCmd::ToggleMaximize));
    assert!(app.bar_press.is_none());
    send(&mut app, Message::Window(WindowCmd::Press));
    send(&mut app, Message::TabBarCursor(Point::new(520.0, 10.0)));
    assert!(app.bar_press.is_none(), "the move started");
}

#[test]
fn moving_left_onto_a_tab_keeps_its_hover_and_close_button() {
    let mut app = headless();
    let profile = app.default_profile();
    app.open_tab(&profile, None);
    app.open_tab(&profile, None);
    let ids = ids(&app);
    // Pointer moves from the third header to the second in one event: the second's enter is
    // published before the third's leave.
    send(&mut app, Message::HoverTab(Some(ids[2])));
    send(&mut app, Message::HoverTab(Some(ids[1])));
    send(&mut app, Message::LeaveTab(ids[2]));
    assert_eq!(app.hovered_tab, Some(ids[1]));
    send(&mut app, Message::LeaveTab(ids[1]));
    assert_eq!(app.hovered_tab, None);
}

#[test]
fn find_opens_the_search_bar_and_escape_closes_it() {
    let mut app = headless();
    act(&mut app, Action::Find);
    let pane = pane_of(&app, 0);
    assert_eq!(app.search.as_ref().map(|s| s.pane), Some(pane));
    send(
        &mut app,
        Message::Search(crate::search::SearchMsg::Input("x".into())),
    );
    assert_eq!(app.search.as_ref().unwrap().query, "x");
    // Find again while open keeps the query.
    act(&mut app, Action::Find);
    assert_eq!(app.search.as_ref().unwrap().query, "x");
    send(&mut app, Message::Escape);
    assert!(app.search.is_none());
    act(&mut app, Action::Find);
    send(&mut app, Message::Search(crate::search::SearchMsg::Close));
    assert!(app.search.is_none());
}
