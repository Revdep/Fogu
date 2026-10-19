//! `fogu-core`: pure config/model crate (config schema, profiles, `SplitTree`, colour
//! schemes, keymap parsing, `Action` enum, recovery-state model). No tokio, no GUI, no I/O
//! (except [`config::load`], the in-place `config.toml` edits, [`builtin`]'s `/etc/shells`
//! read, [`recent`]'s state file and the read-only [`ssh_config`] import; plain `std::fs`).

mod action;
mod background;
pub mod builtin;
pub mod config;
pub mod fsutil;
mod keychord;
mod profile;
mod quick_connect;
pub mod recent;
pub mod recovery;
mod scheme;
mod secret_key;
pub mod shellquote;
mod size;
mod split;
pub mod ssh_config;

pub use action::{Action, Dir, Hotkeys};
pub use background::{Background, EffectiveBackground, Fit, TabBackground, effective_background};
pub use config::Config;
pub use config::route::{Jump, MAX_JUMP_HOPS, resolve_jumps};
pub use config::ssh_opts::{
    Algorithms, Backspace, EffectiveSsh, ForwardKind, LoginScript, PortForward, SshAuth, SshOpts,
};
pub use keychord::{KeyChord, KeyName, Mods, NamedKey};
pub use profile::{
    DEFAULT_LOGIN, EffectiveProfile, Group, GroupId, LocalOpts, LocalProfile, Osc52Policy, Profile,
    ProfileDefaults, ProfileId, ProfileKind, SessionEndBehavior, effective_osc52_write, resolve,
    strip_defaults,
};
pub use quick_connect::{QuickConnect, QuickConnectError};
pub use scheme::{ColorScheme, RgbColor};
pub use secret_key::SecretKey;
pub use size::Size;
pub use split::{Divider, Node, NodePath, Orientation, Rect, Side, SplitTree};
