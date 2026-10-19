//! Profiles: `Profile` (common fields) + `ProfileKind` (per-kind options), profile defaults, and
//! the one `resolve` merge function. Only local and SSH profiles exist; serial/telnet kinds are
//! not modelled.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::background::Background;
use crate::config::ssh_opts::{EffectiveSsh, SshOpts, resolve_ssh, strip_ssh};
use crate::scheme::RgbColor;

/// How to start a local shell in a PTY. The concrete, fully-resolved form of a local profile
/// produced by [`resolve`] from a stored [`LocalOpts`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalProfile {
    /// Program to run; `None` = the user's default shell (`$SHELL` → passwd → `/bin/sh`).
    pub program: Option<String>,
    /// Arguments after argv0. Stored separately from `program` (never `sh -c "<string>"`).
    pub args: Vec<String>,
    /// Working directory; `None` = `$HOME`.
    pub cwd: Option<PathBuf>,
    /// Extra environment; values support `${VAR}` expansion from the parent environment.
    pub env: BTreeMap<String, String>,
    /// Start as a login shell (argv0 prefixed with `-`, e.g. `-zsh`).
    pub login: bool,
}

/// What to do with an OSC 52 clipboard write (an application could otherwise overwrite the clipboard
/// silently); applied by the UI (`ui/src/lifecycle.rs`, `on_clipboard_store`): `Ask` shows a Copy / Deny toast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Osc52Policy {
    /// Prompt the user each time.
    #[default]
    Ask,
    /// Always write to the clipboard.
    Allow,
    /// Always ignore the request.
    Deny,
}

/// Stored (partial) options for a local profile: every field is `Option<T>` so a profile can
/// leave a field unset and inherit it from [`ProfileDefaults::local`], which in turn inherits
/// from [`LocalProfile::default`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalOpts {
    /// See [`LocalProfile::program`].
    pub program: Option<String>,
    /// See [`LocalProfile::args`].
    pub args: Option<Vec<String>>,
    /// See [`LocalProfile::cwd`].
    pub cwd: Option<PathBuf>,
    /// See [`LocalProfile::env`].
    pub env: Option<BTreeMap<String, String>>,
    /// See [`LocalProfile::login`].
    pub login: Option<bool>,
    /// Per-profile override of `terminal.osc52_write`.
    pub osc52_write: Option<Osc52Policy>,
}

/// Per-kind profile options (an enum, not trait objects — the set of kinds is
/// closed and known at compile time).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[expect(
    clippy::large_enum_variant,
    reason = "a handful of profiles, cloned only when a tab opens; boxing would only add noise"
)]
pub enum ProfileKind {
    /// A local shell/command.
    Local(LocalOpts),
    /// An SSH connection.
    Ssh(SshOpts),
}

/// What happens to a pane when its session ends. `Auto`/`Reconnect` are parsed, but for now `Auto` acts like
/// `CloseIfClean` and `Reconnect` like `Keep`. Config key is `behavior_on_session_end`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionEndBehavior {
    /// Leave the pane open, showing the exit status.
    #[serde(rename = "keep")]
    Keep,
    /// Close the pane immediately, whatever the exit status.
    #[serde(rename = "close")]
    Close,
    /// Close the pane when the process exited with code 0, else keep it showing the status.
    #[default]
    #[serde(rename = "close_if_clean")]
    CloseIfClean,
    /// Close when the session was ended deliberately.
    #[serde(rename = "auto")]
    Auto,
    /// Reconnect when the connection drops; kept open for now.
    #[serde(rename = "reconnect")]
    Reconnect,
}

impl SessionEndBehavior {
    /// Whether a connected SSH pane whose session ended reconnects by itself:
    /// `Reconnect` always, `Auto` only when the connection `dropped` (no exit status: a
    /// transport error or the server going away), never after a deliberate exit.
    #[must_use]
    pub fn reconnects(self, dropped: bool) -> bool {
        match self {
            Self::Reconnect => true,
            Self::Auto => dropped,
            Self::Keep | Self::Close | Self::CloseIfClean => false,
        }
    }

    /// Whether a pane whose process ended goes away now. `clean` = exit code 0, not killed by a
    /// signal. A crashed session (spawn failure, I/O error, task panic) is never closed
    /// automatically: the user has to see why.
    #[must_use]
    pub fn closes(self, clean: bool) -> bool {
        match self {
            Self::Keep | Self::Reconnect => false,
            Self::Close => true,
            Self::CloseIfClean | Self::Auto => clean,
        }
    }
}

/// A profile identifier. Newtype so profile/group ids are never confused with plain strings.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(pub String);

/// A profile group identifier.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupId(pub String);

/// A profile group (just the data model for now).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// Stable identifier.
    pub id: GroupId,
    /// Display name.
    pub name: String,
}

