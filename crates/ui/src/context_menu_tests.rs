use super::*;

#[test]
fn parses_custom_hex() {
    let red = Some(RgbColor { r: 255, g: 0, b: 0 });
    assert_eq!(parse_color("#ff0000"), red);
    assert_eq!(parse_color(" FF0000 "), red);
    assert_eq!(parse_color("red"), None);
    assert_eq!(parse_color("#ff00"), None);
    assert_eq!(parse_color(""), None);
}

#[test]
fn menus_flip_inward_near_the_edges() {
    let close = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(close(flip(100.0, 200.0, 1000.0), 100.0), "fits");
    assert!(
        close(flip(990.0, 200.0, 1000.0), 790.0),
        "opens to the left"
    );
    assert!(close(flip(50.0, 200.0, 120.0), 0.0), "too big: clamped");
    let menu = ContextMenu::tab(Point::new(990.0, 640.0), false, true);
    let at = menu.position(Size::new(1000.0, 650.0));
    assert!(close(at.x, 990.0 - MENU_W) && at.y < 640.0, "{at:?}");
}

#[test]
fn arrows_skip_disabled_items_and_enter_runs_the_selection() {
    let mut menu = ContextMenu::new(
        Point::ORIGIN,
        vec![
            MenuItem::new("a", MenuAction::Run(Action::Copy)),
            MenuItem::new("b", MenuAction::Run(Action::Paste)).disabled("no"),
            MenuItem::new("c", MenuAction::Run(Action::Clear)),
        ],
    );
    assert!(menu.chosen().is_none());
    menu.move_selection(true);
    assert_eq!(menu.selected, Some(0));
    menu.move_selection(true);
    assert_eq!(menu.selected, Some(2), "b is skipped");
    menu.move_selection(true);
    assert_eq!(menu.selected, Some(0), "wraps");
    menu.move_selection(false);
    assert!(matches!(
        menu.chosen(),
        Some(Message::Action(Action::Clear))
    ));
}

#[test]
fn hsv_round_trips() {
    for hex in [
        0x00_0000, 0xff_ffff, 0xff_0000, 0x00_ff00, 0x00_00ff, 0x80_8080, 0xe5_484d, 0x8e_4ec6,
    ] {
        let c = RgbColor::from_hex(hex);
        let (h, s, v) = to_hsv(c);
        assert_eq!(from_hsv(h, s, v), c, "{c}");
    }
    assert_eq!(to_hsv(RgbColor::from_hex(0x00_ff00)), (120.0, 100.0, 100.0));
    assert_eq!(from_hsv(359.9, 100.0, 100.0), RgbColor::from_hex(0xff_0000));
}
