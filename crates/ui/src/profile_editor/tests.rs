#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    reason = "tests"
)]

use std::path::PathBuf;

use fogu_core::config::{ConfigIssue, load};
use fogu_core::{
    Algorithms, Background, Backspace, Fit, ForwardKind, LocalOpts, LoginScript, Osc52Policy,
    PortForward, Profile, ProfileId, ProfileKind, SessionEndBehavior, SshAuth, SshOpts,
};

use super::fields::{AlgKind, ForwardCol, ScriptCol};
use super::*;
use crate::app::App;
use crate::forms::FieldValue as V;
use crate::message::Message;
use crate::profiles::ProfilesMsg;
use crate::profiles::tests::{app_in, id, tmp};
use crate::tab::TabId;

fn editor_tab(app: &App) -> TabId {
    app.tabs
        .iter()
        .rev()
        .find(|t| t.editor.is_some())
        .map(|t| t.id)
        .expect("an editor tab")
}

fn editor(app: &App) -> &ProfileEditor {
    let tab = editor_tab(app);
    app.tabs
        .iter()
        .find(|t| t.id == tab)
        .and_then(|t| t.editor.as_deref())
        .unwrap()
}

fn field(app: &mut App, id: FieldId, value: V) {
    let tab = editor_tab(app);
    let _ = app.update(Message::Editor(tab, EditorMsg::Field(id, value)));
}

fn text(app: &mut App, id: FieldId, value: &str) {
    field(app, id, V::Text(value.to_owned()));
}

fn editor_msg(app: &mut App, message: EditorMsg) {
    let tab = editor_tab(app);
    let _ = app.update(Message::Editor(tab, message));
}

fn profiles(app: &mut App, message: ProfilesMsg) {
    let _ = app.update(Message::Profiles(message));
}

fn config_text(t: &crate::profiles::tests::Tmp) -> String {
    std::fs::read_to_string(t.0.join("config.toml")).unwrap()
}

#[test]
fn number_stepper_steps_and_clamps_profile_fields() {
    let profile = Profile {
        kind: ProfileKind::Ssh(SshOpts {
            port: Some(u16::MAX),
            ..SshOpts::default()
        }),
        ..Profile::default_shell()
    };
    let mut editor = ProfileEditor::new(profile, Source::New);
    assert_eq!(
        editor.apply(FieldId::Port, V::Step(1)),
        fields::Effect::None
    );
    let ProfileKind::Ssh(ssh) = &editor.draft.kind else {
        panic!("ssh profile")
    };
    assert_eq!(ssh.port, Some(u16::MAX));

    let _ = editor.apply(FieldId::Port, V::Step(-1));
    let ProfileKind::Ssh(ssh) = &editor.draft.kind else {
        panic!("ssh profile")
    };
    assert_eq!(ssh.port, Some(u16::MAX - 1));
}

#[test]
fn stepping_an_inherited_number_starts_from_its_default() {
    let profile = Profile {
        kind: ProfileKind::Ssh(SshOpts::default()),
        ..Profile::default_shell()
    };
    let mut editor = ProfileEditor::new(profile, Source::New);
    let _ = editor.apply(FieldId::Port, V::Step(1));
    let _ = editor.apply(FieldId::Weight, V::Step(-1));
    let ProfileKind::Ssh(ssh) = &editor.draft.kind else {
        panic!("ssh profile")
    };
    assert_eq!((ssh.port, ssh.weight), (Some(23), Some(-1)));
}

