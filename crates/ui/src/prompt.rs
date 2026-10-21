//! The one secret-prompt widget: the modal a session's `SessionOutput::Prompt` opens over the pane
//! that asked — password, key passphrase, keyboard-interactive answers and the host-key decisions.
//!
//! Secret flow:
//! - iced's `text_input` reports each edit as a whole new `String`. That string rides in
//!   [`PromptMsg::Input`] wrapped in [`PromptText`] (redacted `Debug`, zeroized on drop — every
//!   clone iced makes too); `update()` copies it straight into the field's `SecretString`
//!   buffer and drops it. No other copy is kept.
//! - Submitting carries no text ([`PromptMsg::Submit`]): `update()` *moves* the buffers into
//!   `SessionInput::PromptReply`, leaving none behind.
//! - Limitation: while the dialog is open, iced's `text_input` holds its own copy of the value
//!   it displays (not zeroized); it goes away with the dialog.
//!
//! Server-chosen texts were sanitized by the session; host names, user names and paths come
//! from the (possibly imported) config, so they go through `sanitize_remote` here too.

use std::fmt;

use fogu_session::{
    PROMPT_INSTRUCTIONS_MAX_CHARS, PROMPT_TEXT_MAX_CHARS, PromptId, PromptKind, PromptReply,
    Remember, SessionInput, sanitize_remote,
};
use iced::widget::{
    Id, button, checkbox, column, container, operation, row, scrollable, space, text, text_input,
};
use iced::{Color, Element, Font, Length, Task};
use secrecy::zeroize::Zeroize;
use secrecy::{ExposeSecret, SecretString};

use crate::app::App;
use crate::message::Message;
use crate::modal;
use crate::tab::PaneId;

/// Widest the dialog gets (fingerprints fit on one line).
const DIALOG_W: f32 = 540.0;
/// Header colour of the changed-key dialog.
const DANGER: Color = Color::from_rgb(0.80, 0.16, 0.16);

/// Text typed into a prompt field while it travels inside a `Message`: redacted `Debug`,
/// zeroized on drop.
#[derive(Clone)]
pub struct PromptText(String);

impl PromptText {
    /// Wraps what `text_input` reported.
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// Nothing typed.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A fresh secret buffer holding the text; `self` is zeroized as it drops.
    pub(crate) fn into_secret(self) -> SecretString {
        SecretString::from(self.0.as_str())
    }
}

impl Drop for PromptText {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for PromptText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PromptText(<redacted>)")
    }
}

/// A host-key dialog button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Unknown key: trust for this connection.
    AcceptOnce,
    /// Unknown key: trust and record.
    AcceptAndSave,
    /// Any dialog: decline.
    Cancel,
    /// Changed key, first click on "Replace and connect": ask "Are you sure?".
    ArmReplace,
    /// Changed key, second click: replace the record and connect.
    Replace,
    /// Rejected stored secret: delete it and ask again.
    Forget,
}

/// Prompt dialog messages. None carries a secret except [`PromptMsg::Input`]'s [`PromptText`].
#[derive(Clone, Debug)]
pub enum PromptMsg {
    /// Field `.1` of pane `.0`'s prompt changed.
    Input(PaneId, usize, PromptText),
    /// Enter in field `.1`: focus the next field, or submit after the last one.
    FieldEnter(PaneId, usize),
    /// The "Remember" checkbox of pane `.0`'s prompt.
    Remember(PaneId, bool),
    /// Send the answers.
    Submit(PaneId),
    /// A dialog button.
    Choose(PaneId, Choice),
}

/// An open prompt: what was asked and the answer buffers (one per field).
pub(crate) struct PromptState {
    pub id: PromptId,
    pub kind: PromptKind,
    /// The jump host (`user@host:port`) this prompt is for; `None` = the pane's own host.
    pub via: Option<String>,
    answers: Vec<SecretString>,
    replace_armed: bool,
    /// "Remember" is ticked (starts ticked when a stored secret was just rejected: replace it).
    remember: bool,
}

impl fmt::Debug for PromptState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptState")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("via", &self.via)
            .field("answers", &self.answers.len())
            .field("replace_armed", &self.replace_armed)
            .field("remember", &self.remember)
            .finish()
    }
}

