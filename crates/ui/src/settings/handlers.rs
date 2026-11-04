//! App wiring of the settings tab: opening it, writing each change to config.toml and applying
//! it to the running app (font, scheme, cursor, sidebar width, `~/.ssh/config` hosts). What is
//! read only when a terminal, panel or window is created applies from then on; the page says
//! so under those fields.

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "ssh")]
use fogu_core::SecretKey;
use fogu_core::{Config, config};
use iced::Task;
use iced::advanced::widget::operate;
use iced::advanced::widget::operation::focusable;
use iced::widget::text_editor;
use secrecy::SecretString;

use super::{FieldId, Page, Settings};
use crate::app::App;
use crate::appearance::{cursor_shape, resolve_scheme};
use crate::forms::FieldValue;
use crate::message::Message;
use crate::metrics;
use crate::modal::{Modal, ModalButton};
use crate::notifications::{Level, Notification};
use crate::palette::{Colors, Look};
use crate::tab::{Tab, TabId};

/// Settings tab messages.
#[derive(Clone, Debug)]
pub enum SettingsMsg {
    /// A nav row was clicked.
    Page(Page),
    /// A form widget changed.
    Field(FieldId, FieldValue),
    /// The download folder picker closed (`None` = cancelled).
    Picked(Option<PathBuf>),
    /// The answer to the SSH scrollback explanation.
    ConfirmSshRecovery(bool),
    /// An edit in the config.toml page.
    FileEdit(text_editor::Action),
    /// "Save" on the config.toml page.
    FileSave,
    /// "Revert" on the config.toml page: the file as it is on disk.
    FileRevert,
    /// Loading or creating the SSH scrollback key finished; `enable` came from the UI prompt.
    SshRecoveryKeyLoaded {
        enable: bool,
        result: Result<SecretString, String>,
    },
}

