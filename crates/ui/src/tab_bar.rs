//! The tab bar (stock iced widgets, Nocturne): the active header merges into the terminal below
//! (rounded top, concave flares at its feet), inactive ones are hover pills; each shows its tab
//! colour as a dot, the title and, on hover, a close `×`. `+` follows them, pinned at the
//! right edge once they overflow and scroll horizontally (vertical wheel included).

use fogu_core::Action;
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    Id, Row, button, column, container, image, mouse_area, row, scrollable, space, stack, text,
    text_input, tooltip,
};
use iced::{Alignment, Color, Element, Length};

use crate::context_menu::to_iced;
use crate::drag::{DragMsg, Payload};
use crate::icons::{Icon, icon};
use crate::lifecycle::{warning_detail, warning_title};
use crate::message::{Message, WindowCmd};
use crate::palette::Look;
use crate::tab::{Tab, TabId};
use crate::tokens::{self as t, alpha};

/// Header slot width in logical px while the headers fit (the active header's flares included).
/// All headers share one width, so the drag targets keep a fixed stride.
pub const TAB_W: f32 = 200.0;
/// Narrowest header, still showing the dot and a few title characters; past it the strip
/// scrolls.
pub const MIN_TAB_W: f32 = 120.0;
/// Space between headers.
pub const TAB_GAP: f32 = 2.0;
/// What a header takes besides its title: paddings, the dot, the close button.
const HEADER_CHROME_W: f32 = 70.0;
/// Empty bar always left after the headers' buttons, to grab and move the window.
const GRAB_W: f32 = 96.0;
/// Rough advance of a title character at [`TITLE_SIZE`].
const TITLE_CHAR_W: f32 = 6.5;
/// Height of the bar (it is the window's title bar).
pub const BAR_H: f32 = 40.0;
/// Room between the bar's left end (after [`TRAFFIC_LIGHTS_W`]) and the first header.
pub const STRIP_PAD: f32 = 10.0;
/// macOS: room at the bar's left end for the native window buttons (the title bar is hidden).
pub const TRAFFIC_LIGHTS_W: f32 = 78.0;
/// Titles longer than this many characters end in `…`.
const MAX_TITLE_CHARS: usize = 20;
/// The active header: its height and the size of the concave flares at its feet.
const ACTIVE_H: f32 = 34.0;
const FLARE: f32 = 9.0;
/// Inactive headers: a pill this tall, [`PILL_LIFT`] above the bar's bottom.
const PILL_H: f32 = 28.0;
const PILL_LIFT: f32 = 4.0;
const TITLE_SIZE: f32 = 13.0;
/// Width of the window's resize edges.
const EDGE: f32 = 5.0;
/// Warning badge colour (amber).
const WARN: Color = Color::from_rgb(0.96, 0.65, 0.14);
/// Id of the inline rename field.
pub const RENAME_INPUT: &str = "fogu-rename-input";
/// Id of the scrollable tab strip.
pub const TAB_STRIP: &str = "fogu-tab-strip";

/// Total width of `count` headers `tab_w` wide and their gaps.
pub(crate) fn headers_width(count: usize, tab_w: f32) -> f32 {
    if count == 0 {
        0.0
    } else {
        crate::metrics::cells_f32(count) * (tab_w + TAB_GAP) - TAB_GAP
    }
}

/// Header width for `tabs` headers: [`TAB_W`] while they fit, then shrunk evenly to fill the
/// room, down to [`MIN_TAB_W`].
pub(crate) fn tab_w(width: f32, inset: f32, toggles: usize, tabs: usize) -> f32 {
    let even = (strip_room(width, inset, toggles) + TAB_GAP)
        / crate::metrics::cells_f32(tabs.max(1))
        - TAB_GAP;
    even.floor().clamp(MIN_TAB_W, TAB_W)
}

/// Width available to the scrollable headers after the bar's fixed controls.
pub(crate) fn strip_width(width: f32, inset: f32, toggles: usize, tabs: usize) -> f32 {
    headers_width(tabs, tab_w(width, inset, toggles, tabs)).min(strip_room(width, inset, toggles))
}

/// The bar's width minus its fixed controls.
fn strip_room(width: f32, inset: f32, toggles: usize) -> f32 {
    let window_buttons = if cfg!(target_os = "macos") {
        6.0
    } else {
        13.0 + 3.0 * 44.0
    };
    let others = inset
        + STRIP_PAD
        + 4.0
        + 28.0
        + 22.0
        + crate::metrics::cells_f32(toggles) * 32.0
        + 2.0
        + GRAB_W
        + window_buttons;
    (width - others).max(0.0)
}

