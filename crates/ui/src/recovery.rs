//! Tab recovery: the tabs, splits and per-pane profiles go to `session.toml` once a second when
//! they changed, each pane's scrollback (inert runs, `fogu_term::history`) every 30 s and at a
//! clean exit; at startup they are rebuilt and fresh sessions started below the restored text.

use std::collections::{BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fogu_core::recovery::{self, PaneSpec, RecoveryState, TabState};
use fogu_core::{Profile, ProfileId, ProfileKind, SplitTree};
use fogu_term::history::{self, SavedHistory};
use iced::Task;

use crate::app::App;
use crate::message::Message;
use crate::modal::{Modal, ModalButton};
use crate::notifications::{Level, Notification};
use crate::tab::{Pane, PaneId, Tab, TabId};

/// How often the layout is compared with the last save.
const TICK: Duration = Duration::from_secs(1);
/// Scrollback is captured every this many ticks.
const SCROLLBACK_EVERY: u32 = 30;

/// Saving state; `None` on the app when recovery is off or there is no state directory.
#[derive(Debug)]
pub(crate) struct Recovery {
    dir: PathBuf,
    /// Makes pane keys unique across runs.
    prefix: String,
    /// The `session.toml` text last written.
    last_session: String,
    /// Hash of each pane's last written scrollback.
    written: HashMap<String, u64>,
    ticks: u32,
    /// "Clear saved sessions" waits for its confirmation.
    pub(crate) pending_clear: bool,
}

impl Recovery {
    pub(crate) fn new(dir: PathBuf) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self {
            dir,
            prefix: format!("{nanos:x}-{:x}", std::process::id()),
            last_session: String::new(),
            written: HashMap::new(),
            // The first tick also saves scrollback (restored panes get new keys).
            ticks: SCROLLBACK_EVERY - 1,
            pending_clear: false,
        }
    }

    fn key(&self, pane: PaneId) -> String {
        format!("{}-{:x}", self.prefix, pane.0)
    }
}

/// The lock file the first running Fogu holds in the state directory.
const INSTANCE_LOCK: &str = "instance.lock";

/// Result of trying to become the one window that restores and saves tabs.
#[derive(Debug)]
pub(crate) enum InstanceClaim {
    /// This window owns the lock until the file is dropped.
    Owner(std::fs::File),
    /// Another window owns the lock.
    Other,
    /// Ownership could not be established safely.
    Unconfirmed,
}