/// Sets every SSH field through the reducer; the expected struct names every field (no `..`),
/// so a new `SshOpts` field fails to compile here until the editor handles it.
#[test]
fn every_ssh_field_is_set_through_the_reducer() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::New(true));
    for (id, value) in [
        (FieldId::Host, "example.com"),
        (FieldId::Port, "2200"),
        (FieldId::User, "alice"),
        (FieldId::Term, "xterm"),
        (FieldId::KeepaliveInterval, "15000"),
        (FieldId::KeepaliveCountMax, "3"),
        (FieldId::ReadyTimeout, "9000"),
        (FieldId::PromptTimeout, "30000"),
        (FieldId::ReuseLinger, "30"),
        (FieldId::Weight, "-5"),
        (FieldId::ProxyCommand, "nc %h %p"),
        (FieldId::SocksHost, "127.0.0.1"),
        (FieldId::SocksPort, "9050"),
        (FieldId::HttpHost, "proxy.lan"),
        (FieldId::HttpPort, "3128"),
        (FieldId::RemoteCwd, "/srv"),
        (FieldId::AgentPath, "/run/agent.sock"),
    ] {
        text(&mut app, id, value);
    }
    for id in [
        FieldId::SkipBanner,
        FieldId::AgentForward,
        FieldId::X11,
        FieldId::RememberCwd,
        FieldId::IdentitiesOnly,
    ] {
        field(&mut app, id, V::Bool(true));
    }
    for id in [
        FieldId::WarnOnClose,
        FieldId::ReuseSession,
        FieldId::ClearServiceMessages,
        FieldId::VerifyHostKeys,
        FieldId::SocksRemoteDns,
        FieldId::StoredSecrets,
    ] {
        field(&mut app, id, V::Bool(false));
    }
    field(&mut app, FieldId::Auth, V::Choice(4));
    field(&mut app, FieldId::Backspace, V::Choice(2));
    field(&mut app, FieldId::Osc52, V::Choice(3));
    // Jump host: the choices are "None" then the other SSH profiles (box, pi).
    assert_eq!(editor(&app).jumps[1].0, Some(id("box")));
    field(&mut app, FieldId::JumpHost, V::Choice(1));
    field(
        &mut app,
        FieldId::PrivateKeys,
        V::Picked(PathBuf::from("/k/a")),
    );
    field(&mut app, FieldId::PrivateKeys, V::Add);
    field(&mut app, FieldId::PrivateKeys, V::Item(1, "/k/b".into()));
    field(&mut app, FieldId::PrivateKeys, V::Up(1));
    field(&mut app, FieldId::Forwards, V::Add);
    field(
        &mut app,
        FieldId::Forward(0, ForwardCol::Kind),
        V::Choice(1),
    );
    text(&mut app, FieldId::Forward(0, ForwardCol::Port), "8080");
    text(&mut app, FieldId::Forward(0, ForwardCol::Target), "web");
    text(&mut app, FieldId::Forward(0, ForwardCol::TargetPort), "80");
    text(
        &mut app,
        FieldId::Forward(0, ForwardCol::Description),
        "site",
    );
    field(&mut app, FieldId::Scripts, V::Add);
    text(&mut app, FieldId::Script(0, ScriptCol::Expect), "Password:");
    text(&mut app, FieldId::Script(0, ScriptCol::Send), "x\\n");
    field(
        &mut app,
        FieldId::Script(0, ScriptCol::IsRegex),
        V::Bool(true),
    );
    field(
        &mut app,
        FieldId::Script(0, ScriptCol::Optional),
        V::Bool(true),
    );
    for (kind, name) in [
        (AlgKind::Kex, "curve25519-sha256"),
        (AlgKind::Cipher, "aes256-gcm@openssh.com"),
        (AlgKind::Hmac, "hmac-sha2-256"),
        (AlgKind::HostKey, "ssh-ed25519"),
        (AlgKind::Compression, "none"),
        (AlgKind::Compression, "zlib"),
    ] {
        field(&mut app, FieldId::Alg(kind), V::Toggle(name.into()));
    }
    field(
        &mut app,
        FieldId::Alg(AlgKind::Compression),
        V::Toggle("zlib".into()),
    );
    // The password buttons do nothing (stored passwords come later).
    field(&mut app, FieldId::Password, V::SetSecret);

    let ed = editor(&app);
    assert!(ed.errors.is_empty(), "{:?}", ed.errors);
    let ProfileKind::Ssh(ssh) = &ed.draft.kind else {
        panic!("ssh");
    };
    let expected = SshOpts {
        host: Some("example.com".into()),
        port: Some(2200),
        user: Some("alice".into()),
        auth: Some(SshAuth::KeyboardInteractive),
        private_keys: Some(vec!["/k/b".into(), "/k/a".into()]),
        keepalive_interval: Some(15_000),
        keepalive_count_max: Some(3),
        ready_timeout: Some(9000),
        skip_banner: Some(true),
        warn_on_close: Some(false),
        term: Some("xterm".into()),
        input_backspace: Some(Backspace::CtrlQuestion),
        jump_host: Some(id("box")),
        proxy_command: Some("nc %h %p".into()),
        socks_proxy_host: Some("127.0.0.1".into()),
        socks_proxy_port: Some(9050),
        socks_proxy_remote_dns: Some(false),
        http_proxy_host: Some("proxy.lan".into()),
        http_proxy_port: Some(3128),
        forwarded_ports: Some(vec![PortForward {
            kind: ForwardKind::Remote,
            host: "127.0.0.1".into(),
            port: 8080,
            target_address: "web".into(),
            target_port: 80,
            description: "site".into(),
        }]),
        agent_forward: Some(true),
        x11: Some(true),
        algorithms: Some(Algorithms {
            kex: vec!["curve25519-sha256".into()],
            cipher: vec!["aes256-gcm@openssh.com".into()],
            hmac: vec!["hmac-sha2-256".into()],
            server_host_key: vec!["ssh-ed25519".into()],
            compression: vec!["none".into()],
        }),
        reuse_session: Some(false),
        reuse_linger: Some(30),
        stored_secrets: Some(false),
        scripts: Some(vec![LoginScript {
            expect: "Password:".into(),
            send: "x\\n".into(),
            is_regex: true,
            optional: true,
        }]),
        cwd: Some("/srv".into()),
        remember_cwd: Some(true),
        clear_service_messages_on_connect: Some(false),
        verify_host_keys: Some(false),
        weight: Some(-5),
        osc52_write: Some(Osc52Policy::Deny),
        identities_only: Some(true),
        prompt_timeout: Some(30_000),
        agent_path: Some("/run/agent.sock".into()),
        agent_key_lifetime: None,
        host_key_alias: None,
        ignored_ssh_config: Vec::new(),
    };
    assert_eq!(*ssh, expected);
}

