//! Tab recovery: the tabs, splits and per-pane profiles go to `session.toml` once a second when
//! they changed, each pane's scrollback (inert runs, `fogu_term::history`) every 30 s and at a
//! clean exit; at startup they are rebuilt and fresh sessions started below the restored text.

use std::collections::{BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fogu_core::recovery::{self, PaneSpec, RecoveryState, TabState};
use fogu_core::{Profile, ProfileId, ProfileKind, SplitTree};
use fogu_term::history::{self, SavedHistory};
use iced::Task;
use secrecy::SecretString;
use secrecy::zeroize::Zeroizing;

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
    io: Arc<RecoveryIo>,
    /// Keys retained from panes restored while the system keyring was unavailable.
    restored_keys: HashMap<PaneId, String>,
    /// The SSH scrollback key loaded from the system keyring.
    ssh_key: Option<SecretString>,
    ticks: u32,
    /// Startup is waiting for the keyring before rebuilding the saved tabs.
    pub(crate) pending_restore: bool,
    /// "Clear saved sessions" waits for its confirmation.
    pub(crate) pending_clear: bool,
}

#[derive(Debug, Default)]
struct RecoveryIo {
    generation: AtomicU64,
    lock: Mutex<()>,
    /// Hash of each pane's last successfully written scrollback.
    written: Mutex<HashMap<String, u64>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
            io: Arc::default(),
            restored_keys: HashMap::new(),
            ssh_key: None,
            // The first tick also saves scrollback (restored panes normally get new keys).
            ticks: SCROLLBACK_EVERY - 1,
            pending_restore: false,
            pending_clear: false,
        }
    }

    fn key(&self, pane: PaneId) -> String {
        self.restored_keys
            .get(&pane)
            .cloned()
            .unwrap_or_else(|| format!("{}-{:x}", self.prefix, pane.0))
    }

    pub(crate) fn set_ssh_key(&mut self, key: SecretString) -> Result<(), String> {
        #[cfg(feature = "ssh")]
        {
            fogu_session::validate_recovery_key(&key).map_err(|err| err.to_string())?;
            self.ssh_key = Some(key);
            lock(&self.io.written).clear();
            Ok(())
        }
        #[cfg(not(feature = "ssh"))]
        {
            let _ = key;
            Err("SSH support is not available".to_owned())
        }
    }

    fn clear_ssh_key(&mut self) {
        self.ssh_key = None;
        lock(&self.io.written).clear();
    }

    pub(crate) fn has_ssh_key(&self) -> bool {
        self.ssh_key.is_some()
    }

    /// Invalidates queued writes, waits for an active one, then removes only ciphertext.
    pub(crate) fn clear_encrypted_scrollback(&mut self) -> std::io::Result<()> {
        self.clear_ssh_key();
        self.io.generation.fetch_add(1, Ordering::AcqRel);
        // A cancelled queued job may have carried the latest layout: write it again next tick.
        self.last_session.clear();
        let _guard = lock(&self.io.lock);
        recovery::clear_encrypted_scrollback(&self.dir)
    }

    fn clear_all(&mut self) -> std::io::Result<()> {
        self.io.generation.fetch_add(1, Ordering::AcqRel);
        let _guard = lock(&self.io.lock);
        let result = recovery::clear(&self.dir);
        self.last_session.clear();
        lock(&self.io.written).clear();
        result
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
    pub(crate) fn restore_session(&mut self, recovery: &mut Recovery, enabled: bool) -> bool {
        let state_dir = recovery.dir.clone();
        if !enabled {
            if let Err(err) = recovery.clear_all() {
                tracing::warn!(error = %err, "could not delete saved sessions");
            }
            return false;
        }
        let state = match recovery::load(&state_dir) {
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
        let mut locked_notified = false;
        for tab in state.tabs {
            if let Some(id) = self.restore_tab(
                &state_dir,
                tab,
                &restored_at,
                recovery,
                &mut locked_notified,
            ) {
                restored.push(id);
            }
        }
        if let Some(id) = restored.get(state.active_tab).or(restored.last()) {
            self.activate(*id);
        }
        !restored.is_empty()
    }

    fn restore_tab(
        &mut self,
        dir: &Path,
        saved: TabState,
        at: &str,
        recovery: &mut Recovery,
        locked_notified: &mut bool,
    ) -> Option<TabId> {
        let focused_key = saved
            .layout
            .leaves()
            .get(saved.focused)
            .map(|s| s.key.clone());
        let mut panes: Vec<(String, Pane)> = Vec::new();
        for spec in saved.layout.leaves() {
            if let Some(pane) =
                self.restore_pane(dir, spec, at, recovery.ssh_key.as_ref(), locked_notified)
            {
                if pane.is_ssh()
                    && self.saves_scrollback(&pane.profile)
                    && recovery.ssh_key.is_none()
                    && recovery::encrypted_scrollback_path(dir, &spec.key).exists()
                {
                    recovery.restored_keys.insert(pane.id, spec.key.clone());
                }
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
    fn restore_pane(
        &mut self,
        dir: &Path,
        spec: &PaneSpec,
        at: &str,
        ssh_key: Option<&SecretString>,
        locked_notified: &mut bool,
    ) -> Option<Pane> {
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
            let encrypted = matches!(profile.kind, ProfileKind::Ssh(_));
            match load_history(
                dir,
                &spec.key,
                self.config.recovery.scrollback_lines,
                encrypted,
                ssh_key,
            ) {
                Ok(Some(saved)) => term.restore_history(&saved),
                Ok(None) => {}
                Err(HistoryError::Locked) => {
                    if !*locked_notified {
                        self.toasts.push(
                            Notification::new(Level::Warn, "SSH history kept locked")
                                .detail("The system keyring is not available."),
                        );
                        *locked_notified = true;
                    }
                }
                Err(HistoryError::Invalid(why)) => {
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
        match profile.kind {
            ProfileKind::Local(_) => profile
                .restore_scrollback
                .unwrap_or(self.config.recovery.restore_scrollback),
            ProfileKind::Ssh(_) => {
                self.config.recovery.restore_ssh_scrollback
                    && profile.restore_scrollback.unwrap_or(true)
            }
        }
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

    /// Completes the startup restore after the asynchronous keyring lookup.
    pub(crate) fn finish_deferred_restore(&mut self) -> Task<Message> {
        let Some(mut recovery) = self.recovery.take() else {
            return Task::none();
        };
        if !recovery.pending_restore {
            self.recovery = Some(recovery);
            return Task::none();
        }
        recovery.pending_restore = false;
        if self.tabs.len() == 1
            && self.tabs[0]
                .welcome
                .as_deref()
                .is_some_and(|welcome| welcome.quick.is_empty() && welcome.error.is_none())
        {
            self.tabs.clear();
        }
        let restored = self.restore_session(&mut recovery, true);
        if !restored && self.tabs.is_empty() {
            let profile = self.default_profile();
            self.open_tab(&profile, None);
        }
        self.recovery = Some(recovery);
        self.after(TICK, Message::RecoveryTick)
    }

    /// Writes what changed: the layout, and with `scrollback` the panes' history. `now` writes
    /// on this thread (clean exit); otherwise the files are written on the blocking pool.
    pub(crate) fn save_recovery(&self, recovery: &mut Recovery, scrollback: bool, now: bool) {
        // The saved tabs are not rebuilt yet: saving now would overwrite them with none.
        if recovery.pending_restore {
            return;
        }
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
        let mut writes = Vec::new();
        let mut keep_plain = BTreeSet::new();
        let mut keep_encrypted = BTreeSet::new();
        for pane in self.tabs.iter().flat_map(|tab| tab.panes.values()) {
            if !self.saves_scrollback(&pane.profile) {
                continue;
            }
            let keep = if pane.is_ssh() {
                &mut keep_encrypted
            } else {
                &mut keep_plain
            };
            keep.insert(recovery.key(pane.id));
        }
        if scrollback {
            let lines = self.config.recovery.scrollback_lines;
            for pane in self.tabs.iter().flat_map(|t| t.panes.values()) {
                let key = recovery.key(pane.id);
                let encrypted = pane.is_ssh();
                let kept = if encrypted {
                    keep_encrypted.contains(&key)
                } else {
                    keep_plain.contains(&key)
                };
                if !kept || (encrypted && recovery.ssh_key.is_none()) {
                    continue;
                }
                let saved: SavedHistory = pane.term.capture_history(lines, now_secs());
                let mut json = Zeroizing::new(Vec::new());
                if serde_json::to_writer(&mut *json, &saved).is_err() {
                    continue;
                }
                let hash = hash_rows(&saved);
                if lock(&recovery.io.written).get(&key) != Some(&hash) {
                    writes.push(ScrollbackWrite {
                        key,
                        json,
                        encrypted,
                        hash,
                    });
                }
            }
            lock(&recovery.io.written)
                .retain(|key, _| keep_plain.contains(key) || keep_encrypted.contains(key));
        }
        if !changed && writes.is_empty() {
            return;
        }
        if changed {
            std::mem::swap(&mut recovery.last_session, &mut text);
        }
        let session = changed.then(|| recovery.last_session.clone());
        let ssh_key = recovery.ssh_key.clone();
        let preserve_encrypted = self.config.recovery.restore_ssh_scrollback && ssh_key.is_none();
        let io = Arc::clone(&recovery.io);
        let generation = io.generation.load(Ordering::Acquire);
        let job = move || {
            write_files(
                &dir,
                session.as_deref(),
                &writes,
                &keep_plain,
                &keep_encrypted,
                ssh_key.as_ref(),
                preserve_encrypted,
                &io,
                generation,
            );
        };
        if now {
            job();
        } else {
            let _detached = self.runtime.handle().spawn_blocking(job);
        }
    }

    /// Every pane closed (the last tab went away): nothing to restore next time.
    pub(crate) fn forget_session(&self) {
        if let Some(recovery) = &self.recovery
            && !recovery.pending_restore
        {
            recovery.io.generation.fetch_add(1, Ordering::AcqRel);
            let _guard = lock(&recovery.io.lock);
            if let Err(err) = recovery::clear(&recovery.dir) {
                tracing::warn!(error = %err, "could not delete saved sessions");
            }
        }
    }

    /// The window is closing: save everything now.
    pub(crate) fn save_on_exit(&mut self) {
        if let Some(mut recovery) = self.recovery.take() {
            // An older queued snapshot must not overwrite the final synchronous save.
            recovery.io.generation.fetch_add(1, Ordering::AcqRel);
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
        let result = recovery
            .clear_all()
            .map_err(|err| format!("{}: {err}", recovery.dir.display()));
        match result {
            Ok(()) => self.notify(Notification::new(Level::Info, "Saved sessions cleared")),
            Err(why) => self.notify(
                Notification::new(Level::Error, "Could not clear saved sessions").detail(why),
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

struct ScrollbackWrite {
    key: String,
    json: Zeroizing<Vec<u8>>,
    encrypted: bool,
    hash: u64,
}

#[expect(
    clippy::too_many_arguments,
    reason = "the immutable inputs of one recovery write job"
)]
fn write_files(
    dir: &Path,
    session: Option<&str>,
    writes: &[ScrollbackWrite],
    keep_plain: &BTreeSet<String>,
    keep_encrypted: &BTreeSet<String>,
    ssh_key: Option<&SecretString>,
    preserve_encrypted: bool,
    io: &RecoveryIo,
    generation: u64,
) {
    let _guard = lock(&io.lock);
    if io.generation.load(Ordering::Acquire) != generation {
        return;
    }
    #[cfg(not(feature = "ssh"))]
    let _ = ssh_key;
    if let Some(text) = session
        && let Err(err) = recovery::save_text(dir, text)
    {
        tracing::warn!(error = %err, "could not save the session");
    }
    for write in writes {
        if lock(&io.written).get(&write.key) == Some(&write.hash) {
            continue;
        }
        let mut succeeded = false;
        if write.encrypted {
            #[cfg(feature = "ssh")]
            if let Some(secret) = ssh_key {
                match fogu_session::seal_recovery(secret, &write.key, &write.json) {
                    Ok(sealed) => {
                        if let Err(err) =
                            recovery::save_encrypted_scrollback(dir, &write.key, &sealed)
                        {
                            tracing::warn!(error = %err, "could not save encrypted scrollback");
                        } else {
                            succeeded = true;
                        }
                    }
                    Err(err) => tracing::warn!(error = %err, "could not encrypt scrollback"),
                }
            }
        } else {
            match recovery::save_scrollback(dir, &write.key, &write.json) {
                Ok(()) => succeeded = true,
                Err(err) => tracing::warn!(error = %err, "could not save scrollback"),
            }
        }
        if succeeded {
            lock(&io.written).insert(write.key.clone(), write.hash);
        }
    }
    let keep_encrypted = (!preserve_encrypted).then_some(keep_encrypted);
    if let Err(err) = recovery::prune_scrollback(
        dir,
        keep_plain,
        keep_encrypted,
        recovery::MAX_TOTAL_SCROLLBACK,
    ) {
        tracing::warn!(error = %err, "could not prune saved scrollback");
    }
}

/// A pane's saved history: `Ok(None)` when there is none, `Err` (sanitized reason) when it is
/// too large, unparsable or invalid.
fn load_history(
    dir: &Path,
    key: &str,
    max_lines: usize,
    encrypted: bool,
    ssh_key: Option<&SecretString>,
) -> Result<Option<SavedHistory>, HistoryError> {
    let path = if encrypted {
        recovery::encrypted_scrollback_path(dir, key)
    } else {
        recovery::scrollback_path(dir, key)
    };
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(HistoryError::Invalid(err.to_string())),
    };
    if encrypted && ssh_key.is_none() {
        return Err(HistoryError::Locked);
    }
    // Text cap plus generous room for the JSON around it.
    if meta.len() > (history::MAX_BYTES as u64) * 4 {
        return Err(HistoryError::Invalid("the file is too large".to_owned()));
    }
    let bytes = std::fs::read(&path).map_err(|e| HistoryError::Invalid(e.to_string()))?;
    let plain = if encrypted {
        let secret = ssh_key.ok_or(HistoryError::Locked)?;
        #[cfg(feature = "ssh")]
        {
            fogu_session::open_recovery(secret, key, &bytes)
                .map_err(|err| HistoryError::Invalid(err.to_string()))?
        }
        #[cfg(not(feature = "ssh"))]
        {
            let _ = secret;
            return Err(HistoryError::Locked);
        }
    } else {
        Zeroizing::new(bytes)
    };
    let saved: SavedHistory =
        serde_json::from_slice(&plain).map_err(|e| HistoryError::Invalid(e.to_string()))?;
    history::validate(&saved, max_lines, history::MAX_BYTES)
        .map_err(|e| HistoryError::Invalid(e.to_string()))?;
    Ok(Some(saved))
}

enum HistoryError {
    Locked,
    Invalid(String),
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