/// An inline rename in progress.
#[derive(Clone, Debug)]
pub struct Rename {
    pub tab: TabId,
    pub value: String,
}

/// Everything the bar needs for one `view`.
#[derive(Clone, Copy, Debug)]
pub struct TabBar<'a> {
    pub tabs: &'a [Tab],
    pub active: TabId,
    /// The header slot under the pointer.
    pub hovered: Option<usize>,
    pub renaming: Option<&'a Rename>,
    pub look: Look,
    /// The active tab has an SSH pane: show the SFTP button (`true` = panel open).
    pub sftp: Option<bool>,
    /// The active tab has an SSH pane: show the "Ports" button with this many listening
    /// forwards.
    pub forwards: Option<usize>,
    /// The profile sidebar is open (its toggle button is highlighted).
    pub profiles: bool,
    /// The settings tab is the active one (its button is highlighted).
    pub settings: bool,
    /// Empty room before the headers ([`TRAFFIC_LIGHTS_W`] on macOS).
    pub inset: f32,
    /// The window's width.
    pub width: f32,
}

/// How many title characters a `tab_w`-wide header shows.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "floored and clamped to 3..=MAX_TITLE_CHARS first"
)]
fn title_chars(tab_w: f32) -> usize {
    ((tab_w - HEADER_CHROME_W) / TITLE_CHAR_W)
        .floor()
        .clamp(3.0, crate::metrics::cells_f32(MAX_TITLE_CHARS)) as usize
}

/// `title` cut to `max` characters (at least 1), ending in `…` when cut.
pub fn truncate(title: &str, max: usize) -> String {
    if title.chars().count() <= max {
        return title.to_owned();
    }
    let mut out: String = title.chars().take(max.max(1) - 1).collect();
    out.push('…');
    out
}

fn solid<'a>(color: Color, width: Length, height: f32) -> Element<'a, Message> {
    container(space())
        .width(width)
        .height(height)
        .style(move |_theme: &iced::Theme| container::background(color))
        .into()
}

