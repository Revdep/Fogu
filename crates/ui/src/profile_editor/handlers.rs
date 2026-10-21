//! App wiring of the profile editor: opening "Profile: …" tabs, field changes (re-validated
//! every time), file pickers, Save (in-place `ProfileEdit::Update` / `Add` of the stripped
//! draft), Cancel, Delete (through the sidebar's confirmation) and "Copy to my profiles".

use std::path::PathBuf;

use fogu_core::config::{ProfileEdit, slug, unique_profile_id, unique_profile_name};
use fogu_core::{LocalOpts, Profile, ProfileId, ProfileKind, SshOpts, strip_defaults};
use iced::Task;

use super::fields::{Effect, FieldId};
use super::{ProfileEditor, Source, StoredEntry};
use crate::app::App;
use crate::forms::FieldValue;
use crate::message::Message;
use crate::notifications::{Level, Notification};
use crate::profiles::{Entry, Origin};
use crate::tab::{Tab, TabId};

/// Editor tab messages.
#[derive(Clone, Debug)]
pub enum EditorMsg {
    /// A form widget changed.
    Field(FieldId, FieldValue),
    /// A file picker for the field closed (`None` = cancelled).
    Picked(FieldId, Option<PathBuf>),
    Save,
    /// Cancel / Close: drop the draft and close the tab.
    Cancel,
    /// The left nav: show section `.0`.
    Section(usize),
    /// Ask to delete the stored profile (the sidebar's confirmation dialog).
    Delete,
    /// Read-only `~/.ssh/config` host: save it as a stored profile and edit that.
    CopyToMine,
    /// "Paste" in a text field's right-click menu: the clipboard replaces its text.
    Paste(FieldId),
    /// What the Secret Service holds for this profile's secrets (existence only).
    Stored(Vec<StoredEntry>),
    /// Set / Clear of a stored secret finished (`Err` = why not).
    StoreDone(Result<(), String>),
}

/// Scrolls the editor page so section `i` is at the top: one pass finds where it sits in the
/// page, the next scrolls there.
fn scroll_to_section(i: usize) -> Task<Message> {
    use iced::advanced::widget::operation::{Operation, Outcome, Scrollable};
    use iced::advanced::widget::{Id, operate};
    use iced::{Rectangle, Vector};

    /// The section's top relative to the page's content.
    struct Find {
        page: Id,
        section: Id,
        content_y: Option<f32>,
        found: Option<f32>,
    }
    impl Operation<f32> for Find {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<f32>)) {
            operate(self);
        }
        fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
            if id == Some(&self.section)
                && let Some(top) = self.content_y
            {
                self.found = Some(bounds.y - top);
            }
        }
        fn scrollable(
            &mut self,
            id: Option<&Id>,
            _bounds: Rectangle,
            content: Rectangle,
            _translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(&self.page) {
                self.content_y = Some(content.y);
            }
        }
        fn finish(&self) -> Outcome<f32> {
            self.found.map_or(Outcome::None, Outcome::Some)
        }
    }
    let page = Id::new(super::view::PAGE_SCROLL);
    operate(Find {
        page: page.clone(),
        section: super::view::section_id(i),
        content_y: None,
        found: None,
    })
    .then(move |y| {
        iced::widget::operation::scroll_to(
            page.clone(),
            iced::widget::operation::AbsoluteOffset {
                x: None,
                y: Some(y),
            },
        )
    })
}

/// Why "Browse…" is off when no picker can open.
const NO_PICKER: &str = "No file picker: install xdg-desktop-portal or zenity";

impl App {
    /// The editor of tab `tab`.
    fn editor_mut(&mut self, tab: TabId) -> Option<&mut ProfileEditor> {
        self.tabs
            .iter_mut()
            .find(|t| t.id == tab)
            .and_then(|t| t.editor.as_deref_mut())
    }

    /// Opens the editor for a sidebar/picker entry: a stored profile is edited in place, a
    /// `~/.ssh/config` host is shown read-only, a built-in or quick-connect one is saved as new.
    pub(crate) fn edit_entry(&mut self, entry: &Entry) -> Task<Message> {
        let source = match entry.origin {
            Origin::Stored => Source::Stored(Box::new(entry.profile.clone())),
            Origin::SshConfig => Source::SshConfig,
            Origin::Builtin | Origin::Quick => Source::New,
        };
        let mut draft = entry.profile.clone();
        if source == Source::New {
            draft.id = unique_profile_id(&self.config, "new-profile");
        }
        self.open_editor(draft, source)
    }