impl PromptState {
    pub fn new(id: PromptId, kind: PromptKind, via: Option<String>) -> Self {
        let fields = match &kind {
            PromptKind::Password { .. } | PromptKind::KeyPassphrase { .. } => 1,
            PromptKind::KeyboardInteractive { prompts, .. } => prompts.len(),
            PromptKind::HostKeyUnknown { .. } | PromptKind::HostKeyChanged { .. } => 0,
        };
        Self {
            id,
            via,
            answers: (0..fields).map(|_| SecretString::default()).collect(),
            replace_armed: false,
            remember: matches!(remember_of(&kind), Some(Remember::Rejected)),
            kind,
        }
    }

    /// Number of answer buffers (text fields).
    pub fn fields(&self) -> usize {
        self.answers.len()
    }

    /// Moves the answers into the reply, leaving no buffer behind; `None` for host-key dialogs.
    fn take_answers(&mut self) -> Option<PromptReply> {
        let answers = std::mem::take(&mut self.answers);
        match self.kind {
            PromptKind::Password { .. } | PromptKind::KeyPassphrase { .. } => {
                let remember = self.remember && self.can_remember();
                answers
                    .into_iter()
                    .next()
                    .map(|value| PromptReply::Secret { value, remember })
            }
            PromptKind::KeyboardInteractive { .. } => Some(PromptReply::Secrets(answers)),
            PromptKind::HostKeyUnknown { .. } | PromptKind::HostKeyChanged { .. } => None,
        }
    }

    /// The store takes what is typed.
    fn can_remember(&self) -> bool {
        !matches!(
            remember_of(&self.kind),
            Some(Remember::Unavailable(_)) | None
        )
    }

    /// The reply a button gives; `None` keeps the dialog open. A changed key is only ever
    /// accepted by `Replace` after `ArmReplace` — never by Enter or a single click.
    fn choose(&mut self, choice: Choice) -> Option<PromptReply> {
        match (&self.kind, choice) {
            (_, Choice::Cancel) => Some(PromptReply::Reject),
            (_, Choice::Forget) if remember_of(&self.kind) == Some(&Remember::Rejected) => {
                Some(PromptReply::Forget)
            }
            (PromptKind::HostKeyUnknown { .. }, Choice::AcceptOnce) => Some(PromptReply::Accept),
            (PromptKind::HostKeyUnknown { .. }, Choice::AcceptAndSave) => {
                Some(PromptReply::AcceptAndSave)
            }
            (PromptKind::HostKeyChanged { .. }, Choice::ArmReplace) => {
                self.replace_armed = true;
                None
            }
            (PromptKind::HostKeyChanged { .. }, Choice::Replace) if self.replace_armed => {
                Some(PromptReply::AcceptAndSave)
            }
            _ => None,
        }
    }
}

/// The "Remember" state of a password / passphrase prompt.
fn remember_of(kind: &PromptKind) -> Option<&Remember> {
    match kind {
        PromptKind::Password { remember, .. } | PromptKind::KeyPassphrase { remember, .. } => {
            Some(remember)
        }
        _ => None,
    }
}

/// Widget id of field `idx` of pane `pane`'s prompt.
pub(crate) fn field_id(pane: PaneId, idx: usize) -> Id {
    Id::from(format!("fogu-prompt-{}-{idx}", pane.0))
}

impl App {
    /// The session of pane `pane_id` asks something: show the dialog over that pane (a stale
    /// open question is declined) and focus its first field when the pane is in front.
    pub(crate) fn on_prompt(
        &mut self,
        pane_id: PaneId,
        id: PromptId,
        kind: PromptKind,
        via: Option<String>,
    ) -> Task<Message> {
        let in_front = self.active_pane().is_some_and(|p| p.id == pane_id);
        let Some(pane) = self.pane_mut(pane_id) else {
            return Task::none();
        };
        tracing::debug!(?id, ?kind, ?via, "session prompt");
        let state = PromptState::new(id, kind, via);
        let has_field = state.fields() > 0;
        if let Some(old) = pane.prompt.replace(state) {
            pane.control(SessionInput::PromptReply(old.id, PromptReply::Reject));
        }
        if in_front && has_field {
            return operation::focus(field_id(pane_id, 0));
        }
        Task::none()
    }