impl<'a> TabBar<'a> {
    #[expect(clippy::too_many_lines, reason = "one widget tree, top to bottom")]
    fn header(&self, slot: usize, tab: &'a Tab, tab_w: f32) -> Element<'a, Message> {
        let look = self.look;
        let active = tab.id == self.active;
        let hovered = self.hovered == Some(slot);
        let fg = match (active, hovered) {
            (true, _) | (false, true) => look.fg,
            (false, false) => look.dim,
        };
        let title: Element<'a, Message> = match self.renaming {
            Some(rename) if rename.tab == tab.id => text_input("Tab title", &rename.value)
                .id(Id::new(RENAME_INPUT))
                .on_input(Message::RenameInput)
                .on_submit(Message::CommitRename)
                .size(TITLE_SIZE)
                .padding(2)
                .into(),
            _ => text(truncate(tab.title(), title_chars(tab_w)))
                .size(TITLE_SIZE)
                .color(fg)
                .wrapping(text::Wrapping::None)
                .width(Length::Fill)
                .into(),
        };
        let close: Element<'a, Message> = if hovered {
            button(icon(Icon::Close, 13.0, fg))
                .width(20)
                .height(20)
                .padding(0)
                .style(move |_theme: &iced::Theme, status| {
                    let mut style = hover_style(look, status, false);
                    style.border = iced::Border::default().rounded(5);
                    style
                })
                .on_press(Message::CloseTab(tab.id))
                .into()
        } else {
            space().width(20).into()
        };
        // The tab colour, when the tab or its profile has one, as a small dot before the title.
        let mut content = row![].spacing(8).align_y(Alignment::Center);
        if let Some(color) = tab.color.map(to_iced) {
            content = content.push(container(space()).width(7).height(7).style(
                move |_theme: &iced::Theme| {
                    container::background(color).border(iced::Border::default().rounded(3.5))
                },
            ));
        }
        if let Some(badge) = warning_badge(tab) {
            content = content.push(badge);
        }
        let content = content.push(title).push(close);
        let body: Element<'a, Message> = if active {
            let ground = look.base;
            let face = container(content)
                .padding(iced::Padding {
                    top: 0.0,
                    right: 8.0,
                    bottom: 0.0,
                    left: 12.0,
                })
                .height(ACTIVE_H)
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .style(move |_theme: &iced::Theme| {
                    container::background(ground).border(iced::Border {
                        radius: iced::border::Radius::default()
                            .top_left(t::R_TAB)
                            .top_right(t::R_TAB),
                        ..iced::Border::default()
                    })
                });
            row![flare(ground, true), face, flare(ground, false)]
                .align_y(Alignment::End)
                .width(tab_w)
                .height(BAR_H)
                .into()
        } else {
            let fill = hovered.then_some(alpha(look.fg, 0.08));
            let pill = container(content)
                .padding(iced::Padding {
                    top: 0.0,
                    right: 8.0,
                    bottom: 0.0,
                    left: 12.0,
                })
                .height(PILL_H)
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .style(move |_theme: &iced::Theme| container::Style {
                    background: fill.map(Into::into),
                    border: iced::Border::default().rounded(t::R_BTN),
                    ..container::Style::default()
                });
            container(pill)
                .padding(iced::Padding {
                    top: 0.0,
                    right: 4.0,
                    bottom: PILL_LIFT,
                    left: 4.0,
                })
                .width(tab_w)
                .height(BAR_H)
                .align_y(Alignment::End)
                .into()
        };
        mouse_area(body)
            .on_press(Message::Drag(DragMsg::Start(Payload::Tab(tab.id))))
            .on_middle_press(Message::CloseTab(tab.id))
            .on_double_click(Message::StartRename(tab.id))
            .on_right_press(Message::OpenTabMenu(tab.id))
            .on_enter(Message::HoverTab(slot))
            .on_exit(Message::LeaveTab(slot))
            .into()
    }

    /// The bar: scrollable headers with `+` / `▾` after them (their empty rest moves the
    /// window), the panel toggles, then the window buttons.
    #[expect(clippy::too_many_lines, reason = "one widget tree, left to right")]
    pub fn view(self) -> Element<'a, Message> {
        let look = self.look;
        let plus = square_button(Icon::Plus, 28.0, look, Message::Action(Action::NewTab));
        // Right-click on `+` or the chevron next to it: the profile picker.
        let plus = mouse_area(plus).on_right_press(Message::Action(Action::OpenProfilePicker));
        let picker = square_button(
            Icon::ChevronDown,
            22.0,
            look,
            Message::Action(Action::OpenProfilePicker),
        );
        let toggles = usize::from(self.forwards.is_some()) + usize::from(self.sftp.is_some()) + 2;
        let tab_w = tab_w(self.width, self.inset, toggles, self.tabs.len());
        let headers = Row::with_children(
            self.tabs
                .iter()
                .enumerate()
                .map(|(slot, tab)| self.header(slot, tab, tab_w)),
        )
        .spacing(TAB_GAP);
        let scroller = Scrollbar::new().width(2).scroller_width(2);
        let strip = scrollable(headers)
            .id(Id::new(TAB_STRIP))
            .direction(Direction::Horizontal(scroller))
            .on_scroll(Message::TabStripViewport)
            .width(Length::Fill);
        // As wide as the headers when they fit (so `+` follows the last one), else whatever the
        // bar leaves after `+` and the buttons. Computed, as a `Fill` child of a row is never
        // narrower than its share (iced 0.14), whatever its `max_width`.
        let strip = container(strip).width(strip_width(
            self.width,
            self.inset,
            toggles,
            self.tabs.len(),
        ));
        let tabs = row![
            space().width(self.inset + STRIP_PAD),
            strip,
            space().width(4),
            plus,
            picker,
            space().width(Length::Fill)
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill);
        // The bar's empty part (tab headers and buttons take their own presses first).
        let tabs = mouse_area(tabs)
            .on_press(Message::Window(WindowCmd::Press))
            .on_release(Message::Window(WindowCmd::Release))
            .on_double_click(Message::Window(WindowCmd::ToggleMaximize))
            .on_right_press(Message::TabBarMenu)
            .on_scroll(Message::TabBarWheel);
        let toggle = |glyph: Icon, on: bool, badge: Option<usize>, tip: String, message| {
            let color = if on { look.accent_hi } else { look.dim };
            let button = button(icon(glyph, 16.0, color))
                .width(30)
                .height(30)
                .padding(0)
                .style(move |_theme: &iced::Theme, status| {
                    let mut style = hover_style(look, status, false);
                    if on {
                        style.background = Some(alpha(look.accent, 0.14).into());
                    }
                    style.border = iced::Border::default().rounded(t::R_BTN);
                    style
                })
                .on_press(message);
            let button: Element<'a, Message> = match badge {
                Some(count) => stack![
                    button,
                    container(count_badge(count))
                        .width(30)
                        .height(30)
                        .align_x(Alignment::End)
                        .align_y(Alignment::Start)
                ]
                .into(),
                None => button.into(),
            };
            let tip = container(text(tip).size(12))
                .padding(4)
                .style(container::rounded_box);
            tooltip(button, tip, tooltip::Position::Bottom)
        };
        let mut bar_row = row![tabs].spacing(2).align_y(Alignment::Center);
        if let Some(count) = self.forwards {
            bar_row = bar_row.push(toggle(
                Icon::Ports,
                count > 0,
                (count > 0).then_some(count),
                format!("Port forwarding ({count} listening)"),
                Message::Forwards(crate::forwards_dialog::ForwardsMsg::Open),
            ));
        }
        if let Some(open) = self.sftp {
            bar_row = bar_row.push(toggle(
                Icon::Folder,
                open,
                None,
                "SFTP panel (toggle-sftp)".to_owned(),
                Message::Action(Action::ToggleSftp),
            ));
        }
        bar_row = bar_row.push(toggle(
            Icon::Sidebar,
            self.profiles,
            None,
            "Profiles (toggle-profiles)".to_owned(),
            Message::Action(Action::ToggleProfiles),
        ));
        bar_row = bar_row.push(toggle(
            Icon::Gear,
            self.settings,
            None,
            "Settings (open-settings)".to_owned(),
            Message::Action(Action::OpenSettings),
        ));
        // macOS: the native title bar has the window buttons.
        bar_row = if cfg!(target_os = "macos") {
            bar_row.push(space().width(6))
        } else {
            bar_row.push(window_buttons(look))
        };
        let bar = container(bar_row)
            .width(Length::Fill)
            .height(BAR_H)
            .style(move |_theme: &iced::Theme| container::background(look.raised));
        mouse_area(bar).on_move(Message::TabBarCursor).into()
    }
}