/// One stored profile: fields common to every kind, plus the kind-specific options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Stable identifier (referenced by `Config.default_profile`, tab recovery, `jumpHost`...).
    pub id: ProfileId,
    /// Display name.
    pub name: String,
    /// Group this profile belongs to, if any.
    #[serde(default)]
    pub group: Option<GroupId>,
    /// Icon name.
    #[serde(default)]
    pub icon: Option<String>,
    /// Tab colour; `"#RRGGBB"` in TOML.
    #[serde(default)]
    pub color: Option<RgbColor>,
    /// Suppress OSC-2/0/title-driven tab titles for this profile.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disable_dynamic_title: bool,
    /// See [`SessionEndBehavior`].
    #[serde(default, skip_serializing_if = "is_default")]
    pub behavior_on_session_end: SessionEndBehavior,
    /// Colour or image drawn under this profile's tabs (a tab may override it).
    #[serde(default)]
    pub background: Option<Background>,
    /// Save this profile's scrollback for tab recovery (unset = `recovery.restore_scrollback`);
    /// `false` for SSH hosts whose output must never touch the disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_scrollback: Option<bool>,
    /// Kind-specific options, flattened into the same TOML table (`type = "local"` / `"ssh"`).
    #[serde(flatten)]
    pub kind: ProfileKind,
}

impl Profile {
    /// The built-in "Default shell" profile: generated at runtime, never stored.
    #[must_use]
    pub fn default_shell() -> Self {
        Self {
            id: ProfileId("builtin:local:default".to_owned()),
            name: "Default shell".to_owned(),
            group: None,
            icon: None,
            color: None,
            disable_dynamic_title: false,
            behavior_on_session_end: SessionEndBehavior::default(),
            background: None,
            restore_scrollback: None,
            kind: ProfileKind::Local(LocalOpts::default()),
        }
    }
}

// Serde's `skip_serializing_if` passes a reference.
#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde's signature")]
fn is_false(value: &bool) -> bool {
    !value
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Partial defaults applied under every profile of a kind, themselves falling back to
/// `LocalProfile::default`/builtin SSH defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileDefaults {
    /// Defaults applied to every `Local` profile.
    pub local: LocalOpts,
    /// Defaults applied to every `Ssh` profile.
    pub ssh: SshOpts,
}

/// A profile with every option resolved to a concrete value, ready to open a session.
#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "built once per opened tab and moved into the session task"
)]
pub enum EffectiveProfile {
    /// A local shell/command, ready for `fogu_session::open`.
    Local(LocalProfile),
    /// An SSH connection, ready for `fogu_session::open`.
    Ssh(EffectiveSsh),
}

/// The one profile-defaults merge function:
/// `profile.or(defaults[kind]).or(builtin_default)`.
#[must_use]
pub fn resolve(profile: &Profile, defaults: &ProfileDefaults) -> EffectiveProfile {
    match &profile.kind {
        ProfileKind::Local(opts) => EffectiveProfile::Local(resolve_local(opts, &defaults.local)),
        ProfileKind::Ssh(opts) => EffectiveProfile::Ssh(resolve_ssh(opts, &defaults.ssh)),
    }
}

/// The inverse of [`resolve`]: `profile` with every kind option that resolves to the same
/// value without it set to `None`, and empty `icon`/`group` cleared. What the profile editor
/// saves, so config.toml keeps only real overrides.
#[must_use]
pub fn strip_defaults(profile: &Profile, defaults: &ProfileDefaults) -> Profile {
    let mut out = profile.clone();
    out.icon = out.icon.filter(|icon| !icon.trim().is_empty());
    out.group = out.group.filter(|group| !group.0.trim().is_empty());
    out.kind = match &profile.kind {
        ProfileKind::Local(opts) => ProfileKind::Local(strip_local(opts, &defaults.local)),
        ProfileKind::Ssh(opts) => ProfileKind::Ssh(strip_ssh(opts, &defaults.ssh)),
    };
    out
}

fn strip_local(p: &LocalOpts, d: &LocalOpts) -> LocalOpts {
    let full = resolve_local(p, d);
    let mut out = p.clone();
    macro_rules! strip {
        ($($field:ident),*) => {$(
            if out.$field.is_some() {
                let mut without = out.clone();
                without.$field = None;
                if resolve_local(&without, d) == full {
                    out = without;
                }
            }
        )*};
    }
    strip!(program, args, cwd, env, login);
    out
}

fn resolve_local(opts: &LocalOpts, defaults: &LocalOpts) -> LocalProfile {
    LocalProfile {
        program: opts.program.clone().or_else(|| defaults.program.clone()),
        args: opts
            .args
            .clone()
            .or_else(|| defaults.args.clone())
            .unwrap_or_default(),
        cwd: opts.cwd.clone().or_else(|| defaults.cwd.clone()),
        env: opts
            .env
            .clone()
            .or_else(|| defaults.env.clone())
            .unwrap_or_default(),
        login: opts.login.or(defaults.login).unwrap_or(DEFAULT_LOGIN),
    }
}

/// Whether a local profile starts a login shell when neither it nor the defaults say. On macOS
/// yes, like Terminal.app: a Finder-launched app gets launchd's bare environment, and zsh reads
/// `.zprofile` (Homebrew `PATH`, locale) only as a login shell.
pub const DEFAULT_LOGIN: bool = cfg!(target_os = "macos");