    pub(crate) fn on_prompt_msg(&mut self, message: PromptMsg) -> Task<Message> {
        match message {
            PromptMsg::Input(pane, idx, typed) => {
                let slot = self
                    .pane_mut(pane)
                    .and_then(|p| p.prompt.as_mut())
                    .and_then(|p| p.answers.get_mut(idx));
                if let Some(slot) = slot {
                    *slot = typed.into_secret();
                }
            }
            PromptMsg::FieldEnter(pane, idx) => {
                let fields = self
                    .pane(pane)
                    .and_then(|p| p.prompt.as_ref())
                    .map_or(0, PromptState::fields);
                if idx + 1 < fields {
                    return operation::focus(field_id(pane, idx + 1));
                }
                self.submit_prompt(pane);
            }
            PromptMsg::Remember(pane, on) => {
                if let Some(prompt) = self.pane_mut(pane).and_then(|p| p.prompt.as_mut()) {
                    prompt.remember = on;
                }
            }
            PromptMsg::Submit(pane) => self.submit_prompt(pane),
            PromptMsg::Choose(pane, choice) => {
                let reply = self
                    .pane_mut(pane)
                    .and_then(|p| p.prompt.as_mut())
                    .and_then(|p| p.choose(choice));
                if let Some(reply) = reply {
                    self.answer(pane, reply);
                }
            }
        }
        Task::none()
    }

    fn submit_prompt(&mut self, pane: PaneId) {
        let reply = self
            .pane_mut(pane)
            .and_then(|p| p.prompt.as_mut())
            .and_then(PromptState::take_answers);
        if let Some(reply) = reply {
            self.answer(pane, reply);
        }
    }

    /// Closes pane `pane_id`'s dialog and sends `reply` (moved, never copied) to its session.
    fn answer(&mut self, pane_id: PaneId, reply: PromptReply) {
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        let Some(prompt) = pane.prompt.take() else {
            return;
        };
        tracing::debug!(id = ?prompt.id, ?reply, "prompt answered");
        if !pane.control(SessionInput::PromptReply(prompt.id, reply)) {
            tracing::warn!("session gone; prompt answer dropped");
        }
    }

    /// Enter that no widget took, for the front pane's dialog: submits a secret prompt and
    /// cancels a changed-key dialog (its default); an unknown-key dialog needs a click.
    pub(crate) fn on_enter(&mut self) -> Task<Message> {
        if self.secret_dialog.is_some() {
            return self.on_secret_dialog(crate::secret_dialog::SecretMsg::Save);
        }
        let Some(pane) = self.active_pane() else {
            return Task::none();
        };
        let (id, message) = match pane.prompt.as_ref().map(|p| &p.kind) {
            Some(PromptKind::HostKeyChanged { .. }) => {
                (pane.id, PromptMsg::Choose(pane.id, Choice::Cancel))
            }
            Some(
                PromptKind::Password { .. }
                | PromptKind::KeyPassphrase { .. }
                | PromptKind::KeyboardInteractive { .. },
            ) => (pane.id, PromptMsg::Submit(pane.id)),
            Some(PromptKind::HostKeyUnknown { .. }) | None => return Task::none(),
        };
        tracing::debug!(pane = id.0, "enter in prompt");
        self.on_prompt_msg(message)
    }

    /// Escape closes the innermost overlay: picker, close / delete confirmation, the front
    /// pane's prompt (declined), menu, the sidebar's search (then its keyboard focus), inline
    /// rename.
    pub(crate) fn on_escape(&mut self) -> Task<Message> {
        if self.secret_dialog.take().is_some() || self.search.take().is_some() {
            return Task::none();
        }
        if self.picker.is_some() {
            return self.dismiss_picker();
        }
        if self.forwards.take().is_some()
            || self.pending_close.take().is_some()
            || self.pending_paste.take().is_some()
            || self.profiles.pending_delete.take().is_some()
        {
            return Task::none();
        }
        if let Some(pane) = self.active_pane().filter(|p| p.prompt.is_some()) {
            let id = pane.id;
            self.answer(id, PromptReply::Reject);
            return Task::none();
        }
        if self.menu.take().is_some() || self.sidebar_escape() {
            return Task::none();
        }
        let active = self.active;
        let panel = self.tabs.iter_mut().find(|t| t.id == active);
        if panel
            .and_then(|t| t.sftp.as_mut())
            .is_some_and(crate::sftp::SftpPanelState::cancel_op)
        {
            return Task::none();
        }
        self.renaming = None;
        Task::none()
    }
}

