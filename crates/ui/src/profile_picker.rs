//! Profile picker: every profile of the profile sidebar (built-ins first) in a fuzzy
//! filtered list (`fuzzy_list`) beside the selected host's details. Enter / click opens it in a
//! new tab, Shift+Enter next to the focused pane; Enter with no match quick-connects to the typed
//! `user@host:port`. Opening records it in Recent.

use crate::app::App;
use crate::fuzzy_list::{self, FuzzyRow};
use crate::icons::{Icon, PHOSPHOR, glyph};
use crate::message::Message;
use crate::palette::{Look, mix};
use crate::profiles::{self, Entry, Launch, Origin, now_secs};
use crate::style::{self, Btn};
use crate::tokens::{self as t, alpha};
use fogu_core::ProfileKind;
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    Id, button, column, container, mouse_area, opaque, operation, rich_text, row, scrollable,
    space, span, stack, text, text_input,
};
use iced::{Alignment, Element, Font, Length, Task};

/// Id of the filter field.
pub const PICKER_INPUT: &str = "fogu-picker-input";
const PANEL_W: f32 = 820.0;
const DETAIL_W: f32 = 300.0;

/// The open picker.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProfilePicker {
    pub filter: String,
    /// Position in the filtered list.
    pub selected: usize,
}

/// Picker messages.
#[derive(Clone, Debug)]
pub enum PickerMsg {
    /// The filter changed.
    Filter(String),
    /// Arrow up (`-1`) / down (`1`).
    Move(i8),
    /// The pointer is over the row at this position: it becomes the selected one.
    Hover(usize),
    /// Enter in the filter: open the selected entry in a tab.
    Submit,
    /// Open entry `index` (of [`entries`]) in a tab, or in a split.
    Open { index: usize, split: bool },
    /// Open entry `index` in a tab with the SFTP panel.
    Sftp(usize),
    /// Click outside the panel.
    Close,
}

/// What the picker lists: every profile of the sidebar, as entries and fuzzy rows.
pub(crate) fn listing(app: &App) -> (Vec<Entry>, Vec<FuzzyRow>) {
    let entries = app.all_entries();
    let mut rows = profiles::rows(&entries, &app.config, &app.profiles.recent, now_secs());
    // A flat list: say where a `~/.ssh/config` host comes from (a stored copy looks the same).
    for (row, entry) in rows.iter_mut().zip(&entries) {
        if entry.origin == profiles::Origin::SshConfig {
            row.subtitle.push_str(" · ssh config");
        }
    }
    (entries, rows)
}

impl App {
    /// `OpenProfilePicker` / right-click or `▾` on the tab bar's `+`.
    pub(crate) fn open_picker(&mut self) -> Task<Message> {
        self.menu = None;
        self.picker = Some(ProfilePicker::default());
        Task::batch([
            self.refresh_ssh_hosts(),
            operation::focus(Id::new(PICKER_INPUT)),
        ])
    }

    pub(crate) fn on_picker(&mut self, message: PickerMsg) -> Task<Message> {
        let (entries, rows) = listing(self);
        let Some(picker) = self.picker.as_mut() else {
            return Task::none();
        };
        let shown = fuzzy_list::rank(&picker.filter, &rows);
        let split = |yes| if yes { Launch::Split } else { Launch::Tab };
        let open = match message {
            PickerMsg::Filter(filter) => {
                picker.filter = filter;
                picker.selected = 0;
                None
            }
            PickerMsg::Move(delta) => {
                picker.selected = fuzzy_list::step(picker.selected, delta, shown.len());
                None
            }
            PickerMsg::Hover(pos) => {
                picker.selected = pos.min(shown.len().saturating_sub(1));
                None
            }
            PickerMsg::Submit if shown.is_empty() => {
                let how = split(self.modifiers.shift());
                return match profiles::quick_profile(&picker.filter) {
                    Ok(profile) => {
                        self.picker = None;
                        self.quick_launch(profile, how)
                    }
                    Err(_) => Task::none(),
                };
            }
            PickerMsg::Submit => shown
                .get(picker.selected)
                .map(|&i| (i, split(self.modifiers.shift()))),
            PickerMsg::Open { index, split: yes } => Some((index, split(yes))),
            PickerMsg::Sftp(index) => Some((index, Launch::Sftp)),
            PickerMsg::Close => {
                return self.dismiss_picker();
            }
        };
        let Some((index, how)) = open else {
            return Task::none();
        };
        self.picker = None;
        let Some(Entry { profile, .. }) = entries.into_iter().nth(index) else {
            return Task::none();
        };
        self.launch(&profile, how)
    }

