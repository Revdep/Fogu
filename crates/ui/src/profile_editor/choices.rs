//! The options of the editor's drop-downs: label + value, in display order.

use fogu_core::{Backspace, Fit, ForwardKind, Osc52Policy, SessionEndBehavior, SshAuth};

/// Choice options: label + value (`None` = inherit / unset).
pub const SESSION_END: [(&str, SessionEndBehavior); 5] = [
    ("Close if clean", SessionEndBehavior::CloseIfClean),
    ("Keep open", SessionEndBehavior::Keep),
    ("Always close", SessionEndBehavior::Close),
    ("Auto", SessionEndBehavior::Auto),
    ("Reconnect", SessionEndBehavior::Reconnect),
];
pub const OSC52: [(&str, Option<Osc52Policy>); 4] = [
    ("As in [terminal]", None),
    ("Ask", Some(Osc52Policy::Ask)),
    ("Allow", Some(Osc52Policy::Allow)),
    ("Deny", Some(Osc52Policy::Deny)),
];
pub const AUTH: [(&str, SshAuth); 5] = [
    ("Auto (agent, keys, password, keyboard)", SshAuth::Auto),
    ("Password", SshAuth::Password),
    ("Public key", SshAuth::PublicKey),
    ("ssh-agent", SshAuth::Agent),
    ("Keyboard-interactive", SshAuth::KeyboardInteractive),
];
pub const BACKSPACE: [(&str, Backspace); 4] = [
    ("Backspace (^?)", Backspace::Backspace),
    ("Ctrl+H (^H)", Backspace::CtrlH),
    ("Ctrl+? (^?)", Backspace::CtrlQuestion),
    ("Delete (ESC [3~)", Backspace::Delete),
];
pub const FORWARD_KIND: [(&str, ForwardKind); 3] = [
    ("Local", ForwardKind::Local),
    ("Remote", ForwardKind::Remote),
    ("Dynamic", ForwardKind::Dynamic),
];
pub const BG_KIND: [&str; 3] = ["None", "Colour", "Image"];
pub const BG_FIT: [(&str, Fit); 3] = [
    ("Cover", Fit::Cover),
    ("Contain", Fit::Contain),
    ("Tile", Fit::Tile),
];