/// The count on the Ports button: a small accent pill.
fn count_badge<'a>(count: usize) -> Element<'a, Message> {
    container(text(count.to_string()).size(8.5).color(t::A100))
        .padding([0, 4])
        .height(12)
        .align_y(Alignment::Center)
        .style(|_theme: &iced::Theme| {
            container::background(t::A800).border(iced::Border::default().rounded(999))
        })
        .into()
}

/// A divider, then Minimize, Maximize and Close.
fn window_buttons<'a>(look: Look) -> Element<'a, Message> {
    row![
        space().width(6),
        solid(look.border, Length::Fixed(1.0), 18.0),
        space().width(6),
        window_button(
            Icon::Minimize,
            look,
            false,
            Message::Window(WindowCmd::Minimize),
        ),
        window_button(
            Icon::Maximize,
            look,
            false,
            Message::Window(WindowCmd::ToggleMaximize),
        ),
        window_button(Icon::Close, look, true, Message::CloseRequested),
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Flat chrome button colours: `hover` on hover, `danger` (light text) for the close button.
pub(crate) fn hover_style(look: Look, status: button::Status, danger: bool) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Hovered | button::Status::Pressed if danger => (Some(look.danger), t::N100),
        button::Status::Hovered => (Some(look.hover), look.fg),
        button::Status::Pressed => (Some(alpha(look.fg, 0.13)), look.fg),
        _ => (None, look.fg),
    };
    button::Style {
        background: background.map(Into::into),
        text_color,
        ..button::Style::default()
    }
}

/// A `width` × 28 bar button, radius 7, with a 16 px icon.
fn square_button<'a>(
    glyph: Icon,
    width: f32,
    look: Look,
    message: Message,
) -> Element<'a, Message> {
    button(icon(glyph, 16.0, look.dim))
        .width(width)
        .height(28)
        .padding(0)
        .style(move |_theme: &iced::Theme, status| {
            let mut style = hover_style(look, status, false);
            style.border = iced::Border::default().rounded(t::R_BTN);
            style
        })
        .on_press(message)
        .into()
}

/// Minimize / maximize / close at the bar's right end (close turns light on red). The icon
/// follows the button's text colour, so the close glyph lights up with its red hover.
fn window_button<'a>(
    glyph: Icon,
    look: Look,
    danger: bool,
    message: Message,
) -> Element<'a, Message> {
    button(
        container(
            text(glyph.glyph())
                .font(crate::icons::PHOSPHOR)
                .size(15)
                .line_height(1.0),
        )
        .center(Length::Fill),
    )
    .width(44)
    .height(BAR_H)
    .padding(0)
    .style(move |_theme: &iced::Theme, status| {
        let mut style = hover_style(look, status, danger);
        if !matches!(status, button::Status::Hovered | button::Status::Pressed) {
            style.text_color = look.dim;
        }
        style
    })
    .on_press(message)
    .into()
}

