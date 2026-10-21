//! The start page ("New tab"): shown by a window opened while another Fogu runs (that one keeps
//! the saved tabs). Quick connect, the recent profiles and the usual ways to start.

use std::sync::LazyLock;

use fogu_core::{Action, ProfileId};
use iced::widget::{Id, button, column, container, image, row, text, text_input};
use iced::{Alignment, Element, Font, Length, Task};

use crate::app::App;
use crate::icons::{Icon, glyph};
use crate::message::Message;
use crate::palette::Look;
use crate::profiles::{self, Launch, now_secs};
use crate::style;
use crate::tab::{Tab, TabId};
use crate::tokens::{self as t, alpha};

/// Id of the quick-connect field.
pub const QUICK_INPUT: &str = "fogu-welcome-quick";
/// Recent profiles listed.
const RECENT: usize = 6;
/// The bundled logo, kept under one iced image id across redraws.
static LOGO: LazyLock<image::Handle> = LazyLock::new(|| {
    image::Handle::from_rgba(crate::ICON_SIZE, crate::ICON_SIZE, crate::ICON_RGBA)
});

/// The start page's state.
#[derive(Clone, Debug, Default)]
pub struct Welcome {
    pub quick: String,
    /// Why the typed `user@host:port` was refused.
    pub error: Option<String>,
}

/// Start page messages.
#[derive(Clone, Debug)]
pub enum WelcomeMsg {
    Quick(String),
    QuickSubmit,
    /// Open this profile here.
    Open(ProfileId),
    /// One of the "Start" actions.
    Run(Action),
}

impl App {
    /// Opens a start page tab and focuses its quick-connect field.
    pub(crate) fn open_welcome(&mut self) -> Task<Message> {
        let id = TabId(self.take_id());
        self.tabs.push(Tab::welcome(id));
        self.activate(id);
        iced::widget::operation::focus(Id::new(QUICK_INPUT))
    }

    pub(crate) fn on_welcome(&mut self, tab: TabId, message: WelcomeMsg) -> Task<Message> {
        let page = self
            .tabs
            .iter_mut()
            .find(|t| t.id == tab)
            .and_then(|t| t.welcome.as_deref_mut());
        let Some(page) = page else {
            return Task::none();
        };
        // Opening something replaces the start page.
        let opened = match message {
            WelcomeMsg::Quick(text) => {
                page.quick = text;
                page.error = None;
                return Task::none();
            }
            WelcomeMsg::QuickSubmit => match profiles::quick_profile(&page.quick) {
                Ok(profile) => self.quick_launch(profile, Launch::Tab),
                Err(err) => {
                    page.error = Some(err.to_string());
                    return Task::none();
                }
            },
            WelcomeMsg::Open(id) => {
                let Some(entry) = self.all_entries().into_iter().find(|e| e.profile.id == id)
                else {
                    return Task::none();
                };
                self.launch(&entry.profile, Launch::Tab)
            }
            WelcomeMsg::Run(Action::NewTab) => {
                let profile = self.default_profile();
                self.open_tab(&profile, None);
                Task::none()
            }
            WelcomeMsg::Run(action) => return self.update(Message::Action(action)),
        };
        Task::batch([opened, self.remove_tab(tab)])
    }
}

/// A keyboard chord as a small outlined chip.
fn kbd<'a>(label: String, look: Look) -> Element<'a, Message> {
    let edge = look.border;
    container(text(label).size(10.5).font(Font::MONOSPACE).color(look.dim))
        .padding([2, 6])
        .style(move |_theme: &iced::Theme| {
            container::Style::default()
                .border(iced::Border::default().rounded(4).width(1).color(edge))
        })
        .into()
}

