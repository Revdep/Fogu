#![expect(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

use std::sync::Arc;

use fogu_core::{Action, Config};
use fogu_session::SessionRuntime;
use iced::{Point, Rectangle};

use super::*;
use crate::metrics::{FontUnits, ResolvedFont};

#[test]
fn drop_zones_are_the_nearest_quarter_edge_else_the_centre() {
    let r = Rectangle::new(Point::new(100.0, 50.0), iced::Size::new(400.0, 200.0));
    let cases = [
        ((110.0, 150.0), Zone::Left),
        ((490.0, 150.0), Zone::Right),
        ((300.0, 55.0), Zone::Top),
        ((300.0, 245.0), Zone::Bottom),
        ((300.0, 150.0), Zone::Center),
        // Corner: the nearer edge wins (2 % from the left, 5 % from the top).
        ((108.0, 60.0), Zone::Left),
    ];
    for ((x, y), want) in cases {
        assert_eq!(drop_zone(Point::new(x, y), r), want, "{x},{y}");
    }
}

#[test]
fn insertion_index_picks_the_nearest_gap() {
    let s = TAB_STRIDE;
    assert_eq!(insertion_index(-30.0, 3), 0);
    assert_eq!(insertion_index(s * 0.4, 3), 0);
    assert_eq!(insertion_index(s * 0.6, 3), 1);
    assert_eq!(insertion_index(s * 2.5 + 1.0, 3), 3);
    assert_eq!(insertion_index(s * 10.0, 3), 3);
}

fn app() -> App {
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

fn drag(app: &mut App, payload: Payload, to: Point, target: Option<(PaneId, Zone)>) {
    let _ = app.update(Message::Drag(DragMsg::Start(payload)));
    let _ = app.update(Message::Drag(DragMsg::Move(Point::new(5.0, 300.0))));
    let _ = app.update(Message::Drag(DragMsg::Move(to)));
    let _ = app.update(Message::Drag(DragMsg::Hover(target)));
    let _ = app.update(Message::Drag(DragMsg::Release));
}

#[test]
fn a_tab_dropped_on_a_pane_becomes_a_split_there() {
    let mut app = app();
    let profile = app.default_profile();
    let first = app.active_pane().unwrap().id;
    let first_tab = app.active;
    app.open_tab(&profile, None);
    let second_tab = app.active;
    let moved = app.active_pane().unwrap().id;
    app.activate(first_tab);
    drag(
        &mut app,
        Payload::Tab(second_tab),
        Point::new(600.0, 300.0),
        Some((first, Zone::Right)),
    );
    assert_eq!(app.tabs.len(), 1, "the dragged tab closed");
    let tab = &app.tabs[0];
    assert_eq!(tab.layout.leaves(), vec![&first, &moved]);
    assert!(tab.panes.contains_key(&moved), "its session moved along");
    assert_eq!(tab.focused, moved);
}

#[test]
fn a_pane_dropped_on_the_centre_swaps_and_on_an_edge_moves() {
    let mut app = app();
    let a = app.active_pane().unwrap().id;
    app.split_now(a, fogu_core::Side::Right, None, None);
    let b = app.active_pane().unwrap().id;
    drag(
        &mut app,
        Payload::Pane(a),
        Point::new(600.0, 300.0),
        Some((b, Zone::Center)),
    );
    assert_eq!(app.tabs[0].layout.leaves(), vec![&b, &a]);
    drag(
        &mut app,
        Payload::Pane(a),
        Point::new(600.0, 300.0),
        Some((b, Zone::Bottom)),
    );
    let rects = app.tabs[0].layout.layout(
        fogu_core::Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        },
        0.0,
    );
    let below = rects.iter().find(|(id, _)| *id == a).unwrap().1;
    assert!(
        below.y > 0.0 && below.w > 99.0,
        "a is now under b: {below:?}"
    );
    // Onto itself: nothing happens.
    let before = app.tabs[0].layout.clone();
    drag(
        &mut app,
        Payload::Pane(a),
        Point::new(600.0, 300.0),
        Some((a, Zone::Left)),
    );
    assert_eq!(app.tabs[0].layout, before);
}

