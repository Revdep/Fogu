use fogu_core::Action;
use fogu_core::config::{CursorShapeCfg, load};

use super::{FieldId, Page, SettingsMsg};
use crate::app::App;
use crate::forms::FieldValue;
use crate::message::Message;
use crate::metrics::CellMetrics;
use crate::notifications::Level;
use crate::profiles::tests::{Tmp, app_in, tmp};

fn field(app: &mut App, id: FieldId, value: FieldValue) {
    let _ = app.update(Message::Settings(SettingsMsg::Field(id, value)));
}

fn text(app: &mut App, id: FieldId, s: &str) {
    field(app, id, FieldValue::Text(s.to_owned()));
}

fn file(t: &Tmp) -> String {
    std::fs::read_to_string(t.0.join("config.toml")).unwrap()
}

fn opened(t: &Tmp) -> App {
    let mut app = app_in(&t.0);
    let _ = app.update(Message::Action(Action::OpenSettings));
    app
}

#[test]
fn open_settings_opens_one_tab_and_reuses_it() {
    let t = tmp();
    let mut app = opened(&t);
    assert_eq!(app.active_tab().unwrap().title(), "Settings");
    let settings = app.active;
    let _ = app.update(Message::Action(Action::NewTab));
    let _ = app.update(Message::Action(Action::OpenSettings));
    assert_eq!(app.active, settings);
    assert_eq!(app.tabs.iter().filter(|t| t.settings.is_some()).count(), 1);
}

#[test]
fn font_family_and_size_are_written_applied_and_loaded_again() {
    let t = tmp();
    let mut app = opened(&t);
    let before = app.metrics;
    let settings = app
        .tabs
        .iter_mut()
        .find_map(|tab| tab.settings.as_deref_mut())
        .unwrap();
    settings.fonts = vec!["monospace".to_owned(), "Fogu Test Font".to_owned()];
    field(&mut app, FieldId::FontFamily, FieldValue::Choice(1));
    text(&mut app, FieldId::FontSize, "13");
    text(&mut app, FieldId::WheelLines, "7");
    field(&mut app, FieldId::CursorShape, FieldValue::Choice(1));
    let written = file(&t);
    assert!(written.starts_with("# hand-written"), "{written}");
    assert!(
        written.contains("font_family = \"Fogu Test Font\""),
        "{written}"
    );
    assert!(written.contains("font_size = 13.0"), "{written}");
    assert!(written.contains("wheel_lines = 7"), "{written}");
    assert!(written.contains("cursor_shape = \"beam\""), "{written}");
    assert_eq!(app.font.family, "Fogu Test Font");
    assert!((app.font.size_pt - 13.0).abs() < f32::EPSILON);
    assert!((app.default_font_size_pt - 13.0).abs() < f32::EPSILON);
    assert_ne!(app.metrics, before);
    assert_eq!(
        app.metrics,
        CellMetrics::new(app.resolved.units, app.font.size_px(), app.scale)
    );
    assert_eq!(app.config.appearance.cursor_shape, CursorShapeCfg::Beam);
    let _ = app.update(Message::Action(Action::ZoomIn));
    let _ = app.update(Message::Action(Action::ZoomReset));
    assert!((app.font.size_pt - 13.0).abs() < f32::EPSILON);
    let (back, _) = load(&t.0);
    assert_eq!(back.appearance.font_family, "Fogu Test Font");
    assert!((back.appearance.font_size - 13.0).abs() < f32::EPSILON);
    assert_eq!(back.appearance, app.config.appearance);
    assert_eq!(back.terminal, app.config.terminal);
}

#[test]
fn number_steppers_use_field_units_clamp_and_apply_font_size_live() {
    let t = tmp();
    let mut app = opened(&t);
    field(&mut app, FieldId::FontSize, FieldValue::Step(1));
    assert!((app.font.size_pt - 13.0).abs() < f32::EPSILON);
    assert!(file(&t).contains("font_size = 13.0"));

    text(&mut app, FieldId::FontSize, "40");
    field(&mut app, FieldId::FontSize, FieldValue::Step(1));
    assert!((app.font.size_pt - 40.0).abs() < f32::EPSILON);

    field(&mut app, FieldId::PaneResizeStep, FieldValue::Step(1));
    assert!((app.config.terminal.pane_resize_step - 0.11).abs() < f32::EPSILON);
}

