//! Drawing of the SFTP panel (stock iced widgets, Nocturne): header (host tag, Follow shell),
//! navigation and breadcrumb, actions and filter, column headers, a virtualized entry list, and
//! the transfers strip; plus the conflict dialog.
//! Every remote string shown here went through `display_name`.

use std::fmt::Write as _;

use iced::widget::{
    Id, Space, button, column, container, mouse_area, row, scrollable, text, text_input, tooltip,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use super::breadcrumb::segments;
use super::{PanelMsg, SftpMsg, SftpPanelState};
use crate::icons::{Icon, PHOSPHOR, glyph, icon};
use crate::message::Message;
use crate::palette::Colors;
use crate::style::{self, Btn};
use crate::tab::TabId;
use crate::tokens as t;

/// Height of one list row (keyboard scrolling uses it too).
pub const ROW_H: f32 = 26.0;
/// The entry list's scrollable.
pub const LIST_ID: &str = "fogu-sftp-list";
/// The filter field.
pub const FILTER_INPUT: &str = "fogu-sftp-filter";
/// The typed-path field.
pub const PATH_INPUT: &str = "fogu-sftp-path";
pub(super) const SIZE: f32 = 13.0;

/// Height in pixels of `rows` list rows.
#[expect(
    clippy::cast_precision_loss,
    reason = "row counts stay far below 2^24, where f32 is exact"
)]
pub fn rows_px(rows: usize) -> f32 {
    rows as f32 * ROW_H
}

/// Whole rows in `px` pixels (negative = 0).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "floored and clamped to 0..=1e9 before the cast"
)]
pub(super) fn px_rows(px: f32) -> usize {
    (px / ROW_H).floor().clamp(0.0, 1e9) as usize
}

pub(crate) use crate::palette::Look;

pub(super) fn msg(tab: TabId, message: PanelMsg) -> Message {
    Message::Sftp(SftpMsg::Panel(tab, message))
}

/// A flat text button in the panel's colours; `None` = disabled.
pub(crate) fn flat<'a>(
    label: impl Into<String>,
    look: Look,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let fg = look.fg;
    button(text(label.into()).size(SIZE))
        .padding([2, 6])
        .style(move |_theme: &Theme, status| {
            let hover = Color { a: 0.14, ..fg };
            button::Style {
                background: match status {
                    button::Status::Hovered | button::Status::Pressed => {
                        Some(Background::Color(hover))
                    }
                    _ => None,
                },
                text_color: match status {
                    button::Status::Disabled => Color { a: 0.35, ..fg },
                    _ => fg,
                },
                border: Border::default().rounded(3),
                ..button::Style::default()
            }
        })
        .on_press_maybe(on_press)
        .into()
}

pub(super) fn label<'a>(value: impl text::IntoFragment<'a>, color: Color) -> text::Text<'a> {
    text(value)
        .size(SIZE)
        .color(color)
        .wrapping(text::Wrapping::None)
}

/// A 28 px icon button with a tooltip; disabled when `on_press` is `None`.
fn icon_button(
    glyph: Icon,
    tip: &str,
    look: Look,
    on_press: Option<Message>,
) -> Element<'_, Message> {
    let color = if on_press.is_some() {
        look.fg
    } else {
        Color {
            a: 0.35,
            ..look.dim
        }
    };
    tooltip(
        button(icon(glyph, 15.0, color))
            .width(28)
            .height(28)
            .padding(0)
            .style(style::icon_btn(look, false))
            .on_press_maybe(on_press),
        container(text(tip).size(12))
            .padding(4)
            .style(container::rounded_box),
        tooltip::Position::Bottom,
    )
    .into()
}

