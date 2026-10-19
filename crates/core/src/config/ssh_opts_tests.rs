#![expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::path::PathBuf;

use super::*;
use crate::profile::{EffectiveProfile, Profile, ProfileDefaults, ProfileKind, resolve};

/// A profile setting every SSH field (plus the common ones), as a user would write it.
const EVERY_FIELD: &str = r#"
id = "ssh:full"
name = "Every field"
type = "ssh"
behavior_on_session_end = "reconnect"
host = "example.com"
port = 2222
user = "alice"
auth = "keyboard_interactive"
private_keys = ["~/.ssh/id_ed25519", "/keys/work.ppk"]
keepalive_interval = 15000
keepalive_count_max = 3
ready_timeout = 9000
skip_banner = true
warn_on_close = false
term = "xterm"
input_backspace = "ctrl-?"
jump_host = "ssh:bastion"
proxy_command = "nc %h %p"
socks_proxy_host = "127.0.0.1"
socks_proxy_port = 9050
socks_proxy_remote_dns = false
http_proxy_host = "proxy.lan"
http_proxy_port = 3128
agent_forward = true
x11 = true
reuse_session = false
reuse_linger = 0
stored_secrets = false
cwd = "/srv"
remember_cwd = true
clear_service_messages_on_connect = false
verify_host_keys = false
weight = -5
osc52_write = "deny"
prompt_timeout = 30000
agent_path = "/run/agent.sock"

[[forwarded_ports]]
type = "dynamic"
host = "127.0.0.1"
port = 1080
target_address = ""
target_port = 0
description = "socks"

[algorithms]
kex = ["curve25519-sha256"]
cipher = ["chacha20-poly1305@openssh.com"]
hmac = []
server_host_key = ["ssh-ed25519"]
compression = ["none"]

[[scripts]]
expect = "Password:"
send = "x\n"
is_regex = false
optional = true
"#;

#[test]
fn every_field_round_trips_through_toml() {
    let profile: Profile = toml::from_str(EVERY_FIELD).unwrap();
    let ProfileKind::Ssh(opts) = &profile.kind else {
        panic!("expected ssh");
    };
    assert_eq!(opts.auth, Some(SshAuth::KeyboardInteractive));
    assert_eq!(opts.input_backspace, Some(Backspace::CtrlQuestion));
    assert_eq!(
        opts.forwarded_ports.as_ref().unwrap()[0].kind,
        ForwardKind::Dynamic
    );
    assert_eq!(opts.scripts.as_ref().unwrap()[0].expect, "Password:");
    assert_eq!(opts.weight, Some(-5));
    let text = toml::to_string(&profile).unwrap();
    let back: Profile = toml::from_str(&text).unwrap();
    assert_eq!(back, profile);
}

#[test]
fn every_field_resolves_and_is_reported_unsupported() {
    let profile: Profile = toml::from_str(EVERY_FIELD).unwrap();
    let EffectiveProfile::Ssh(ssh) = resolve(&profile, &ProfileDefaults::default()) else {
        panic!("expected ssh");
    };
    assert_eq!(
        (ssh.host.as_str(), ssh.port, ssh.user.as_str()),
        ("example.com", 2222, "alice")
    );
    assert_eq!(ssh.keepalive_interval, Some(Duration::from_secs(15)));
    assert_eq!(ssh.ready_timeout, Some(Duration::from_secs(9)));
    assert_eq!(ssh.socks_proxy, Some(("127.0.0.1".to_owned(), 9050)));
    assert!(!ssh.socks_remote_dns);
    assert_eq!(ssh.http_proxy, Some(("proxy.lan".to_owned(), 3128)));
    assert!(!ssh.verify_host_keys && !ssh.clear_service_messages_on_connect);
    assert!(!ssh.warn_on_close);
    assert!(ssh.agent_forward && ssh.x11 && !ssh.reuse_session);
    assert_eq!(ssh.reuse_linger, Duration::ZERO);
    assert!(!ssh.stored_secrets);
    assert_eq!(ssh.unsupported(), ["input_backspace"]);
}