#[test]
fn a_bad_number_is_shown_and_not_written() {
    let t = tmp();
    let mut app = opened(&t);
    let before = file(&t);
    text(&mut app, FieldId::FontSize, "99");
    assert_eq!(file(&t), before);
    let settings = app.tabs.iter().find_map(|t| t.settings.as_deref()).unwrap();
    assert!(
        settings
            .errors
            .get(&FieldId::FontSize)
            .unwrap()
            .contains("6 to 40")
    );
    assert_eq!(settings.shown(FieldId::FontSize, 0), "99");
    text(&mut app, FieldId::FontSize, "13");
    let settings = app.tabs.iter().find_map(|t| t.settings.as_deref()).unwrap();
    assert!(settings.errors.is_empty());
}

#[test]
fn picking_a_scheme_recolours_the_app() {
    let t = tmp();
    let mut app = opened(&t);
    let index = fogu_core::ColorScheme::builtins()
        .iter()
        .position(|s| s.name == "Dracula")
        .unwrap();
    field(&mut app, FieldId::Scheme, FieldValue::Choice(index));
    assert_eq!(app.config.appearance.color_scheme, "Dracula");
    let dracula = fogu_core::ColorScheme::by_name("Dracula").unwrap();
    assert_eq!(app.colors.background().r, dracula.bg.r);
    assert!(file(&t).contains("color_scheme = \"Dracula\""));
}

#[test]
fn opacity_and_optional_keys() {
    let t = tmp();
    let mut app = opened(&t);
    field(&mut app, FieldId::WindowOpacity, FieldValue::Number(0.05));
    assert!((app.config.appearance.window_opacity - super::MIN_OPACITY).abs() < f32::EPSILON);
    // Not created transparent: the terminals stay opaque until a restart.
    assert!((app.window_alpha() - 1.0).abs() < f32::EPSILON);
    field(&mut app, FieldId::TerminalBgAuto, FieldValue::Bool(false));
    assert!(file(&t).contains("terminal_bg_alpha = 0.75"));
    field(&mut app, FieldId::TerminalBgAuto, FieldValue::Bool(true));
    assert!(!file(&t).contains("terminal_bg_alpha"));
    text(&mut app, FieldId::MaxDownload, "1024");
    assert_eq!(app.config.sftp.max_download_bytes, Some(1024));
    text(&mut app, FieldId::MaxDownload, " ");
    assert_eq!(app.config.sftp.max_download_bytes, None);
    assert!(!file(&t).contains("max_download_bytes"));
}

#[test]
fn nav_switches_pages() {
    let t = tmp();
    let mut app = opened(&t);
    let _ = app.update(Message::Settings(SettingsMsg::Page(Page::Sftp)));
    let settings = app.tabs.iter().find_map(|t| t.settings.as_deref()).unwrap();
    assert_eq!(settings.page, Page::Sftp);
}

#[test]
fn ssh_recovery_toggle_asks_before_it_changes_the_setting() {
    let t = tmp();
    let mut app = opened(&t);
    field(
        &mut app,
        FieldId::RecoverySshScrollback,
        FieldValue::Bool(true),
    );
    assert!(!app.config.recovery.restore_ssh_scrollback);
    let modal = app.ssh_recovery_modal().unwrap();
    assert_eq!(modal.title, "Keep SSH history after restart?");
    assert_eq!(modal.buttons[0].label, "Turn on");
    assert_eq!(modal.buttons[1].label, "Cancel");
    let _ = app.update(Message::Settings(SettingsMsg::ConfirmSshRecovery(false)));
    assert!(app.ssh_recovery_modal().is_none());
    assert!(!file(&t).contains("restore_ssh_scrollback"));
}

#[test]
fn a_keyring_failure_leaves_ssh_recovery_off_and_shows_one_warning() {
    let t = tmp();
    let mut app = opened(&t);
    let _ = app.update(Message::Settings(SettingsMsg::SshRecoveryKeyLoaded {
        enable: true,
        result: Err("locked".to_owned()),
    }));
    assert!(!app.config.recovery.restore_ssh_scrollback);
    let notes: Vec<_> = app
        .toasts
        .iter()
        .filter(|note| note.title == "SSH history not turned on")
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].detail, "The system keyring is locked or missing.");
}