/// A button with an icon and a label in the `kind` style.
fn labeled_button<'a>(
    glyph: Icon,
    label: &str,
    kind: Btn,
    look: Look,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let tint = match kind {
        Btn::Ghost | Btn::Primary => look.accent_hi,
        _ => look.dim,
    };
    button(
        row![
            glyph_of(glyph, 13.0, tint),
            text(label.to_owned()).size(12.5)
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([4, 10])
    .style(style::btn(look, kind))
    .on_press_maybe(on_press)
    .into()
}

fn glyph_of<'a>(icon: Icon, size: f32, color: Color) -> Element<'a, Message> {
    glyph(icon, size, color).into()
}

/// The whole panel of tab `tab`; `host` is shown as a tag in the header.
#[expect(clippy::too_many_lines, reason = "one widget tree, top to bottom")]
pub fn view<'a>(
    tab: TabId,
    panel: &'a SftpPanelState,
    colors: &Colors,
    host: String,
) -> Element<'a, Message> {
    let look = Look::new(colors);
    let has_path = panel.path.is_some();
    let mut header = row![
        glyph(Icon::Folder, 16.0, look.accent_hi),
        text("SFTP")
            .size(13.5)
            .font(crate::UI_MEDIUM)
            .color(look.fg),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if !host.is_empty() {
        header = header.push(
            container(text(host).size(11).color(t::A100))
                .padding([2, 7])
                .style(|_theme: &Theme| style::tag(t::A800)),
        );
    }
    let header = header
        .push(Space::new().width(Length::Fill))
        .push(follow_controls(tab, panel, look))
        .push(
            button(icon(Icon::Close, 14.0, look.dim))
                .width(26)
                .height(26)
                .padding(0)
                .style(style::icon_btn(look, false))
                .on_press(msg(tab, PanelMsg::Close)),
        );
    let nav = row![
        icon_button(
            Icon::ArrowLeft,
            "Back",
            look,
            (!panel.back.is_empty()).then(|| msg(tab, PanelMsg::Back))
        ),
        icon_button(
            Icon::ArrowUp,
            "Up",
            look,
            has_path.then(|| msg(tab, PanelMsg::Up))
        ),
        icon_button(Icon::Home, "Home", look, Some(msg(tab, PanelMsg::Home))),
        icon_button(
            Icon::Refresh,
            "Refresh",
            look,
            Some(msg(tab, PanelMsg::Refresh))
        ),
        breadcrumb(tab, panel, look),
    ]
    .spacing(2)
    .align_y(Alignment::Center);
    let filter = text_input("Filter", &panel.listing.filter)
        .id(Id::new(FILTER_INPUT))
        .on_input(move |f| msg(tab, PanelMsg::Filter(f)))
        .icon(text_input::Icon {
            font: PHOSPHOR,
            code_point: Icon::Filter.glyph(),
            size: Some(13.into()),
            spacing: 6.0,
            side: text_input::Side::Left,
        })
        .size(12.5)
        .padding([5, 8])
        .width(Length::Fill)
        .style(style::input(look));
    let hidden = row![
        crate::forms::switch(
            look,
            panel.listing.show_hidden,
            msg(tab, PanelMsg::ShowHidden(!panel.listing.show_hidden))
        ),
        label("Hidden", look.dim).size(12),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let actions = row![
        labeled_button(
            Icon::Upload,
            "Upload",
            Btn::Secondary,
            look,
            has_path.then(|| msg(tab, PanelMsg::Upload))
        ),
        labeled_button(
            Icon::Download,
            "Download",
            Btn::Secondary,
            look,
            (!panel.listing.selected.is_empty()).then(|| msg(tab, PanelMsg::Download))
        ),
        labeled_button(
            Icon::FolderPlus,
            "New folder",
            Btn::Ghost,
            look,
            has_path.then(|| msg(tab, PanelMsg::NewFolder))
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .wrap()
    .vertical_spacing(6);
    let filter = row![hidden, filter].spacing(8).align_y(Alignment::Center);
    let status = label(status_text(panel), look.dim).size(11);
    let mut body = column![
        header,
        nav,
        actions,
        filter,
        super::list_view::column_headers(tab, panel, look),
        super::list_view::entries(tab, panel, look),
        status,
    ]
    .spacing(8);
    if !panel.transfers.items.is_empty() {
        body = body.push(super::transfer_view::transfers(tab, panel, look));
    }
    let (bg, edge) = (look.bg, look.border);
    let content = container(body)
        .padding([12, 14])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_theme: &Theme| {
            container::background(bg).border(Border::default().width(1).color(edge))
        });
    mouse_area(content)
        .on_enter(msg(tab, PanelMsg::Hover(true)))
        .on_exit(msg(tab, PanelMsg::Hover(false)))
        .into()
}

/// "Follow shell" switch; when off and the terminal is elsewhere, a "resume" link.
fn follow_controls(tab: TabId, panel: &SftpPanelState, look: Look) -> Element<'_, Message> {
    let toggle = row![
        label("Follow shell", look.dim).size(12),
        crate::forms::switch(
            look,
            panel.follow,
            msg(tab, PanelMsg::Follow(!panel.follow))
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let elsewhere = panel
        .shell_cwd
        .as_ref()
        .is_some_and(|cwd| panel.path.as_ref() != Some(cwd));
    if panel.follow || !elsewhere {
        return toggle.into();
    }
    row![
        button(text("resume").size(12))
            .padding([2, 6])
            .style(style::btn(look, Btn::Ghost))
            .on_press(msg(tab, PanelMsg::Follow(true))),
        toggle,
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

fn status_text(panel: &SftpPanelState) -> String {
    if panel.loading.is_some() {
        return "Loading…".to_owned();
    }
    let shown = panel.listing.visible().len();
    let selected = panel.listing.selected.len();
    let mut out = format!("{shown} items");
    let hidden = panel.listing.hidden_count();
    if hidden > 0 {
        let _infallible = write!(out, ", {hidden} hidden");
    }
    if panel.listing.truncated {
        out.push_str(" (truncated)");
    }
    if selected > 0 {
        let _infallible = write!(out, ", {selected} selected");
    }
    out
}

/// Clickable segments (hover pills, the current one in the accent) and "cd here"; a click on
/// the empty bar switches to a path field.
fn breadcrumb(tab: TabId, panel: &SftpPanelState, look: Look) -> Element<'_, Message> {
    if let Some(value) = &panel.editing {
        return text_input("/path/to/dir", value)
            .id(Id::new(PATH_INPUT))
            .on_input(move |v| msg(tab, PanelMsg::PathInput(v)))
            .on_submit(msg(tab, PanelMsg::PathSubmit))
            .size(12.5)
            .padding([5, 8])
            .style(style::input(look))
            .into();
    }
    let mut parts = row![].spacing(1).align_y(Alignment::Center);
    let segs = panel.path.as_deref().map(segments).unwrap_or_default();
    let last = segs.len().saturating_sub(1);
    for (i, seg) in segs.into_iter().enumerate() {
        if i > 1 {
            parts = parts.push(glyph(Icon::ChevronRight, 10.0, t::N600));
        }
        let current = i == last;
        parts = parts.push(
            button(text(seg.label).size(12.5))
                .padding([2, 5])
                .style(move |theme: &Theme, status| {
                    let mut s = style::btn(look, Btn::Flat)(theme, status);
                    s.border.radius = 5.0.into();
                    if current {
                        s.text_color = if look.dark { t::A300 } else { look.accent };
                    }
                    s
                })
                .on_press(msg(tab, PanelMsg::Go(seg.path))),
        );
    }
    let crumbs = mouse_area(
        container(
            // Anchored right: a deep path shows its last segments.
            scrollable(parts)
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(0).scroller_width(0),
                ))
                .anchor_right(),
        )
        .width(Length::Fill),
    )
    .on_press(msg(tab, PanelMsg::EditPath));
    let cd = button(text("cd here").size(12))
        .padding([2, 6])
        .style(style::btn(look, Btn::Ghost))
        .on_press_maybe(panel.path.is_some().then(|| msg(tab, PanelMsg::CdHere)));
    let (surface, edge) = (look.surface, look.border);
    container(row![crumbs, cd].spacing(4).align_y(Alignment::Center))
        .width(Length::Fill)
        .padding([2, 4])
        .style(move |_theme: &Theme| {
            container::background(surface).border(Border::default().rounded(7).width(1).color(edge))
        })
        .into()
}