#[test]
fn every_common_and_local_field_is_set_through_the_reducer() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::New(false));
    text(&mut app, FieldId::Name, "Work");
    field(&mut app, FieldId::Group, V::Choice(1));
    text(&mut app, FieldId::Icon, "🐟");
    text(&mut app, FieldId::Color, "#30a46c");
    field(&mut app, FieldId::DisableDynamicTitle, V::Bool(true));
    field(&mut app, FieldId::SessionEnd, V::Choice(1));
    field(&mut app, FieldId::Osc52, V::Choice(2));
    field(&mut app, FieldId::BgKind, V::Choice(2));
    assert!(
        editor(&app).errors.contains_key(&FieldId::BgImage),
        "empty image path"
    );
    field(&mut app, FieldId::BgImage, V::Picked("/img/a.png".into()));
    field(&mut app, FieldId::BgFit, V::Choice(2));
    text(&mut app, FieldId::BgOpacity, "40");
    text(&mut app, FieldId::Program, "/usr/bin/fish");
    field(&mut app, FieldId::Args, V::Add);
    field(&mut app, FieldId::Args, V::Item(0, "-l".into()));
    text(&mut app, FieldId::LocalCwd, "/work");
    field(&mut app, FieldId::Env, V::Add);
    field(&mut app, FieldId::Env, V::Item(0, "EDITOR".into()));
    field(&mut app, FieldId::Env, V::Value(0, "nvim".into()));
    field(&mut app, FieldId::Env, V::Add); // an empty row is not a variable
    field(&mut app, FieldId::Login, V::Bool(true));

    let ed = editor(&app);
    assert!(ed.errors.is_empty(), "{:?}", ed.errors);
    let p = &ed.draft;
    assert_eq!(
        (
            p.name.as_str(),
            p.group.as_ref().map(|g| g.0.as_str()),
            p.icon.as_deref()
        ),
        ("Work", Some("servers"), Some("🐟"))
    );
    assert_eq!(p.color.map(|c| c.to_string()).as_deref(), Some("#30a46c"));
    assert!(p.disable_dynamic_title);
    assert_eq!(p.behavior_on_session_end, SessionEndBehavior::Keep);
    assert_eq!(
        p.background,
        Some(Background::Image {
            path: "/img/a.png".into(),
            opacity: 0.4,
            fit: Fit::Tile,
            blur: false
        })
    );
    let ProfileKind::Local(local) = &p.kind else {
        panic!("local");
    };
    let expected = LocalOpts {
        program: Some("/usr/bin/fish".into()),
        args: Some(vec!["-l".into()]),
        cwd: Some("/work".into()),
        env: Some([("EDITOR".to_owned(), "nvim".to_owned())].into()),
        login: Some(true),
        osc52_write: Some(Osc52Policy::Allow),
    };
    assert_eq!(*local, expected);
    assert_eq!(ed.title, "Profile: Work");
}

