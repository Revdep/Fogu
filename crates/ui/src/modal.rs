//! The one modal overlay: dims the view behind it and blocks every event from reaching it.
//! [`overlay`] shows a plain title / body / buttons dialog (paste confirmation, close
//! confirmation); [`over`] + [`frame`] carry richer content (the SSH prompt dialogs).

use iced::Element;
use iced::widget::{button, center, column, container, opaque, row, stack, text};

use crate::message::Message;

/// One modal button; clicking it publishes `on_press`.
#[derive(Debug)]
pub struct ModalButton {
    /// Button label.
    pub label: &'static str,
    /// Message published on click.
    pub on_press: Message,
}

/// A modal's content: a title, body text, and buttons left to right.
#[derive(Debug)]
pub struct Modal {
    /// Title line.
    pub title: &'static str,
    /// Body text.
    pub body: String,
    /// Buttons, left to right.
    pub buttons: Vec<ModalButton>,
}

/// Stacks `modal` over `base`; just `base` when `modal` is `None`.
pub fn overlay(base: Element<'_, Message>, modal: Option<Modal>) -> Element<'_, Message> {
    let Some(modal) = modal else { return base };
    // The first button is the action (outlined in the accent), the others back out.
    let mut buttons = row![].spacing(8);
    for (i, b) in modal.buttons.into_iter().enumerate() {
        let style = if i == 0 {
            crate::style::primary
        } else {
            crate::style::secondary
        };
        buttons = buttons.push(
            button(text(b.label).size(13))
                .padding([6, 14])
                .style(style)
                .on_press(b.on_press),
        );
    }
    let content = column![
        text(modal.title).size(19).font(crate::UI_MEDIUM),
        text(modal.body).size(13),
        buttons
    ]
    .spacing(14);
    over(base, frame(content.into(), 380.0))
}

/// The dialog box around `content`, at most `max_width` wide.
pub fn frame(content: Element<'_, Message>, max_width: f32) -> Element<'_, Message> {
    container(content)
        .padding([20, 22])
        .max_width(max_width)
        .style(crate::style::dialog)
        .into()
}

/// Centers `dialog` over `base`, which is dimmed and receives no events.
pub fn over<'a>(base: Element<'a, Message>, dialog: Element<'a, Message>) -> Element<'a, Message> {
    let backdrop = center(dialog)
        .style(|_theme: &iced::Theme| crate::tokens::alpha(crate::tokens::N900, 0.6).into());
    stack![base, opaque(backdrop)].into()
}