    /// A new profile drafted from `draft` (pane menu "Save as profile…"): a fresh id, the
    /// editor opens on it unsaved.
    pub(crate) fn new_profile_from(&mut self, mut draft: Profile) -> Task<Message> {
        draft.id = unique_profile_id(&self.config, "new-profile");
        self.open_editor(draft, Source::New)
    }

    /// New ▸ Local / SSH: an empty draft.
    pub(crate) fn new_profile(&mut self, ssh: bool) -> Task<Message> {
        let kind = if ssh {
            ProfileKind::Ssh(SshOpts::default())
        } else {
            ProfileKind::Local(LocalOpts::default())
        };
        let draft = Profile {
            id: unique_profile_id(&self.config, "new-profile"),
            name: String::new(),
            kind,
            ..Profile::default_shell()
        };
        self.open_editor(draft, Source::New)
    }

    /// A new editor tab on `draft` (or the one already open for that stored profile).
    fn open_editor(&mut self, mut draft: Profile, source: Source) -> Task<Message> {
        // A copy of a built-in / quick-connect profile must not clash with an earlier copy: two
        // stored profiles with one name cannot be saved.
        if source == Source::New && !draft.name.trim().is_empty() {
            draft.name = unique_profile_name(&self.config, &draft.name);
        }
        self.menu = None;
        self.profiles.focused = false;
        // Opened from a menu over the sidebar: the sidebar never saw the pointer leave (the menu
        // had it), and a stale hover would hand it the keyboard on the next click in the page.
        self.profiles.hovered = false;
        let existing = self.tabs.iter().find(|t| {
            t.editor
                .as_ref()
                .is_some_and(|e| e.source != Source::New && e.draft.id == draft.id)
        });
        if let Some(id) = existing.map(|t| t.id) {
            self.activate(id);
            return Task::none();
        }
        let mut editor = ProfileEditor::new(draft, source);
        editor.picker = if cfg!(test) || crate::sftp::picker_available() {
            Ok(())
        } else {
            Err(NO_PICKER)
        };
        editor.refresh(&self.config, &self.profiles.ssh_hosts.profiles);
        let id = TabId(self.take_id());
        self.tabs.push(Tab::editor(id, editor));
        self.activate(id);
        self.store_query(id)
    }

    pub(crate) fn on_editor(&mut self, tab: TabId, message: EditorMsg) -> Task<Message> {
        if let EditorMsg::Field(id, FieldValue::Menu(at, text)) = message {
            return self.field_menu(tab, id, at, text);
        }
        if let EditorMsg::Field(field, FieldValue::Expand) = message {
            if let Some(editor) = self.editor_mut(tab)
                && !editor.expanded.remove(&field)
            {
                editor.expanded.insert(field);
            }
            return Task::none();
        }
        // A `~/.ssh/config` host can't be changed in place: the first change copies it to the
        // user's profiles (as "Copy to my profiles" does) and applies to the copy.
        if matches!(message, EditorMsg::Field(..))
            && self.editor_mut(tab).is_some_and(|e| e.read_only())
        {
            let copied = self.copy_editor(tab);
            if self.editor_mut(tab).is_some_and(|e| e.read_only()) {
                return copied; // not copied; the toast says why
            }
            return Task::batch([copied, self.on_editor(tab, message)]);
        }
        if let EditorMsg::Field(FieldId::Group | FieldId::NewGroup, value) = &message
            && let Some(task) = self.editor_new_group(tab, value)
        {
            return task;
        }
        match message {
            EditorMsg::Field(field @ (FieldId::Password | FieldId::KeyPassphrase(_)), value) => {
                return self.secret_field(tab, field, &value);
            }
            EditorMsg::Stored(entries) => {
                if let Some(editor) = self.editor_mut(tab) {
                    editor.stored = entries;
                }
            }
            EditorMsg::StoreDone(result) => return self.store_done(tab, result),
            EditorMsg::Field(field, value) => {
                let config = self.config.clone();
                let hosts = self.profiles.ssh_hosts.profiles.clone();
                let Some(editor) = self.editor_mut(tab) else {
                    return Task::none();
                };
                let effect = editor.apply(field, value);
                editor.refresh(&config, &hosts);
                if let Effect::Browse(field) = effect {
                    return self.editor_pick(tab, field);
                }
            }
            EditorMsg::Picked(field, Some(path)) => {
                return self.on_editor(tab, EditorMsg::Field(field, FieldValue::Picked(path)));
            }
            EditorMsg::Picked(_, None) => {}
            EditorMsg::Save => return self.save_editor(tab),
            EditorMsg::Cancel => return self.remove_tab(tab),
            EditorMsg::Section(section) => {
                if let Some(editor) = self.editor_mut(tab) {
                    editor.section = section;
                }
                return scroll_to_section(section);
            }
            EditorMsg::Delete => {
                if let Some(Source::Stored(old)) = self.editor_mut(tab).map(|e| e.source.clone()) {
                    self.profiles.pending_delete = Some(old.id);
                }
            }
            EditorMsg::CopyToMine => return self.copy_editor(tab),
            EditorMsg::Paste(id) => {
                self.menu = None;
                return iced::clipboard::read().and_then(move |text| {
                    let text = text.trim_end_matches(['\r', '\n']).to_owned();
                    Task::done(Message::Editor(
                        tab,
                        EditorMsg::Field(id, FieldValue::Text(text)),
                    ))
                });
            }
        }
        Task::none()
    }

