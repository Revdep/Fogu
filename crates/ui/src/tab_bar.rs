//! The tab bar (stock iced widgets): one header per tab with title, close `×` (hover/active),
//! colour underline; `+` right after them, pinned at the right edge once they overflow and
//! scroll horizontally (vertical wheel included).

use fogu_core::Action;
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    Id, Row, button, column, container, mouse_area, row, scrollable, space, text, text_input,
    tooltip,
};
use iced::{Alignment, Color, Element, Length};

use crate::context_menu::to_iced;
use crate::drag::{DragMsg, Payload};
use crate::icons::{Icon, icon};
use crate::lifecycle::{warning_detail, warning_title};
use crate::message::{Message, WindowCmd};
use crate::palette::{Look, mix};
use crate::tab::{Tab, TabId};

/// Header width in logical px.
pub const TAB_W: f32 = 200.0;
/// A header plus the 1 px between headers (drag targets in the bar).
pub const TAB_STRIDE: f32 = TAB_W + 1.0;
/// Height of the bar (it is the window's title bar).
pub const BAR_H: f32 = 36.0;
/// Titles longer than this many characters end in `…`.
const MAX_TITLE_CHARS: usize = 20;
/// Height of the colour / active underline.
const UNDERLINE_H: f32 = 2.0;
const TITLE_SIZE: f32 = 13.0;
/// Width of the window's resize edges.
const EDGE: f32 = 5.0;
/// Warning badge colour (amber).
const WARN: Color = Color::from_rgb(0.96, 0.65, 0.14);
/// Id of the inline rename field.
pub const RENAME_INPUT: &str = "fogu-rename-input";
/// Id of the scrollable tab strip.
pub const TAB_STRIP: &str = "fogu-tab-strip";

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
    pub hovered: Option<TabId>,
    pub renaming: Option<&'a Rename>,
    pub look: Look,
    /// The active tab has an SSH pane: show the SFTP button (`true` = panel open).
    pub sftp: Option<bool>,
    /// The active tab has an SSH pane: show the "Ports" button with this many listening
    /// forwards.
    pub forwards: Option<usize>,
    /// The profile sidebar is open (its toggle button is highlighted).
    pub profiles: bool,
}

