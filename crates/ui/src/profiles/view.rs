//! Drawing of the profile sidebar (stock iced widgets, Nocturne): title, search, quick connect,
//! then the sections and rows from [`lines`]; a row's dot says whether its profile is live, and
//! hovering it swaps its detail for Split / Edit buttons.

use std::collections::HashSet;

use fogu_core::config::ProfileClick;
use fogu_core::{Action, Config, ProfileId};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    Id, button, column, container, mouse_area, row, scrollable, space, text, text_input,
};
use iced::{Alignment, Element, Font, Length, mouse};

use super::handlers::{Launch, ProfilesMsg};
use super::{Line, ProfilesView, QUICK_INPUT, SEARCH_INPUT, entries, lines, now_secs, rows};
use crate::drag::{DragMsg, Payload};
use crate::fuzzy_list::{self, Mark, RowEvents, RowLook};
use crate::icons::{Icon, PHOSPHOR, glyph, icon};
use crate::message::Message;
use crate::palette::Look;
use crate::style;
use crate::tokens::{self as t, alpha};

/// Width of the drag handle on the sidebar's left edge.
pub(crate) const HANDLE_W: f32 = 4.0;

fn msg(m: ProfilesMsg) -> Message {
    Message::Profiles(m)
}

/// `body` with the sidebar on its right when open; while its edge is dragged, pointer moves
/// anywhere in the window resize it.
/// `live` holds the profiles with a running session (their dot is lit).
pub(crate) fn with_sidebar<'a>(
    body: Element<'a, Message>,
    view: &'a ProfilesView,
    config: &Config,
    live: &HashSet<ProfileId>,
    look: Look,
) -> Element<'a, Message> {
    if !view.open {
        return body;
    }
    let (edge, bg) = (look.border, look.bg);
    // A 1 px divider, the rest of the grab area in the panel's colour.
    let handle = mouse_area(row![
        container(space())
            .width(1)
            .height(Length::Fill)
            .style(move |_theme: &iced::Theme| container::background(edge)),
        container(space())
            .width(HANDLE_W - 1.0)
            .height(Length::Fill)
            .style(move |_theme: &iced::Theme| container::background(bg)),
    ])
    .interaction(mouse::Interaction::ResizingHorizontally)
    .on_press(msg(ProfilesMsg::ResizeStart));
    let all = row![body, handle, sidebar(view, config, live, look)];
    if !view.resizing {
        return all.into();
    }
    mouse_area(all)
        .on_move(|p| msg(ProfilesMsg::ResizeTo(p.x)))
        .on_release(msg(ProfilesMsg::ResizeEnd))
        .into()
}

/// "Profiles", then "+" (New ▸) and "×" (close the sidebar).
fn title_row<'a>(look: Look) -> Element<'a, Message> {
    let square = |glyph: Icon, on_press: Message| {
        button(icon(glyph, 15.0, look.dim))
            .width(26)
            .height(26)
            .padding(0)
            .style(style::icon_btn(look, false))
            .on_press(on_press)
    };
    row![
        text("Profiles")
            .size(13.5)
            .font(crate::UI_MEDIUM)
            .color(look.fg)
            .width(Length::Fill),
        square(Icon::Plus, msg(ProfilesMsg::NewMenu)),
        square(Icon::Close, Message::Action(Action::ToggleProfiles)),
    ]
    .spacing(2)
    .align_y(Alignment::Center)
    .into()
}