    /// The file (or, for the start directory, folder) picker for `field`.
    fn editor_pick(&mut self, tab: TabId, field: FieldId) -> Task<Message> {
        if !crate::sftp::picker_available() {
            return self.notify(crate::sftp::no_picker_note());
        }
        let dialog = rfd::AsyncFileDialog::new();
        let done =
            move |path: Option<PathBuf>| Message::Editor(tab, EditorMsg::Picked(field, path));
        let to_path = |f: Option<rfd::FileHandle>| f.map(|f| f.path().to_path_buf());
        match field {
            FieldId::LocalCwd => Task::perform(
                dialog.set_title("Start directory").pick_folder(),
                move |f| done(to_path(f)),
            ),
            FieldId::BgImage => Task::perform(
                dialog
                    .set_title("Background image")
                    .add_filter("Images (PNG, JPEG, WebP)", &["png", "jpg", "jpeg", "webp"])
                    .pick_file(),
                move |f| done(to_path(f)),
            ),
            FieldId::PrivateKeys => {
                let mut dialog = dialog.set_title("Private key");
                if let Some(ssh_dir) = dirs::home_dir().map(|h| h.join(".ssh")) {
                    dialog = dialog.set_directory(ssh_dir);
                }
                Task::perform(dialog.pick_file(), move |f| done(to_path(f)))
            }
            _ => Task::perform(dialog.set_title("Choose a file").pick_file(), move |f| {
                done(to_path(f))
            }),
        }
    }

    /// Writes the draft (stripped of inherited values) and closes the editor; open tabs of the
    /// profile keep running and use the new settings when they reconnect or restart.
    fn save_editor(&mut self, tab: TabId) -> Task<Message> {
        // A group name typed without Enter is still meant: create it and pick it first.
        let pending = self
            .editor_mut(tab)
            .and_then(|e| e.new_group.as_deref().map(str::trim).map(str::is_empty))
            == Some(false);
        let added = if pending {
            self.editor_new_group(tab, &FieldValue::Add)
                .unwrap_or_else(Task::none)
        } else {
            Task::none()
        };
        Task::batch([added, self.save_draft(tab)])
    }

    fn save_draft(&mut self, tab: TabId) -> Task<Message> {
        let Some(editor) = self.editor_mut(tab).map(|e| e.clone()) else {
            return Task::none();
        };
        if !editor.can_save() {
            return Task::none();
        }
        let mut draft = editor.draft.clone();
        // Rows added with "+ Add" and left empty are not saved.
        match &mut draft.kind {
            ProfileKind::Ssh(ssh) => {
                if let Some(keys) = &mut ssh.private_keys {
                    keys.retain(|k| !k.as_os_str().is_empty());
                }
            }
            ProfileKind::Local(local) => {
                if let Some(args) = &mut local.args {
                    args.retain(|a| !a.is_empty());
                }
            }
        }
        let mut new = strip_defaults(&draft, &self.config.profile_defaults);
        let edit = match &editor.source {
            Source::Stored(opened) => {
                let Some(current) = self.config.profiles.iter().find(|p| p.id == opened.id) else {
                    let note = Notification::new(Level::Error, "Profile not saved")
                        .detail("It is no longer in config.toml.");
                    return self.notify(note);
                };
                ProfileEdit::Update {
                    old: Box::new(current.clone()),
                    new: Box::new(new.clone()),
                }
            }
            Source::New => {
                new.id = unique_profile_id(&self.config, &slug(&new.name, "profile"));
                ProfileEdit::Add(Box::new(new.clone()))
            }
            Source::SshConfig => return Task::none(),
        };
        if let Err(task) = self.write_edits(&[edit]) {
            return task;
        }
        self.use_saved_profile(&new);
        let closed = self.remove_tab(tab);
        let note = Notification::new(Level::Info, "Profile saved")
            .detail(fogu_term::sanitize_title(&new.name));
        Task::batch([closed, self.notify(note)])
    }

