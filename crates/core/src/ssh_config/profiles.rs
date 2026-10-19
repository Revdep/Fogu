//! Mapping a host's effective `ssh_config` options into a read-only SSH [`Profile`]. Keywords
//! with no Fogu equivalent, values we can't use and the security keywords we never obey end
//! up in [`SshOpts::ignored_ssh_config`], which the session lists in its connect-time
//! "not supported yet" warning: nothing is dropped silently.

use std::path::PathBuf;

use super::lex::Directive;
use super::{Env, SshConfig};
use crate::config::ssh_opts::{ForwardKind, PortForward, SshOpts};
use crate::profile::{Profile, ProfileId, ProfileKind};

/// Id prefix of `~/.ssh/config` profiles (`ssh-config:<alias>`).
pub const ID_PREFIX: &str = "ssh-config:";

/// Keywords whose values accumulate over every matching block instead of first-wins.
pub(super) const MULTI: &[&str] = &[
    "identityfile",
    "certificatefile",
    "localforward",
    "remoteforward",
    "dynamicforward",
    "sendenv",
    "setenv",
];

/// Ignored-option notes per host, and each note's length, are capped.
const MAX_NOTES: usize = 32;
const MAX_NOTE_CHARS: usize = 120;

impl SshConfig {
    /// One SSH profile per plain `Host` alias, in file order.
    #[must_use]
    pub fn profiles(&self, env: &Env) -> Vec<Profile> {
        self.aliases
            .iter()
            .map(|alias| self.profile(alias, env))
            .collect()
    }

    /// The read-only profile for `alias` (`ssh-config:<alias>`, named after the alias).
    #[must_use]
    pub fn profile(&self, alias: &str, env: &Env) -> Profile {
        Profile {
            id: ProfileId(format!("{ID_PREFIX}{alias}")),
            name: alias.to_owned(),
            kind: ProfileKind::Ssh(self.ssh_opts(alias, env)),
            ..Profile::default_shell()
        }
    }

    /// `alias`'s effective options as profile options (see the module docs).
    #[must_use]
    pub fn ssh_opts(&self, alias: &str, env: &Env) -> SshOpts {
        let options = self.options_for(alias);
        let mut m = Mapper {
            opts: SshOpts::default(),
            notes: Vec::new(),
        };
        let first = |key: &str| {
            options
                .iter()
                .find(|d| d.key == key)
                .and_then(|d| d.args.first())
        };
        // HostName defaults to the alias; `%h` in it is the alias.
        let host = first("hostname")
            .and_then(|name| m.hostname(name, alias))
            .unwrap_or_else(|| alias.to_owned());
        let user = first("user").cloned();
        let port = first("port").map_or(Some(22), |p| p.parse::<u16>().ok().filter(|&p| p > 0));
        let tokens = Tokens {
            env,
            alias,
            host: &host,
            user: user.as_deref().unwrap_or(&env.user),
            port: port.map_or_else(|| "22".to_owned(), |p| p.to_string()),
        };
        m.opts.host = Some(host.clone());
        m.opts.user.clone_from(&user);
        for d in options {
            m.directive(d, &tokens, self);
        }
        let mut opts = m.opts;
        opts.ignored_ssh_config = m.notes;
        opts
    }
}

/// Values for `%` tokens in `IdentityFile`.
struct Tokens<'a> {
    env: &'a Env,
    alias: &'a str,
    host: &'a str,
    user: &'a str,
    port: String,
}

struct Mapper {
    opts: SshOpts,
    notes: Vec<String>,
}

impl Mapper {
    /// Records an ignored option (control characters dropped, capped).
    fn note(&mut self, text: &str) {
        let mut clean: String = text.chars().filter(|c| !c.is_control()).collect();
        if clean.chars().count() > MAX_NOTE_CHARS {
            clean = clean.chars().take(MAX_NOTE_CHARS).collect::<String>() + "…";
        }
        if self.notes.len() < MAX_NOTES && !self.notes.contains(&clean) {
            self.notes.push(clean);
        }
    }