impl App {
    /// The settings tab (there is at most one).
    fn settings_tab(&mut self) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.settings.is_some())
    }

    /// `open-settings`: the settings tab, opened or brought forward.
    pub(crate) fn open_settings(&mut self) -> Task<Message> {
        self.menu = None;
        self.profiles.focused = false;
        self.profiles.hovered = false;
        let unfocus = operate(focusable::unfocus());
        if let Some(id) = self.settings_tab().map(|t| t.id) {
            self.activate(id);
            return unfocus;
        }
        let settings = Settings {
            fonts: metrics::monospace_families(),
            no_picker: (!cfg!(test) && !crate::sftp::picker_available())
                .then_some("No file picker: install xdg-desktop-portal or zenity"),
            ..Settings::default()
        };
        let id = TabId(self.take_id());
        self.tabs.push(Tab::settings(id, settings));
        self.activate(id);
        unfocus
    }

    pub(crate) fn on_settings(&mut self, message: SettingsMsg) -> Task<Message> {
        let (id, value) = match message {
            SettingsMsg::ConfirmSshRecovery(yes) => return self.confirm_ssh_recovery(yes),
            SettingsMsg::SshRecoveryKeyLoaded { enable, result } => {
                return self.ssh_recovery_key_loaded(enable, result);
            }
            SettingsMsg::Page(page) => {
                let dir = config::config_dir(self.config_dir.as_deref());
                if let Some(s) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) {
                    s.page = page;
                    // Unsaved typing survives a page switch; otherwise show the file as it is now.
                    if page == Page::File && s.file.text() == s.file_saved {
                        read_file(s, &dir);
                    }
                }
                return Task::none();
            }
            SettingsMsg::FileEdit(action) => {
                if let Some(s) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) {
                    s.file.perform(action);
                }
                return Task::none();
            }
            SettingsMsg::FileRevert => {
                let dir = config::config_dir(self.config_dir.as_deref());
                if let Some(s) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) {
                    read_file(s, &dir);
                }
                return Task::none();
            }
            SettingsMsg::FileSave => return self.save_file(),
            SettingsMsg::Picked(None) => return Task::none(),
            SettingsMsg::Picked(Some(path)) => (FieldId::DownloadDir, FieldValue::Picked(path)),
            SettingsMsg::Field(FieldId::DownloadDir, FieldValue::Browse) => {
                let dialog = rfd::AsyncFileDialog::new().set_title("Download folder");
                return Task::perform(dialog.pick_folder(), |folder| {
                    Message::Settings(SettingsMsg::Picked(folder.map(|f| f.path().to_path_buf())))
                });
            }
            SettingsMsg::Field(id, value) => (id, value),
        };
        if id == FieldId::RecoverySshScrollback
            && matches!(value, FieldValue::Bool(true))
            && !self.config.recovery.restore_ssh_scrollback
        {
            if let Some(settings) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) {
                settings.pending_ssh_recovery = true;
            }
            return Task::none();
        }
        self.save_setting(id, &value)
    }

    fn save_setting(&mut self, id: FieldId, value: &FieldValue) -> Task<Message> {
        let mut new = Config::clone(&self.config);
        let Some(settings) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) else {
            return Task::none();
        };
        if settings.reduce(&mut new, id, value).is_err() || new == *self.config {
            return Task::none();
        }
        let dir = config::config_dir(self.config_dir.as_deref());
        if let Err(why) = config::edit_settings(&dir, &self.config, &new) {
            let note =
                Notification::new(Level::Error, "Settings not saved").detail(why.to_string());
            return self.notify(note);
        }
        self.remember_config_file();
        let old = std::mem::replace(&mut self.config, Arc::new(new));
        self.apply_settings(&old)
    }

    /// "Save" on the config.toml page: written only when it loads, then applied like an edit
    /// made in another editor.
    fn save_file(&mut self) -> Task<Message> {
        let dir = config::config_dir(self.config_dir.as_deref());
        let Some(s) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) else {
            return Task::none();
        };
        let text = s.file.text();
        match config::write_text(&dir, &text) {
            Ok(new) => {
                s.file_saved = text;
                s.file_error = None;
                self.remember_config_file();
                self.use_config(new)
            }
            Err(why) => {
                s.file_error = Some(why.to_string());
                Task::none()
            }
        }
    }

    pub(crate) fn ssh_recovery_modal(&self) -> Option<Modal> {
        self.tabs
            .iter()
            .find_map(|tab| tab.settings.as_deref())?
            .pending_ssh_recovery
            .then(|| Modal {
                title: "Keep SSH history after restart?",
                body: "Fogu locks it with your system's password vault. Your computer may ask for your password once."
                    .to_owned(),
                buttons: vec![
                    ModalButton {
                        label: "Turn on",
                        on_press: Message::Settings(SettingsMsg::ConfirmSshRecovery(true)),
                    },
                    ModalButton {
                        label: "Cancel",
                        on_press: Message::Settings(SettingsMsg::ConfirmSshRecovery(false)),
                    },
                ],
            })
    }

    fn confirm_ssh_recovery(&mut self, yes: bool) -> Task<Message> {
        if let Some(settings) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) {
            settings.pending_ssh_recovery = false;
        }
        if !yes {
            return Task::none();
        }
        self.load_ssh_recovery_key(true)
    }

    /// Reads the persistent key without blocking the UI, creating it when the keyring is empty.
    pub(crate) fn load_ssh_recovery_key(&mut self, enable: bool) -> Task<Message> {
        #[cfg(feature = "ssh")]
        {
            let secrets = self.runtime.secrets().clone();
            Task::perform(
                async move {
                    // Two lookups racing on an empty keyring would each create a different key.
                    static LOOKUP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
                    let _one_at_a_time = LOOKUP.lock().await;
                    if let Some(key) = secrets
                        .get(&SecretKey::RecoveryKey)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        Ok(key)
                    } else {
                        let key = fogu_session::new_recovery_key().map_err(|e| e.to_string())?;
                        secrets
                            .set(&SecretKey::RecoveryKey, key.clone())
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(key)
                    }
                },
                move |result| {
                    Message::Settings(SettingsMsg::SshRecoveryKeyLoaded { enable, result })
                },
            )
        }
        #[cfg(not(feature = "ssh"))]
        {
            let _ = enable;
            self.notify(
                Notification::new(Level::Warn, "SSH history not turned on")
                    .detail("The system keyring is locked or missing."),
            )
        }
    }

    fn ssh_recovery_key_loaded(
        &mut self,
        enable: bool,
        result: Result<SecretString, String>,
    ) -> Task<Message> {
        let key = match result {
            Ok(key) => key,
            Err(err) => {
                tracing::warn!(error = %err, "SSH recovery key unavailable");
                let note = self.notify(
                    Notification::new(Level::Warn, "SSH history not turned on")
                        .detail("The system keyring is locked or missing."),
                );
                return Task::batch([note, self.finish_deferred_restore()]);
            }
        };
        #[cfg(feature = "ssh")]
        if let Err(err) = fogu_session::validate_recovery_key(&key) {
            tracing::warn!(error = %err, "invalid SSH recovery key");
            let note = self.notify(
                Notification::new(Level::Warn, "SSH history not turned on")
                    .detail("The system keyring is locked or missing."),
            );
            return Task::batch([note, self.finish_deferred_restore()]);
        }
        if (enable || self.config.recovery.restore_ssh_scrollback)
            && let Some(recovery) = &mut self.recovery
            && let Err(err) = recovery.set_ssh_key(key)
        {
            tracing::warn!(error = %err, "invalid SSH recovery key");
        }
        let restore = self.finish_deferred_restore();
        let setting = if enable && !self.config.recovery.restore_ssh_scrollback {
            self.save_setting(FieldId::RecoverySshScrollback, &FieldValue::Bool(true))
        } else {
            Task::none()
        };
        Task::batch([restore, setting])
    }

    /// Records the current config file stamp so an edit Fogu just made is not hot-reloaded.
    pub(crate) fn remember_config_file(&mut self) {
        let dir = config::config_dir(self.config_dir.as_deref());
        self.config_file_stamp = config_file_stamp(&dir);
        self.config_watched = true;
    }

    /// Applies one externally changed `config.toml`; a bad version is remembered but ignored,
    /// so it creates one toast rather than another toast on every poll.
    pub(crate) fn reload_config_if_changed(&mut self) -> Task<Message> {
        let dir = config::config_dir(self.config_dir.as_deref());
        let stamp = config_file_stamp(&dir);
        if !self.config_watched || stamp == self.config_file_stamp {
            return Task::none();
        }
        self.config_file_stamp = stamp;
        let (new, error) = config::load(&dir);
        if let Some(error) = error {
            tracing::error!(%error, "failed to hot-reload config");
            let note = Notification::new(Level::Error, "Config change not applied")
                .detail(error.to_string());
            return self.notify(note);
        }
        self.use_config(new)
    }

    /// Runs on `new`, a config.toml changed as a whole (another editor, the config.toml page).
    fn use_config(&mut self, new: Config) -> Task<Message> {
        if new == *self.config {
            return Task::none();
        }
        if let Some(settings) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) {
            settings.texts.clear();
            settings.errors.clear();
        }
        let old = std::mem::replace(&mut self.config, Arc::new(new));
        self.reloaded_profiles(&old);
        self.apply_settings(&old)
    }

    /// After a reload: open tabs use the new version of their stored profile, editors re-check
    /// their draft (groups, jump hosts, names taken), a delete waiting for a profile that is
    /// gone is dropped.
    fn reloaded_profiles(&mut self, old: &Config) {
        let changed: Vec<_> = self
            .config
            .profiles
            .iter()
            .filter(|p| !old.profiles.contains(p))
            .cloned()
            .collect();
        for profile in &changed {
            self.use_saved_profile(profile);
        }
        let (config, hosts) = (&self.config, &self.profiles.ssh_hosts.profiles);
        for editor in self.tabs.iter_mut().filter_map(|t| t.editor.as_deref_mut()) {
            editor.refresh(config, hosts);
        }
        let gone = self
            .profiles
            .pending_delete
            .as_ref()
            .is_some_and(|id| !self.config.profiles.iter().any(|p| &p.id == id));
        if gone {
            self.profiles.pending_delete = None;
        }
    }

    /// Applies what changed from `old` to the running config to the app.
    #[expect(clippy::float_cmp, reason = "detects an edited value, no arithmetic")]
    fn apply_settings(&mut self, old: &Config) -> Task<Message> {
        let mut tasks = Vec::new();
        let (was, now) = (&old.appearance, self.config.appearance.clone());
        if was.font_family != now.font_family || was.fallback_fonts != now.fallback_fonts {
            let (resolved, notes) = metrics::resolve(&now.font_family, &now.fallback_fonts);
            self.font.family.clone_from(&now.font_family);
            self.resolved = resolved;
            if let Some(s) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) {
                s.font_notes = notes;
            }
            self.apply_metrics();
        }
        if was.font_size != now.font_size {
            self.default_font_size_pt = now.font_size;
            self.set_font_size(now.font_size);
        }
        if was.color_scheme != now.color_scheme {
            let (scheme, _) = resolve_scheme(&now.color_scheme);
            self.colors = Colors::new(&scheme);
            self.look = Look::new(&self.colors);
            self.theme = self.look.theme(&self.colors);
            for pane in self.tabs.iter_mut().flat_map(|t| t.panes.values_mut()) {
                pane.term.set_palette(self.colors.palette());
            }
        }
        if was.cursor_shape != now.cursor_shape {
            for pane in self.tabs.iter_mut().flat_map(|t| t.panes.values_mut()) {
                pane.term
                    .set_default_cursor_shape(cursor_shape(now.cursor_shape));
            }
        }
        if was.profiles_sidebar_width != now.profiles_sidebar_width {
            self.profiles.width = now.profiles_sidebar_width;
        }
        if old.ssh.read_ssh_config != self.config.ssh.read_ssh_config {
            tasks.push(self.refresh_ssh_hosts());
        }
        if old.recovery.restore_ssh_scrollback && !self.config.recovery.restore_ssh_scrollback {
            if let Some(recovery) = &mut self.recovery {
                if let Err(err) = recovery.clear_encrypted_scrollback() {
                    tracing::warn!(error = %err, "could not delete encrypted scrollback");
                }
            } else if let Some(dir) = &self.state_dir
                && let Err(err) = fogu_core::recovery::clear_encrypted_scrollback(dir)
            {
                tracing::warn!(error = %err, "could not delete encrypted scrollback");
            }
        } else if !old.recovery.restore_ssh_scrollback
            && self.config.recovery.restore_ssh_scrollback
            && self
                .recovery
                .as_ref()
                .is_none_or(|recovery| !recovery.has_ssh_key())
        {
            tasks.push(self.load_ssh_recovery_key(false));
        }
        Task::batch(tasks)
    }
}

/// Loads `dir/config.toml` into the page (a missing file reads as empty).
fn read_file(s: &mut Settings, dir: &std::path::Path) {
    let text = match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            s.file_error = Some(err.to_string());
            return;
        }
    };
    s.file = text_editor::Content::with_text(&text);
    // The editor's own rendering of it, so "unchanged" compares like with like.
    s.file_saved = s.file.text();
    s.file_error = None;
}

/// Modification stamp used by the one-second config poll. Length also catches coarse timestamp
/// filesystems when an editor replaces the file more than once within one timestamp tick.
fn config_file_stamp(dir: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let metadata = std::fs::metadata(dir.join("config.toml")).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}