#[test]
fn edit_and_save_writes_only_the_changed_lines_and_closes_the_tab() {
    let t = tmp();
    let mut app = app_in(&t.0);
    let before = config_text(&t);
    let tabs = app.tabs.len();
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    assert_eq!(app.tabs.len(), tabs + 1);
    assert_eq!(app.title(), "Profile: Pi");
    assert!(app.active_pane().is_none(), "an editor tab has no pane");
    // Editing it again activates the same tab.
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    assert_eq!(app.tabs.len(), tabs + 1);
    text(&mut app, FieldId::Port, "2200");
    field(
        &mut app,
        FieldId::PrivateKeys,
        V::Picked("~/.ssh/id_ed25519".into()),
    );
    // An empty row left by "+ Add" is not saved.
    field(&mut app, FieldId::PrivateKeys, V::Add);
    // Ticking a box to the value it already has (verify host keys = default on) writes nothing.
    field(&mut app, FieldId::VerifyHostKeys, V::Bool(true));
    editor_msg(&mut app, EditorMsg::Save);
    assert_eq!(app.tabs.len(), tabs, "Save closes the editor");
    let after = config_text(&t);
    assert_eq!(
        after,
        before.replace(
            "port = 2222\nuser = \"pi\"\nicon = \"🍓\"\n",
            "port = 2200\nuser = \"pi\"\nicon = \"🍓\"\nprivate_keys = [\"~/.ssh/id_ed25519\"]\n"
        ),
    );
    assert_eq!(load(&t.0).0, *app.config, "running config follows the file");
    // Back to the default port: the line goes away.
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    text(&mut app, FieldId::Port, "22");
    editor_msg(&mut app, EditorMsg::Save);
    assert!(!config_text(&t).contains("port ="), "{}", config_text(&t));
}

#[test]
fn new_ssh_profile_needs_a_name_and_a_host_and_is_saved_with_an_id_from_its_name() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::New(true));
    let ed = editor(&app);
    assert_eq!(ed.title, "New SSH profile");
    assert!(matches!(
        ed.issues[..],
        [
            ConfigIssue::EmptyProfileName(_),
            ConfigIssue::MissingHost(_)
        ]
    ));
    assert!(!ed.can_save());
    editor_msg(&mut app, EditorMsg::Save);
    assert_eq!(app.config.profiles.len(), 3, "Save is refused");
    text(&mut app, FieldId::Name, "Pi");
    assert!(matches!(
        editor(&app).issues[..],
        [
            ConfigIssue::DuplicateProfileName { .. },
            ConfigIssue::MissingHost(_)
        ]
    ));
    text(&mut app, FieldId::Name, "Build box");
    text(&mut app, FieldId::Host, "build.lan");
    assert!(editor(&app).can_save(), "{:?}", editor(&app).issues);
    editor_msg(&mut app, EditorMsg::Save);
    let saved = app.config.profiles.last().unwrap();
    assert_eq!(saved.id, id("build-box"));
    assert!(config_text(&t).ends_with(
        "\n[[profiles]]\nid = \"build-box\"\nname = \"Build box\"\ntype = \"ssh\"\nhost = \"build.lan\"\n"
    ));
    assert!(app.toasts.iter().any(|n| n.title == "Profile saved"));
}

#[test]
fn bad_numbers_keep_the_text_mark_the_field_and_block_save() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    text(&mut app, FieldId::Port, "70000");
    let ed = editor(&app);
    assert_eq!(ed.texts[&FieldId::Port], "70000");
    assert!(ed.errors.contains_key(&FieldId::Port));
    assert!(!ed.can_save());
    text(&mut app, FieldId::Color, "#12345");
    assert!(editor(&app).errors.contains_key(&FieldId::Color));
    text(&mut app, FieldId::Port, "");
    text(&mut app, FieldId::Color, "");
    assert!(editor(&app).can_save());
}

