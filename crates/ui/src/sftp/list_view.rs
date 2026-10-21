//! The panel's entry list: column headers and a virtualized list (only rows in view are
//! built), with the inline name field for rename / new folder. Every remote string shown here
//! went through `display_name`.

use fogu_session::sftp::Entry;
use iced::widget::{
    Id, Space, column, container, mouse_area, row, scrollable, text, text_input, tooltip,
};
use iced::{Alignment, Background, Border, Color, Element, Font, Length, Theme};

use super::listing::{self, SortKey, shown_name};
use super::panel::{LIST_ID, Look, ROW_H, SIZE, label, msg, px_rows, rows_px};
use super::{PanelMsg, SftpPanelState};
use crate::icons::{Icon, glyph};
use crate::message::Message;
use crate::tab::TabId;
use crate::tokens as t;

/// The inline name field (rename / new folder).
pub const EDIT_INPUT: &str = "fogu-sftp-edit";
/// Rows drawn before the first scroll event says how tall the list is.
const FIRST_ROWS: usize = 60;
/// Names longer than this (in characters) get a tooltip.
const LONG_NAME: usize = 24;
const SIZE_W: f32 = 68.0;
const DATE_W: f32 = 112.0;
const MODE_W: f32 = 78.0;
const ICON_W: f32 = 20.0;

/// An entry's icon and its colour: folders in the accent, archives, other files.
fn entry_icon(entry: &Entry, look: Look) -> (Icon, Color) {
    if listing::is_dir(entry) {
        return (Icon::Folder, look.accent_hi);
    }
    let name = entry.name.to_ascii_lowercase();
    let archive = [".gz", ".tgz", ".zip", ".xz", ".bz2", ".zst", ".7z", ".tar"]
        .iter()
        .any(|ext| name.ends_with(ext));
    (
        if archive {
            Icon::FileZip
        } else {
            Icon::FileText
        },
        look.dim,
    )
}

pub(super) fn column_headers<'a>(
    tab: TabId,
    panel: &SftpPanelState,
    look: Look,
) -> Element<'a, Message> {
    let l = &panel.listing;
    let head = |name: &str, key: SortKey| {
        let arrow = match (l.sort == key, l.descending) {
            (true, false) => " ↑",
            (true, true) => " ↓",
            (false, _) => "",
        };
        iced::widget::button(text(format!("{}{arrow}", name.to_uppercase())).size(10.5))
            .padding([2, 0])
            .style(move |_theme: &Theme, status| iced::widget::button::Style {
                text_color: if matches!(status, iced::widget::button::Status::Hovered) {
                    if look.dark { t::N300 } else { look.fg }
                } else {
                    look.dim
                },
                ..iced::widget::button::Style::default()
            })
            .on_press(msg(tab, PanelMsg::Sort(key)))
    };
    let edge = look.border;
    let rule = container(Space::new())
        .width(Length::Fill)
        .height(1)
        .style(move |_theme: &Theme| container::background(edge));
    column![
        row![
            Space::new().width(ICON_W),
            container(head("Name", SortKey::Name)).width(Length::Fill),
            container(head("Size", SortKey::Size))
                .width(SIZE_W)
                .align_x(Alignment::End),
            container(head("Modified", SortKey::Modified)).width(DATE_W),
            container(head("Perms", SortKey::Mode)).width(MODE_W),
        ]
        .spacing(8)
        .padding([0, 8]),
        rule,
    ]
    .spacing(4)
    .into()
}

/// The entry list, drawing only the rows in view.
pub(super) fn entries(tab: TabId, panel: &SftpPanelState, look: Look) -> Element<'_, Message> {
    let list = rows(tab, panel, look);
    match panel.ops.edit.as_ref().filter(|e| e.old.is_none()) {
        // "New folder": the name field sits above the list.
        Some(edit) => {
            let field = row![
                container(glyph(Icon::Folder, 14.0, look.accent_hi)).width(ICON_W),
                name_field(tab, look, &edit.value, "New folder name, Enter to create"),
            ]
            .spacing(4)
            .align_y(Alignment::Center);
            column![container(field).padding([0, 4]), list]
                .spacing(2)
                .into()
        }
        None => list,
    }
}

/// The inline name field (Enter submits, Esc cancels).
fn name_field<'a>(tab: TabId, look: Look, value: &str, placeholder: &str) -> Element<'a, Message> {
    text_input(placeholder, value)
        .id(Id::new(EDIT_INPUT))
        .on_input(move |v| msg(tab, PanelMsg::NameInput(v)))
        .on_submit(msg(tab, PanelMsg::NameSubmit))
        .size(SIZE)
        .padding([1, 3])
        .width(Length::Fill)
        .style(crate::style::input(look))
        .into()
}