#[test]
fn builtin_defaults() {
    let ssh = EffectiveSsh::new("h");
    assert_eq!(ssh.port, 22);
    assert_ne!(ssh.user, "", "user defaults to $USER or root");
    assert_eq!(ssh.auth, SshAuth::Auto);
    assert_eq!(ssh.keepalive_interval, Some(Duration::from_millis(5000)));
    assert_eq!(ssh.keepalive_count_max, 10);
    assert_eq!(ssh.ready_timeout, None);
    assert_eq!(ssh.term, "xterm-256color");
    assert!(ssh.reuse_session && ssh.verify_host_keys && ssh.clear_service_messages_on_connect);
    assert_eq!(ssh.reuse_linger, Duration::from_secs(10));
    assert!(ssh.warn_on_close && ssh.socks_remote_dns && ssh.stored_secrets);
    assert!(!ssh.skip_banner && !ssh.agent_forward && !ssh.x11 && !ssh.remember_cwd);
    assert_eq!(ssh.unsupported(), Vec::<&str>::new());
}

#[test]
fn profile_overrides_defaults_and_zero_keepalive_disables() {
    let defaults = SshOpts {
        user: Some("ops".to_owned()),
        keepalive_interval: Some(0),
        term: Some("vt100".to_owned()),
        ..SshOpts::default()
    };
    let profile = SshOpts {
        host: Some("h".to_owned()),
        term: Some("xterm".to_owned()),
        ..SshOpts::default()
    };
    let ssh = resolve_ssh(&profile, &defaults);
    assert_eq!(ssh.user, "ops");
    assert_eq!(ssh.term, "xterm");
    assert_eq!(ssh.keepalive_interval, None);
}

#[test]
fn prompt_timeout_defaults_to_two_minutes_and_zero_means_no_limit() {
    assert_eq!(
        EffectiveSsh::new("h").prompt_timeout,
        Some(Duration::from_secs(120))
    );
    let profile: Profile = toml::from_str(EVERY_FIELD).unwrap();
    let EffectiveProfile::Ssh(ssh) = resolve(&profile, &ProfileDefaults::default()) else {
        panic!("expected ssh");
    };
    assert_eq!(ssh.prompt_timeout, Some(Duration::from_secs(30)));
    assert_eq!(ssh.agent_path, Some(PathBuf::from("/run/agent.sock")));
    let off = SshOpts {
        prompt_timeout: Some(0),
        ..SshOpts::default()
    };
    assert_eq!(resolve_ssh(&off, &SshOpts::default()).prompt_timeout, None);
}

/// Profiles to check `strip_defaults` on: every field set, every field set to its builtin
/// default, and a local profile.
fn strip_fixtures() -> Vec<Profile> {
    let every: Profile = toml::from_str(EVERY_FIELD).unwrap();
    let explicit_defaults: Profile = toml::from_str(
        r#"
        id = "d"
        name = "Defaults spelled out"
        type = "ssh"
        host = "h"
        port = 22
        auth = "auto"
        private_keys = []
        keepalive_interval = 5000
        keepalive_count_max = 10
        skip_banner = false
        warn_on_close = true
        term = "xterm-256color"
        input_backspace = "backspace"
        forwarded_ports = []
        agent_forward = false
        x11 = false
        algorithms = {}
        reuse_session = true
        scripts = []
        remember_cwd = false
        clear_service_messages_on_connect = true
        verify_host_keys = true
        weight = 0
        identities_only = false
        prompt_timeout = 120000
    "#,
    )
    .unwrap();
    let local: Profile = toml::from_str(
        r#"
        id = "l"
        name = "Local"
        type = "local"
        program = "/bin/zsh"
        args = []
        env = {}
        login = false
        icon = " "
    "#,
    )
    .unwrap();
    vec![every, explicit_defaults, local]
}

#[test]
fn strip_defaults_keeps_what_resolve_sees_and_is_idempotent() {
    use crate::profile::{LocalOpts, strip_defaults};
    let defaults = [
        ProfileDefaults::default(),
        ProfileDefaults {
            local: LocalOpts {
                program: Some("/bin/zsh".to_owned()),
                ..LocalOpts::default()
            },
            ssh: SshOpts {
                port: Some(2222),
                term: Some("vt100".to_owned()),
                x11: Some(true),
                ..SshOpts::default()
            },
        },
    ];
    for d in &defaults {
        for p in strip_fixtures() {
            let stripped = strip_defaults(&p, d);
            assert_eq!(resolve(&stripped, d), resolve(&p, d), "{}", p.name);
            assert_eq!(strip_defaults(&stripped, d), stripped, "{}", p.name);
        }
    }
}