/// A section header: caret, LABEL, count; hover lightens it.
fn section_header<'a>(
    label: &str,
    count: usize,
    collapsed: bool,
    look: Look,
    on_press: Message,
) -> Element<'a, Message> {
    button(
        row![
            glyph(
                if collapsed {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronDown
                },
                10.0,
                look.dim
            ),
            text(label.to_uppercase()).size(10.5),
            text(count.to_string()).size(10.5).color(t::N700),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(iced::Padding {
        top: 10.0,
        right: 4.0,
        bottom: 4.0,
        left: 6.0,
    })
    .style(move |_theme: &iced::Theme, status| button::Style {
        text_color: if matches!(status, button::Status::Hovered) {
            if look.dark { t::N300 } else { look.fg }
        } else {
            look.dim
        },
        ..button::Style::default()
    })
    .on_press(on_press)
    .into()
}

/// The hover actions of a row: Split and Edit, 24 px square.
fn row_actions<'a>(id: &ProfileId, look: Look) -> Element<'a, Message> {
    let square = |glyph: Icon, on_press: Message| {
        button(icon(glyph, 14.0, look.dim))
            .width(24)
            .height(24)
            .padding(0)
            .style(style::icon_btn(look, false))
            .on_press(on_press)
    };
    row![
        square(
            Icon::Split,
            msg(ProfilesMsg::RowOpen(id.clone(), Launch::Split))
        ),
        square(Icon::Pencil, msg(ProfilesMsg::Edit(id.clone()))),
    ]
    .spacing(2)
    .into()
}

#[expect(clippy::too_many_lines, reason = "one widget tree, top to bottom")]
fn sidebar<'a>(
    view: &'a ProfilesView,
    config: &Config,
    live: &HashSet<ProfileId>,
    look: Look,
) -> Element<'a, Message> {
    let all = entries(
        config,
        &view.builtins,
        &view.ssh_hosts.profiles,
        Some(&view.recent),
    );
    let mut rows = rows(&all, config, &view.recent, now_secs());
    for (row, entry) in rows.iter_mut().zip(&all) {
        if live.contains(&entry.profile.id) {
            row.mark = Mark::Dot(look.accent_hi);
        }
    }
    let lines = lines(view, config, &all, &rows);
    let row_look = RowLook {
        fg: look.fg,
        dim: look.dim,
        selected: alpha(look.accent, 0.16),
        hover: alpha(look.fg, 0.06),
    };
    let mut list = column![].spacing(1);
    let mut pos = 0;
    for line in lines {
        match line {
            Line::Header {
                section,
                label,
                count,
                collapsed,
            } => {
                list = list.push(section_header(
                    &label,
                    count,
                    collapsed,
                    look,
                    msg(ProfilesMsg::Toggle(section)),
                ));
            }
            Line::Item(index) => {
                let (Some(entry), Some(item)) = (all.get(index), rows.get(index)) else {
                    continue;
                };
                let id = entry.profile.id.clone();
                let open = msg(ProfilesMsg::RowOpen(id.clone(), Launch::Tab));
                let (press, double) = match config.appearance.profiles_click {
                    ProfileClick::Single => (open, None),
                    ProfileClick::Double => (msg(ProfilesMsg::Select(pos)), Some(open)),
                };
                // The press may start a drag onto a pane; released in place it is a click.
                let press = Payload::Profile(id.clone(), Box::new(press));
                let hovered = view.hovered_row == Some(pos);
                let actions = hovered.then(|| row_actions(&id, look));
                let events = RowEvents {
                    press: Some(Message::Drag(DragMsg::Start(press))),
                    double,
                    middle: Some(msg(ProfilesMsg::RowOpen(id.clone(), Launch::Split))),
                    right: Some(msg(ProfilesMsg::Menu(id, pos))),
                    enter: Some(msg(ProfilesMsg::HoverRow(pos, true))),
                    exit: Some(msg(ProfilesMsg::HoverRow(pos, false))),
                };
                let selected = view.focused && pos == view.selected;
                list = list.push(fuzzy_list::row_view(
                    item, selected, hovered, row_look, events, actions,
                ));
                pos += 1;
            }
        }
    }
    if pos == 0 && !view.query.trim().is_empty() {
        list = list.push(text("No matching profile").size(12).color(look.dim));
    }
    let title = title_row(look);
    let lead = |glyph: Icon| text_input::Icon {
        font: PHOSPHOR,
        code_point: glyph.glyph(),
        size: Some(14.into()),
        spacing: 7.0,
        side: text_input::Side::Left,
    };
    let search = text_input("Search profiles", &view.query)
        .id(Id::new(SEARCH_INPUT))
        .on_input(|q| msg(ProfilesMsg::Query(q)))
        .on_submit(msg(ProfilesMsg::Submit))
        .icon(lead(Icon::Search))
        .size(13)
        .padding([7, 9])
        .style(style::input(look));
    let quick_style = style::input(look);
    let quick = text_input("Quick connect: user@host:port", &view.quick)
        .id(Id::new(QUICK_INPUT))
        .on_input(|q| msg(ProfilesMsg::Quick(q)))
        .on_submit(msg(ProfilesMsg::QuickSubmit))
        .icon(lead(Icon::Lightning))
        .font(Font::MONOSPACE)
        .size(12)
        .padding([8, 9])
        .style(move |theme: &iced::Theme, status| text_input::Style {
            icon: look.accent_hi,
            ..quick_style(theme, status)
        });
    let mut top = column![title, search, quick].spacing(8);
    if let Some(err) = &view.quick_error {
        top = top.push(text(err.clone()).size(11).color(look.danger));
    }
    let scroller = Scrollbar::new().width(4).scroller_width(4);
    let list = scrollable(list)
        .direction(Direction::Vertical(scroller))
        .style(crate::style::scrollbar)
        .height(Length::Fill);
    let body = column![top, list].spacing(10).padding([12, 12]);
    let bg = look.bg;
    let panel = container(body)
        .width(view.width)
        .height(Length::Fill)
        .style(move |_theme: &iced::Theme| container::background(bg));
    mouse_area(panel)
        .on_enter(msg(ProfilesMsg::Hover(true)))
        .on_exit(msg(ProfilesMsg::Hover(false)))
        .on_move(|p| msg(ProfilesMsg::Cursor(p)))
        .into()
}
