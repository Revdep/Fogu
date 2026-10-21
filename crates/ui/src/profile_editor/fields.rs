//! [`FieldId`] → profile field: the editor's reducer. Every form widget reports a
//! `Field(FieldId, FieldValue)`; [`ProfileEditor::apply`] writes it into the draft (one `match`).
//! Number and colour fields keep the typed text and an error until it parses.

use std::path::PathBuf;
use std::str::FromStr;

use fogu_core::{
    Algorithms, Background, Fit, ForwardKind, LoginScript, PortForward, ProfileKind, RgbColor,
    SshOpts,
};

use super::ProfileEditor;
pub use super::choices::*;
use crate::forms::FieldValue;

/// Every editable field of a profile (common, local, SSH); list rows carry their index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldId {
    // Common.
    Name,
    Group,
    /// The name typed after picking "New group…" in Group (Enter = `FieldValue::Add`).
    NewGroup,
    Icon,
    Color,
    DisableDynamicTitle,
    RestoreScrollback,
    SessionEnd,
    Osc52,
    BgKind,
    BgColor,
    BgImage,
    BgFit,
    BgOpacity,
    // Local.
    Program,
    Args,
    LocalCwd,
    Env,
    Login,
    // SSH: connection.
    Host,
    Port,
    User,
    Term,
    KeepaliveInterval,
    KeepaliveCountMax,
    ReadyTimeout,
    PromptTimeout,
    SkipBanner,
    ClearServiceMessages,
    VerifyHostKeys,
    WarnOnClose,
    Weight,
    ReuseSession,
    ReuseLinger,
    // SSH: authentication.
    Auth,
    StoredSecrets,
    PrivateKeys,
    Password,
    /// The stored passphrase of private key row `.0`.
    KeyPassphrase(usize),
    AgentPath,
    IdentitiesOnly,
    HostKeyAlias,
    AgentKeyLifetime,
    // SSH: network.
    JumpHost,
    ProxyCommand,
    SocksHost,
    SocksPort,
    SocksRemoteDns,
    HttpHost,
    HttpPort,
    AgentForward,
    X11,
    Forwards,
    Forward(usize, ForwardCol),
    // SSH: algorithms, scripts, terminal.
    Alg(AlgKind),
    Scripts,
    Script(usize, ScriptCol),
    RemoteCwd,
    RememberCwd,
    Backspace,
}

/// A column of the port forwards table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ForwardCol {
    Kind,
    Host,
    Port,
    Target,
    TargetPort,
    Description,
}

/// A column of the login scripts table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScriptCol {
    Expect,
    Send,
    IsRegex,
    Optional,
}

/// An algorithm category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AlgKind {
    Kex,
    Cipher,
    Hmac,
    HostKey,
    Compression,
}

/// The colour a new solid background starts with (dark grey).
const NEW_BG_COLOR: u32 = 0x20_2020;

/// What the app must do after a field changed.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Open a file picker for this field.
    Browse(FieldId),
}

/// A number field: its range (inclusive) as `i64`.
fn range(id: FieldId) -> Option<(i64, i64)> {
    let ms_day = 86_400_000;
    Some(match id {
        FieldId::Port
        | FieldId::SocksPort
        | FieldId::HttpPort
        | FieldId::Forward(_, ForwardCol::Port) => (1, 65_535),
        FieldId::Forward(_, ForwardCol::TargetPort) => (0, 65_535),
        FieldId::KeepaliveInterval | FieldId::PromptTimeout => (0, ms_day),
        FieldId::ReadyTimeout => (1, ms_day),
        FieldId::KeepaliveCountMax => (0, 10_000),
        FieldId::AgentKeyLifetime => (0, i64::from(u32::MAX)),
        FieldId::ReuseLinger => (0, 3600),
        FieldId::Weight => (i64::from(i32::MIN), i64::from(i32::MAX)),
        FieldId::BgOpacity => (0, 100),
        _ => return None,
    })
}