/// Tries to become the first Fogu running. The lock is released when its file is closed, also
/// on a crash; an unusable lock is never treated as ownership.
pub(crate) fn claim_first(dir: &Path) -> InstanceClaim {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(INSTANCE_LOCK));
    let file = match file {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!(error = %err, "instance lock not opened");
            return InstanceClaim::Unconfirmed;
        }
    };
    match file.try_lock() {
        Ok(()) => InstanceClaim::Owner(file),
        Err(std::fs::TryLockError::WouldBlock) => InstanceClaim::Other,
        Err(std::fs::TryLockError::Error(err)) => {
            tracing::warn!(error = %err, "instance lock not taken");
            InstanceClaim::Unconfirmed
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl App {
    /// Startup: clears the files when recovery is off, else rebuilds the saved tabs. Returns
    /// `true` when at least one tab was restored.
    pub(crate) fn restore_session(&mut self, state_dir: &Path, enabled: bool) -> bool {
        if !enabled {
            if let Err(err) = recovery::clear(state_dir) {
                tracing::warn!(error = %err, "could not delete saved sessions");
            }
            return false;
        }
        let state = match recovery::load(state_dir) {
            Ok(Some(state)) => state,
            Ok(None) => return false,
            Err(err) => {
                tracing::warn!(error = %err, "saved session refused");
                let _kept = std::fs::rename(
                    state_dir.join(recovery::SESSION_FILE),
                    state_dir.join(recovery::CORRUPT_FILE),
                );
                self.toasts.push(
                    Notification::new(Level::Warn, "Saved tabs not restored").detail(format!(
                        "{err}. The file was kept as {}.",
                        recovery::CORRUPT_FILE
                    )),
                );
                return false;
            }
        };
        let restored_at = format!(
            "{} UTC",
            crate::sftp::listing::utc_minutes(i64::try_from(state.saved_at).unwrap_or(0))
        );
        let mut restored = Vec::new();
        for tab in state.tabs {
            if let Some(id) = self.restore_tab(state_dir, tab, &restored_at) {
                restored.push(id);
            }
        }
        if let Some(id) = restored.get(state.active_tab).or(restored.last()) {
            self.activate(*id);
        }
        !restored.is_empty()
    }

    fn restore_tab(&mut self, dir: &Path, saved: TabState, at: &str) -> Option<TabId> {
        let focused_key = saved
            .layout
            .leaves()
            .get(saved.focused)
            .map(|s| s.key.clone());
        let mut panes: Vec<(String, Pane)> = Vec::new();
        for spec in saved.layout.leaves() {
            if let Some(pane) = self.restore_pane(dir, spec, at) {
                panes.push((spec.key.clone(), pane));
            }
        }
        let ids: HashMap<&str, PaneId> = panes.iter().map(|(k, p)| (k.as_str(), p.id)).collect();
        let mut layout: SplitTree<Option<PaneId>> = saved
            .layout
            .clone()
            .map(|s| ids.get(s.key.as_str()).copied());
        while layout.remove(&None).is_some() {}
        let layout = layout.map(|id| id.unwrap_or(PaneId(0)));
        let focused = focused_key
            .and_then(|k| ids.get(k.as_str()).copied())
            .or_else(|| layout.leaves().first().map(|id| **id));
        let mut panes = panes.into_iter().map(|(_, p)| p);
        let first = panes.next()?;
        let id = TabId(self.take_id());
        let mut tab = Tab::new(id, first);
        tab.panes.extend(panes.map(|p| (p.id, p)));
        tab.layout = layout;
        tab.focused = focused.unwrap_or(tab.focused);
        tab.title_override = saved.title_override;
        tab.color = saved.color;
        tab.background = saved.background;
        self.tabs.push(tab);
        Some(id)
    }

    /// A pane of the saved profile: its scrollback (if saved and valid) restored as inert text
    /// under a "restored from" line, then a fresh session.
    fn restore_pane(&mut self, dir: &Path, spec: &PaneSpec, at: &str) -> Option<Pane> {
        let Some(profile) = spec
            .ephemeral
            .clone()
            .or_else(|| self.find_profile(&spec.profile_id))
        else {
            self.toasts.push(
                Notification::new(Level::Warn, "Saved tab skipped").detail(format!(
                    "Profile \"{}\" no longer exists.",
                    fogu_term::sanitize_title(&spec.profile_id.0)
                )),
            );
            return None;
        };
        let size = crate::actions::INITIAL_SIZE;
        let mut term = self.new_term(size);
        if self.saves_scrollback(&profile) {
            match load_history(dir, &spec.key, self.config.recovery.scrollback_lines) {
                Ok(Some(saved)) => term.restore_history(&saved),
                Ok(None) => {}
                Err(why) => {
                    tracing::warn!(%why, "saved scrollback refused");
                    self.toasts.push(
                        Notification::new(Level::Warn, "Saved scrollback not restored").detail(why),
                    );
                }
            }
        }
        term.feed(
            format!("\x1b[2m\u{2500}\u{2500} restored from {at} \u{2500}\u{2500}\x1b[0m\r\n")
                .as_bytes(),
        );
        let cwd = spec.cwd.clone().map(PathBuf::from);
        Some(self.start_pane(&profile, cwd, (term, true), size))
    }

    /// A stored, built-in or `~/.ssh/config` profile by id.
    pub(crate) fn find_profile(&self, id: &ProfileId) -> Option<Profile> {
        self.config
            .profiles
            .iter()
            .chain(&self.profiles.builtins)
            .chain(&self.profiles.ssh_hosts.profiles)
            .find(|p| &p.id == id)
            .cloned()
    }

    fn saves_scrollback(&self, profile: &Profile) -> bool {
        profile
            .restore_scrollback
            .unwrap_or(self.config.recovery.restore_scrollback)
    }

    /// The layout as it would be saved now (profile editor tabs are not saved).
    fn session_state(&self, recovery: &Recovery) -> RecoveryState {
        let saved: Vec<&Tab> = self.tabs.iter().filter(|t| !t.is_page()).collect();
        let tabs = saved
            .iter()
            .map(|tab| {
                let leaves = tab.layout.leaves();
                TabState {
                    title_override: tab.title_override.clone(),
                    color: tab.color,
                    background: tab.background.clone(),
                    focused: leaves
                        .iter()
                        .position(|id| **id == tab.focused)
                        .unwrap_or(0),
                    layout: tab.layout.clone().map(|id| {
                        let pane = tab.panes.get(&id);
                        let profile =
                            pane.map_or_else(Profile::default_shell, |p| p.profile.clone());
                        let cwd = pane.and_then(|p| match &profile.kind {
                            ProfileKind::Ssh(_) => p.remote_cwd.clone(),
                            ProfileKind::Local(_) => p
                                .local_cwd
                                .as_ref()
                                .map(|c| c.to_string_lossy().into_owned()),
                        });
                        PaneSpec {
                            key: recovery.key(id),
                            ephemeral: profile.id.0.starts_with("quick:").then(|| profile.clone()),
                            profile_id: profile.id,
                            cwd,
                        }
                    }),
                }
            })
            .collect();
        RecoveryState {
            version: recovery::VERSION,
            saved_at: now_secs(),
            active_tab: saved.iter().position(|t| t.id == self.active).unwrap_or(0),
            tabs,
        }
    }

    /// Once a second: rewrite `session.toml` if the layout changed; every 30 s also the
    /// scrollback of every pane whose profile allows it, and ask local shells for their cwd.
    pub(crate) fn recovery_tick(&mut self) -> Task<Message> {
        let Some(mut recovery) = self.recovery.take() else {
            return Task::none();
        };
        recovery.ticks += 1;
        let scrollback = recovery.ticks >= SCROLLBACK_EVERY;
        if scrollback {
            recovery.ticks = 0;
            self.query_local_cwds();
        }
        self.save_recovery(&mut recovery, scrollback, false);
        self.recovery = Some(recovery);
        self.after(TICK, Message::RecoveryTick)
    }

    /// Writes what changed: the layout, and with `scrollback` the panes' history. `now` writes
    /// on this thread (clean exit); otherwise the files are written on the blocking pool.
    pub(crate) fn save_recovery(&self, recovery: &mut Recovery, scrollback: bool, now: bool) {
        let state = self.session_state(recovery);
        let mut text = match toml::to_string(&state) {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!(error = %err, "could not serialise the session");
                return;
            }
        };
        // `saved_at` changes every second: compare without it.
        let changed = strip_saved_at(&text) != strip_saved_at(&recovery.last_session);
        let dir = recovery.dir.clone();
        let mut writes: Vec<(String, Vec<u8>)> = Vec::new();
        let keep: BTreeSet<String> = self
            .tabs
            .iter()
            .flat_map(|t| t.panes.values())
            .filter(|p| self.saves_scrollback(&p.profile))
            .map(|p| recovery.key(p.id))
            .collect();
        if scrollback {
            let lines = self.config.recovery.scrollback_lines;
            for pane in self.tabs.iter().flat_map(|t| t.panes.values()) {
                let key = recovery.key(pane.id);
                if !keep.contains(&key) {
                    continue;
                }
                let saved: SavedHistory = pane.term.capture_history(lines, now_secs());
                let Ok(json) = serde_json::to_vec(&saved) else {
                    continue;
                };
                let hash = hash_rows(&saved);
                if recovery.written.get(&key) != Some(&hash) {
                    recovery.written.insert(key.clone(), hash);
                    writes.push((key, json));
                }
            }
            recovery.written.retain(|k, _| keep.contains(k));
        }
        if !changed && writes.is_empty() {
            return;
        }
        if changed {
            std::mem::swap(&mut recovery.last_session, &mut text);
        }
        let session = changed.then(|| recovery.last_session.clone());
        let job = move || write_files(&dir, session.as_deref(), &writes, &keep);
        if now {
            job();
        } else {
            let _detached = self.runtime.handle().spawn_blocking(job);
        }
    }

    /// Every pane closed (the last tab went away): nothing to restore next time.
    pub(crate) fn forget_session(&mut self) {
        if let Some(recovery) = &self.recovery
            && let Err(err) = recovery::clear(&recovery.dir)
        {
            tracing::warn!(error = %err, "could not delete saved sessions");
        }
    }

    /// The window is closing: save everything now.
    pub(crate) fn save_on_exit(&mut self) {
        if let Some(mut recovery) = self.recovery.take() {
            recovery.last_session.clear();
            self.save_recovery(&mut recovery, true, true);
            self.recovery = Some(recovery);
        }
    }

    /// "Clear saved sessions?" while the action waits for confirmation.
    pub(crate) fn clear_sessions_modal(&self) -> Option<Modal> {
        self.recovery.as_ref()?.pending_clear.then(|| Modal {
            title: "Clear saved sessions?",
            body: format!(
                "The saved tabs and their scrollback are deleted (the open tabs are saved again as you \
                 keep working). {}",
                recovery::PRIVACY_NOTE
            ),
            buttons: vec![
                ModalButton {
                    label: "Clear",
                    on_press: Message::ConfirmClearSessions(true),
                },
                ModalButton {
                    label: "Cancel",
                    on_press: Message::ConfirmClearSessions(false),
                },
            ],
        })
    }

    /// The modal's answer: delete the files (the current tabs are saved again from scratch).
    pub(crate) fn confirm_clear_sessions(&mut self, yes: bool) -> Task<Message> {
        let Some(recovery) = &mut self.recovery else {
            return Task::none();
        };
        recovery.pending_clear = false;
        if !yes {
            return Task::none();
        }
        let result = recovery::clear(&recovery.dir);
        recovery.last_session.clear();
        recovery.written.clear();
        match result {
            Ok(()) => self.notify(Notification::new(Level::Info, "Saved sessions cleared")),
            Err(err) => self.notify(
                Notification::new(Level::Error, "Could not clear saved sessions")
                    .detail(err.to_string()),
            ),
        }
    }
}

/// The session text without its `saved_at` line.
fn strip_saved_at(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with("saved_at"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn hash_rows(saved: &SavedHistory) -> u64 {
    let mut hasher = DefaultHasher::new();
    saved.cols.hash(&mut hasher);
    format!("{:?}", saved.rows).hash(&mut hasher);
    hasher.finish()
}

fn write_files(
    dir: &Path,
    session: Option<&str>,
    writes: &[(String, Vec<u8>)],
    keep: &BTreeSet<String>,
) {
    if let Some(text) = session
        && let Err(err) = recovery::save_text(dir, text)
    {
        tracing::warn!(error = %err, "could not save the session");
    }
    for (key, json) in writes {
        if let Err(err) = recovery::save_scrollback(dir, key, json) {
            tracing::warn!(error = %err, "could not save scrollback");
        }
    }
    if let Err(err) = recovery::prune_scrollback(dir, keep, recovery::MAX_TOTAL_SCROLLBACK) {
        tracing::warn!(error = %err, "could not prune saved scrollback");
    }
}

/// A pane's saved history: `Ok(None)` when there is none, `Err` (sanitized reason) when it is
/// too large, unparsable or invalid.
fn load_history(dir: &Path, key: &str, max_lines: usize) -> Result<Option<SavedHistory>, String> {
    let path = recovery::scrollback_path(dir, key);
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    // Text cap plus generous room for the JSON around it.
    if meta.len() > (history::MAX_BYTES as u64) * 4 {
        return Err("the file is too large".to_owned());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let saved: SavedHistory = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    history::validate(&saved, max_lines, history::MAX_BYTES).map_err(|e| e.to_string())?;
    Ok(Some(saved))
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
