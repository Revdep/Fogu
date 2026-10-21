//! App wiring of the settings tab: opening it, writing each change to config.toml and applying
//! it to the running app (font, scheme, cursor, sidebar width, `~/.ssh/config` hosts). What is
//! read only when a terminal, panel or window is created applies from then on; the page says
//! so under those fields.

use std::path::PathBuf;
use std::sync::Arc;

use fogu_core::{Config, config};
use iced::Task;
use iced::advanced::widget::operate;
use iced::advanced::widget::operation::focusable;

use super::{FieldId, Page, Settings};
use crate::app::App;
use crate::appearance::{cursor_shape, resolve_scheme};
use crate::forms::FieldValue;
use crate::message::Message;
use crate::metrics;
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
            SettingsMsg::Page(page) => {
                if let Some(s) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) {
                    s.page = page;
                }
                return Task::none();
            }
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
        let mut new = Config::clone(&self.config);
        let Some(settings) = self.settings_tab().and_then(|t| t.settings.as_deref_mut()) else {
            return Task::none();
        };
        if settings.reduce(&mut new, id, &value).is_err() || new == *self.config {
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
            return self.refresh_ssh_hosts();
        }
        Task::none()
    }
}

/// Modification stamp used by the one-second config poll. Length also catches coarse timestamp
/// filesystems when an editor replaces the file more than once within one timestamp tick.
fn config_file_stamp(dir: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let metadata = std::fs::metadata(dir.join("config.toml")).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}
