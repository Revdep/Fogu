//! "Set stored password / passphrase" from the profile editor: a small modal with one masked
//! field. The typed text rides in [`PromptText`] (redacted `Debug`, zeroized on drop) straight
//! into the dialog's `SecretString`; Save moves it into the Secret Service call, so no copy stays
//! in the app state. Nothing is shown back: the editor only says "stored" / "not stored".

use fogu_core::SecretKey;
use fogu_session::sanitize_remote;
use iced::widget::{Id, button, column, row, text, text_input};
use iced::{Element, Length, Task};
use secrecy::{ExposeSecret as _, SecretString};

use crate::app::App;
use crate::message::Message;
use crate::modal;
use crate::prompt::PromptText;
use crate::tab::TabId;

const DIALOG_W: f32 = 440.0;

/// The open dialog.
#[derive(Debug)]
pub struct SecretDialog {
    /// The editor tab that asked (refreshed after saving).
    pub tab: TabId,
    pub key: SecretKey,
    /// What is being set, e.g. `password for alice@host` (sanitized).
    pub what: String,
    value: SecretString,
    /// Nothing typed yet (Save does nothing).
    empty: bool,
}

impl SecretDialog {
    pub fn new(tab: TabId, key: SecretKey, what: &str) -> Self {
        Self {
            tab,
            key,
            what: sanitize_remote(what, 200, false),
            value: SecretString::default(),
            empty: true,
        }
    }
}

/// Dialog input. None carries a secret except [`PromptText`], which is redacted.
#[derive(Clone, Debug)]
pub enum SecretMsg {
    Input(PromptText),
    Save,
    Cancel,
}

fn field_id() -> Id {
    Id::new("fogu-secret-dialog")
}

impl App {
    /// Opens the dialog and focuses its field.
    pub(crate) fn open_secret_dialog(&mut self, dialog: SecretDialog) -> Task<Message> {
        self.secret_dialog = Some(dialog);
        iced::widget::operation::focus(field_id())
    }

    pub(crate) fn on_secret_dialog(&mut self, message: SecretMsg) -> Task<Message> {
        match message {
            SecretMsg::Input(typed) => {
                if let Some(dialog) = self.secret_dialog.as_mut() {
                    dialog.empty = typed.is_empty();
                    dialog.value = typed.into_secret();
                }
            }
            SecretMsg::Cancel => self.secret_dialog = None,
            SecretMsg::Save => return self.save_secret(),
        }
        Task::none()
    }

    /// Moves the typed secret into the keyring call; an empty one is not stored.
    fn save_secret(&mut self) -> Task<Message> {
        let Some(dialog) = self.secret_dialog.take() else {
            return Task::none();
        };
        let SecretDialog {
            tab,
            key,
            value,
            empty,
            ..
        } = dialog;
        if empty {
            return Task::none();
        }
        self.store_set(tab, key, value)
    }
}

/// The dialog box.
pub(crate) fn view(dialog: &SecretDialog) -> Element<'_, Message> {
    let input = text_input("", dialog.value.expose_secret())
        .id(field_id())
        .secure(true)
        .on_input(|typed| Message::SecretDialog(SecretMsg::Input(PromptText::new(typed))))
        .on_submit(Message::SecretDialog(SecretMsg::Save))
        .padding(6);
    let content = column![
        text("Stored secret").size(18),
        text(format!(
            "Set the {}. It is kept in your desktop's Secret Service, never in config.toml.",
            dialog.what
        )),
        input,
        row![
            button(text("Save"))
                .style(crate::style::primary)
                .on_press(Message::SecretDialog(SecretMsg::Save)),
            button(text("Cancel"))
                .style(crate::style::secondary)
                .on_press(Message::SecretDialog(SecretMsg::Cancel)),
        ]
        .spacing(8),
    ]
    .spacing(12)
    .width(Length::Fill);
    modal::frame(content.into(), DIALOG_W)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_is_redacted_in_messages_and_in_the_dialog_state() {
        let typed = PromptText::new("hunter2".to_owned());
        let message = Message::SecretDialog(SecretMsg::Input(typed.clone()));
        assert!(!format!("{message:?}").contains("hunter2"));
        let mut dialog = SecretDialog::new(
            TabId(1),
            SecretKey::KeyPassphrase {
                key_fingerprint: "f".into(),
            },
            "passphrase of /k/a",
        );
        dialog.value = typed.into_secret();
        assert!(!format!("{dialog:?}").contains("hunter2"));
        assert_eq!(dialog.value.expose_secret(), "hunter2");
    }
}