    /// Open panes and tabs of `profile` remember the saved version (restart, reconnect).
    pub(crate) fn use_saved_profile(&mut self, profile: &Profile) {
        for tab in &mut self.tabs {
            if tab.profile.id == profile.id {
                tab.profile = profile.clone();
            }
            for pane in tab.panes.values_mut() {
                if pane.profile.id == profile.id {
                    pane.profile = profile.clone();
                }
            }
        }
    }

    /// Closes every editor tab of stored profile `id` (it was deleted).
    pub(crate) fn close_editors_of(&mut self, id: &ProfileId) -> Task<Message> {
        let tabs: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| {
                t.editor
                    .as_ref()
                    .is_some_and(|e| matches!(&e.source, Source::Stored(p) if &p.id == id))
            })
            .map(|t| t.id)
            .collect();
        Task::batch(tabs.into_iter().map(|t| self.remove_tab(t)))
    }

    /// "Copy to my profiles" in a read-only editor: the copy is saved and the editor switches
    /// to it.
    /// "New group…" in Group, typing its name, and Enter (adds the group to the config file
    /// and picks it). `None`: an ordinary Group choice, for `apply`.
    fn editor_new_group(&mut self, tab: TabId, value: &FieldValue) -> Option<Task<Message>> {
        let editor = self.editor_mut(tab)?;
        match value {
            FieldValue::Choice(i) if *i == editor.groups.len() => {
                editor.new_group = Some(String::new());
                Some(iced::widget::operation::focus(iced::widget::Id::new(
                    super::view::NEW_GROUP_INPUT,
                )))
            }
            FieldValue::Text(name) => {
                editor.new_group = Some(name.clone());
                Some(Task::none())
            }
            FieldValue::Add => {
                let name = editor.new_group.take().unwrap_or_default();
                let name = name.trim();
                if name.is_empty() {
                    return Some(Task::none());
                }
                let group = fogu_core::Group {
                    id: fogu_core::config::unique_group_id(&self.config, name),
                    name: name.to_owned(),
                };
                let id = group.id.clone();
                if let Err(task) = self.write_edits(&[ProfileEdit::AddGroup(group)]) {
                    return Some(task);
                }
                let (config, hosts) = (
                    self.config.clone(),
                    self.profiles.ssh_hosts.profiles.clone(),
                );
                let editor = self.editor_mut(tab)?;
                editor.refresh(&config, &hosts);
                let index = editor
                    .groups
                    .iter()
                    .position(|(g, _)| g.as_ref() == Some(&id))?;
                Some(self.on_editor(
                    tab,
                    EditorMsg::Field(FieldId::Group, FieldValue::Choice(index)),
                ))
            }
            FieldValue::Choice(_) => {
                editor.new_group = None;
                None
            }
            _ => None,
        }
    }

    /// Copy / Paste for text field `id` (showing `text`), at `at`.
    fn field_menu(
        &mut self,
        tab: TabId,
        id: FieldId,
        at: iced::Point,
        text: String,
    ) -> Task<Message> {
        use crate::context_menu::{ContextMenu, MenuAction, MenuItem};
        let empty = text.is_empty();
        let copy = MenuItem::new("Copy", MenuAction::Ui(Message::CopyLink(text)))
            .enabled_if(!empty, "The field is empty");
        let paste = MenuItem::new(
            "Paste",
            MenuAction::Ui(Message::Editor(tab, EditorMsg::Paste(id))),
        );
        self.menu = Some(ContextMenu::new(at, vec![copy, paste]));
        Task::none()
    }

    fn copy_editor(&mut self, tab: TabId) -> Task<Message> {
        let Some(draft) = self.editor_mut(tab).map(|e| e.draft.clone()) else {
            return Task::none();
        };
        let before = self.config.profiles.len();
        let task = self.copy_from_ssh_config(&draft);
        if self.config.profiles.len() == before {
            return task; // not saved; the toast says why
        }
        let Some(copy) = self.config.profiles.last().cloned() else {
            return task;
        };
        let (config, hosts) = (
            self.config.clone(),
            self.profiles.ssh_hosts.profiles.clone(),
        );
        if let Some(editor) = self.editor_mut(tab) {
            *editor = ProfileEditor {
                picker: editor.picker,
                ..ProfileEditor::new(copy.clone(), Source::Stored(Box::new(copy)))
            };
            editor.refresh(&config, &hosts);
        }
        task
    }

    /// The active tab is an editor and nothing is in front of it: it gets keyboard shortcuts.
    pub(crate) fn editor_has_keys(&self) -> bool {
        self.active_tab().is_some_and(Tab::is_page)
            && self.picker.is_none()
            && self.menu.is_none()
            && self.renaming.is_none()
            && self.pending_close.is_none()
            && self.profiles.pending_delete.is_none()
            && !(self.profiles.open && self.profiles.focused)
    }
}