    /// Closes the picker, returning keyboard focus to a visible start page.
    pub(crate) fn dismiss_picker(&mut self) -> Task<Message> {
        self.picker = None;
        if self.active_tab().is_some_and(|tab| tab.welcome.is_some()) {
            return operation::focus(Id::new(crate::welcome::QUICK_INPUT));
        }
        Task::none()
    }
}

fn msg(m: PickerMsg) -> Message {
    Message::Picker(m)
}

/// Up to two initials of `name` ("web backup" → "WB").
fn initials(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// `title` with the characters `filter` matches in accent-300, medium weight.
fn highlighted<'a>(title: &str, filter: &str, look: Look) -> Element<'a, Message> {
    let marks = fuzzy_list::matched(filter, title);
    let highlight = if look.dark { t::A300 } else { look.accent };
    // Runs of equally-marked characters, one span each.
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut hot = false;
    let push = |spans: &mut Vec<_>, run: &mut String, hot: bool| {
        if !run.is_empty() {
            let piece: text::Span<'a, ()> = span(std::mem::take(run));
            spans.push(if hot {
                piece.color(highlight).font(crate::UI_MEDIUM)
            } else {
                piece.color(look.fg)
            });
        }
    };
    for (c, &m) in title
        .chars()
        .zip(marks.iter().chain(std::iter::repeat(&false)))
    {
        if m != hot {
            push(&mut spans, &mut run, hot);
            hot = m;
        }
        run.push(c);
    }
    push(&mut spans, &mut run, hot);
    rich_text(spans)
        .size(13)
        .wrapping(text::Wrapping::None)
        .into()
}

/// One list row: icon tile, highlighted name over its subtitle.
fn entry_row<'a>(
    pos: usize,
    index: usize,
    entry: &Entry,
    item: &FuzzyRow,
    filter: &str,
    selected: bool,
    look: Look,
) -> Element<'a, Message> {
    let kind = match entry.profile.kind {
        ProfileKind::Ssh(_) => Icon::Ssh,
        ProfileKind::Local(_) => Icon::Local,
    };
    let (tile_fill, tile_glyph) = if selected {
        (t::A800, t::A200)
    } else {
        (t::N900, t::N400)
    };
    let tile =
        container(glyph(kind, 15.0, tile_glyph))
            .center(28)
            .style(move |_theme: &iced::Theme| {
                container::background(tile_fill).border(iced::Border::default().rounded(7))
            });
    let label = column![
        highlighted(&item.title, filter, look),
        text(item.subtitle.clone())
            .size(11.5)
            .color(look.dim)
            .wrapping(text::Wrapping::None),
    ]
    .spacing(1)
    .width(Length::Fill)
    .clip(true);
    let bg = selected.then_some(alpha(look.accent, 0.16));
    let body = container(row![tile, label].spacing(10).align_y(Alignment::Center))
        .padding([7, 8])
        .width(Length::Fill)
        .style(move |_theme: &iced::Theme| container::Style {
            background: bg.map(Into::into),
            border: iced::Border::default().rounded(t::R_MD),
            ..container::Style::default()
        });
    mouse_area(body)
        .on_press(msg(PickerMsg::Open {
            index,
            split: false,
        }))
        .on_middle_press(msg(PickerMsg::Open { index, split: true }))
        .on_enter(msg(PickerMsg::Hover(pos)))
        .into()
}