/// One concave flare at the active header's foot: a [`FLARE`]-px square of `color` with a
/// quarter circle cut out of its top outer corner, so the header curves into the content.
fn flare<'a>(color: Color, left: bool) -> Element<'a, Message> {
    image(crate::icons::notch(color, FLARE, !left, false))
        .width(FLARE)
        .height(FLARE)
        .into()
}

/// Invisible strips along the window's edges and corners that start a resize (there are no OS
/// decorations to do it); layered over everything else.
pub fn resize_edges<'a>() -> Element<'a, Message> {
    use iced::mouse::Interaction;
    use iced::window::Direction as D;
    let edge = |w: Length, h: Length, dir: D, cursor: Interaction| -> Element<'a, Message> {
        mouse_area(space().width(w).height(h))
            .interaction(cursor)
            .on_press(Message::Window(WindowCmd::Resize(dir)))
            .into()
    };
    let (e, fill) = (Length::Fixed(EDGE), Length::Fill);
    let (h, v) = (
        Interaction::ResizingHorizontally,
        Interaction::ResizingVertically,
    );
    let (up, down) = (
        Interaction::ResizingDiagonallyUp,
        Interaction::ResizingDiagonallyDown,
    );
    column![
        row![
            edge(e, e, D::NorthWest, down),
            edge(fill, e, D::North, v),
            edge(e, e, D::NorthEast, up)
        ],
        row![
            edge(e, fill, D::West, h),
            space().width(fill),
            edge(e, fill, D::East, h)
        ]
        .height(fill),
        row![
            edge(e, e, D::SouthWest, up),
            edge(fill, e, D::South, v),
            edge(e, e, D::SouthEast, down)
        ],
    ]
    .into()
}

/// A persistent `!` badge when a pane of `tab` reported a warning (e.g. host key not
/// verified); the tooltip says what.
fn warning_badge(tab: &Tab) -> Option<Element<'_, Message>> {
    let lines: Vec<String> = tab
        .warnings()
        .map(|w| format!("{}: {}", warning_title(w), warning_detail(w)))
        .collect();
    if lines.is_empty() {
        return None;
    }
    let badge = container(text("!").size(11).color(Color::BLACK))
        .padding([0, 5])
        .style(|theme: &iced::Theme| container::rounded_box(theme).background(WARN));
    let tip = container(text(lines.join("\n")).size(12))
        .padding(6)
        .max_width(320)
        .style(container::rounded_box);
    Some(tooltip(badge, tip, tooltip::Position::Bottom).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_titles_with_ellipsis() {
        assert_eq!(truncate("zsh", MAX_TITLE_CHARS), "zsh");
        let long = "a".repeat(40);
        let cut = truncate(&long, MAX_TITLE_CHARS);
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
        assert!(cut.ends_with('…'));
        assert_eq!(
            truncate(&"é".repeat(MAX_TITLE_CHARS), MAX_TITLE_CHARS)
                .chars()
                .count(),
            MAX_TITLE_CHARS
        );
    }

    #[test]
    fn headers_shrink_to_fit_then_scroll() {
        let room = |tabs| {
            strip_room(1400.0, 0.0, 2) + TAB_GAP
                - headers_width(tabs, tab_w(1400.0, 0.0, 2, tabs))
                - TAB_GAP
        };
        // Few tabs: full width, the strip shorter than the room.
        assert!((tab_w(1400.0, 0.0, 2, 2) - TAB_W).abs() < f32::EPSILON);
        // More: narrower, all of them still in the room.
        let w = tab_w(1400.0, 0.0, 2, 8);
        assert!(w < TAB_W && w > MIN_TAB_W && room(8) >= 0.0, "{w}");
        assert!((strip_width(1400.0, 0.0, 2, 8) - headers_width(8, w)).abs() < f32::EPSILON);
        // Too many: the floor, and the strip scrolls (headers wider than the room).
        assert!((tab_w(1400.0, 0.0, 2, 40) - MIN_TAB_W).abs() < f32::EPSILON);
        assert!(room(40) < 0.0);
        assert!(title_chars(MIN_TAB_W) >= 3 && title_chars(TAB_W) == MAX_TITLE_CHARS);
    }
}