/// The effective `terminal.osc52_write` for a profile: its own override, else the global default.
#[must_use]
pub fn effective_osc52_write(profile: &Profile, global_default: Osc52Policy) -> Osc52Policy {
    let override_ = match &profile.kind {
        ProfileKind::Local(opts) => opts.osc52_write,
        ProfileKind::Ssh(opts) => opts.osc52_write,
    };
    override_.unwrap_or(global_default)
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, clippy::panic, reason = "tests")]

    use super::*;

    #[test]
    fn resolve_local_merges_profile_over_defaults_over_builtin() {
        let defaults = ProfileDefaults {
            local: LocalOpts {
                program: Some("/bin/bash".to_owned()),
                login: Some(true),
                ..LocalOpts::default()
            },
            ssh: SshOpts::default(),
        };
        // Profile overrides `program`, inherits `login` from defaults, `args`/`env`/`cwd` fall
        // back to `LocalProfile::default`.
        let profile = Profile {
            kind: ProfileKind::Local(LocalOpts {
                program: Some("/usr/bin/fish".to_owned()),
                ..LocalOpts::default()
            }),
            ..Profile::default_shell()
        };
        let EffectiveProfile::Local(effective) = resolve(&profile, &defaults) else {
            panic!("expected a local profile");
        };
        assert_eq!(effective.program, Some("/usr/bin/fish".to_owned()));
        assert!(effective.login);
        assert_eq!(effective.args, Vec::<String>::new());
        assert_eq!(effective.cwd, None);
    }

    #[test]
    fn resolve_local_with_nothing_set_is_the_builtin_default() {
        let profile = Profile::default_shell();
        let EffectiveProfile::Local(effective) = resolve(&profile, &ProfileDefaults::default())
        else {
            panic!("expected a local profile");
        };
        let builtin = LocalProfile {
            login: DEFAULT_LOGIN,
            ..LocalProfile::default()
        };
        assert_eq!(effective, builtin);
    }

    #[test]
    fn resolve_ssh_defaults_the_port() {
        let profile = Profile {
            kind: ProfileKind::Ssh(SshOpts {
                host: Some("example.com".to_owned()),
                ..SshOpts::default()
            }),
            ..Profile::default_shell()
        };
        let EffectiveProfile::Ssh(effective) = resolve(&profile, &ProfileDefaults::default())
        else {
            panic!("expected an ssh profile");
        };
        assert_eq!(effective.host, "example.com");
        assert_eq!(effective.port, 22);
        assert_ne!(effective.user, "");
    }

    #[test]
    fn profile_osc52_override_wins_over_global_default() {
        let mut profile = Profile::default_shell();
        profile.kind = ProfileKind::Local(LocalOpts {
            osc52_write: Some(Osc52Policy::Allow),
            ..LocalOpts::default()
        });
        assert_eq!(
            effective_osc52_write(&profile, Osc52Policy::Deny),
            Osc52Policy::Allow
        );
        let unset = Profile::default_shell();
        assert_eq!(
            effective_osc52_write(&unset, Osc52Policy::Deny),
            Osc52Policy::Deny
        );
    }

    #[test]
    fn on_exit_policy_table() {
        use SessionEndBehavior::{Close, CloseIfClean, Keep};
        // (behavior, clean exit, closes)
        let rows = [
            (Keep, true, false),
            (Keep, false, false),
            (Close, true, true),
            (Close, false, true),
            (CloseIfClean, true, true),
            (CloseIfClean, false, false),
        ];
        for (behavior, clean, want) in rows {
            assert_eq!(behavior.closes(clean), want, "{behavior:?} clean={clean}");
        }
        assert_eq!(SessionEndBehavior::default(), CloseIfClean);
    }

    #[test]
    fn on_exit_values_parse_from_toml() {
        for (text, want) in [
            ("keep", SessionEndBehavior::Keep),
            ("close", SessionEndBehavior::Close),
            ("close_if_clean", SessionEndBehavior::CloseIfClean),
            ("auto", SessionEndBehavior::Auto),
            ("reconnect", SessionEndBehavior::Reconnect),
        ] {
            let p: Profile = toml::from_str(&format!(
                "id = \"x\"\nname = \"x\"\ntype = \"local\"\nbehavior_on_session_end = \"{text}\""
            ))
            .unwrap();
            assert_eq!(p.behavior_on_session_end, want);
        }
    }

    #[test]
    fn profile_round_trips_through_toml() {
        let profile = Profile {
            color: Some(RgbColor::from_hex(0x12_3456)),
            kind: ProfileKind::Ssh(SshOpts {
                host: Some("example.com".to_owned()),
                port: Some(2222),
                user: Some("alice".to_owned()),
                ..SshOpts::default()
            }),
            ..Profile::default_shell()
        };
        let text = toml::to_string(&profile).unwrap();
        assert!(text.contains("type = \"ssh\""));
        let back: Profile = toml::from_str(&text).unwrap();
        assert_eq!(back, profile);
    }
}