/// One line of config- or server-supplied text, safe to show.
fn safe(text: &str) -> String {
    sanitize_remote(text, PROMPT_TEXT_MAX_CHARS, false)
}

fn mono<'a>(value: String) -> Element<'a, Message> {
    text(value).font(Font::MONOSPACE).size(13).into()
}

/// `label` above a monospace `value`.
fn labelled(label: &str, value: String) -> Element<'_, Message> {
    column![text(label).size(12), mono(value)].spacing(2).into()
}

fn choose_button(label: &str, pane: PaneId, choice: Choice) -> button::Button<'_, Message> {
    button(text(label))
        .style(crate::style::primary)
        .on_press(Message::Prompt(PromptMsg::Choose(pane, choice)))
}

fn field(pane: PaneId, idx: usize, value: &SecretString, secret: bool) -> Element<'_, Message> {
    text_input("", value.expose_secret())
        .id(field_id(pane, idx))
        .secure(secret)
        .on_input(move |typed| Message::Prompt(PromptMsg::Input(pane, idx, PromptText::new(typed))))
        .on_submit(Message::Prompt(PromptMsg::FieldEnter(pane, idx)))
        .padding(6)
        .into()
}

/// The "Remember" part of a secret prompt: a checkbox when the Secret Service takes it (it
/// replaces a rejected stored secret, with a Forget button beside the note), else why not.
fn remember<'a>(pane: PaneId, state: &PromptState, what: &'static str) -> Element<'a, Message> {
    let tick = |label: String| {
        checkbox(state.remember)
            .label(label)
            .on_toggle(move |on| Message::Prompt(PromptMsg::Remember(pane, on)))
    };
    match remember_of(&state.kind) {
        Some(Remember::Available) => tick(format!("Remember {what}")).into(),
        Some(Remember::Rejected) => column![
            text(format!("The stored {what} was rejected.")).color(DANGER),
            row![
                tick(format!("Replace the stored {what}")),
                space().width(Length::Fill),
                choose_button("Forget it", pane, Choice::Forget).style(crate::style::secondary),
            ]
            .align_y(iced::Alignment::Center),
        ]
        .spacing(6)
        .into(),
        Some(Remember::Unavailable(why)) => text(format!("Cannot remember it: {}", safe(why)))
            .size(12)
            .into(),
        None => column![].into(),
    }
}

/// `[primary] [Cancel]` for a secret prompt.
fn submit_row(pane: PaneId, label: &str) -> Element<'_, Message> {
    row![
        button(text(label))
            .style(crate::style::primary)
            .on_press(Message::Prompt(PromptMsg::Submit(pane))),
        choose_button("Cancel", pane, Choice::Cancel).style(crate::style::secondary),
    ]
    .spacing(8)
    .into()
}