/// The start page of tab `tab`.
pub fn page<'a>(tab: TabId, page: &'a Welcome, app: &'a App) -> Element<'a, Message> {
    let look = app.look;
    let msg = move |m: WelcomeMsg| Message::Welcome(tab, m);
    let logo = image((*LOGO).clone()).width(40).height(40);
    let brand = row![logo, text("FOGU TERMINAL").size(10.5).color(look.accent_hi)]
        .spacing(14)
        .align_y(Alignment::Center);
    let head = column![
        text("Where to?")
            .size(32)
            .font(crate::UI_MEDIUM)
            .color(look.fg),
        text("Pick a host, open a local shell, or type user@host:port.")
            .size(14)
            .color(look.dim),
    ]
    .spacing(6);
    let field = text_input("user@host:port", &page.quick)
        .id(Id::new(QUICK_INPUT))
        .on_input(move |q| msg(WelcomeMsg::Quick(q)))
        .on_submit(msg(WelcomeMsg::QuickSubmit))
        .font(Font::MONOSPACE)
        .size(15)
        .padding(0)
        .style(style::bare_input(look));
    let ring = if page.error.is_some() {
        look.danger
    } else {
        look.border
    };
    let surface = look.surface;
    let quick = container(
        row![
            glyph(Icon::Lightning, 16.0, look.accent_hi),
            field,
            kbd("↵".to_owned(), look)
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .height(42)
    .padding([0, 14])
    .align_y(Alignment::Center)
    .max_width(520)
    .style(move |_theme: &iced::Theme| {
        container::background(surface).border(
            iced::Border::default()
                .rounded(t::R_MD)
                .width(1)
                .color(ring),
        )
    });
    let mut top = column![brand, head, quick].spacing(26);
    if let Some(err) = &page.error {
        top = top.push(text(err.clone()).size(12).color(look.danger));
    }
    let columns = row![recent(app, msg, look), start(app, msg, look)].spacing(56);
    let body = column![top, columns]
        .spacing(30)
        .max_width(760)
        .padding([64, 72]);
    // A soft accent wash from the top-left over the ground.
    let wash = iced::gradient::Linear::new(iced::Radians(2.4))
        .add_stop(0.0, alpha(look.accent, 0.10))
        .add_stop(0.55, alpha(look.accent, 0.0));
    let (base, dark) = (look.base, look.dark);
    container(
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_theme: &iced::Theme| container::Style {
                background: dark.then(|| wash.into()),
                ..container::Style::default()
            }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_theme: &iced::Theme| container::background(base))
    .into()
}

/// A small uppercase column title.
fn label<'a>(title: &str, look: Look) -> Element<'a, Message> {
    text(title.to_owned()).size(10.5).color(look.dim).into()
}

/// The recently used profiles: initials tile, name over subtitle, when.
fn recent(app: &App, msg: impl Fn(WelcomeMsg) -> Message, look: Look) -> Element<'_, Message> {
    let entries = app.all_entries();
    let rows = profiles::rows(&entries, &app.config, &app.profiles.recent, now_secs());
    let mut used: Vec<(u64, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| Some((app.profiles.recent.last_used(&e.profile.id)?, i)))
        .collect();
    used.sort_by(|a, b| b.cmp(a));
    let mut list = column![label("RECENT", look)].spacing(4);
    if used.is_empty() {
        list = list.push(text("Nothing yet.").size(12.5).color(look.dim));
    }
    for (_, i) in used.into_iter().take(RECENT) {
        let (Some(entry), Some(item)) = (entries.get(i), rows.get(i)) else {
            continue;
        };
        let initials: String = item
            .title
            .split(|c: char| !c.is_alphanumeric())
            .filter_map(|w| w.chars().next())
            .take(1)
            .flat_map(char::to_uppercase)
            .collect();
        let tile = container(text(initials).size(11.5).color(t::A300))
            .center(28)
            .style(|_theme: &iced::Theme| {
                container::background(t::A900).border(iced::Border::default().rounded(7))
            });
        let line = row![
            tile,
            column![
                text(item.title.clone()).size(13).color(look.fg),
                text(item.subtitle.clone()).size(11.5).color(look.dim),
            ]
            .spacing(1)
            .width(Length::Fill),
            text(item.detail.clone()).size(11).color(look.dim),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        list = list.push(
            button(line)
                .width(Length::Fill)
                .padding([6, 8])
                .style(style::btn(look, style::Btn::Flat))
                .on_press(msg(WelcomeMsg::Open(entry.profile.id.clone()))),
        );
    }
    container(list).width(320).into()
}

/// The ways to start, each with its key chord.
fn start(app: &App, msg: impl Fn(WelcomeMsg) -> Message, look: Look) -> Element<'_, Message> {
    let actions = [
        (Icon::Local, "New local shell", Action::NewTab),
        (
            Icon::Search,
            "Open profile picker",
            Action::OpenProfilePicker,
        ),
        (Icon::Sidebar, "Show profiles", Action::ToggleProfiles),
        (Icon::Gear, "Settings", Action::OpenSettings),
    ];
    let mut list = column![label("START", look)].spacing(4);
    for (icon, title, action) in actions {
        let chord = app
            .config
            .hotkeys
            .chords_for(action)
            .first()
            // Config syntax is `Ctrl-Shift-T`; shown the usual way.
            .map(|c| c.to_string().replace('-', "+"));
        let mut line = row![
            glyph(icon, 15.0, look.dim),
            text(title).size(13).color(look.fg).width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        if let Some(chord) = chord {
            line = line.push(kbd(chord, look));
        }
        list = list.push(
            button(line)
                .width(Length::Fill)
                .padding([7, 8])
                .style(style::btn(look, style::Btn::Flat))
                .on_press(msg(WelcomeMsg::Run(action))),
        );
    }
    container(list).width(260).into()
}
