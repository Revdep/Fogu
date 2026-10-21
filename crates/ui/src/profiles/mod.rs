//! The profile manager sidebar: search, quick connect, Recent, the groups tree and Built-ins, with
//! mouse-first row actions. This file holds the state and the pure model (which profiles exist,
//! which lines are shown); drawing is in `view.rs`, App wiring in `handlers.rs`, the row menu in
//! `menu.rs`.
//!
//! Every profile shown comes from [`entries`]: built-ins, stored profiles, the read-only
//! `~/.ssh/config` hosts (`ssh_config.rs`), then quick-connect profiles remembered in Recent.
//! The menu limits editing to [`Origin::Stored`].

mod handlers;
mod menu;
pub(crate) mod ssh_config;
pub(crate) mod view;

use std::collections::BTreeSet;
use std::path::PathBuf;

use fogu_core::recent::Recent;
use fogu_core::{
    Config, EffectiveProfile, GroupId, Profile, ProfileId, ProfileKind, QuickConnect,
    QuickConnectError, SshOpts, resolve,
};
use fogu_term::sanitize_title;
use iced::Point;

use crate::fuzzy_list::{self, FuzzyRow, Mark};

pub use handlers::{Launch, ProfilesMsg};
pub use ssh_config::SshHosts;

/// Id of the search field.
pub const SEARCH_INPUT: &str = "fogu-profiles-search";
/// Id of the quick-connect field.
pub const QUICK_INPUT: &str = "fogu-profiles-quick";
/// Id prefix of quick-connect profiles (`quick:user@host:port`).
pub const QUICK_PREFIX: &str = "quick:";
/// Narrowest sidebar, logical px.
pub const MIN_WIDTH: f32 = 180.0;

/// Where a profile comes from; only stored ones can be edited, moved or deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// `[[profiles]]` in config.toml.
    Stored,
    /// "Default shell" and `/etc/shells`.
    Builtin,
    /// A quick-connect string remembered in Recent.
    Quick,
    /// A `Host` of `~/.ssh/config` (read-only).
    SshConfig,
}

/// One profile the sidebar or picker can show.
#[derive(Clone, Debug)]
pub struct Entry {
    pub profile: Profile,
    pub origin: Origin,
    /// `user@host[:port]` or the program.
    pub subtitle: String,
}

/// A collapsible part of the tree.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Recent,
    Group(GroupId),
    Ungrouped,
    SshConfig,
    Builtins,
}

/// One line of the sidebar list.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Header {
        section: Section,
        label: String,
        count: usize,
        collapsed: bool,
    },
    /// Entry index into [`entries`].
    Item(usize),
}

/// The sidebar's state.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags of one sidebar (open, keyboard, pointer, drag)"
)]
pub struct ProfilesView {
    pub open: bool,
    /// The sidebar has the keyboard (last click was inside it, or it was just opened).
    pub focused: bool,
    pub hovered: bool,
    /// The row under the pointer (its detail turns into Split / Edit buttons).
    pub hovered_row: Option<usize>,
    pub query: String,
    pub quick: String,
    /// Why the last quick connect was refused.
    pub quick_error: Option<String>,
    /// Sections drawn as a header only (all expanded by default).
    pub collapsed: BTreeSet<Section>,
    /// Position among the shown items.
    pub selected: usize,
    pub width: f32,
    /// Pointer position over the sidebar (window coordinates): the row menu's anchor.
    pub cursor: Point,
    /// The resize handle is being dragged.
    pub resizing: bool,
    /// The next mouse press launched a profile from a row: it doesn't focus the sidebar.
    pub skip_press: bool,
    /// The profile the open row menu acts on.
    pub menu_target: Option<ProfileId>,
    /// A stored profile waiting for the delete confirmation.
    pub pending_delete: Option<ProfileId>,
    pub recent: Recent,
    /// `$XDG_STATE_HOME/fogu/recent.toml`; `None` = don't persist (tests, no state dir).
    pub recent_path: Option<PathBuf>,
    /// "Default shell" + `/etc/shells`, read once at startup.
    pub builtins: Vec<Profile>,
    /// `~/.ssh/config` hosts.
    pub ssh_hosts: SshHosts,
}

impl ProfilesView {
    pub fn new(width: f32, builtins: Vec<Profile>) -> Self {
        Self {
            open: false,
            focused: false,
            hovered: false,
            hovered_row: None,
            query: String::new(),
            quick: String::new(),
            quick_error: None,
            collapsed: BTreeSet::new(),
            selected: 0,
            width: width.max(MIN_WIDTH),
            cursor: Point::ORIGIN,
            resizing: false,
            skip_press: false,
            menu_target: None,
            pending_delete: None,
            recent: Recent::default(),
            recent_path: None,
            builtins,
            ssh_hosts: SshHosts::default(),
        }
    }
}

