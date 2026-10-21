//! The profile editor: a "Profile: <name>" tab whose page is a form over every field of a local or
//! SSH profile. The draft is checked with `core::validate` on every change; Save writes only the
//! keys that changed, in place (`ProfileEdit::Update`, after `strip_defaults`), so
//! config.toml keeps the user's comments and stays short. `~/.ssh/config` hosts open read-only.
//!
//! `fields.rs` maps each [`FieldId`] to the profile field it edits (the reducer), `view.rs` draws
//! the sections, `handlers.rs` wires it into the app (open, save, delete, file pickers).

mod choices;
mod fields;
mod handlers;
mod stored;
mod view;
mod view_lists;
mod view_ssh;

/// `key = value` lines for every leaf of `new` that differs from `old` (see
/// [`ProfileEditor::changes`]).
fn changes(old: &Profile, new: &Profile) -> Vec<(String, String)> {
    fn flatten(prefix: &str, value: toml::Value, out: &mut BTreeMap<String, toml::Value>) {
        match value {
            toml::Value::Table(table) => {
                for (key, value) in table {
                    let key = if prefix.is_empty() {
                        key
                    } else {
                        format!("{prefix}.{key}")
                    };
                    flatten(&key, value, out);
                }
            }
            leaf => {
                out.insert(prefix.to_owned(), leaf);
            }
        }
    }
    let leaves = |p: &Profile| {
        let mut out = BTreeMap::new();
        if let Ok(value) = toml::Value::try_from(p) {
            flatten("", value, &mut out);
        }
        out
    };
    let (old, new) = (leaves(old), leaves(new));
    let keys: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    keys.into_iter()
        .filter(|k| old.get(*k) != new.get(*k))
        .map(|k| {
            let value = new
                .get(k)
                .map_or_else(|| "(default)".to_owned(), ToString::to_string);
            (k.clone(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use fogu_core::config::{ConfigIssue, validate};
use fogu_core::{Config, GroupId, Profile, ProfileId, ProfileKind};

pub use fields::FieldId;
pub use handlers::EditorMsg;
pub use stored::StoredEntry;
pub use view::page;

/// Where the edited profile comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Not saved yet (New ▸, "Save as profile…"); an id is made from the name on Save.
    New,
    /// A `[[profiles]]` entry, as it was when the editor opened (the `Update` baseline).
    Stored(Box<Profile>),
    /// A `~/.ssh/config` host: shown read-only, with "Copy to my profiles".
    SshConfig,
}

/// One open editor (the state of a "Profile: …" tab).
#[derive(Clone, Debug)]
pub struct ProfileEditor {
    /// The profile as edited so far.
    pub draft: Profile,
    /// The draft when the editor opened (closing asks first when they differ).
    pub original: Profile,
    pub source: Source,
    /// Issues `core::validate` finds for the draft (inline, block Save).
    pub issues: Vec<ConfigIssue>,
    /// Fields whose text does not parse (number out of range, bad colour), with the reason.
    pub errors: BTreeMap<FieldId, String>,
    /// What the user typed into number/colour fields (shown instead of the parsed value).
    pub texts: BTreeMap<FieldId, String>,
    /// The env rows as typed (a map can't hold an empty or duplicate name while typing).
    pub env_rows: Vec<(String, String)>,
    /// Group choices: `None` = Ungrouped, then every group ("New group…" follows them).
    pub groups: Vec<(Option<GroupId>, String)>,
    /// "New group…" picked: the name being typed.
    pub new_group: Option<String>,
    /// Jump host choices: `None` = direct, then every other SSH profile.
    pub jumps: Vec<(Option<ProfileId>, String)>,
    /// The tab header ([`Self::tab_title`], kept current by [`Self::refresh`]).
    pub title: String,
    /// Whether "Browse…" can open a file picker (else why not), checked when opened.
    pub picker: Result<(), &'static str>,
    /// What the Secret Service holds for the password and each private key (existence only).
    pub stored: Vec<StoredEntry>,
    /// The section last picked in the left nav (it is highlighted).
    pub section: usize,
    /// Colour fields showing the full picker.
    pub expanded: std::collections::BTreeSet<FieldId>,
}

impl ProfileEditor {
    /// An editor on `draft`; call [`Self::refresh`] before showing it.
    pub fn new(draft: Profile, source: Source) -> Self {
        let env_rows = match &draft.kind {
            ProfileKind::Local(local) => local
                .env
                .iter()
                .flatten()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            ProfileKind::Ssh(_) => Vec::new(),
        };
        Self {
            original: draft.clone(),
            draft,
            source,
            issues: Vec::new(),
            errors: BTreeMap::new(),
            texts: BTreeMap::new(),
            env_rows,
            groups: Vec::new(),
            new_group: None,
            jumps: Vec::new(),
            picker: Ok(()),
            stored: Vec::new(),
            title: String::new(),
            section: 0,
            expanded: std::collections::BTreeSet::new(),
        }
    }

    /// Unsaved changes.
    pub fn dirty(&self) -> bool {
        self.draft != self.original
    }

    /// The draft's values that differ from the original, as `key = value` TOML lines (dotted
    /// keys; a removed value reads `key = (default)`).
    pub fn changes(&self) -> Vec<(String, String)> {
        changes(&self.original, &self.draft)
    }

    /// Read-only (a `~/.ssh/config` host).
    pub fn read_only(&self) -> bool {
        self.source == Source::SshConfig
    }

    /// An SSH profile (else local).
    pub fn is_ssh(&self) -> bool {
        matches!(self.draft.kind, ProfileKind::Ssh(_))
    }

    /// Header text of the editor tab.
    pub fn tab_title(&self) -> String {
        let name = fogu_term::sanitize_title(self.draft.name.trim());
        match (&self.source, name.is_empty()) {
            (Source::New, true) if self.is_ssh() => "New SSH profile".to_owned(),
            (Source::New, true) => "New local profile".to_owned(),
            _ => format!("Profile: {name}"),
        }
    }

    /// Save is possible: editable, no issue, every field parses.
    pub fn can_save(&self) -> bool {
        !self.read_only() && self.issues.is_empty() && self.errors.is_empty()
    }

    /// Re-reads the group / jump host choices from `config` + `ssh_hosts` and validates the
    /// draft as if it were saved into `config`.
    pub fn refresh(&mut self, config: &Config, ssh_hosts: &[Profile]) {
        self.groups = std::iter::once((None, "Ungrouped".to_owned()))
            .chain(
                config
                    .groups
                    .iter()
                    .map(|g| (Some(g.id.clone()), fogu_term::sanitize_title(&g.name))),
            )
            .collect();
        let me = &self.draft.id;
        let ssh = |p: &&Profile| matches!(p.kind, ProfileKind::Ssh(_)) && &p.id != me;
        self.jumps = std::iter::once((None, "None (connect directly)".to_owned()))
            .chain(
                config
                    .profiles
                    .iter()
                    .chain(ssh_hosts)
                    .filter(ssh)
                    .map(|p| (Some(p.id.clone()), fogu_term::sanitize_title(&p.name))),
            )
            .collect();
        // A reference the lists don't have stays selectable (and visible) as "unknown: id".
        if let Some(group) = &self.draft.group
            && !self.groups.iter().any(|(g, _)| g.as_ref() == Some(group))
        {
            let label = format!("unknown: {}", fogu_term::sanitize_title(&group.0));
            self.groups.push((Some(group.clone()), label));
        }
        if let ProfileKind::Ssh(ssh) = &self.draft.kind
            && let Some(jump) = &ssh.jump_host
            && !self.jumps.iter().any(|(j, _)| j.as_ref() == Some(jump))
        {
            // `ssh-config:a,b` (a `ProxyJump` chain) is valid without being a listed host.
            let label = match jump.0.strip_prefix(fogu_core::ssh_config::ID_PREFIX) {
                Some(spec) => format!("ssh config: {}", fogu_term::sanitize_title(spec)),
                None => format!("unknown: {}", fogu_term::sanitize_title(&jump.0)),
            };
            self.jumps.push((Some(jump.clone()), label));
        }
        self.issues = self.validate(config);
        self.title = self.tab_title();
    }

    /// `validate` on `config` with the draft in place of (or added to) the stored profiles;
    /// only the issues about the draft are kept.
    fn validate(&self, config: &Config) -> Vec<ConfigIssue> {
        if self.read_only() {
            return Vec::new();
        }
        let mut candidate = config.clone();
        let stored_id = match &self.source {
            Source::Stored(old) => Some(&old.id),
            Source::New | Source::SshConfig => None,
        };
        match candidate
            .profiles
            .iter_mut()
            .find(|p| Some(&p.id) == stored_id)
        {
            Some(slot) => *slot = self.draft.clone(),
            None => candidate.profiles.push(self.draft.clone()),
        }
        let mut issues: Vec<ConfigIssue> = validate(&candidate)
            .into_iter()
            .filter(|i| i.profile() == Some(&self.draft.id))
            .collect();
        if let ProfileKind::Ssh(ssh) = &self.draft.kind
            && let Some(algorithms) = &ssh.algorithms
        {
            let names = view_lists::unknown(algorithms);
            if !names.is_empty() {
                issues.push(ConfigIssue::UnknownAlgorithms {
                    profile: self.draft.id.clone(),
                    names,
                });
            }
        }
        issues.dedup();
        issues
    }
}