/// Where a −/+ press starts on an empty field: the built-in default the field shows (a
/// `[profile_defaults]` override is not taken into account), else the range's low end.
fn step_start(id: FieldId, lo: i64) -> i64 {
    use fogu_core::config::ssh_opts::{
        DEFAULT_AGENT_KEY_LIFETIME_S, DEFAULT_KEEPALIVE_COUNT_MAX, DEFAULT_KEEPALIVE_INTERVAL_MS,
        DEFAULT_PROMPT_TIMEOUT_MS, DEFAULT_REUSE_LINGER_S, DEFAULT_SSH_PORT,
    };
    match id {
        FieldId::Port => i64::from(DEFAULT_SSH_PORT),
        FieldId::SocksPort => 1080,
        FieldId::HttpPort => 8080,
        FieldId::KeepaliveInterval => i64::try_from(DEFAULT_KEEPALIVE_INTERVAL_MS).unwrap_or(lo),
        FieldId::KeepaliveCountMax => i64::from(DEFAULT_KEEPALIVE_COUNT_MAX),
        FieldId::PromptTimeout => i64::try_from(DEFAULT_PROMPT_TIMEOUT_MS).unwrap_or(lo),
        FieldId::ReadyTimeout => 20_000,
        FieldId::AgentKeyLifetime => i64::from(DEFAULT_AGENT_KEY_LIFETIME_S),
        FieldId::ReuseLinger => i64::from(DEFAULT_REUSE_LINGER_S),
        FieldId::Weight => 0,
        FieldId::BgOpacity => 85,
        _ => lo,
    }
}