/// Built-ins (when shown), stored profiles, `~/.ssh/config` hosts not copied to them, then
/// quick-connect profiles from `recent`.
pub fn entries(
    config: &Config,
    builtins: &[Profile],
    ssh_hosts: &[Profile],
    recent: Option<&Recent>,
) -> Vec<Entry> {
    let entry = |profile: Profile, origin| Entry {
        subtitle: if origin == Origin::Quick {
            "quick connect".to_owned()
        } else {
            subtitle(&profile, config)
        },
        profile,
        origin,
    };
    let shown_builtins = builtins
        .iter()
        .filter(|_| config.appearance.show_builtin_profiles)
        .map(|p| entry(p.clone(), Origin::Builtin));
    // Ascending `weight`; a stable sort keeps file order among equal weights.
    let mut by_weight: Vec<&Profile> = config.profiles.iter().collect();
    by_weight.sort_by_key(|p| match resolve(p, &config.profile_defaults) {
        EffectiveProfile::Ssh(ssh) => ssh.weight,
        EffectiveProfile::Local(_) => 0,
    });
    let stored = by_weight
        .into_iter()
        .map(|p| entry(p.clone(), Origin::Stored));
    // A host already copied to the stored profiles (same name) is listed once, as the copy: the
    // one the user edits.
    let key = |name: &str| name.trim().to_lowercase();
    let copied = |host: &Profile| {
        config
            .profiles
            .iter()
            .any(|p| key(&p.name) == key(&host.name))
    };
    let from_ssh_config = ssh_hosts
        .iter()
        .filter(|host| !copied(host))
        .map(|p| entry(p.clone(), Origin::SshConfig));
    let quick = recent
        .into_iter()
        .flat_map(|r| &r.recent)
        .filter_map(|e| e.id.0.strip_prefix(QUICK_PREFIX))
        .filter_map(|s| quick_profile(s).ok())
        .map(|p| entry(p, Origin::Quick));
    shown_builtins
        .chain(stored)
        .chain(from_ssh_config)
        .chain(quick)
        .collect()
}

/// `user@host[:port]` for SSH, the program (or "local shell") for local profiles.
pub fn subtitle(profile: &Profile, config: &Config) -> String {
    let text = match resolve(profile, &config.profile_defaults) {
        EffectiveProfile::Ssh(ssh) if ssh.port == 22 => format!("{}@{}", ssh.user, ssh.host),
        EffectiveProfile::Ssh(ssh) => format!("{}@{}:{}", ssh.user, ssh.host, ssh.port),
        EffectiveProfile::Local(local) => local.program.unwrap_or_else(|| "local shell".into()),
    };
    sanitize_title(&text)
}

/// The ephemeral SSH profile for a quick-connect string; its id is the normalised string, so
/// connecting twice to the same address is one Recent entry.
pub fn quick_profile(input: &str) -> Result<Profile, QuickConnectError> {
    let qc = QuickConnect::parse(input)?;
    let mut name = qc
        .user
        .as_ref()
        .map_or_else(String::new, |u| format!("{u}@"));
    name.push_str(&qc.host);
    if let Some(port) = qc.port {
        name = format!("{name}:{port}");
    }
    Ok(Profile {
        id: ProfileId(format!("{QUICK_PREFIX}{name}")),
        name: sanitize_title(&name),
        kind: ProfileKind::Ssh(SshOpts {
            host: Some(qc.host),
            port: qc.port,
            user: qc.user,
            ..SshOpts::default()
        }),
        ..Profile::default_shell()
    })
}