/// The selected entry's details: avatar, name, facts, then Open / Split right / SFTP.
#[expect(clippy::too_many_lines, reason = "one widget tree, top to bottom")]
fn detail<'a>(index: usize, entry: &Entry, item: &FuzzyRow, look: Look) -> Element<'a, Message> {
    let avatar = container(
        text(initials(&item.title))
            .size(14)
            .font(crate::UI_MEDIUM)
            .color(t::A300),
    )
    .center(40)
    .style(|_theme: &iced::Theme| {
        container::background(t::A900).border(iced::Border::default().rounded(10))
    });
    let head = row![
        avatar,
        column![
            text(item.title.clone())
                .size(16)
                .font(crate::UI_MEDIUM)
                .color(look.fg)
                .wrapping(text::Wrapping::None),
            text(item.subtitle.clone())
                .size(11.5)
                .color(look.dim)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(2)
        .clip(true),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let source = match entry.origin {
        Origin::Stored => "config.toml",
        Origin::Builtin => "built-in",
        Origin::Quick => "quick connect",
        Origin::SshConfig => "~/.ssh/config",
    };
    let last = if item.detail.is_empty() {
        "never".to_owned()
    } else {
        item.detail.clone()
    };
    let mut facts: Vec<(&str, String)> = match &entry.profile.kind {
        ProfileKind::Ssh(ssh) => vec![
            (
                "Host",
                ssh.host.clone().unwrap_or_else(|| item.title.clone()),
            ),
            ("User", ssh.user.clone().unwrap_or_else(|| "—".to_owned())),
            ("Port", ssh.port.unwrap_or(22).to_string()),
            (
                "Identity",
                ssh.private_keys
                    .as_ref()
                    .and_then(|keys| keys.first())
                    .map_or_else(|| "default".to_owned(), |k| k.display().to_string()),
            ),
        ],
        ProfileKind::Local(local) => vec![
            (
                "Program",
                local
                    .program
                    .clone()
                    .unwrap_or_else(|| "login shell".to_owned()),
            ),
            (
                "Directory",
                local
                    .cwd
                    .as_ref()
                    .map_or_else(|| "~".to_owned(), |d| d.display().to_string()),
            ),
        ],
    };
    facts.extend([("Source", source.to_owned()), ("Last used", last)]);
    let facts = column(facts.into_iter().map(|(label, value)| {
        row![
            text(label).size(11.5).color(look.dim).width(88),
            text(value)
                .size(11.5)
                .font(Font::MONOSPACE)
                .color(look.fg)
                .wrapping(text::Wrapping::None),
        ]
        .into()
    }))
    .spacing(7)
    .clip(true);
    let action = |label: &str, kind: Btn, on_press: Message| {
        button(text(label.to_owned()).size(12.5))
            .padding([5, 10])
            .style(style::btn(look, kind))
            .on_press(on_press)
    };
    let mut actions = row![
        action(
            "Open",
            Btn::Primary,
            msg(PickerMsg::Open {
                index,
                split: false
            })
        ),
        action(
            "Split right",
            Btn::Secondary,
            msg(PickerMsg::Open { index, split: true })
        ),
    ]
    .spacing(6);
    if matches!(entry.profile.kind, ProfileKind::Ssh(_)) {
        actions = actions.push(action("SFTP", Btn::Secondary, msg(PickerMsg::Sftp(index))));
    }
    column![head, facts, actions]
        .spacing(18)
        .padding([18, 16])
        .into()
}

/// The picker panel over `base`; a click outside closes it.
#[expect(clippy::too_many_lines, reason = "one widget tree, top to bottom")]
pub(crate) fn overlay<'a>(
    base: Element<'a, Message>,
    picker: Option<&'a ProfilePicker>,
    entries: &[Entry],
    rows: &[FuzzyRow],
    look: Look,
) -> Element<'a, Message> {
    let Some(picker) = picker else { return base };
    let shown = fuzzy_list::rank(&picker.filter, rows);
    let mut list = column![].spacing(1);
    for (pos, &index) in shown.iter().enumerate() {
        if let (Some(entry), Some(item)) = (entries.get(index), rows.get(index)) {
            let selected = pos == picker.selected;
            list = list.push(entry_row(
                pos,
                index,
                entry,
                item,
                &picker.filter,
                selected,
                look,
            ));
        }
    }
    if shown.is_empty() {
        let hint = if profiles::quick_profile(&picker.filter).is_ok() {
            format!("↵ quick-connect to {}", picker.filter.trim())
        } else {
            "No matching profile".to_owned()
        };
        list = list.push(container(text(hint).size(12).color(look.dim)).padding(8));
    }
    let filter = text_input("Open a profile or user@host:port", &picker.filter)
        .id(Id::new(PICKER_INPUT))
        .on_input(|f| msg(PickerMsg::Filter(f)))
        .on_submit(msg(PickerMsg::Submit))
        .icon(text_input::Icon {
            font: PHOSPHOR,
            code_point: Icon::Search.glyph(),
            size: Some(17.into()),
            spacing: 10.0,
            side: text_input::Side::Left,
        })
        .size(15)
        .padding(0)
        .style({
            let borderless = style::bare_input(look);
            move |theme: &iced::Theme, status| text_input::Style {
                icon: look.accent_hi,
                ..borderless(theme, status)
            }
        });
    let esc = container(text("esc").size(10.5).color(look.dim))
        .padding([1, 6])
        .style(move |_theme: &iced::Theme| {
            container::Style::default().border(
                iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(look.border),
            )
        });
    let input_row = row![filter, esc]
        .spacing(10)
        .padding([12, 14])
        .align_y(Alignment::Center);
    let rule = |w: Length, h: Length| {
        let edge = look.border;
        container(space())
            .width(w)
            .height(h)
            .style(move |_theme: &iced::Theme| container::background(edge))
    };
    let scroller = Scrollbar::new().width(4).scroller_width(4);
    let list = scrollable(list.padding(6))
        .direction(Direction::Vertical(scroller))
        .style(crate::style::scrollbar)
        .width(Length::Fill)
        .height(Length::Fill);
    let selected = shown.get(picker.selected).copied();
    let side: Element<'a, Message> =
        match selected.and_then(|i| Some((i, entries.get(i)?, rows.get(i)?))) {
            Some((index, entry, item)) => detail(index, entry, item, look),
            None => space().into(),
        };
    let side_bg = mix(look.surface, look.base, 0.5);
    let side = container(side)
        .width(DETAIL_W)
        .height(Length::Fill)
        .style(move |_theme: &iced::Theme| container::background(side_bg));
    let middle = row![list, rule(Length::Fixed(1.0), Length::Fill), side].height(400);
    let footer = row![
        text("↑↓ navigate  ·  ↵ open  ·  ⇧↵ split")
            .size(11)
            .color(look.dim)
            .width(Length::Fill),
        text("open-profile-picker").size(11).color(look.dim),
    ]
    .padding([8, 14]);
    let panel = container(column![
        input_row,
        rule(Length::Fill, Length::Fixed(1.0)),
        middle,
        rule(Length::Fill, Length::Fixed(1.0)),
        footer
    ])
    .width(PANEL_W)
    .clip(true)
    .style(move |_theme: &iced::Theme| style::floating(look, t::R_PICKER));
    let dim = alpha(t::N900, 0.45);
    // The backdrop starts below the bar; a click anywhere outside the panel closes it.
    let click_away = mouse_area(column![
        space().height(crate::tab_bar::BAR_H),
        container(space())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_theme: &iced::Theme| container::background(dim)),
    ])
    .on_press(msg(PickerMsg::Close));
    let placed = container(opaque(panel))
        .padding(iced::Padding {
            top: 58.0,
            right: 20.0,
            bottom: 20.0,
            left: 20.0,
        })
        .center_x(Length::Fill);
    stack![base, opaque(click_away), placed].into()
}