/// `title` cut to [`MAX_TITLE_CHARS`] characters, ending in `…` when cut.
pub fn truncate(title: &str) -> String {
    if title.chars().count() <= MAX_TITLE_CHARS {
        return title.to_owned();
    }
    let mut out: String = title.chars().take(MAX_TITLE_CHARS - 1).collect();
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
    fn header(&self, tab: &'a Tab) -> Element<'a, Message> {
        let (bg, fg) = (self.look.base, self.look.fg);
        let active = tab.id == self.active;
        let hovered = self.hovered == Some(tab.id);
        let title: Element<'a, Message> = match self.renaming {
            Some(rename) if rename.tab == tab.id => text_input("Tab title", &rename.value)
                .id(Id::new(RENAME_INPUT))
                .on_input(Message::RenameInput)
                .on_submit(Message::CommitRename)
                .size(TITLE_SIZE)
                .padding(2)
                .into(),
            _ => text(truncate(tab.title()))
                .size(TITLE_SIZE)
                .color(if active { fg } else { mix(fg, bg, 0.4) })
                .wrapping(text::Wrapping::None)
                .width(Length::Fill)
                .into(),
        };
        let close: Element<'a, Message> = if active || hovered {
            let look = self.look;
            button(icon(Icon::Close, 10.0, fg))
                .width(20)
                .height(20)
                .padding(0)
                .style(move |_theme: &iced::Theme, status| {
                    let mut style = hover_style(look, status, false);
                    style.border = iced::Border::default().rounded(3);
                    style
                })
                .on_press(Message::CloseTab(tab.id))
                .into()
        } else {
            space().width(20).into()
        };
        let title = match warning_badge(tab) {
            Some(badge) => row![badge, title]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
            None => title,
        };
        let underline = match tab.color {
            Some(color) => to_iced(color),
            None if active => self.look.accent,
            None => Color::TRANSPARENT,
        };
        let header_bg = match (active, hovered) {
            (true, _) => bg,
            (false, true) => mix(self.look.raised, fg, 0.08),
            (false, false) => self.look.raised,
        };
        let content = column![
            container(row![title, close].spacing(6).align_y(Alignment::Center))
                .padding([0, 12])
                .height(BAR_H - UNDERLINE_H)
                .align_y(Alignment::Center)
                .width(Length::Fill),
            solid(underline, Length::Fill, UNDERLINE_H),
        ];
        let body = container(content)
            .width(TAB_W)
            .style(move |_theme: &iced::Theme| container::background(header_bg));
        mouse_area(body)
            .on_press(Message::Drag(DragMsg::Start(Payload::Tab(tab.id))))
            .on_middle_press(Message::CloseTab(tab.id))
            .on_double_click(Message::StartRename(tab.id))
            .on_right_press(Message::OpenTabMenu(tab.id))
            .on_enter(Message::HoverTab(Some(tab.id)))
            .on_exit(Message::LeaveTab(tab.id))
            .into()
    }

    /// The bar: scrollable headers with `+` / `▾` after them (their empty rest moves the
    /// window), the panel toggles, then the window buttons.
    pub fn view(self) -> Element<'a, Message> {
        let look = self.look;
        let plus = icon_button(Icon::Plus, 36.0, look, Message::Action(Action::NewTab));
        // Right-click on `+` or the chevron next to it: the profile picker.
        let plus = mouse_area(plus).on_right_press(Message::Action(Action::OpenProfilePicker));
        let picker = icon_button(
            Icon::ChevronDown,
            28.0,
            look,
            Message::Action(Action::OpenProfilePicker),
        );
        let headers = Row::with_children(self.tabs.iter().map(|tab| self.header(tab))).spacing(1);
        let scroller = Scrollbar::new().width(2).scroller_width(2);
        let strip = scrollable(headers)
            .id(Id::new(TAB_STRIP))
            .direction(Direction::Horizontal(scroller))
            .on_scroll(Message::TabStripViewport)
            .width(Length::Fill);
        // As wide as the headers when they fit (so `+` follows the last one), else whatever the
        // bar leaves after `+` and the buttons; the tiny spacer portion takes the rest.
        let strip = container(strip)
            .width(Length::FillPortion(1000))
            .max_width(crate::metrics::cells_f32(self.tabs.len()) * TAB_STRIDE);
        let tabs = row![strip, plus, picker, space().width(Length::FillPortion(1))]
            .align_y(Alignment::Center)
            .width(Length::Fill);
        // The bar's empty part (tab headers and buttons take their own presses first).
        let tabs = mouse_area(tabs)
            .on_press(Message::Window(WindowCmd::Press))
            .on_release(Message::Window(WindowCmd::Release))
            .on_double_click(Message::Window(WindowCmd::ToggleMaximize))
            .on_right_press(Message::TabBarMenu)
            .on_scroll(Message::TabBarWheel);
        let toggle = |glyph: Icon, on: bool, tip: String, message| {
            let color = if on { look.accent } else { look.fg };
            let button = button(icon(glyph, 16.0, color))
                .width(36)
                .height(BAR_H)
                .padding(0)
                .style(move |_theme: &iced::Theme, status| hover_style(look, status, false))
                .on_press(message);
            let tip = container(text(tip).size(12))
                .padding(4)
                .style(container::rounded_box);
            tooltip(button, tip, tooltip::Position::Bottom)
        };
        let mut bar_row = row![tabs].align_y(Alignment::Center);
        if let Some(count) = self.forwards {
            bar_row = bar_row.push(toggle(
                Icon::Ports,
                count > 0,
                format!("Port forwarding ({count} listening)"),
                Message::Forwards(crate::forwards_dialog::ForwardsMsg::Open),
            ));
        }
        if let Some(open) = self.sftp {
            bar_row = bar_row.push(toggle(
                Icon::Folder,
                open,
                "SFTP panel (toggle-sftp)".to_owned(),
                Message::Action(Action::ToggleSftp),
            ));
        }
        bar_row = bar_row.push(toggle(
            Icon::Sidebar,
            self.profiles,
            "Profiles (toggle-profiles)".to_owned(),
            Message::Action(Action::ToggleProfiles),
        ));
        // macOS: the native title bar has the window buttons.
        if !cfg!(target_os = "macos") {
            bar_row = bar_row
                .push(space().width(8))
                .push(solid(look.border, Length::Fixed(1.0), 18.0))
                .push(space().width(8))
                .push(window_button(
                    Icon::Minimize,
                    look,
                    false,
                    Message::Window(WindowCmd::Minimize),
                ))
                .push(window_button(
                    Icon::Maximize,
                    look,
                    false,
                    Message::Window(WindowCmd::ToggleMaximize),
                ))
                .push(window_button(
                    Icon::Close,
                    look,
                    true,
                    Message::CloseRequested,
                ));
        }
        let bar = container(bar_row)
            .width(Length::Fill)
            .height(BAR_H)
            .style(move |_theme: &iced::Theme| container::background(look.raised));
        mouse_area(bar).on_move(Message::TabBarCursor).into()
    }
}

/// Flat chrome button colours: `hover` on hover, `danger` (white text) for the close button.
fn hover_style(look: Look, status: button::Status, danger: bool) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Hovered | button::Status::Pressed if danger => {
            (Some(look.danger), Color::WHITE)
        }
        button::Status::Hovered => (Some(look.hover), look.fg),
        button::Status::Pressed => (Some(Color { a: 0.18, ..look.fg }), look.fg),
        _ => (None, look.fg),
    };
    button::Style {
        background: background.map(Into::into),
        text_color,
        ..button::Style::default()
    }
}

/// A flat bar button `width` px wide with a 16 px icon.
fn icon_button<'a>(glyph: Icon, width: f32, look: Look, message: Message) -> Element<'a, Message> {
    button(icon(glyph, 16.0, look.fg))
        .width(width)
        .height(BAR_H)
        .padding(0)
        .style(move |_theme: &iced::Theme, status| hover_style(look, status, false))
        .on_press(message)
        .into()
}

/// Minimize / maximize / close at the bar's right end (close turns white on red).
fn window_button<'a>(
    glyph: Icon,
    look: Look,
    danger: bool,
    message: Message,
) -> Element<'a, Message> {
    button(icon(glyph, 16.0, look.fg))
        .width(46)
        .height(BAR_H)
        .padding(0)
        .style(move |_theme: &iced::Theme, status| hover_style(look, status, danger))
        .on_press(message)
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
        assert_eq!(truncate("zsh"), "zsh");
        let long = "a".repeat(40);
        let cut = truncate(&long);
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
        assert!(cut.ends_with('…'));
        assert_eq!(
            truncate(&"é".repeat(MAX_TITLE_CHARS)).chars().count(),
            MAX_TITLE_CHARS
        );
    }
}
