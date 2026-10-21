#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "tests"
)]

use std::path::PathBuf;

use super::*;
use crate::config::load;
use crate::config::ssh_opts::SshOpts;
use crate::profile::ProfileKind;

const FIXTURE: &str = "\
# my config
version = 1

# --- work ---
[[profiles]]
id = \"work\"   # keep
name = \"Work\"
type = \"ssh\"
host = \"work.lan\"
port = 2222

[[profiles]]
id = \"play\"
name = \"Play\"
type = \"local\"
env = { A = \"1\" }

# trailing comment
";

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp(text: Option<&str>) -> Tmp {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("fogu-edit-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(text) = text {
        std::fs::write(dir.join(FILE_NAME), text).unwrap();
    }
    Tmp(dir)
}

fn read(t: &Tmp) -> String {
    std::fs::read_to_string(t.0.join(FILE_NAME)).unwrap()
}

fn id(s: &str) -> ProfileId {
    ProfileId(s.to_owned())
}

/// Applies `edits` to the file and to the loaded config; both must agree afterwards.
fn edit_and_check(t: &Tmp, edits: &[ProfileEdit]) -> Config {
    let (mut config, err) = load(&t.0);
    assert!(err.is_none());
    edit_profiles(&t.0, edits).unwrap();
    for e in edits {
        e.apply(&mut config);
    }
    let (reloaded, err) = load(&t.0);
    assert!(err.is_none(), "{err:?}\n{}", read(t));
    assert_eq!(reloaded, config, "{}", read(t));
    reloaded
}

#[test]
fn duplicate_appends_a_copy_and_keeps_everything_else() {
    let t = tmp(Some(FIXTURE));
    let edit = ProfileEdit::Duplicate {
        from: id("work"),
        id: id("work-copy"),
        name: "Work copy".to_owned(),
    };
    let config = edit_and_check(&t, &[edit]);
    assert_eq!(config.profiles.len(), 3);
    assert_eq!(config.profiles[2].name, "Work copy");
    assert_eq!(config.profiles[2].kind, config.profiles[0].kind);
    let after = read(&t);
    // The original text is untouched; the copy comes after the last profile.
    assert!(
        after.starts_with(&FIXTURE[..FIXTURE.find("# trailing").unwrap()]),
        "{after}"
    );
    assert_eq!(after.matches("# --- work ---").count(), 1, "{after}");
    assert!(
        after.contains("[[profiles]]\nid = \"work-copy\"\nname = \"Work copy\"\ntype = \"ssh\""),
        "{after}"
    );
}

#[test]
fn new_group_and_move_touch_only_those_lines() {
    let t = tmp(Some(FIXTURE));
    let group = Group {
        id: GroupId("servers".to_owned()),
        name: "Servers".to_owned(),
    };
    let edits = [
        ProfileEdit::AddGroup(group.clone()),
        ProfileEdit::SetGroup {
            id: id("play"),
            group: Some(group.id.clone()),
        },
    ];
    let config = edit_and_check(&t, &edits);
    assert_eq!(config.groups, std::slice::from_ref(&group));
    assert_eq!(config.profiles[1].group, Some(group.id));
    let after = read(&t);
    assert!(
        after.contains("[[groups]]\nid = \"servers\"\nname = \"Servers\""),
        "{after}"
    );
    assert_eq!(
        after
            .replace("group = \"servers\"\n", "")
            .lines()
            .filter(|l| !FIXTURE.contains(l))
            .count(),
        3, // [[groups]], id, name
        "{after}"
    );
    // Ungrouping removes the line again.
    let ungroup = ProfileEdit::SetGroup {
        id: id("play"),
        group: None,
    };
    let config = edit_and_check(&t, &[ungroup]);
    assert_eq!(config.profiles[1].group, None);
}

#[test]
fn delete_removes_only_that_profile() {
    let t = tmp(Some(FIXTURE));
    let config = edit_and_check(&t, &[ProfileEdit::Delete(id("play"))]);
    assert_eq!(config.profiles.len(), 1);
    let after = read(&t);
    assert!(!after.contains("Play"), "{after}");
    assert!(
        after.starts_with("# my config\nversion = 1\n\n# --- work ---\n"),
        "{after}"
    );
    assert!(after.contains("# trailing comment"), "{after}");
}

#[test]
fn unknown_profile_or_broken_file_changes_nothing() {
    let t = tmp(Some(FIXTURE));
    let err = edit_profiles(&t.0, &[ProfileEdit::Delete(id("gone"))]).unwrap_err();
    assert!(matches!(err, ConfigError::NoSuchProfile { .. }), "{err}");
    // A later bad edit also cancels an earlier good one.
    let edits = [
        ProfileEdit::Delete(id("work")),
        ProfileEdit::SetGroup {
            id: id("gone"),
            group: None,
        },
    ];
    assert!(edit_profiles(&t.0, &edits).is_err());
    assert_eq!(read(&t), FIXTURE);
    std::fs::write(t.0.join(FILE_NAME), "not toml [[[").unwrap();
    assert!(edit_profiles(&t.0, &[ProfileEdit::Delete(id("work"))]).is_err());
    assert_eq!(read(&t), "not toml [[[");
    assert!(!t.0.join("config.toml.bak").exists());
}

#[test]
fn adding_to_a_missing_file_creates_it() {
    let t = tmp(None);
    let profile = Profile {
        id: id("quick"),
        name: "me@h".to_owned(),
        kind: ProfileKind::Ssh(SshOpts {
            host: Some("h".to_owned()),
            user: Some("me".to_owned()),
            ..SshOpts::default()
        }),
        ..Profile::default_shell()
    };
    let config = edit_and_check(&t, &[ProfileEdit::Add(Box::new(profile.clone()))]);
    assert_eq!(config.profiles, [profile]);
    // Anything else needs an existing file.
    let t = tmp(None);
    assert!(edit_profiles(&t.0, &[ProfileEdit::Delete(id("x"))]).is_err());
}

#[test]
fn unique_ids() {
    let (config, _) = load(&tmp(Some(FIXTURE)).0);
    assert_eq!(unique_profile_id(&config, "work").0, "work-2");
    assert_eq!(unique_profile_id(&config, "new").0, "new");
    let mut config = config;
    config.groups.push(Group {
        id: GroupId("my-work".to_owned()),
        name: String::new(),
    });
    assert_eq!(unique_group_id(&config, "  My  Work! ").0, "my-work-2");
    assert_eq!(unique_group_id(&config, "Ünï").0, "ünï");
    assert_eq!(unique_group_id(&config, "!!!").0, "group");
}

const EDITED: &str = "\
# my config
version = 1

# --- work ---
[[profiles]]
id = \"work\"   # keep
name = \"Work\"
type = \"ssh\"
host = \"work.lan\"
port = 2222   # odd port
future_option = \"kept\"   # not a Fogu key
user = \"me\"

# trailing comment
";

fn stored(t: &Tmp, id_: &str) -> Profile {
    let (config, err) = load(&t.0);
    assert!(err.is_none());
    config.profiles.into_iter().find(|p| p.id.0 == id_).unwrap()
}

fn ssh_mut(p: &mut Profile) -> &mut SshOpts {
    match &mut p.kind {
        ProfileKind::Ssh(ssh) => ssh,
        ProfileKind::Local(_) => panic!("not ssh"),
    }
}

#[test]
fn update_rewrites_only_the_changed_keys_in_place() {
    let t = tmp(Some(EDITED));
    let old = stored(&t, "work");
    let mut new = old.clone();
    ssh_mut(&mut new).port = Some(2200);
    ssh_mut(&mut new).user = None;
    ssh_mut(&mut new).private_keys = Some(vec![PathBuf::from("~/.ssh/id_ed25519")]);
    new.disable_dynamic_title = false;
    let edit = ProfileEdit::Update {
        old: Box::new(old),
        new: Box::new(new.clone()),
    };
    let config = edit_and_check(&t, &[edit]);
    assert_eq!(config.profiles, [new]);
    assert_eq!(
        read(&t),
        "\
# my config
version = 1

# --- work ---
[[profiles]]
id = \"work\"   # keep
name = \"Work\"
type = \"ssh\"
host = \"work.lan\"
port = 2200   # odd port
future_option = \"kept\"   # not a Fogu key
disable_dynamic_title = false
private_keys = [\"~/.ssh/id_ed25519\"]

# trailing comment
"
    );
    assert_eq!(
        std::fs::read_to_string(t.0.join("config.toml.bak")).unwrap(),
        EDITED
    );
}

#[test]
fn update_writes_nested_values_inline_and_back_to_default_removes_them() {
    use crate::config::ssh_opts::{Algorithms, ForwardKind, PortForward};
    let t = tmp(Some(EDITED));
    let old = stored(&t, "work");
    let mut new = old.clone();
    ssh_mut(&mut new).forwarded_ports = Some(vec![PortForward {
        kind: ForwardKind::Local,
        host: "127.0.0.1".into(),
        port: 8080,
        target_address: "web".into(),
        target_port: 80,
        description: String::new(),
    }]);
    ssh_mut(&mut new).algorithms = Some(Algorithms {
        kex: vec!["curve25519-sha256".into()],
        ..Algorithms::default()
    });
    let edit = ProfileEdit::Update {
        old: Box::new(old.clone()),
        new: Box::new(new.clone()),
    };
    edit_and_check(&t, &[edit]);
    let after = read(&t);
    let added: Vec<&str> = after.lines().filter(|l| !EDITED.contains(l)).collect();
    assert_eq!(
        added,
        [
            "forwarded_ports = [{ type = \"local\", host = \"127.0.0.1\", port = 8080, target_address = \"web\", target_port = 80 }]",
            "algorithms = { kex = [\"curve25519-sha256\"], cipher = [], hmac = [], server_host_key = [], compression = [] }",
        ],
        "{after}"
    );
    // Editing back to the defaults (strip_defaults turns them into None) restores the file.
    let back = crate::profile::strip_defaults(&old, &crate::profile::ProfileDefaults::default());
    let edit = ProfileEdit::Update {
        old: Box::new(new),
        new: Box::new(back),
    };
    edit_and_check(&t, &[edit]);
    assert_eq!(read(&t), EDITED);
}

#[test]
fn update_of_a_missing_profile_or_a_broken_file_writes_nothing() {
    let t = tmp(Some(EDITED));
    let mut old = stored(&t, "work");
    old.id = id("gone");
    let edit = ProfileEdit::Update {
        old: Box::new(old.clone()),
        new: Box::new(old),
    };
    let err = edit_profiles(&t.0, std::slice::from_ref(&edit)).unwrap_err();
    assert!(matches!(err, ConfigError::NoSuchProfile { .. }), "{err}");
    assert_eq!(read(&t), EDITED);
    std::fs::write(t.0.join(FILE_NAME), "id = [").unwrap();
    assert!(edit_profiles(&t.0, &[edit]).is_err());
    assert_eq!(read(&t), "id = [");
    assert!(!t.0.join("config.toml.bak").exists());
}

#[test]
fn unique_profile_name_skips_taken_names_ignoring_case() {
    let mut config = Config::default();
    config.profiles.push(Profile {
        id: crate::ProfileId("a".into()),
        name: "Prod".into(),
        ..Profile::default_shell()
    });
    assert_eq!(crate::config::unique_profile_name(&config, "dev"), "dev");
    assert_eq!(
        crate::config::unique_profile_name(&config, " prod "),
        "prod-2"
    );
}

#[test]
fn settings_write_only_the_changed_keys_in_place() {
    let text = "# top\n[appearance]\nfont_size = 12 # mine\ncolor_scheme = \"Dracula\"\n\n[[profiles]]\nid = \"a\"\nname = \"A\"\ntype = \"local\"\n";
    let t = tmp(Some(text));
    let (old, err) = load(&t.0);
    assert!(err.is_none());
    let mut new = old.clone();
    new.appearance.font_size = 14.0;
    new.terminal.wheel_lines = 5;
    new.appearance.terminal_bg_alpha = Some(0.5);
    edit_settings(&t.0, &old, &new).unwrap();
    let written = read(&t);
    assert!(written.contains("font_size = 14.0 # mine"), "{written}");
    assert!(written.contains("color_scheme = \"Dracula\""), "{written}");
    assert!(written.contains("# top"), "{written}");
    assert!(written.contains("[terminal]\nwheel_lines = 5"), "{written}");
    // Keys that did not change are not spelled out.
    assert!(!written.contains("scrollback_lines"), "{written}");
    let (back, _) = load(&t.0);
    assert_eq!(back.appearance, new.appearance);
    assert_eq!(back.terminal, new.terminal);
    assert_eq!(back.profiles, old.profiles);

    // An option set back to "unset" removes its key.
    edit_settings(&t.0, &new, &old).unwrap();
    assert!(!read(&t).contains("terminal_bg_alpha"));
}

#[test]
fn settings_create_a_missing_file() {
    let t = tmp(None);
    let old = Config::default();
    let mut new = old.clone();
    new.recovery.enabled = false;
    edit_settings(&t.0, &old, &new).unwrap();
    assert_eq!(read(&t).trim(), "[recovery]\nenabled = false");
}

#[test]
fn fractional_settings_are_written_at_f32_precision() {
    let t = tmp(None);
    let old = Config::default();
    let mut new = old.clone();
    new.terminal.pane_resize_step = 0.11;
    new.appearance.window_opacity = 0.37;
    edit_settings(&t.0, &old, &new).unwrap();
    let written = read(&t);
    assert!(written.contains("pane_resize_step = 0.11\n"), "{written}");
    assert!(written.contains("window_opacity = 0.37\n"), "{written}");
}

#[test]
fn a_nan_float_in_the_file_does_not_block_settings_writes() {
    let t = tmp(Some("[appearance]\nwindow_opacity = nan\n"));
    let (old, _) = load(&t.0);
    let mut new = old.clone();
    new.terminal.wheel_lines = 4;
    edit_settings(&t.0, &old, &new).unwrap();
    assert!(read(&t).contains("wheel_lines = 4"));
}

#[test]
fn floats_inside_a_profile_table_are_written_at_f32_precision() {
    let text = "[[profiles]]\nid = \"a\"\nname = \"A\"\ntype = \"local\"\n";
    let t = tmp(Some(text));
    let (config, _) = load(&t.0);
    let old = config.profiles.first().unwrap().clone();
    let mut new = old.clone();
    new.background = Some(crate::Background::Image {
        path: PathBuf::from("/tmp/bg.png"),
        opacity: 0.85,
        fit: crate::Fit::Cover,
        blur: false,
    });
    edit_profiles(
        &t.0,
        &[ProfileEdit::Update {
            old: Box::new(old),
            new: Box::new(new),
        }],
    )
    .unwrap();
    let written = read(&t);
    assert!(written.contains("opacity = 0.85"), "{written}");
}