#[test]
fn strip_defaults_drops_values_equal_to_what_would_be_inherited() {
    use crate::profile::strip_defaults;
    let fixtures = strip_fixtures();
    let none = ProfileDefaults::default();
    let ProfileKind::Ssh(ssh) = strip_defaults(&fixtures[1], &none).kind else {
        panic!("expected ssh");
    };
    assert_eq!(
        ssh,
        SshOpts {
            host: Some("h".to_owned()),
            ..SshOpts::default()
        }
    );
    // With `profile_defaults.ssh.port = 2222`, an explicit 22 is a real override.
    let d = ProfileDefaults {
        ssh: SshOpts {
            port: Some(2222),
            ..SshOpts::default()
        },
        ..ProfileDefaults::default()
    };
    let ProfileKind::Ssh(ssh) = strip_defaults(&fixtures[1], &d).kind else {
        panic!("expected ssh");
    };
    assert_eq!(ssh.port, Some(22));
    // The user (defaults to the local `$USER`) and osc52_write are kept as written.
    let every = strip_defaults(&fixtures[0], &none);
    let ProfileKind::Ssh(ssh) = &every.kind else {
        panic!("expected ssh");
    };
    assert_eq!(ssh.user.as_deref(), Some("alice"));
    assert_eq!(ssh.osc52_write, Some(crate::profile::Osc52Policy::Deny));
    // A local profile: everything equal to the builtin default goes, a blank icon too.
    let local = strip_defaults(&fixtures[2], &none);
    assert_eq!(local.icon, None);
    let text = toml::to_string(&local).unwrap();
    // The fixture's `login = false` is the builtin default only off macOS.
    let login = if crate::DEFAULT_LOGIN {
        "login = false\n"
    } else {
        ""
    };
    assert_eq!(
        text,
        format!("id = \"l\"\nname = \"Local\"\ntype = \"local\"\nprogram = \"/bin/zsh\"\n{login}")
    );
}

#[test]
fn not_supported_yet_names_every_option_unsupported_warns_about() {
    let profile: Profile = toml::from_str(EVERY_FIELD).unwrap();
    let EffectiveProfile::Ssh(ssh) = resolve(&profile, &ProfileDefaults::default()) else {
        panic!("expected ssh");
    };
    let ProfileKind::Ssh(opts) = &profile.kind else {
        panic!("expected ssh");
    };
    let keys = toml::Table::try_from(opts).unwrap();
    for name in NOT_SUPPORTED_YET {
        assert!(keys.contains_key(*name), "{name} is not an SshOpts key");
    }
    for warned in ssh.unsupported() {
        assert!(NOT_SUPPORTED_YET.contains(&warned.as_str()), "{warned}");
    }
}

#[test]
fn forward_bind_host_exposure_and_problems() {
    let fwd = |kind, host: &str, port, target: &str, target_port| PortForward {
        kind,
        host: host.to_owned(),
        port,
        target_address: target.to_owned(),
        target_port,
        description: String::new(),
    };
    let local = fwd(ForwardKind::Local, " ", 8080, "web", 80);
    assert_eq!(local.bind_host(), "127.0.0.1");
    assert!(!local.is_exposed());
    assert_eq!(local.problem(), None);
    assert_eq!(local.label(), "L 127.0.0.1:8080 → web:80");
    for host in ["localhost", "::1", "[::1]", "127.0.0.2"] {
        assert!(
            !fwd(ForwardKind::Local, host, 1, "t", 1).is_exposed(),
            "{host}"
        );
    }
    for host in ["0.0.0.0", "::", "192.0.2.5", "box.lan", "*"] {
        assert!(
            fwd(ForwardKind::Remote, host, 1, "t", 1).is_exposed(),
            "{host}"
        );
    }
    assert!(fwd(ForwardKind::Local, "", 0, "t", 1).problem().is_some());
    assert!(fwd(ForwardKind::Local, "", 1, " ", 1).problem().is_some());
    assert!(fwd(ForwardKind::Remote, "", 1, "t", 0).problem().is_some());
    let socks = fwd(ForwardKind::Dynamic, "", 1080, "", 0);
    assert_eq!(socks.problem(), None);
    assert_eq!(socks.label(), "D 127.0.0.1:1080 (SOCKS)");
}

#[test]
fn unknown_algorithm_names_are_listed_once_per_category_order() {
    let known = [
        vec!["curve25519-sha256".to_owned()],
        vec!["aes256-gcm@openssh.com".to_owned()],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ];
    let algorithms = Algorithms {
        kex: vec!["curve25519-sha256".to_owned(), "bogus".to_owned()],
        cipher: vec!["bogus".to_owned(), "aes256-gcm@openssh.com".to_owned()],
        hmac: vec!["hmac-md5".to_owned()],
        ..Algorithms::default()
    };
    assert_eq!(algorithms.unknown(&known), ["bogus", "hmac-md5"]);
    assert_eq!(Algorithms::default().unknown(&known), Vec::<String>::new());
}