#[test]
fn a_jump_host_loop_is_reported() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("box")));
    field(&mut app, FieldId::JumpHost, V::Choice(1)); // pi
    editor_msg(&mut app, EditorMsg::Save);
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    let box_index = editor(&app)
        .jumps
        .iter()
        .position(|(j, _)| j.as_ref() == Some(&id("box")))
        .unwrap();
    field(&mut app, FieldId::JumpHost, V::Choice(box_index));
    let ed = editor(&app);
    assert!(
        matches!(&ed.issues[..], [ConfigIssue::JumpHostCycle { chain, .. }] if chain.join(">") == "Pi>Web box>Pi"),
        "{:?}",
        ed.issues
    );
    assert!(!ed.can_save());
}

#[test]
fn ssh_config_hosts_open_read_only_and_can_be_copied() {
    let t = tmp();
    let mut app = app_in(&t.0);
    let host = Profile {
        id: ProfileId("ssh-config:nas".into()),
        name: "nas".into(),
        kind: ProfileKind::Ssh(SshOpts {
            host: Some("10.1.1.1".into()),
            ..SshOpts::default()
        }),
        ..Profile::default_shell()
    };
    app.profiles.ssh_hosts.profiles.push(host);
    profiles(
        &mut app,
        ProfilesMsg::Edit(ProfileId("ssh-config:nas".into())),
    );
    assert!(editor(&app).read_only());
    editor_msg(&mut app, EditorMsg::Save);
    assert_eq!(app.config.profiles.len(), 3, "nothing to save in place");
    // The first change copies the host to the user's profiles and applies to the copy.
    text(&mut app, FieldId::Host, "elsewhere");
    assert_eq!(app.config.profiles.len(), 4);
    let ed = editor(&app);
    assert!(matches!(&ed.source, Source::Stored(p) if p.id == id("nas")));
    assert!(!ed.read_only());
    let ProfileKind::Ssh(ssh) = &ed.draft.kind else {
        panic!("ssh");
    };
    assert_eq!(ssh.host.as_deref(), Some("elsewhere"));
}

#[test]
fn expanding_a_read_only_colour_card_does_not_copy_the_profile() {
    let t = tmp();
    let mut app = app_in(&t.0);
    app.profiles.ssh_hosts.profiles.push(Profile {
        id: ProfileId("ssh-config:nas".into()),
        name: "nas".into(),
        kind: ProfileKind::Ssh(SshOpts::default()),
        ..Profile::default_shell()
    });
    profiles(
        &mut app,
        ProfilesMsg::Edit(ProfileId("ssh-config:nas".into())),
    );
    let before = config_text(&t);
    let profiles = app.config.profiles.len();
    field(&mut app, FieldId::Color, V::Expand);
    assert!(editor(&app).read_only());
    assert!(editor(&app).expanded.contains(&FieldId::Color));
    assert_eq!(app.config.profiles.len(), profiles);
    assert_eq!(config_text(&t), before);
}

#[test]
fn new_group_from_the_editor_is_added_and_picked() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("fish")));
    let new = editor(&app).groups.len();
    field(&mut app, FieldId::Group, V::Choice(new));
    assert_eq!(editor(&app).new_group.as_deref(), Some(""));
    text(&mut app, FieldId::NewGroup, "Databases");
    field(&mut app, FieldId::NewGroup, V::Add);
    assert!(app.config.groups.iter().any(|g| g.name == "Databases"));
    let ed = editor(&app);
    assert!(ed.new_group.is_none());
    let group = ed.draft.group.as_ref().expect("picked");
    assert!(app.config.groups.iter().any(|g| &g.id == group));
    assert!(ed.dirty(), "Save writes the profile's new group");
}