/// The dialog for pane `pane`'s open prompt.
pub(crate) fn view(pane: PaneId, state: &PromptState) -> Element<'_, Message> {
    let title = |t: &'static str| text(t).size(18);
    let content: Element<'_, Message> = match &state.kind {
        PromptKind::HostKeyUnknown {
            host,
            port,
            key_type,
            fingerprint,
        } => column![
            title("Unknown host key"),
            text(format!(
                "The authenticity of {}:{port} can't be established. Compare the fingerprint \
                 with the server's (ssh-keygen -lf) before trusting it.",
                safe(host)
            )),
            labelled("Key type", safe(key_type)),
            labelled("Fingerprint", safe(fingerprint)),
            row![
                choose_button("Accept once", pane, Choice::AcceptOnce)
                    .style(crate::style::secondary),
                choose_button("Accept and save", pane, Choice::AcceptAndSave),
                space().width(Length::Fill),
                choose_button("Cancel", pane, Choice::Cancel).style(crate::style::secondary),
            ]
            .spacing(8),
        ]
        .spacing(12)
        .into(),
        PromptKind::HostKeyChanged { .. } => changed_view(pane, state),
        PromptKind::Password { user, host, .. } => column![
            title("Password"),
            text(format!("Password for {}@{}", safe(user), safe(host))),
            first_field(pane, state),
            remember(pane, state, "password"),
            submit_row(pane, "Log in"),
        ]
        .spacing(12)
        .into(),
        PromptKind::KeyPassphrase { path, .. } => column![
            title("Key passphrase"),
            text("Passphrase for the private key"),
            mono(safe(&path.display().to_string())),
            first_field(pane, state),
            remember(pane, state, "passphrase"),
            submit_row(pane, "Unlock"),
        ]
        .spacing(12)
        .into(),
        PromptKind::KeyboardInteractive {
            name,
            instructions,
            prompts,
        } => {
            let heading = if name.is_empty() {
                "Authentication".to_owned()
            } else {
                safe(name)
            };
            let mut col = column![text(heading).size(18)].spacing(12);
            if !instructions.is_empty() {
                col = col.push(text(sanitize_remote(
                    instructions,
                    PROMPT_INSTRUCTIONS_MAX_CHARS,
                    true,
                )));
            }
            for (idx, (prompt, answer)) in prompts.iter().zip(&state.answers).enumerate() {
                col = col.push(
                    column![
                        text(safe(&prompt.text)),
                        field(pane, idx, answer, !prompt.echo)
                    ]
                    .spacing(4),
                );
            }
            col.push(submit_row(pane, "Submit")).into()
        }
    };
    // A jump host's prompt says so above everything else (its host differs from the tab's).
    let content = match &state.via {
        Some(via) => column![
            text(format!(
                "Jump host {} (on the way to this tab's host)",
                safe(via)
            ))
            .size(13),
            content
        ]
        .spacing(8)
        .into(),
        None => content,
    };
    modal::frame(scrollable(content).into(), DIALOG_W)
}

/// The masked field of a password / passphrase prompt.
fn first_field(pane: PaneId, state: &PromptState) -> Element<'_, Message> {
    state
        .answers
        .first()
        .map_or_else(|| column![].into(), |answer| field(pane, 0, answer, true))
}

/// The red "HOST KEY CHANGED" screen: `Cancel` is the default; replacing takes two clicks.
fn changed_view(pane: PaneId, state: &PromptState) -> Element<'_, Message> {
    let PromptKind::HostKeyChanged {
        host,
        port,
        key_type,
        fingerprint,
        old_fingerprint,
        known_hosts_path,
        line,
    } = &state.kind
    else {
        return column![].into();
    };
    let header = container(text("HOST KEY CHANGED").size(18).color(Color::WHITE))
        .padding([6, 10])
        .width(Length::Fill)
        .style(|_theme: &iced::Theme| container::background(DANGER));
    let replace = if state.replace_armed {
        choose_button("Are you sure? Replace and connect", pane, Choice::Replace)
    } else {
        choose_button("Replace and connect", pane, Choice::ArmReplace)
    };
    column![
        header,
        text(format!(
            "The host key of {}:{port} does not match the recorded one. Someone may be \
             intercepting the connection (man-in-the-middle attack), or the server's key was \
             replaced. Do not continue unless you know why it changed.",
            safe(host)
        )),
        labelled("Recorded fingerprint", safe(old_fingerprint)),
        labelled(
            "Offered fingerprint",
            format!("{} {}", safe(key_type), safe(fingerprint))
        ),
        labelled(
            "Stale entry",
            format!("{}:{line}", safe(&known_hosts_path.display().to_string()))
        ),
        row![
            choose_button("Cancel", pane, Choice::Cancel).style(crate::style::primary),
            space().width(Length::Fill),
            replace.style(button::danger),
        ]
        .spacing(8),
    ]
    .spacing(12)
    .into()
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
