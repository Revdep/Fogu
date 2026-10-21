//! The transfers strip of the SFTP panel.

use iced::widget::{Space, column, container, progress_bar, row, scrollable};
use iced::{Alignment, Background, Border, Element, Length, Theme};

use super::listing::human_bytes;
use super::panel::{Look, flat, label, msg};
use super::transfers::{Direction, Transfer, TransferState};
use super::{PanelMsg, SftpPanelState};
use crate::message::Message;
use crate::tab::TabId;

/// The transfers strip: header with collapse / clear, then one block per transfer.
pub(super) fn transfers(tab: TabId, panel: &SftpPanelState, look: Look) -> Element<'_, Message> {
    let list = &panel.transfers;
    let active = list.active_count();
    let arrow = if list.collapsed { "▸" } else { "▾" };
    let header = row![
        flat(
            format!("{arrow} Transfers ({active} running)"),
            look,
            Some(msg(tab, PanelMsg::ToggleTransfers))
        ),
        Space::new().width(Length::Fill),
        flat(
            "Clear",
            look,
            (list.items.len() > active).then(|| msg(tab, PanelMsg::ClearTransfers))
        ),
    ]
    .align_y(Alignment::Center);
    let mut strip = column![header].spacing(4);
    if !list.collapsed {
        let rows = column(list.items.iter().rev().map(|t| transfer_row(tab, t, look))).spacing(6);
        strip = strip.push(
            scrollable(rows)
                .height(Length::Shrink)
                .style(crate::style::scrollbar),
        );
    }
    container(strip)
        .max_height(220)
        .padding([8, 10])
        .style(move |_theme: &Theme| crate::style::card(look, 8.0))
        .into()
}

fn transfer_row(tab: TabId, t: &Transfer, look: Look) -> Element<'_, Message> {
    // Finished transfers fade until they are swept away.
    let fg = if t.is_active() { look.fg } else { look.dim };
    let arrow = match t.direction {
        Direction::Download => "↓",
        Direction::Upload => "↑",
    };
    let state = match &t.state {
        TransferState::Planning => "preparing…".to_owned(),
        TransferState::Running if t.rate > 0 => format!("{}/s", human_bytes(t.rate)),
        TransferState::Running => String::new(),
        TransferState::Done if t.failed > 0 && t.done_files == 0 => "failed".to_owned(),
        TransferState::Done if t.done_files == 0 && t.skipped > 0 => "skipped".to_owned(),
        TransferState::Done if t.failed > 0 => format!("done, {} failed", t.failed),
        TransferState::Done => "done".to_owned(),
        TransferState::Failed(why) => format!("failed: {why}"),
        TransferState::Cancelled => "cancelled".to_owned(),
    };
    let action = if t.is_active() {
        flat("Cancel", look, Some(msg(tab, PanelMsg::Cancel(t.id))))
    } else {
        Space::new().into()
    };
    let top = row![
        label(arrow, look.accent),
        container(label(t.label.as_str(), fg))
            .width(Length::Fill)
            .clip(true),
        action,
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let accent = look.accent;
    let bar = progress_bar(0.0..=1.0, t.fraction())
        .girth(3)
        .style(move |_theme: &Theme| progress_bar::Style {
            background: Background::Color(crate::tokens::N900),
            bar: Background::Color(accent),
            border: Border::default().rounded(2),
        });
    let bytes = format!("{} / {}", human_bytes(t.bytes), human_bytes(t.total));
    let bottom = row![
        label(bytes, fg),
        container(label(state, fg)).width(Length::Fill).clip(true),
    ]
    .spacing(8);
    column![top, bar, bottom].spacing(2).into()
}
