//! The panel's modal dialogs: "file exists" (`conflict.rs`), delete confirmation and
//! permissions. One at a time, in that order.

use iced::widget::{Space, button, checkbox, column, container, row, text, text_input};
use iced::{Alignment, Element, Length};

use super::chmod::CLASSES;
use super::ops::delete_text;
use super::panel::msg;
use super::{PanelMsg, SftpPanelState};
use crate::message::Message;
use crate::modal;
use crate::tab::TabId;
use fogu_session::display_name;

/// The dialog the panel needs answered, if any.
pub fn dialog(tab: TabId, panel: &SftpPanelState) -> Option<Element<'_, Message>> {
    super::conflict::conflict_dialog(tab, panel)
        .or_else(|| delete_dialog(tab, panel))
        .or_else(|| chmod_dialog(tab, panel))
}

fn delete_dialog(tab: TabId, panel: &SftpPanelState) -> Option<Element<'_, Message>> {
    let d = panel.ops.delete.as_ref()?;
    let ready = d.waiting.is_empty();
    // No default button: Enter never deletes.
    let buttons = row![
        button(text("Delete"))
            .style(button::danger)
            .on_press_maybe(ready.then(|| msg(tab, PanelMsg::DeleteConfirm(true)))),
        button(text("Cancel"))
            .style(crate::style::secondary)
            .on_press(msg(tab, PanelMsg::DeleteConfirm(false))),
    ]
    .spacing(8);
    let content = column![
        text("Delete").size(18),
        text(delete_text(d)).wrapping(text::Wrapping::WordOrGlyph),
        buttons,
    ]
    .spacing(12);
    Some(modal::frame(content.into(), 440.0))
}

fn chmod_dialog(tab: TabId, panel: &SftpPanelState) -> Option<Element<'_, Message>> {
    let d = panel.ops.chmod.as_ref()?;
    let cell = |content: Element<'static, Message>| container(content).width(64);
    let mut grid = column![row![
        cell(Space::new().into()),
        cell(text("Read").into()),
        cell(text("Write").into()),
        cell(text("Execute").into()),
    ]]
    .spacing(6);
    for (class, bits) in CLASSES {
        let mut line = row![cell(text(class).into())].align_y(Alignment::Center);
        for bit in bits {
            let boxed =
                checkbox(d.mode & bit != 0).on_toggle(move |_| msg(tab, PanelMsg::ChmodBit(bit)));
            line = line.push(cell(boxed.into()));
        }
        grid = grid.push(line);
    }
    let valid = d.result().is_some();
    let octal = row![
        text("Octal"),
        text_input("644", &d.octal)
            .on_input(move |v| msg(tab, PanelMsg::ChmodOctal(v)))
            .on_submit_maybe(valid.then(|| msg(tab, PanelMsg::ChmodDone(true))))
            .width(80)
            .padding(4),
        text(if valid { "" } else { "1\u{2013}4 octal digits" }).size(12),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let buttons = row![
        button(text("Apply"))
            .style(crate::style::primary)
            .on_press_maybe(valid.then(|| msg(tab, PanelMsg::ChmodDone(true)))),
        button(text("Cancel"))
            .style(crate::style::secondary)
            .on_press(msg(tab, PanelMsg::ChmodDone(false))),
    ]
    .spacing(8);
    let content = column![
        text("Permissions").size(18),
        text(format!("\u{201c}{}\u{201d}", display_name(&d.name)))
            .wrapping(text::Wrapping::WordOrGlyph),
        grid,
        octal,
        buttons,
    ]
    .spacing(12)
    .width(Length::Shrink);
    Some(modal::frame(content.into(), 420.0))
}
