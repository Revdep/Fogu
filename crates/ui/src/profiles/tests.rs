use std::path::{Path, PathBuf};
use std::sync::Arc;

use fogu_core::config::load;
use fogu_core::recent::Recent;
use fogu_core::{Action, EffectiveProfile, Group, resolve};
use fogu_session::SessionRuntime;

use super::*;
use crate::app::App;
use crate::message::Message;
use crate::metrics::{FontUnits, ResolvedFont};

const CONFIG: &str = "\
# hand-written
[[groups]]
id = \"servers\"
name = \"Servers\"

[[profiles]]   # the box
id = \"box\"
name = \"Web box\"
type = \"ssh\"
host = \"web.lan\"
user = \"alice\"
group = \"servers\"

[[profiles]]
id = \"pi\"
name = \"Pi\"
type = \"ssh\"
host = \"10.0.0.7\"
port = 2222
user = \"pi\"
icon = \"🍓\"

[[profiles]]
id = \"fish\"
name = \"Fish\"
type = \"local\"
program = \"/usr/bin/fish\"
";

fn ssh(id: &str, host: &str, port: Option<u16>) -> Profile {
    Profile {
        id: ProfileId(id.to_owned()),
        name: id.to_owned(),
        kind: ProfileKind::Ssh(SshOpts {
            host: Some(host.to_owned()),
            port,
            user: Some("alice".to_owned()),
            ..SshOpts::default()
        }),
        ..Profile::default_shell()
    }
}