    /// `%%` and the given tokens; an unknown token is noted and gives `None`.
    fn expand(&mut self, text: &str, tokens: &[(char, &str)], keyword: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => out.push('%'),
                Some(t) if tokens.iter().any(|(k, _)| *k == t) => {
                    out.extend(tokens.iter().filter(|(k, _)| *k == t).map(|(_, v)| *v));
                }
                other => {
                    let token = other.map_or_else(String::new, String::from);
                    self.note(&format!("{keyword} {text} (unsupported token %{token})"));
                    return None;
                }
            }
        }
        Some(out)
    }

    /// `HostName` with `%h` = the alias; `None` (noted) for an unusable value.
    fn hostname(&mut self, name: &str, alias: &str) -> Option<String> {
        let host = self.expand(name, &[('h', alias)], "HostName")?;
        if host.is_empty() || host.chars().any(|c| c.is_control() || c.is_whitespace()) {
            self.note(&format!("HostName {name} (invalid value)"));
            return None;
        }
        Some(host)
    }

    fn directive(&mut self, d: &Directive, t: &Tokens<'_>, config: &SshConfig) {
        let arg = d.args.first().map_or("", String::as_str);
        let bad = |m: &mut Self| m.note(&format!("{} {} (invalid value)", d.name, d.raw));
        match d.key.as_str() {
            "hostname" | "user" => {}
            // Apple's OpenSSH options, in nearly every macOS config: not worth a note each.
            "usekeychain" | "addkeystoagent" if cfg!(target_os = "macos") => {}
            "port" => match arg.parse::<u16>() {
                Ok(port) if port > 0 => self.opts.port = Some(port),
                _ => bad(self),
            },
            "identityfile" => self.identity_file(arg, t),
            "identitiesonly" => self.flag(d, |o, v| o.identities_only = Some(v)),
            "forwardagent" => self.flag(d, |o, v| o.agent_forward = Some(v)),
            "connecttimeout" => match arg.parse::<u64>() {
                Ok(secs) => self.opts.ready_timeout = Some(secs.saturating_mul(1000)),
                Err(_) => bad(self),
            },
            "serveraliveinterval" => match arg.parse::<u64>() {
                Ok(secs) => self.opts.keepalive_interval = Some(secs.saturating_mul(1000)),
                Err(_) => bad(self),
            },
            "serveralivecountmax" => match arg.parse::<u32>() {
                Ok(n) => self.opts.keepalive_count_max = Some(n),
                Err(_) => bad(self),
            },
            "proxycommand" if arg.eq_ignore_ascii_case("none") => {}
            "proxycommand" => self.proxy_command(d),
            "proxyjump" => self.proxy_jump(d, config),
            "localforward" | "remoteforward" | "dynamicforward" => self.forward(d),
            "hostkeyalias"
                if !arg.is_empty() && !arg.chars().any(|c| c.is_control() || c.is_whitespace()) =>
            {
                self.opts.host_key_alias = Some(arg.to_owned());
            }
            "hostkeyalias" => bad(self),
            // Fogu never checks the server's IP against known_hosts: `no` is what it does.
            "checkhostip" if arg.eq_ignore_ascii_case("no") => {}
            "stricthostkeychecking" if arg.eq_ignore_ascii_case("ask") => {}
            "stricthostkeychecking" => {
                self.note(&format!(
                    "{} {} (host keys are still verified)",
                    d.name, d.raw
                ));
            }
            "userknownhostsfile"
                if d.args.iter().all(|a| {
                    matches!(a.as_str(), "~/.ssh/known_hosts" | "~/.ssh/known_hosts2")
                }) => {}
            "userknownhostsfile" => {
                self.note(&format!(
                    "{} {} (Fogu's known_hosts is used)",
                    d.name, d.raw
                ));
            }
            _ => self.note(&d.name),
        }
    }

    fn flag(&mut self, d: &Directive, set: impl FnOnce(&mut SshOpts, bool)) {
        match d.args.first().map(|a| a.to_ascii_lowercase()).as_deref() {
            Some("yes" | "true") => set(&mut self.opts, true),
            Some("no" | "false") => set(&mut self.opts, false),
            _ => self.note(&format!("{} {} (unsupported value)", d.name, d.raw)),
        }
    }

    fn identity_file(&mut self, arg: &str, t: &Tokens<'_>) {
        let home = t.env.home.to_string_lossy();
        let tokens = [
            ('d', &*home),
            ('u', t.env.user.as_str()),
            ('h', t.host),
            ('r', t.user),
            ('p', t.port.as_str()),
            ('n', t.alias),
        ];
        let Some(path) = self.expand(arg, &tokens, "IdentityFile") else {
            return;
        };
        let path = match path.strip_prefix("~/") {
            Some(rest) => t.env.home.join(rest),
            None if path.starts_with('~') => {
                self.note(&format!("IdentityFile {arg} (only ~/ is supported)"));
                return;
            }
            None => PathBuf::from(path),
        };
        self.opts
            .private_keys
            .get_or_insert_with(Vec::new)
            .push(path);
    }

    /// `ProxyJump` (`none`, or `[user@]host[:port]` hops separated by commas, first hop first)
    /// becomes `jump_host = "ssh-config:<value>"`, followed hop by hop by
    /// [`SshConfig::jump_opts`]; a single plain alias keeps that alias's own profile id.
    /// Unusable values are reported (and refuse to connect). `ProxyJump` and `ProxyCommand`
    /// exclude each other: the first one obtained wins, like OpenSSH.
    fn proxy_jump(&mut self, d: &Directive, config: &SshConfig) {
        let value = d.args.first().map_or("", String::as_str);
        if value.eq_ignore_ascii_case("none") || self.opts.proxy_command.is_some() {
            return;
        }
        if d.args.len() > 1 || value.split(',').any(|hop| parse_hop(hop).is_none()) {
            self.note(&format!("{} {} (invalid value)", d.name, d.raw));
            return;
        }
        let alias = config
            .aliases
            .iter()
            .find(|a| a.eq_ignore_ascii_case(value));
        let id = alias.map_or(value, String::as_str);
        self.opts.jump_host = Some(ProfileId(format!("{ID_PREFIX}{id}")));
    }

    /// `ProxyCommand` keeps the `%h %p %r %%` tokens the session substitutes; any other token
    /// is reported (and the profile refuses to connect).
    fn proxy_command(&mut self, d: &Directive) {
        if self.opts.jump_host.is_some() {
            return;
        }
        let mut chars = d.raw.chars();
        while let Some(c) = chars.next() {
            if c == '%' {
                match chars.next() {
                    Some('h' | 'p' | 'r' | '%') => {}
                    other => {
                        let token = other.map_or_else(String::new, String::from);
                        self.note(&format!(
                            "{} {} (unsupported token %{token})",
                            d.name, d.raw
                        ));
                        return;
                    }
                }
            }
        }
        self.opts.proxy_command = Some(d.raw.clone());
    }

    fn forward(&mut self, d: &Directive) {
        let kind = match d.key.as_str() {
            "localforward" => ForwardKind::Local,
            "remoteforward" => ForwardKind::Remote,
            _ => ForwardKind::Dynamic,
        };
        let forward = match (kind, d.args.as_slice()) {
            (ForwardKind::Dynamic, [listen]) => listen_on(listen).map(|(host, port)| PortForward {
                kind,
                host,
                port,
                ..PortForward::default()
            }),
            (ForwardKind::Local | ForwardKind::Remote, [listen, target]) => listen_on(listen)
                .zip(host_port(target))
                .map(|((host, port), (to, to_port))| PortForward {
                    kind,
                    host,
                    port,
                    target_address: to,
                    target_port: to_port,
                    description: String::new(),
                }),
            _ => None,
        };
        match forward {
            Some(f) => self
                .opts
                .forwarded_ports
                .get_or_insert_with(Vec::new)
                .push(f),
            None => self.note(&format!("{} {} (unsupported form)", d.name, d.raw)),
        }
    }
}