#[test]
fn delete_from_the_editor_asks_then_closes_the_editor_and_keeps_running_tabs() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(
        &mut app,
        ProfilesMsg::Open(id("fish"), crate::profiles::Launch::Tab),
    );
    let tabs = app.tabs.len();
    profiles(&mut app, ProfilesMsg::Edit(id("fish")));
    editor_msg(&mut app, EditorMsg::Delete);
    assert!(app.delete_modal().is_some());
    profiles(&mut app, ProfilesMsg::ConfirmDelete(true));
    assert_eq!(
        app.tabs.len(),
        tabs,
        "editor closed, the fish tab still open"
    );
    assert!(app.config.profiles.iter().all(|p| p.id != id("fish")));
    assert!(!config_text(&t).contains("Fish"));
}

#[test]
fn quick_connect_offers_to_save_it_as_a_profile() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Quick("me@host.lan:2022".into()));
    profiles(&mut app, ProfilesMsg::QuickSubmit);
    let toast = app
        .toasts
        .iter()
        .find(|n| {
            matches!(
                n.action,
                Some(crate::notifications::ToastAction::SaveAsProfile(_))
            )
        })
        .expect("a save-as-profile toast")
        .id;
    let _ = app.update(Message::ToastAccept(toast));
    assert!(app.toasts.get(toast).is_none(), "the toast goes");
    assert_eq!(editor(&app).source, Source::New);
    editor_msg(&mut app, EditorMsg::Save);
    let saved = app.config.profiles.last().unwrap();
    let ProfileKind::Ssh(ssh) = &saved.kind else {
        panic!("ssh");
    };
    assert_eq!(
        (ssh.host.as_deref(), ssh.port, ssh.user.as_deref()),
        (Some("host.lan"), Some(2022), Some("me"))
    );
}

#[test]
fn closing_an_editor_tab_needs_no_session() {
    let t = tmp();
    let mut app = app_in(&t.0);
    let tabs = app.tabs.len();
    profiles(&mut app, ProfilesMsg::New(false));
    let tab = editor_tab(&app);
    let _ = app.update(Message::CloseTab(tab));
    assert_eq!(app.tabs.len(), tabs);
}

#[test]
fn unknown_algorithm_names_block_save_until_unticked() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    let toggle = |name: &str| V::Toggle(name.to_owned());
    field(&mut app, FieldId::Alg(AlgKind::Cipher), toggle("bogus"));
    assert!(
        matches!(&editor(&app).issues[..], [ConfigIssue::UnknownAlgorithms { names, .. }] if names == &["bogus"]),
        "{:?}",
        editor(&app).issues
    );
    assert!(!editor(&app).can_save());
    field(&mut app, FieldId::Alg(AlgKind::Cipher), toggle("bogus"));
    // A known but weaker name is allowed (the form shows a warning instead).
    field(&mut app, FieldId::Alg(AlgKind::Hmac), toggle("hmac-sha1"));
    assert!(editor(&app).can_save(), "{:?}", editor(&app).issues);
}

#[test]
fn closing_an_edited_editor_tab_asks_first() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    let tab = editor_tab(&app);
    // Untouched: closes at once.
    let _ = app.update(Message::CloseTab(tab));
    assert!(app.tabs.iter().all(|t| t.id != tab));
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    let tab = editor_tab(&app);
    text(&mut app, FieldId::Port, "2200");
    let _ = app.update(Message::CloseTab(tab));
    assert_eq!(app.pending_discard, Some(tab), "asks first");
    let _ = app.update(Message::ConfirmDiscard(false));
    assert!(app.tabs.iter().any(|t| t.id == tab), "kept editing");
    let _ = app.update(Message::CloseTab(tab));
    let _ = app.update(Message::ConfirmDiscard(true));
    assert!(app.tabs.iter().all(|t| t.id != tab));
}

/// The group of profile `pid` as a fresh load of `config.toml` sees it (what a restart shows).
fn group_on_disk(t: &crate::profiles::tests::Tmp, pid: &str) -> Option<String> {
    let (config, err) = load(&t.0);
    assert!(err.is_none(), "{err:?}");
    config
        .profiles
        .iter()
        .find(|p| p.id.0 == pid)
        .unwrap_or_else(|| panic!("{pid} not in config.toml:\n{}", config_text(t)))
        .group
        .as_ref()
        .map(|g| g.0.clone())
}