fn opt(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// `items` with row `i` moved one up / removed.
fn list_op<T: Default>(items: &mut Vec<T>, value: &FieldValue) {
    match *value {
        FieldValue::Add => items.push(T::default()),
        FieldValue::Remove(i) if i < items.len() => {
            items.remove(i);
        }
        FieldValue::Up(i) if i > 0 && i < items.len() => items.swap(i - 1, i),
        _ => {}
    }
}

impl ProfileEditor {
    /// Applies one widget event to the draft. Ignored in a read-only editor.
    pub fn apply(&mut self, id: FieldId, value: FieldValue) -> Effect {
        if self.read_only() {
            return Effect::None;
        }
        if value == FieldValue::Browse {
            return Effect::Browse(id);
        }
        if let Some((lo, hi)) = range(id) {
            match &value {
                FieldValue::Text(text) => {
                    if let Ok(number) = self.number(id, text, (lo, hi)) {
                        self.set_number(id, number);
                    }
                    return Effect::None;
                }
                FieldValue::Step(by) => {
                    // An inherited (empty) field steps from the built-in default it shows.
                    let current = self
                        .current_number(id)
                        .unwrap_or_else(|| step_start(id, lo));
                    let number = current.saturating_add(i64::from(*by)).clamp(lo, hi);
                    self.texts.remove(&id);
                    self.errors.remove(&id);
                    self.set_number(id, Some(number));
                    return Effect::None;
                }
                _ => {}
            }
        }
        if self.apply_common(id, &value) || self.apply_local(id, &value) {
            return Effect::None;
        }
        if let ProfileKind::Ssh(ssh) = &mut self.draft.kind {
            apply_ssh(ssh, id, value, &self.jumps);
        }
        Effect::None
    }

    /// Parses `text` for number field `id`, remembering the text and any error (`Err`).
    /// `Ok(None)` = the field was cleared (inherit).
    fn number(&mut self, id: FieldId, text: &str, (lo, hi): (i64, i64)) -> Result<Option<i64>, ()> {
        self.texts.insert(id, text.to_owned());
        self.errors.remove(&id);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        match i64::from_str(trimmed) {
            Ok(n) if (lo..=hi).contains(&n) => Ok(Some(n)),
            _ => {
                self.errors
                    .insert(id, format!("a whole number from {lo} to {hi}"));
                Err(())
            }
        }
    }

    fn set_number(&mut self, id: FieldId, n: Option<i64>) {
        let u16_ = |n: Option<i64>| n.and_then(|n| u16::try_from(n).ok());
        let u64_ = |n: Option<i64>| n.and_then(|n| u64::try_from(n).ok());
        if id == FieldId::BgOpacity {
            if let Some(Background::Image { opacity, .. }) = &mut self.draft.background {
                // Percent; cleared = the default opacity.
                *opacity = n.map_or(0.85, |p| f32::from(u8::try_from(p).unwrap_or(100)) / 100.0);
            }
            return;
        }
        let ProfileKind::Ssh(ssh) = &mut self.draft.kind else {
            return;
        };
        match id {
            FieldId::Port => ssh.port = u16_(n),
            FieldId::SocksPort => ssh.socks_proxy_port = u16_(n),
            FieldId::HttpPort => ssh.http_proxy_port = u16_(n),
            FieldId::KeepaliveInterval => ssh.keepalive_interval = u64_(n),
            FieldId::PromptTimeout => ssh.prompt_timeout = u64_(n),
            FieldId::ReadyTimeout => ssh.ready_timeout = u64_(n),
            FieldId::KeepaliveCountMax => {
                ssh.keepalive_count_max = n.and_then(|n| u32::try_from(n).ok());
            }
            FieldId::AgentKeyLifetime => {
                ssh.agent_key_lifetime = n.and_then(|n| u32::try_from(n).ok());
            }
            FieldId::ReuseLinger => ssh.reuse_linger = n.and_then(|n| u32::try_from(n).ok()),
            FieldId::Weight => ssh.weight = n.and_then(|n| i32::try_from(n).ok()),
            FieldId::Forward(row, col) => {
                if let Some(forward) = ssh.forwarded_ports.as_mut().and_then(|f| f.get_mut(row)) {
                    let port = u16_(n).unwrap_or(0);
                    match col {
                        ForwardCol::Port => forward.port = port,
                        ForwardCol::TargetPort => forward.target_port = port,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// The explicit value behind a number field (`None` means inherited / empty).
    fn current_number(&self, id: FieldId) -> Option<i64> {
        if id == FieldId::BgOpacity {
            return match &self.draft.background {
                Some(Background::Image { opacity, .. }) => Some((opacity * 100.0).round() as i64),
                _ => None,
            };
        }
        let ProfileKind::Ssh(ssh) = &self.draft.kind else {
            return None;
        };
        match id {
            FieldId::Port => ssh.port.map(i64::from),
            FieldId::SocksPort => ssh.socks_proxy_port.map(i64::from),
            FieldId::HttpPort => ssh.http_proxy_port.map(i64::from),
            FieldId::KeepaliveInterval => ssh.keepalive_interval.and_then(|n| n.try_into().ok()),
            FieldId::PromptTimeout => ssh.prompt_timeout.and_then(|n| n.try_into().ok()),
            FieldId::ReadyTimeout => ssh.ready_timeout.and_then(|n| n.try_into().ok()),
            FieldId::KeepaliveCountMax => ssh.keepalive_count_max.map(i64::from),
            FieldId::AgentKeyLifetime => ssh.agent_key_lifetime.map(i64::from),
            FieldId::ReuseLinger => ssh.reuse_linger.map(i64::from),
            FieldId::Weight => ssh.weight.map(i64::from),
            FieldId::Forward(row, col) => ssh
                .forwarded_ports
                .as_ref()
                .and_then(|forwards| forwards.get(row))
                .and_then(|forward| match col {
                    ForwardCol::Port => Some(i64::from(forward.port)),
                    ForwardCol::TargetPort => Some(i64::from(forward.target_port)),
                    _ => None,
                }),
            _ => None,
        }
    }

    /// Fields every profile has; `false` when `id` is not one of them.
    fn apply_common(&mut self, id: FieldId, value: &FieldValue) -> bool {
        let p = &mut self.draft;
        match (id, value) {
            (FieldId::Name, FieldValue::Text(t)) => p.name.clone_from(t),
            (FieldId::Icon, FieldValue::Text(t)) => p.icon = opt(t),
            (FieldId::Group, FieldValue::Choice(i)) => {
                p.group = self.groups.get(*i).and_then(|(g, _)| g.clone());
            }
            (FieldId::Color, FieldValue::Color(c)) => {
                p.color = *c;
                self.texts.remove(&id);
                self.errors.remove(&id);
            }
            (FieldId::Color, FieldValue::Text(t)) => {
                let current = p.color;
                self.draft.color = self.hex(id, t, current);
            }
            (FieldId::DisableDynamicTitle, FieldValue::Bool(b)) => p.disable_dynamic_title = *b,
            (FieldId::RestoreScrollback, FieldValue::Bool(b)) => p.restore_scrollback = Some(*b),
            (FieldId::SessionEnd, FieldValue::Choice(i)) => {
                p.behavior_on_session_end =
                    SESSION_END.get(*i).map_or_else(Default::default, |c| c.1);
            }
            (FieldId::Osc52, FieldValue::Choice(i)) => {
                let policy = OSC52.get(*i).and_then(|c| c.1);
                match &mut p.kind {
                    ProfileKind::Local(local) => local.osc52_write = policy,
                    ProfileKind::Ssh(ssh) => ssh.osc52_write = policy,
                }
            }
            (FieldId::BgKind | FieldId::BgColor | FieldId::BgImage | FieldId::BgFit, _) => {
                self.apply_background(id, value);
            }
            _ => return false,
        }
        true
    }

    /// `text` as `#rrggbb` (empty = no colour); an unparsable text keeps `current` and marks the
    /// field.
    fn hex(&mut self, id: FieldId, text: &str, current: Option<RgbColor>) -> Option<RgbColor> {
        self.texts.insert(id, text.to_owned());
        self.errors.remove(&id);
        if text.trim().is_empty() {
            return None;
        }
        if let Ok(color) = RgbColor::parse_hex(text.trim()) {
            return Some(color);
        }
        self.errors.insert(id, "a colour like #30a46c".to_owned());
        current
    }

    fn apply_background(&mut self, id: FieldId, value: &FieldValue) {
        let bg = &mut self.draft.background;
        match (id, value) {
            (FieldId::BgKind, FieldValue::Choice(0)) => *bg = None,
            (FieldId::BgKind, FieldValue::Choice(1))
                if !matches!(bg, Some(Background::Color(_))) =>
            {
                *bg = Some(Background::Color(RgbColor::from_hex(NEW_BG_COLOR)));
            }
            (FieldId::BgKind, FieldValue::Choice(2))
                if !matches!(bg, Some(Background::Image { .. })) =>
            {
                *bg = Some(Background::Image {
                    path: PathBuf::new(),
                    opacity: 0.85,
                    fit: Fit::Cover,
                    blur: false,
                });
            }
            (FieldId::BgColor, FieldValue::Color(Some(c))) => *bg = Some(Background::Color(*c)),
            (FieldId::BgColor, FieldValue::Text(t)) => {
                let current = match bg {
                    Some(Background::Color(c)) => Some(*c),
                    _ => None,
                };
                if let Some(c) = self.hex(id, t, current) {
                    self.draft.background = Some(Background::Color(c));
                }
            }
            (FieldId::BgImage, FieldValue::Text(t)) => set_image(bg, PathBuf::from(t.trim())),
            (FieldId::BgImage, FieldValue::Picked(path)) => set_image(bg, path.clone()),
            (FieldId::BgFit, FieldValue::Choice(i)) => {
                if let (Some(Background::Image { fit, .. }), Some((_, f))) = (bg, BG_FIT.get(*i)) {
                    *fit = *f;
                }
            }
            _ => {}
        }
        let empty_image = matches!(&self.draft.background, Some(Background::Image { path, .. }) if path.as_os_str().is_empty());
        if empty_image {
            self.errors
                .insert(FieldId::BgImage, "choose an image file".to_owned());
        } else {
            self.errors.remove(&FieldId::BgImage);
        }
    }

    /// Local fields; `false` when `id` is not one of them (or the draft is not local).
    fn apply_local(&mut self, id: FieldId, value: &FieldValue) -> bool {
        let ProfileKind::Local(local) = &mut self.draft.kind else {
            return false;
        };
        match (id, value) {
            (FieldId::Program, FieldValue::Text(t)) => local.program = opt(t),
            (FieldId::LocalCwd, FieldValue::Text(t)) => local.cwd = opt(t).map(PathBuf::from),
            (FieldId::LocalCwd, FieldValue::Picked(path)) => local.cwd = Some(path.clone()),
            (FieldId::Login, FieldValue::Bool(b)) => local.login = Some(*b),
            (FieldId::Args, FieldValue::Item(i, t)) => {
                if let Some(arg) = local.args.get_or_insert_default().get_mut(*i) {
                    arg.clone_from(t);
                }
            }
            (FieldId::Args, op) => list_op(local.args.get_or_insert_default(), op),
            (FieldId::Env, _) => {
                match value {
                    FieldValue::Item(i, t) => {
                        if let Some(row) = self.env_rows.get_mut(*i) {
                            row.0.clone_from(t);
                        }
                    }
                    FieldValue::Value(i, t) => {
                        if let Some(row) = self.env_rows.get_mut(*i) {
                            row.1.clone_from(t);
                        }
                    }
                    op => list_op(&mut self.env_rows, op),
                }
                let env = self
                    .env_rows
                    .iter()
                    .filter(|(k, _)| !k.trim().is_empty())
                    .map(|(k, v)| (k.trim().to_owned(), v.clone()))
                    .collect();
                local.env = Some(env);
            }
            _ => return false,
        }
        true
    }
}

fn set_image(bg: &mut Option<Background>, new: PathBuf) {
    if let Some(Background::Image { path, .. }) = bg {
        *path = new;
    }
}

/// SSH fields (numbers are handled by `set_number`).
fn apply_ssh(
    ssh: &mut SshOpts,
    id: FieldId,
    value: FieldValue,
    jumps: &[(Option<fogu_core::ProfileId>, String)],
) {
    match (id, value) {
        (FieldId::Host, FieldValue::Text(t)) => ssh.host = opt(&t),
        (FieldId::User, FieldValue::Text(t)) => ssh.user = opt(&t),
        (FieldId::Term, FieldValue::Text(t)) => ssh.term = opt(&t),
        (FieldId::HostKeyAlias, FieldValue::Text(t)) => ssh.host_key_alias = opt(&t),
        (FieldId::ProxyCommand, FieldValue::Text(t)) => ssh.proxy_command = opt(&t),
        (FieldId::SocksHost, FieldValue::Text(t)) => ssh.socks_proxy_host = opt(&t),
        (FieldId::HttpHost, FieldValue::Text(t)) => ssh.http_proxy_host = opt(&t),
        (FieldId::RemoteCwd, FieldValue::Text(t)) => ssh.cwd = opt(&t),
        (FieldId::AgentPath, FieldValue::Text(t)) => ssh.agent_path = opt(&t).map(PathBuf::from),
        (FieldId::AgentPath, FieldValue::Picked(path)) => ssh.agent_path = Some(path),
        (FieldId::SkipBanner, FieldValue::Bool(b)) => ssh.skip_banner = Some(b),
        (FieldId::StoredSecrets, FieldValue::Bool(b)) => ssh.stored_secrets = Some(b),
        (FieldId::ClearServiceMessages, FieldValue::Bool(b)) => {
            ssh.clear_service_messages_on_connect = Some(b);
        }
        (FieldId::VerifyHostKeys, FieldValue::Bool(b)) => ssh.verify_host_keys = Some(b),
        (FieldId::WarnOnClose, FieldValue::Bool(b)) => ssh.warn_on_close = Some(b),
        (FieldId::ReuseSession, FieldValue::Bool(b)) => ssh.reuse_session = Some(b),
        (FieldId::IdentitiesOnly, FieldValue::Bool(b)) => ssh.identities_only = Some(b),
        (FieldId::SocksRemoteDns, FieldValue::Bool(b)) => ssh.socks_proxy_remote_dns = Some(b),
        (FieldId::AgentForward, FieldValue::Bool(b)) => ssh.agent_forward = Some(b),
        (FieldId::X11, FieldValue::Bool(b)) => ssh.x11 = Some(b),
        (FieldId::RememberCwd, FieldValue::Bool(b)) => ssh.remember_cwd = Some(b),
        (FieldId::Auth, FieldValue::Choice(i)) => ssh.auth = AUTH.get(i).map(|c| c.1),
        (FieldId::Backspace, FieldValue::Choice(i)) => {
            ssh.input_backspace = BACKSPACE.get(i).map(|c| c.1);
        }
        (FieldId::JumpHost, FieldValue::Choice(i)) => {
            ssh.jump_host = jumps.get(i).and_then(|(j, _)| j.clone());
        }
        (FieldId::PrivateKeys, FieldValue::Item(i, t)) => {
            if let Some(key) = ssh.private_keys.get_or_insert_default().get_mut(i) {
                *key = PathBuf::from(t);
            }
        }
        (FieldId::PrivateKeys, FieldValue::Picked(path)) => {
            ssh.private_keys.get_or_insert_default().push(path);
        }
        (FieldId::PrivateKeys, op) => list_op(ssh.private_keys.get_or_insert_default(), &op),
        (FieldId::Forwards, FieldValue::Add) => {
            ssh.forwarded_ports
                .get_or_insert_default()
                .push(PortForward {
                    host: "127.0.0.1".to_owned(),
                    ..PortForward::default()
                });
        }
        (FieldId::Forwards, op) => list_op(ssh.forwarded_ports.get_or_insert_default(), &op),
        (FieldId::Forward(row, col), value) => {
            if let Some(f) = ssh.forwarded_ports.as_mut().and_then(|f| f.get_mut(row)) {
                apply_forward(f, col, value);
            }
        }
        (FieldId::Scripts, op) => list_op(ssh.scripts.get_or_insert_default(), &op),
        (FieldId::Script(row, col), value) => {
            if let Some(s) = ssh.scripts.as_mut().and_then(|s| s.get_mut(row)) {
                apply_script(s, col, value);
            }
        }
        (FieldId::Alg(kind), FieldValue::Toggle(name)) => {
            let list = alg_list(ssh.algorithms.get_or_insert_default(), kind);
            match list.iter().position(|n| *n == name) {
                Some(at) => {
                    list.remove(at);
                }
                None => list.push(name),
            }
        }
        // `Password` / `KeyPassphrase`: Set… / Clear are handled by the app (`stored.rs`).
        _ => {}
    }
}

fn apply_forward(f: &mut PortForward, col: ForwardCol, value: FieldValue) {
    match (col, value) {
        (ForwardCol::Kind, FieldValue::Choice(i)) => {
            f.kind = FORWARD_KIND.get(i).map_or(ForwardKind::Local, |c| c.1);
        }
        (ForwardCol::Host, FieldValue::Text(t)) => f.host = t,
        (ForwardCol::Target, FieldValue::Text(t)) => f.target_address = t,
        (ForwardCol::Description, FieldValue::Text(t)) => f.description = t,
        _ => {}
    }
}

fn apply_script(s: &mut LoginScript, col: ScriptCol, value: FieldValue) {
    match (col, value) {
        (ScriptCol::Expect, FieldValue::Text(t)) => s.expect = t,
        (ScriptCol::Send, FieldValue::Text(t)) => s.send = t,
        (ScriptCol::IsRegex, FieldValue::Bool(b)) => s.is_regex = b,
        (ScriptCol::Optional, FieldValue::Bool(b)) => s.optional = b,
        _ => {}
    }
}

/// The list of category `kind`.
pub fn alg_list(algs: &mut Algorithms, kind: AlgKind) -> &mut Vec<String> {
    match kind {
        AlgKind::Kex => &mut algs.kex,
        AlgKind::Cipher => &mut algs.cipher,
        AlgKind::Hmac => &mut algs.hmac,
        AlgKind::HostKey => &mut algs.server_host_key,
        AlgKind::Compression => &mut algs.compression,
    }
}