/// `[bind:]port`; the bind address defaults to loopback (OpenSSH without `GatewayPorts`).
fn listen_on(text: &str) -> Option<(String, u16)> {
    match text.parse::<u16>() {
        Ok(port) => Some(("127.0.0.1".to_owned(), port)),
        Err(_) => host_port(text),
    }
}

/// One `ProxyJump` hop: user, host, port.
pub(super) type Hop = (Option<String>, String, Option<u16>);

/// `[ssh://][user@]host[:port]` (IPv6 as `[v6]` or `[v6]:port`); `None` for anything else
/// (empty parts, blanks, controls, a host starting with `-`, paths).
pub(super) fn parse_hop(text: &str) -> Option<Hop> {
    let text = text.strip_prefix("ssh://").unwrap_or(text);
    if text.is_empty() || text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (user, rest) = match text.rsplit_once('@') {
        Some((user, rest)) => (Some(user.to_owned()), rest),
        None => (None, text),
    };
    let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?),
        };
        (host, port)
    } else {
        match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        }
    };
    let port = match port {
        Some(p) => Some(p.parse::<u16>().ok().filter(|&p| p > 0)?),
        None => None,
    };
    let bad = |s: &str| s.is_empty() || s.starts_with('-') || s.contains(['/', '[', ']', ',']);
    if bad(host) || user.as_deref().is_some_and(|u| bad(u) || u.contains('@')) {
        return None;
    }
    Some((user, host.to_owned(), port))
}

/// `host:port` or `[v6]:port`.
fn host_port(text: &str) -> Option<(String, u16)> {
    let (host, port) = match text.strip_prefix('[') {
        Some(rest) => rest.split_once("]:")?,
        None => text.rsplit_once(':')?,
    };
    let port = port.parse::<u16>().ok()?;
    (!host.is_empty() && !host.contains(['/', '[', ']'])).then(|| (host.to_owned(), port))
}