pub(crate) struct Tmp(pub(crate) PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn tmp() -> Tmp {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("fogu-profiles-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Tmp(dir)
}

/// A headless app whose config is `dir/config.toml` (written from [`CONFIG`]) and whose Recent
/// list lives in `dir/state`.
pub(crate) fn app_in(dir: &Path) -> App {
    std::fs::write(dir.join("config.toml"), CONFIG).unwrap();
    let (config, err) = load(dir);
    assert!(err.is_none());
    let runtime = Arc::new(SessionRuntime::new().unwrap());
    let resolved = ResolvedFont {
        font: iced::Font::MONOSPACE,
        units: FontUnits::FALLBACK,
    };
    let mut app = App::new(runtime, Arc::new(config), resolved);
    app.config_dir = Some(dir.to_path_buf());
    app.load_recent(dir.join("state").join(fogu_core::recent::RECENT_FILE));
    let profile = app.default_profile();
    app.open_tab(&profile, None);
    app
}

pub(super) fn send(app: &mut App, message: ProfilesMsg) {
    let _task = app.update(Message::Profiles(message));
}

pub(crate) fn id(s: &str) -> ProfileId {
    ProfileId(s.to_owned())
}

/// The sidebar as text: `"> Label (n)"` headers and item titles.
pub(super) fn tree(app: &App) -> Vec<String> {
    let all = app.all_entries();
    let rows = rows(&all, &app.config, &app.profiles.recent, now_secs());
    lines(&app.profiles, &app.config, &all, &rows)
        .into_iter()
        .map(|l| match l {
            Line::Header {
                label,
                count,
                collapsed,
                ..
            } => format!("{} {label} ({count})", if collapsed { ">" } else { "v" }),
            Line::Item(i) => rows[i].title.clone(),
        })
        .collect()
}

#[test]
fn entries_list_builtins_then_stored_then_quick_with_subtitles() {
    let mut config = Config {
        profiles: vec![
            ssh("Work", "work.lan", None),
            ssh("Pi", "10.0.0.7", Some(2222)),
        ],
        ..Config::default()
    };
    let mut recent = Recent::default();
    recent.touch(&id("quick:bob@db:2200"), 1);
    recent.touch(&id("quick:not a host:x"), 2);
    let builtins = [Profile::default_shell()];
    let all = entries(&config, &builtins, &[], Some(&recent));
    let shown: Vec<(&str, &str, Origin)> = all
        .iter()
        .map(|e| (e.profile.name.as_str(), e.subtitle.as_str(), e.origin))
        .collect();
    assert_eq!(
        shown,
        [
            ("Default shell", "local shell", Origin::Builtin),
            ("Work", "alice@work.lan", Origin::Stored),
            ("Pi", "alice@10.0.0.7:2222", Origin::Stored),
            ("bob@db:2200", "quick connect", Origin::Quick),
        ]
    );
    config.appearance.show_builtin_profiles = false;
    assert_eq!(entries(&config, &builtins, &[], None).len(), 2);
}

#[test]
fn quick_connect_strings_become_ephemeral_ssh_profiles() {
    let p = quick_profile(" me@host.lan:2222 ").unwrap();
    assert_eq!(p.id.0, "quick:me@host.lan:2222");
    assert_eq!(p.name, "me@host.lan:2222");
    let ProfileKind::Ssh(opts) = &p.kind else {
        panic!("ssh")
    };
    assert_eq!(
        (opts.user.as_deref(), opts.host.as_deref(), opts.port),
        (Some("me"), Some("host.lan"), Some(2222))
    );
    assert_eq!(quick_profile("host").unwrap().id.0, "quick:host");
    assert!(quick_profile("host:99999").is_err());
    assert!(quick_profile("").is_err());
}

#[test]
fn ssh_command_only_for_plain_user_and_host() {
    let config = Config::default();
    let with = |user: &str, host: &str| {
        let mut p = ssh("x", host, Some(2222));
        if let ProfileKind::Ssh(o) = &mut p.kind {
            o.user = Some(user.to_owned());
        }
        ssh_command(&p, &config)
    };
    assert_eq!(
        with("alice", "web-1.lan").as_deref(),
        Some("ssh -p 2222 alice@web-1.lan")
    );
    for (user, host) in [
        ("alice", "-oProxyCommand=x"),
        ("-l", "h"),
        ("a;rm", "h"),
        ("alice", "h$(id)"),
        ("al ice", "h"),
        ("alice", "h\n"),
        ("alice", ""),
        ("ålice", "h"),
    ] {
        assert_eq!(with(user, host), None, "{user:?}@{host:?}");
    }
    assert_eq!(ssh_command(&Profile::default_shell(), &config), None);
}

#[test]
fn relative_times_and_glyphs() {
    for (ago, want) in [
        (0, "just now"),
        (59, "just now"),
        (60, "1 min ago"),
        (3599, "59 min ago"),
        (7200, "2 h ago"),
        (86_400 * 3, "3 d ago"),
    ] {
        assert_eq!(relative_time(1_000_000, 1_000_000 - ago), want);
    }
    assert_eq!(relative_time(5, 10), "just now", "clock went back");
    assert_eq!(glyph(Some("🍓")).as_deref(), Some("🍓"));
    assert_eq!(glyph(Some(" ★ ")).as_deref(), Some("★"));
    assert_eq!(glyph(Some("fas fa-server")), None);
    assert_eq!(glyph(Some("server")), None);
    assert_eq!(glyph(Some("\u{1b}[31m")), None);
    assert_eq!(glyph(None), None);
}

#[test]
fn tree_has_groups_with_counts_ungrouped_and_builtins_and_collapses() {
    let t = tmp();
    let mut app = app_in(&t.0);
    assert_eq!(
        tree(&app),
        [
            "v Servers (1)",
            "Web box",
            "v Ungrouped (2)",
            "Pi",
            "Fish",
            "v Built-ins (1)",
            "Default shell"
        ]
    );
    send(&mut app, ProfilesMsg::Toggle(Section::Ungrouped));
    assert_eq!(
        tree(&app)[2..],
        ["> Ungrouped (2)", "v Built-ins (1)", "Default shell"]
    );
    // Search: ranked flat list, no headers.
    send(&mut app, ProfilesMsg::Query("10.0".into()));
    assert_eq!(tree(&app), ["Pi"]);
    send(&mut app, ProfilesMsg::Query("srv".into()));
    assert_eq!(tree(&app), ["Web box"], "group names are searched");
}

#[test]
fn enter_connects_the_top_hit_and_records_recent_0600() {
    let t = tmp();
    let mut app = app_in(&t.0);
    let _ = app.update(Message::Action(Action::ToggleProfiles));
    assert!(app.profiles.open && app.profiles.focused);
    send(&mut app, ProfilesMsg::Query("web".into()));
    send(&mut app, ProfilesMsg::Submit);
    assert_eq!(app.tabs.len(), 2);
    let EffectiveProfile::Ssh(ssh) = app.stub_profiles.last().unwrap() else {
        panic!("ssh")
    };
    assert_eq!(ssh.host, "web.lan");
    // Recent is first in the tree and saved privately.
    send(&mut app, ProfilesMsg::Query(String::new()));
    assert_eq!(tree(&app)[..2], ["v Recent (1)", "Web box"]);
    let file = t.0.join("state").join(fogu_core::recent::RECENT_FILE);
    let saved = Recent::load(&file).unwrap();
    assert_eq!(saved.recent[0].id, id("box"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // ↓ + Enter walks the visible items (Recent's "Web box", then the group's).
    send(&mut app, ProfilesMsg::Move(1));
    send(&mut app, ProfilesMsg::Move(1));
    send(&mut app, ProfilesMsg::Submit);
    assert_eq!(app.tabs.len(), 3);
    assert_eq!(
        app.title(),
        "Pi",
        "profiles keep their name as tab title by default"
    );
}

#[test]
fn double_click_opens_a_tab_middle_click_a_split_and_sftp_a_panel() {
    let t = tmp();
    let mut app = app_in(&t.0);
    send(&mut app, ProfilesMsg::Open(id("fish"), Launch::Tab));
    assert_eq!(app.tabs.len(), 2);
    send(&mut app, ProfilesMsg::Open(id("pi"), Launch::Split));
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active_tab().unwrap().panes.len(), 2);
    assert!(app.active_pane().unwrap().is_ssh());
    send(&mut app, ProfilesMsg::Open(id("box"), Launch::Sftp));
    assert_eq!(app.tabs.len(), 3);
    assert!(
        app.active_tab()
            .unwrap()
            .sftp
            .as_ref()
            .is_some_and(|p| p.visible)
    );
}

#[test]
fn quick_connect_opens_an_ssh_tab_or_says_why_not() {
    let t = tmp();
    let mut app = app_in(&t.0);
    send(&mut app, ProfilesMsg::Quick("me@h:99999".into()));
    send(&mut app, ProfilesMsg::QuickSubmit);
    assert_eq!(app.tabs.len(), 1);
    assert!(app.profiles.quick_error.is_some());
    send(&mut app, ProfilesMsg::Quick("me@h.lan:2222".into()));
    assert!(app.profiles.quick_error.is_none());
    send(&mut app, ProfilesMsg::QuickSubmit);
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.profiles.quick, "");
    let EffectiveProfile::Ssh(ssh) = app.stub_profiles.last().unwrap() else {
        panic!("ssh")
    };
    assert_eq!(
        (ssh.user.as_str(), ssh.host.as_str(), ssh.port),
        ("me", "h.lan", 2222)
    );
    assert_eq!(tree(&app)[..2], ["v Recent (1)", "me@h.lan:2222"]);
}

#[test]
fn duplicate_move_to_new_group_and_delete_are_saved_in_place() {
    let t = tmp();
    let mut app = app_in(&t.0);
    // Duplicate through the row menu.
    send(&mut app, ProfilesMsg::Menu(id("pi"), 0));
    assert!(app.menu.is_some());
    send(&mut app, ProfilesMsg::Duplicate(id("pi")));
    assert!(app.menu.is_none());
    let (on_disk, _) = load(&t.0);
    assert_eq!(on_disk.profiles[3].name, "Pi copy");
    assert_eq!(on_disk.profiles[3].id, id("pi-copy"));
    assert_eq!(on_disk, *app.config, "running config follows the file");
    // Move to group ▸ New group "Work".
    send(&mut app, ProfilesMsg::Menu(id("pi-copy"), 0));
    send(&mut app, ProfilesMsg::MoveMenu);
    let _ = app.update(Message::MenuInput("Work".into()));
    let _ = app.update(Message::MenuSubmit);
    assert!(
        tree(&app).contains(&"v Work (1)".to_owned()),
        "{:?}",
        tree(&app)
    );
    let (on_disk, _) = load(&t.0);
    assert_eq!(
        on_disk.groups[1],
        Group {
            id: fogu_core::GroupId("work".into()),
            name: "Work".into()
        }
    );
    // Delete asks first; Cancel keeps it, Delete removes it.
    send(&mut app, ProfilesMsg::AskDelete(id("pi-copy")));
    assert!(app.delete_modal().is_some());
    send(&mut app, ProfilesMsg::ConfirmDelete(false));
    assert_eq!(app.config.profiles.len(), 4);
    send(&mut app, ProfilesMsg::AskDelete(id("pi-copy")));
    send(&mut app, ProfilesMsg::ConfirmDelete(true));
    assert_eq!(app.config.profiles.len(), 3);
    assert_eq!(load(&t.0).0, *app.config);
    let text = std::fs::read_to_string(t.0.join("config.toml")).unwrap();
    assert!(text.starts_with("# hand-written\n"), "{text}");
    assert!(text.contains("[[profiles]]   # the box"), "{text}");
}

#[test]
fn builtins_duplicate_into_a_stored_profile_and_cannot_be_deleted() {
    let t = tmp();
    let mut app = app_in(&t.0);
    let builtin = Profile::default_shell().id;
    send(&mut app, ProfilesMsg::AskDelete(builtin.clone()));
    assert!(app.profiles.pending_delete.is_none());
    send(&mut app, ProfilesMsg::Duplicate(builtin));
    let last = app.config.profiles.last().unwrap();
    assert_eq!(
        (last.id.0.as_str(), last.name.as_str()),
        ("default-shell-copy", "Default shell copy")
    );
    assert_eq!(load(&t.0).0, *app.config);
}

#[test]
fn a_broken_config_is_not_touched_and_reported() {
    let t = tmp();
    let mut app = app_in(&t.0);
    std::fs::write(t.0.join("config.toml"), "broken [[[").unwrap();
    send(&mut app, ProfilesMsg::Duplicate(id("pi")));
    assert_eq!(app.config.profiles.len(), 3);
    assert_eq!(
        std::fs::read_to_string(t.0.join("config.toml")).unwrap(),
        "broken [[["
    );
    assert!(!app.toasts.is_empty());
}

#[test]
fn keyboard_focus_survives_the_opening_click_and_returns_on_a_pane_click() {
    let t = tmp();
    let mut app = app_in(&t.0);
    // The ☰ click opens the sidebar; its MousePressed may arrive afterwards.
    let _ = app.update(Message::Action(Action::ToggleProfiles));
    let _ = app.update(Message::MousePressed);
    assert!(app.sidebar_has_keys());
    let pane = app.active_pane().unwrap().id;
    let _ = app.update(Message::FocusPane(pane));
    assert!(!app.sidebar_has_keys());
    // A press over the sidebar takes the keyboard again.
    send(&mut app, ProfilesMsg::Hover(true));
    let _ = app.update(Message::MousePressed);
    assert!(app.sidebar_has_keys());
    // Launching hands it to the new terminal, also when the double-click's press is reported
    // afterwards.
    send(&mut app, ProfilesMsg::RowOpen(id("fish"), Launch::Tab));
    let _ = app.update(Message::MousePressed);
    assert!(!app.sidebar_has_keys());
    // The next press over the sidebar counts again.
    let _ = app.update(Message::MousePressed);
    assert!(app.sidebar_has_keys());
}

#[test]
fn jump_hosts_resolve_through_stored_profiles_and_bad_ones_are_kept_as_reasons() {
    let t = tmp();
    let mut app = app_in(&t.0);
    let mut config = (*app.config).clone();
    let with_jump = |name: &str, jump: &str| {
        let mut p = ssh(name, &format!("{name}.lan"), None);
        if let ProfileKind::Ssh(opts) = &mut p.kind {
            opts.jump_host = Some(id(jump));
        }
        p
    };
    config.profiles.extend([
        with_jump("inner", "pi"),
        with_jump("to-local", "fish"),
        with_jump("to-nowhere", "gone"),
        with_jump("loop-a", "loop-b"),
        with_jump("loop-b", "loop-a"),
    ]);
    app.config = Arc::new(config);
    let resolved = |app: &App, name: &str| {
        let profile = app.config.profiles.iter().find(|p| p.id.0 == name).unwrap();
        let EffectiveProfile::Ssh(mut ssh) = resolve(profile, &app.config.profile_defaults) else {
            panic!("ssh")
        };
        app.resolve_jumps(&profile.id, &mut ssh);
        ssh
    };
    let inner = resolved(&app, "inner");
    let fogu_core::Jump::Via(hop) = &inner.jump else {
        panic!("{:?}", inner.jump)
    };
    assert_eq!((hop.host.as_str(), hop.port), ("10.0.0.7", 2222));
    for (name, want) in [
        ("to-local", "jump_host fish: Fish is not an SSH profile"),
        ("to-nowhere", "jump_host gone: no profile with id gone"),
        (
            "loop-a",
            "jump_host loop-b: jump host loop: loop-a → loop-b → loop-a",
        ),
    ] {
        assert_eq!(resolved(&app, name).route_problems(), [want], "{name}");
    }
    // Opening a tab resolves the chain before the session starts.
    send(&mut app, ProfilesMsg::Open(id("inner"), Launch::Tab));
    let EffectiveProfile::Ssh(opened) = app.stub_profiles.last().unwrap() else {
        panic!("ssh")
    };
    assert!(matches!(opened.jump, fogu_core::Jump::Via(_)));
}

#[test]
fn stored_profiles_follow_their_weight() {
    let mut light = ssh("Light", "a.lan", None);
    if let ProfileKind::Ssh(o) = &mut light.kind {
        o.weight = Some(-1);
    }
    let mut heavy = ssh("Heavy", "b.lan", None);
    if let ProfileKind::Ssh(o) = &mut heavy.kind {
        o.weight = Some(5);
    }
    let config = Config {
        profiles: vec![heavy, ssh("Plain", "c.lan", None), light],
        ..Config::default()
    };
    let names: Vec<String> = entries(&config, &[], &[], None)
        .iter()
        .map(|e| e.profile.name.clone())
        .collect();
    assert_eq!(names, ["Light", "Plain", "Heavy"]);
}

#[test]
fn an_ssh_config_host_copied_to_the_profiles_is_listed_once_as_the_copy() {
    let host = Profile {
        id: fogu_core::ProfileId("ssh-config:box".into()),
        name: "box".into(),
        ..Profile::default_shell()
    };
    let other = Profile {
        id: fogu_core::ProfileId("ssh-config:other".into()),
        name: "other".into(),
        ..Profile::default_shell()
    };
    let mut config = Config::default();
    config.profiles.push(Profile {
        id: fogu_core::ProfileId("box".into()),
        name: "Box".into(),
        ..Profile::default_shell()
    });
    let listed = super::entries(&config, &[], &[host, other], None);
    let names: Vec<_> = listed
        .iter()
        .map(|e| (e.profile.name.as_str(), e.origin))
        .collect();
    assert_eq!(
        names,
        [
            ("Box", super::Origin::Stored),
            ("other", super::Origin::SshConfig)
        ]
    );
}

#[test]
fn use_for_new_tabs_sets_and_clears_the_default_profile() {
    let t = tmp();
    let mut app = app_in(&t.0);
    let shell = Profile::default_shell().id;
    assert!(is_default(&app.config, &shell));
    send(&mut app, ProfilesMsg::SetDefault(id("pi")));
    assert_eq!(app.config.default_profile, Some(id("pi")));
    assert_eq!(app.default_profile().id, id("pi"));
    assert!(is_default(&app.config, &id("pi")) && !is_default(&app.config, &shell));
    assert_eq!(load(&t.0).0, *app.config);
    // Back to the built-in default shell: the key goes away.
    send(&mut app, ProfilesMsg::SetDefault(shell.clone()));
    assert_eq!(app.config.default_profile, None);
    assert_eq!(app.default_profile().id, shell);
    let text = std::fs::read_to_string(t.0.join("config.toml")).unwrap();
    assert!(!text.contains("default_profile") && text.starts_with("# hand-written\n"));
}