fn group_index(app: &App, gid: &str) -> usize {
    editor(app)
        .groups
        .iter()
        .position(|(g, _)| g.as_ref().is_some_and(|g| g.0 == gid))
        .unwrap()
}

#[test]
fn an_existing_group_picked_in_the_editor_is_on_disk_after_save() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("fish")));
    let servers = group_index(&app, "servers");
    field(&mut app, FieldId::Group, V::Choice(servers));
    editor_msg(&mut app, EditorMsg::Save);
    assert_eq!(group_on_disk(&t, "fish").as_deref(), Some("servers"));
    // And back to Ungrouped.
    profiles(&mut app, ProfilesMsg::Edit(id("fish")));
    field(&mut app, FieldId::Group, V::Choice(0));
    editor_msg(&mut app, EditorMsg::Save);
    assert_eq!(group_on_disk(&t, "fish"), None);
}

#[test]
fn a_new_group_made_in_the_editor_is_on_disk_after_save() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    let new = editor(&app).groups.len();
    field(&mut app, FieldId::Group, V::Choice(new));
    text(&mut app, FieldId::NewGroup, "Databases");
    field(&mut app, FieldId::NewGroup, V::Add);
    editor_msg(&mut app, EditorMsg::Save);
    assert_eq!(group_on_disk(&t, "pi").as_deref(), Some("databases"));
    let (config, _) = load(&t.0);
    assert!(config.groups.iter().any(|g| g.name == "Databases"));
}

#[test]
fn an_ssh_config_host_gets_its_group_on_disk_after_save() {
    let t = tmp();
    let mut app = app_in(&t.0);
    app.profiles.ssh_hosts.profiles.push(Profile {
        id: ProfileId("ssh-config:nas".into()),
        name: "nas".into(),
        kind: ProfileKind::Ssh(SshOpts {
            host: Some("10.1.1.1".into()),
            ..SshOpts::default()
        }),
        ..Profile::default_shell()
    });
    profiles(
        &mut app,
        ProfilesMsg::Edit(ProfileId("ssh-config:nas".into())),
    );
    let servers = group_index(&app, "servers");
    field(&mut app, FieldId::Group, V::Choice(servers));
    editor_msg(&mut app, EditorMsg::Save);
    assert_eq!(group_on_disk(&t, "nas").as_deref(), Some("servers"));
}

#[test]
fn a_new_group_name_typed_without_enter_is_created_by_save() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    let new = editor(&app).groups.len();
    field(&mut app, FieldId::Group, V::Choice(new));
    text(&mut app, FieldId::NewGroup, "Databases");
    editor_msg(&mut app, EditorMsg::Save);
    assert_eq!(group_on_disk(&t, "pi").as_deref(), Some("databases"));
}

#[test]
fn right_click_on_a_text_field_opens_copy_and_paste() {
    let t = tmp();
    let mut app = app_in(&t.0);
    profiles(&mut app, ProfilesMsg::Edit(id("pi")));
    let before = editor(&app).draft.clone();
    let at = iced::Point::new(10.0, 20.0);
    field(&mut app, FieldId::Color, V::Menu(at, "#30a46c".into()));
    let menu = app.menu.as_ref().unwrap();
    assert_eq!(menu.anchor, at);
    let labels: Vec<_> = menu
        .items
        .iter()
        .map(|i| (i.label.as_str(), i.enabled))
        .collect();
    assert_eq!(labels, [("Copy", true), ("Paste", true)]);
    assert_eq!(
        editor(&app).draft,
        before,
        "opening the menu changes nothing"
    );
    field(&mut app, FieldId::Host, V::Menu(at, String::new()));
    assert!(
        !app.menu.as_ref().unwrap().items[0].enabled,
        "nothing to copy"
    );
}

#[test]
fn changes_list_only_the_edited_leaves_as_toml() {
    let mut ed = ProfileEditor::new(Profile::default_shell(), Source::New);
    assert_eq!(ed.changes(), []);
    ed.draft.name = "Work".to_owned();
    ed.draft.color = Some(fogu_core::RgbColor::from_hex(0x91_84d9));
    let changes = ed.changes();
    let keys: Vec<&str> = changes.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["color", "name"]);
    assert!(changes.contains(&("name".to_owned(), "\"Work\"".to_owned())));
}