#[test]
fn a_short_press_is_not_a_drag_and_tabs_reorder_in_the_bar() {
    let mut app = app();
    let profile = app.default_profile();
    let first = app.active;
    app.open_tab(&profile, None);
    app.open_tab(&profile, None);
    let third = app.active;
    // A press and release in place: no move.
    let _ = app.update(Message::Drag(DragMsg::Start(Payload::Tab(first))));
    let _ = app.update(Message::Drag(DragMsg::Move(Point::new(50.0, 10.0))));
    let _ = app.update(Message::Drag(DragMsg::Move(Point::new(52.0, 11.0))));
    let _ = app.update(Message::Drag(DragMsg::Release));
    assert_eq!(app.tabs[0].id, first);
    // Dragged past the third header: last.
    let _ = app.update(Message::Drag(DragMsg::Start(Payload::Tab(first))));
    let _ = app.update(Message::Drag(DragMsg::Move(Point::new(50.0, 10.0))));
    let _ = app.update(Message::Drag(DragMsg::Move(Point::new(
        TAB_STRIDE * 3.0,
        10.0,
    ))));
    let _ = app.update(Message::Drag(DragMsg::Release));
    let order: Vec<_> = app.tabs.iter().map(|t| t.id).collect();
    assert_eq!(order[2], first);
    assert_eq!(order[1], third);
    assert_eq!(app.active, first);
}

#[test]
fn maximize_shows_one_pane_until_toggled_or_the_layout_changes() {
    let mut app = app();
    let a = app.active_pane().unwrap().id;
    let _ = app.update(Message::Action(Action::MaximizePane));
    assert!(app.tabs[0].solo.is_none(), "one pane: nothing to maximize");
    app.split_now(a, fogu_core::Side::Right, None, None);
    let b = app.active_pane().unwrap().id;
    let _ = app.update(Message::Action(Action::MaximizePane));
    assert_eq!(app.tabs[0].solo.as_ref().unwrap().leaves(), vec![&b]);
    let _ = app.update(Message::Action(Action::MaximizePane));
    assert!(app.tabs[0].solo.is_none());
    let _ = app.update(Message::Action(Action::MaximizePane));
    app.focus_pane(a);
    assert!(app.tabs[0].solo.is_none(), "focus moved to a hidden pane");
}

#[test]
fn resize_hotkeys_move_the_divider_by_the_configured_step() {
    let mut app = app();
    let a = app.active_pane().unwrap().id;
    app.split_now(a, fogu_core::Side::Right, None, None);
    app.focus_pane(a);
    let _ = app.update(Message::Action(Action::ResizePane(fogu_core::Dir::Right)));
    let rects = app.tabs[0].layout.layout(
        fogu_core::Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        },
        0.0,
    );
    let left = rects.iter().find(|(id, _)| *id == a).unwrap().1;
    assert!((left.w - 60.0).abs() < 0.5, "{left:?}");
}

/// Full screen entered outside the action (macOS's green button) is reported as a window mode:
/// the bar drops the traffic-light room, and the action then leaves full screen.
#[test]
fn reported_window_mode_drives_the_fullscreen_state() {
    let mut app = app();
    let _ = app.update(Message::WindowMode(iced::window::Mode::Fullscreen));
    assert!(app.fullscreen);
    assert!(app.bar_inset().abs() < f32::EPSILON);
    let _ = app.update(Message::Action(Action::ToggleFullscreen));
    assert!(!app.fullscreen, "the action leaves full screen");
    // The report after the exit animation settles brings the traffic-light room back.
    let _ = app.update(Message::WindowMode(iced::window::Mode::Fullscreen));
    let _ = app.update(Message::WindowMode(iced::window::Mode::Windowed));
    assert!(!app.fullscreen);
    let room = if cfg!(target_os = "macos") {
        crate::tab_bar::TRAFFIC_LIGHTS_W
    } else {
        0.0
    };
    assert!((app.bar_inset() - room).abs() < f32::EPSILON);
}