#[cfg(feature = "ssh")]
#[test]
fn turning_ssh_recovery_off_removes_encrypted_files() {
    let t = tmp();
    let mut app = opened(&t);
    let state = t.0.join("state");
    app.state_dir = Some(state.clone());
    fogu_core::recovery::save_encrypted_scrollback(&state, "pane", b"sealed").unwrap();
    let key = fogu_session::new_recovery_key().unwrap();
    let _ = app.update(Message::Settings(SettingsMsg::SshRecoveryKeyLoaded {
        enable: true,
        result: Ok(key),
    }));
    assert!(app.config.recovery.restore_ssh_scrollback);
    field(
        &mut app,
        FieldId::RecoverySshScrollback,
        FieldValue::Bool(false),
    );
    assert!(!app.config.recovery.restore_ssh_scrollback);
    assert!(!fogu_core::recovery::encrypted_scrollback_path(&state, "pane").exists());
}

#[test]
fn changed_config_file_is_applied_to_the_running_app() {
    let t = tmp();
    let mut app = opened(&t);
    app.remember_config_file();
    let edited = format!(
        "{}\n[appearance]\nfont_size = 17\ncolor_scheme = \"Dracula\"\ncursor_shape = \"beam\"\nprofiles_sidebar_width = 333\n",
        file(&t)
    );
    std::fs::write(t.0.join("config.toml"), edited).unwrap();

    let _ = app.reload_config_if_changed();

    assert!((app.font.size_pt - 17.0).abs() < f32::EPSILON);
    assert!((app.default_font_size_pt - 17.0).abs() < f32::EPSILON);
    assert_eq!(app.config.appearance.color_scheme, "Dracula");
    assert_eq!(app.config.appearance.cursor_shape, CursorShapeCfg::Beam);
    assert!((app.profiles.width - 333.0).abs() < f32::EPSILON);
}

#[test]
fn broken_config_file_is_kept_and_reported_once_per_change() {
    let t = tmp();
    let mut app = opened(&t);
    app.remember_config_file();
    let before = app.config.clone();
    std::fs::write(t.0.join("config.toml"), "broken [[[").unwrap();

    let _ = app.reload_config_if_changed();
    let _ = app.reload_config_if_changed();

    assert_eq!(app.config, before);
    let errors = app
        .toasts
        .iter()
        .filter(|note| note.level == Level::Error && note.title == "Config change not applied")
        .count();
    assert_eq!(errors, 1);
}

#[test]
fn reload_updates_open_tabs_and_drops_a_delete_for_a_gone_profile() {
    let t = tmp();
    let mut app = opened(&t);
    app.remember_config_file();
    let text = format!(
        "{}\n[[profiles]]\nid = \"box\"\nname = \"Box\"\ntype = \"local\"\nprogram = \"sh\"\n",
        file(&t)
    );
    std::fs::write(t.0.join("config.toml"), &text).unwrap();
    let _ = app.reload_config_if_changed();
    app.profiles.pending_delete = Some(fogu_core::ProfileId("box".to_owned()));
    if let Some(tab) = app.tabs.first_mut() {
        tab.profile = app.config.profiles.first().unwrap().clone();
    }
    let renamed = text.replace("name = \"Box\"", "name = \"Crate\"");
    std::fs::write(t.0.join("config.toml"), &renamed).unwrap();
    let _ = app.reload_config_if_changed();
    assert_eq!(
        app.tabs.first().unwrap().profile.name,
        "Crate",
        "open tabs use the reloaded profile"
    );
    assert!(app.profiles.pending_delete.is_some());

    std::fs::write(
        t.0.join("config.toml"),
        file(&t).replace("id = \"box\"", "id = \"other\""),
    )
    .unwrap();
    let _ = app.reload_config_if_changed();
    assert!(app.profiles.pending_delete.is_none(), "its profile is gone");
}

#[test]
fn a_message_that_writes_the_config_first_takes_in_an_external_edit() {
    let t = tmp();
    let mut app = opened(&t);
    app.remember_config_file();
    let text = format!(
        "{}\n[[profiles]]\nid = \"late\"\nname = \"Late\"\ntype = \"local\"\n",
        file(&t)
    );
    std::fs::write(t.0.join("config.toml"), text).unwrap();
    let _ = app.update(Message::Settings(SettingsMsg::Page(Page::Terminal)));
    assert!(app.config.profiles.iter().any(|p| p.id.0 == "late"));
}

#[test]
fn wayland_drag_and_drop_is_off_by_default_and_written_when_turned_on() {
    let t = tmp();
    let mut app = opened(&t);
    assert!(!app.config.sftp.wayland_drag_and_drop);
    field(&mut app, FieldId::WaylandDragDrop, FieldValue::Bool(true));
    assert!(
        file(&t).contains("wayland_drag_and_drop = true"),
        "{}",
        file(&t)
    );
    let (back, _) = load(&t.0);
    assert!(back.sftp.wayland_drag_and_drop);
}