/// `ssh -p <port> user@host` when user and host are plain (letters, digits, `.`, `_`, `-`, not
/// starting with `-`), so pasting it into a shell can't run anything else.
pub fn ssh_command(profile: &Profile, config: &Config) -> Option<String> {
    let EffectiveProfile::Ssh(ssh) = resolve(profile, &config.profile_defaults) else {
        return None;
    };
    let plain = |s: &str| {
        !s.is_empty()
            && !s.starts_with('-')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    (plain(&ssh.user) && plain(&ssh.host))
        .then(|| format!("ssh -p {} {}@{}", ssh.port, ssh.user, ssh.host))
}

/// `icon` when it is a short glyph/emoji string (icon-font names like `fas fa-server` are
/// not shown).
pub fn glyph(icon: Option<&str>) -> Option<String> {
    let icon = icon?.trim();
    let ok = !icon.is_empty()
        && icon.chars().count() <= 8
        && icon
            .chars()
            .all(|c| !c.is_ascii_alphanumeric() && !c.is_control() && !c.is_whitespace());
    ok.then(|| icon.to_owned())
}

/// "just now", "5 min ago", "3 h ago", "2 d ago".
pub fn relative_time(now: u64, then: u64) -> String {
    match now.saturating_sub(then) {
        s if s < 60 => "just now".to_owned(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86_400),
    }
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What each entry looks like in the fuzzy list (and what is searched: name, `user@host`,
/// group name).
pub fn rows(entries: &[Entry], config: &Config, recent: &Recent, now: u64) -> Vec<FuzzyRow> {
    entries
        .iter()
        .map(|e| {
            let p = &e.profile;
            let mark = match p.kind {
                ProfileKind::Ssh(_) => Mark::Ring(crate::tokens::A700),
                ProfileKind::Local(_) => Mark::Ring(crate::tokens::N700),
            };
            let group = p
                .group
                .as_ref()
                .and_then(|g| config.groups.iter().find(|x| &x.id == g))
                .map(|g| g.name.clone());
            FuzzyRow {
                title: sanitize_title(&p.name),
                subtitle: e.subtitle.clone(),
                detail: recent
                    .last_used(&p.id)
                    .map(|t| relative_time(now, t))
                    .unwrap_or_default(),
                keywords: group
                    .into_iter()
                    .chain((e.origin == Origin::SshConfig).then(|| "ssh config".to_owned()))
                    .collect(),
                mark,
                icon: glyph(p.icon.as_deref()),
            }
        })
        .collect()
}

/// The sidebar's lines: with a query, the ranked matches; else Recent (up to
/// `appearance.recent_profiles`), each group, Ungrouped, SSH config and Built-ins, collapsed ones as a
/// header only. Empty sections other than groups are left out.
pub fn lines(
    view: &ProfilesView,
    config: &Config,
    entries: &[Entry],
    rows: &[FuzzyRow],
) -> Vec<Line> {
    if !view.query.trim().is_empty() {
        return fuzzy_list::rank(&view.query, rows)
            .into_iter()
            .map(Line::Item)
            .collect();
    }
    let index_of = |id: &ProfileId| entries.iter().position(|e| &e.profile.id == id);
    let recent: Vec<usize> = view
        .recent
        .recent
        .iter()
        .filter_map(|r| index_of(&r.id))
        .take(config.appearance.recent_profiles)
        .collect();
    let stored = |pred: &dyn Fn(&Profile) -> bool| -> Vec<usize> {
        (0..entries.len())
            .filter(|&i| {
                entries
                    .get(i)
                    .is_some_and(|e| e.origin == Origin::Stored && pred(&e.profile))
            })
            .collect()
    };
    let known = |g: &GroupId| config.groups.iter().any(|x| &x.id == g);
    let mut sections = vec![(Section::Recent, "Recent".to_owned(), recent)];
    for group in &config.groups {
        let items = stored(&|p| p.group.as_ref() == Some(&group.id));
        sections.push((
            Section::Group(group.id.clone()),
            sanitize_title(&group.name),
            items,
        ));
    }
    let ungrouped = stored(&|p| !p.group.as_ref().is_some_and(known));
    sections.push((Section::Ungrouped, "Ungrouped".to_owned(), ungrouped));
    let of = |origin| {
        (0..entries.len())
            .filter(|&i| entries.get(i).is_some_and(|e| e.origin == origin))
            .collect()
    };
    sections.push((
        Section::SshConfig,
        "SSH config".to_owned(),
        of(Origin::SshConfig),
    ));
    sections.push((
        Section::Builtins,
        "Built-ins".to_owned(),
        of(Origin::Builtin),
    ));

    let mut out = Vec::new();
    for (section, label, items) in sections {
        if items.is_empty() && !matches!(section, Section::Group(_)) {
            continue;
        }
        let collapsed = view.collapsed.contains(&section);
        out.push(Line::Header {
            count: items.len(),
            section,
            label,
            collapsed,
        });
        if !collapsed {
            out.extend(items.into_iter().map(Line::Item));
        }
    }
    out
}

/// Entry indices of the item lines, in order (what ↑/↓ and Enter walk).
pub fn items(lines: &[Line]) -> Vec<usize> {
    lines
        .iter()
        .filter_map(|l| match l {
            Line::Item(i) => Some(*i),
            Line::Header { .. } => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "tests.rs"]
pub(crate) mod tests;