fn rows(tab: TabId, panel: &SftpPanelState, look: Look) -> Element<'_, Message> {
    if let Some(error) = &panel.error {
        return container(label(error.clone(), look.dim).wrapping(text::Wrapping::Word))
            .height(Length::Fill)
            .into();
    }
    let visible = panel.listing.visible();
    if visible.is_empty() && panel.loading.is_none() && panel.path.is_some() {
        let hidden = panel.listing.hidden_count();
        let why = if panel.listing.entries.is_empty() {
            "Empty folder".to_owned()
        } else if hidden == panel.listing.entries.len() {
            format!("Only hidden files here ({hidden})")
        } else {
            "Nothing matches the filter".to_owned()
        };
        let mut body = column![label(why, look.dim)].spacing(6);
        if hidden > 0 {
            body = body.push(
                iced::widget::button(label("Show hidden files", look.fg))
                    .padding([4, 10])
                    .style(crate::style::btn(look, crate::style::Btn::Secondary))
                    .on_press(msg(tab, PanelMsg::ShowHidden(true))),
            );
        }
        return container(body).padding(8).height(Length::Fill).into();
    }
    let first = px_rows(panel.scroll_y);
    let count = if panel.view_h > 0.0 {
        px_rows(panel.view_h) + 3
    } else {
        FIRST_ROWS
    };
    let first = first.min(visible.len());
    let end = (first + count).min(visible.len());
    let mut list = column![Space::new().height(rows_px(first))];
    for entry in visible.get(first..end).unwrap_or_default() {
        list = list.push(entry_row(tab, panel, entry, look));
    }
    list = list.push(Space::new().height(rows_px(visible.len() - end)));
    scrollable(list)
        .id(Id::new(LIST_ID))
        .style(crate::style::scrollbar)
        .height(Length::Fill)
        .on_scroll(move |v| {
            msg(
                tab,
                PanelMsg::Scrolled(v.absolute_offset().y, v.bounds().height),
            )
        })
        .into()
}

/// The name, clipped; a long one gets a tooltip with the whole (sanitized) name.
fn name_cell<'a>(entry: &Entry, color: Color) -> Element<'a, Message> {
    let name = shown_name(entry);
    let long = name.chars().count() > LONG_NAME;
    let cell = container(label(name.clone(), color))
        .width(Length::Fill)
        .clip(true);
    if !long {
        return cell.into();
    }
    let tip = container(text(name).size(12).wrapping(text::Wrapping::WordOrGlyph))
        .padding(4)
        .max_width(420)
        .style(container::rounded_box);
    tooltip(cell, tip, tooltip::Position::Bottom).into()
}

fn entry_row<'a>(
    tab: TabId,
    panel: &SftpPanelState,
    entry: &Entry,
    look: Look,
) -> Element<'a, Message> {
    let selected = panel.listing.selected.contains(&entry.name);
    let cursor = panel.listing.cursor.as_deref() == Some(entry.name.as_str());
    let name_color = look.fg;
    let (kind, tint) = entry_icon(entry, look);
    let renaming = panel
        .ops
        .edit
        .as_ref()
        .filter(|e| e.old.as_deref() == Some(entry.name.as_str()));
    let name = match renaming {
        Some(edit) => name_field(tab, look, &edit.value, "New name, Enter to rename"),
        None => name_cell(entry, name_color),
    };
    let cells = row![
        container(glyph(kind, 14.0, tint)).width(ICON_W),
        name,
        label(listing::human_size(entry), look.dim)
            .size(12)
            .width(SIZE_W)
            .align_x(Alignment::End),
        label(listing::modified(entry), look.dim)
            .size(12)
            .width(DATE_W),
        label(listing::permissions(entry), look.dim)
            .font(Font::MONOSPACE)
            .size(11)
            .width(MODE_W),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let (fg, accent) = (look.fg, look.accent);
    let body = container(cells)
        .height(ROW_H)
        .padding([0, 8])
        .align_y(Alignment::Center)
        .style(move |_theme: &Theme| container::Style {
            background: selected.then_some(Background::Color(Color { a: 0.16, ..accent })),
            border: Border {
                color: if cursor {
                    Color { a: 0.35, ..fg }
                } else {
                    Color::TRANSPARENT
                },
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        });
    mouse_area(body)
        .on_press(msg(tab, PanelMsg::Click(entry.name.clone())))
        .on_double_click(msg(tab, PanelMsg::Open(entry.name.clone())))
        .on_right_press(msg(tab, PanelMsg::EntryMenu(entry.name.clone())))
        .into()
}
