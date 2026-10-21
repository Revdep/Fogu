//! The editor page: a left nav (avatar, name, one row per section), the chosen section under the
//! header (kicker, name, Save / Cancel / Delete), and, when there is room, the lines Save will
//! write to config.toml. Sections: General, then the kind's (`view_ssh.rs` for SSH).

use std::fmt::Display;

use fogu_core::config::ConfigIssue;
use fogu_core::config::ssh_opts::NOT_SUPPORTED_YET;
use fogu_core::{Background, Config, LocalOpts, ProfileKind};
use iced::widget::{Column, button, column, container, row, scrollable, space, text};
use iced::{Alignment, Element, Font, Length};

use super::fields::{AlgKind, BG_FIT, BG_KIND, FieldId, OSC52, SESSION_END};
use super::handlers::EditorMsg;
use super::{ProfileEditor, Source};
use crate::forms::{self, ERROR, Field, FieldValue, WARN};
use crate::icons::Icon;
use crate::message::Message;
use crate::sftp::panel::Look;
use crate::style::{self, Btn};
use crate::tab::TabId;
use crate::tokens::{self as t, alpha};

/// One form row (or table) of the page.
pub(super) type Row<'a> = Element<'a, Field<FieldId>>;
/// A section: its nav title, icon and form.
pub(super) type Section<'a> = (&'static str, Icon, Row<'a>);

/// Widest the form gets, logical px.
const MAX_W: f32 = 760.0;
const NAV_W: f32 = 190.0;
const DIFF_W: f32 = 300.0;
/// Next to an option whose behaviour is not implemented yet.
pub(super) const NOT_YET: &str = "Not supported yet: saved, but has no effect for now.";

/// Id of the editor page's scrollable.
pub(crate) const PAGE_SCROLL: &str = "fogu-editor-page";

/// Id of section `i`'s container (the nav scrolls to it).
pub(crate) fn section_id(i: usize) -> iced::widget::Id {
    iced::widget::Id::from(format!("fogu-editor-section-{i}"))
}

/// The page of editor tab `tab`; `diff` adds the config.toml column (the sidebar is closed).
pub fn page<'a>(
    tab: TabId,
    ed: &'a ProfileEditor,
    config: &Config,
    diff: bool,
    look: Look,
) -> Element<'a, Message> {
    let changes = ed.changes();
    let mut sections: Vec<Section<'a>> = vec![
        ("General", Icon::IdCard, general(ed, look)),
        ("Appearance", Icon::Palette, appearance(ed, config, look)),
        ("Session", Icon::History, session(ed, look)),
    ];
    match &ed.draft.kind {
        ProfileKind::Local(local) => {
            sections.push((
                "Command",
                Icon::Local,
                local_sections(ed, local, config, look),
            ));
        }
        ProfileKind::Ssh(ssh) => sections.extend(super::view_ssh::sections(ed, ssh, config, look)),
    }
    let current = ed.section.min(sections.len().saturating_sub(1));
    let nav = nav(tab, ed, config, &sections, current, look);
    let form: Element<'a, Field<FieldId>> = Column::with_children(
        sections
            .into_iter()
            .enumerate()
            .map(|(i, (_, _, form))| container(form).id(section_id(i)).into()),
    )
    .into();
    let form = form.map(move |Field(id, value)| Message::Editor(tab, EditorMsg::Field(id, value)));
    let body = column![header(tab, ed, changes.len(), look), form]
        .spacing(18)
        .max_width(MAX_W)
        .padding([26, 32]);
    let bg = look.base;
    let scroll = scrollable(body)
        .id(iced::widget::Id::new(PAGE_SCROLL))
        .style(crate::style::scrollbar);
    let content = container(scroll)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_theme: &iced::Theme| container::background(bg));
    let mut page = row![nav, rule(look), content];
    if diff && !ed.read_only() {
        page = page.push(rule(look)).push(diff_column(&changes, look));
    }
    page.into()
}

/// A 1 px vertical divider.
fn rule<'a>(look: Look) -> Element<'a, Message> {
    let edge = look.border;
    container(space())
        .width(1)
        .height(Length::Fill)
        .style(move |_theme: &iced::Theme| container::background(edge))
        .into()
}

/// The profile's avatar: its icon, else its initials, tinted with its tab colour.
fn avatar<'a>(ed: &ProfileEditor, size: f32) -> Element<'a, Message> {
    let p = &ed.draft;
    let mark = crate::profiles::glyph(p.icon.as_deref()).unwrap_or_else(|| {
        p.name
            .split(|c: char| !c.is_alphanumeric())
            .filter_map(|w| w.chars().next())
            .take(2)
            .flat_map(char::to_uppercase)
            .collect()
    });
    let (fill, fg) = match p.color {
        Some(c) => {
            let c = crate::context_menu::to_iced(c);
            (alpha(c, 0.22), c)
        }
        None => (t::A900, t::A300),
    };
    container(text(mark).size(size * 0.4).font(crate::UI_MEDIUM).color(fg))
        .center(size)
        .style(move |_theme: &iced::Theme| {
            container::background(fill).border(iced::Border::default().rounded(size * 0.28))
        })
        .into()
}

/// Avatar, name, "<GROUP> · SSH profile", then one row per section.
fn nav<'a>(
    tab: TabId,
    ed: &ProfileEditor,
    config: &Config,
    sections: &[Section<'a>],
    current: usize,
    look: Look,
) -> Element<'a, Message> {
    let group = ed
        .draft
        .group
        .as_ref()
        .and_then(|g| config.groups.iter().find(|x| &x.id == g))
        .map(|g| format!("{} · ", g.name.to_uppercase()))
        .unwrap_or_default();
    let kind = if ed.is_ssh() {
        "SSH profile"
    } else {
        "Local profile"
    };
    let name = fogu_term::sanitize_title(ed.draft.name.trim());
    let who = row![
        avatar(ed, 34.0),
        column![
            text(if name.is_empty() {
                "Untitled".to_owned()
            } else {
                name
            })
            .size(13)
            .font(crate::UI_MEDIUM)
            .color(look.fg)
            .wrapping(text::Wrapping::None),
            text(format!("{group}{kind}"))
                .size(10.5)
                .color(look.dim)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(2)
        .clip(true),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let mut items = column![container(who).padding([4, 6]), space().height(10)].spacing(2);
    for (i, (title, icon, _)) in sections.iter().enumerate() {
        items = items.push(forms::nav_item(
            look,
            *icon,
            title,
            i == current,
            false,
            Message::Editor(tab, EditorMsg::Section(i)),
        ));
    }
    let bg = look.bg;
    container(items.padding([14, 8]))
        .width(NAV_W)
        .height(Length::Fill)
        .style(move |_theme: &iced::Theme| container::background(bg))
        .into()
}

/// "Will be written to config.toml": the changed keys as `+ key = value` lines.
fn diff_column<'a>(changes: &[(String, String)], look: Look) -> Element<'a, Message> {
    let mut lines = Column::new().spacing(4);
    if changes.is_empty() {
        lines = lines.push(
            text("No changes yet. Edited values show up here.")
                .size(12)
                .color(look.dim),
        );
    }
    let diff_color = if look.dark { t::A300 } else { look.accent };
    for (key, value) in changes {
        lines = lines.push(
            text(format!("+ {key} = {value}"))
                .size(11.5)
                .font(Font::MONOSPACE)
                .color(diff_color),
        );
    }
    let bg = look.bg;
    container(
        column![
            text("WILL BE WRITTEN TO CONFIG.TOML")
                .size(10.5)
                .color(look.dim),
            lines
        ]
        .spacing(12)
        .padding([26, 16]),
    )
    .width(DIFF_W)
    .height(Length::Fill)
    .style(move |_theme: &iced::Theme| container::background(bg))
    .into()
}

fn header(tab: TabId, ed: &ProfileEditor, change_count: usize, look: Look) -> Element<'_, Message> {
    let msg = move |m: EditorMsg| Message::Editor(tab, m);
    let action = |label: String, kind: Btn, on: Option<Message>| {
        button(text(label).size(13))
            .padding([6, 14])
            .style(style::btn(look, kind))
            .on_press_maybe(on)
    };
    let mut buttons = row![].spacing(8).align_y(Alignment::Center);
    if ed.read_only() {
        buttons = buttons.push(action(
            "Copy to my profiles".to_owned(),
            Btn::Primary,
            Some(msg(EditorMsg::CopyToMine)),
        ));
        buttons = buttons.push(action(
            "Close".to_owned(),
            Btn::Secondary,
            Some(msg(EditorMsg::Cancel)),
        ));
    } else {
        if matches!(ed.source, Source::Stored(_)) {
            buttons = buttons.push(action(
                "Delete…".to_owned(),
                Btn::Danger,
                Some(msg(EditorMsg::Delete)),
            ));
        }
        buttons = buttons.push(action(
            "Cancel".to_owned(),
            Btn::Secondary,
            Some(msg(EditorMsg::Cancel)),
        ));
        let count = change_count;
        let save: Element<'_, Message> = if count > 0 {
            row![
                text("Save").size(13),
                container(text(count.to_string()).size(10).color(t::A100))
                    .padding([1, 6])
                    .style(|_theme: &iced::Theme| {
                        container::background(t::A800).border(iced::Border::default().rounded(999))
                    })
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            text("Save").size(13).into()
        };
        buttons = buttons.push(
            button(save)
                .padding([6, 14])
                .style(style::btn(look, Btn::Primary))
                .on_press_maybe(ed.can_save().then(|| msg(EditorMsg::Save))),
        );
    }
    let hint = if ed.read_only() {
        "From ~/.ssh/config. Changing a field copies it to your profiles first; the file itself \
         is never changed."
    } else {
        "Save writes only the values you changed — empty fields use the default shown."
    };
    let name = fogu_term::sanitize_title(ed.draft.name.trim());
    let title = if name.is_empty() {
        ed.tab_title()
    } else {
        name
    };
    let head = forms::page_head(look, Some("Profile"), &title, hint).width(Length::Fill);
    let mut head = column![row![head, buttons].spacing(16).align_y(Alignment::Start)].spacing(6);
    if ed.read_only() {
        if let ProfileKind::Ssh(ssh) = &ed.draft.kind
            && !ssh.ignored_ssh_config.is_empty()
        {
            let unused = format!("Not used: {}", ssh.ignored_ssh_config.join(", "));
            head = head.push(text(unused).size(13).color(look.dim));
        }
        return head.into();
    }
    // One status line, always there, so the form doesn't jump while typing; the problems
    // themselves are shown under their fields.
    let status = if ed.issues.is_empty() && ed.errors.is_empty() {
        text("")
    } else {
        text("Fix what is marked in red to save.").color(ERROR)
    };
    head.push(status.size(13)).into()
}

/// The field a validation issue is shown under.
fn issue_field(issue: &ConfigIssue) -> FieldId {
    match issue {
        ConfigIssue::MissingHost(_) => FieldId::Host,
        ConfigIssue::JumpHostCycle { .. } => FieldId::JumpHost,
        ConfigIssue::BadForward { .. } => FieldId::Forwards,
        ConfigIssue::UnknownAlgorithms { .. } => FieldId::Alg(AlgKind::Kex),
        ConfigIssue::UnknownGroup { .. } => FieldId::Group,
        _ => FieldId::Name,
    }
}

/// The value shown in a number/colour field: what was typed, else the stored value.
pub(super) fn shown<T: Display>(ed: &ProfileEditor, id: FieldId, value: Option<T>) -> String {
    ed.texts
        .get(&id)
        .cloned()
        .unwrap_or_else(|| value.map(|v| v.to_string()).unwrap_or_default())
}

/// `row` plus its parse error (red), else its "not supported yet" note when `key` is listed.
pub(super) fn with_notes<'a>(ed: &ProfileEditor, id: FieldId, key: &str, row: Row<'a>) -> Row<'a> {
    let issue = ed
        .issues
        .iter()
        .find(|i| issue_field(i) == id)
        .map(|i| capitalized(&i.to_string()));
    let line = if let Some(error) = ed.errors.get(&id) {
        Some((error.as_str(), ERROR))
    } else if let Some(issue) = &issue {
        Some((issue.as_str(), ERROR))
    } else if NOT_SUPPORTED_YET.contains(&key) {
        Some((NOT_YET, WARN))
    } else {
        None
    };
    forms::note_maybe(row, line)
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Index of the option equal to `value`.
pub(super) fn index_of<T: PartialEq>(options: &[(&str, T)], value: &T) -> usize {
    options.iter().position(|(_, v)| v == value).unwrap_or(0)
}

pub(super) fn labels<T>(options: &[(&str, T)]) -> Vec<String> {
    options.iter().map(|(l, _)| (*l).to_owned()).collect()
}

/// Id of the new group's name field.
pub(crate) const NEW_GROUP_INPUT: &str = "fogu-editor-new-group";

/// The icon quick picks (the "…" field takes anything else).
const EMOJI: [&str; 10] = ["🤖", "🔥", "🏠", "🐘", "🧪", "🛡️", "🌐", "📦", "⚡", "🐳"];

fn general(ed: &ProfileEditor, look: Look) -> Row<'_> {
    let p = &ed.draft;
    let group_names: Vec<String> = ed.groups.iter().skip(1).map(|(_, l)| l.clone()).collect();
    // `ed.groups[0]` is Ungrouped: the pills show the real groups, clicking the chosen one
    // again ungroups the profile.
    let selected = ed
        .groups
        .iter()
        .position(|(g, _)| g == &p.group)
        .and_then(|i| i.checked_sub(1));
    let new_group = Field(FieldId::Group, FieldValue::Choice(ed.groups.len()));
    let pills = forms::pills(
        look,
        &group_names,
        selected,
        |i| Field(FieldId::Group, FieldValue::Choice(i + 1)),
        Some(&Field(FieldId::Group, FieldValue::Choice(0))),
        Some(("New group", new_group)),
    );
    let mut rows = vec![
        with_notes(
            ed,
            FieldId::Name,
            "name",
            forms::text_field(look, FieldId::Name, "Name", &p.name, "Profile name"),
        ),
        with_notes(
            ed,
            FieldId::Group,
            "group",
            forms::labeled(look, "Group", pills),
        ),
    ];
    if let Some(name) = &ed.new_group {
        let field = forms::input(look, FieldId::NewGroup, name, "Group name (Enter or Save)")
            .id(iced::widget::Id::new(NEW_GROUP_INPUT))
            .on_submit(forms::Field(FieldId::NewGroup, FieldValue::Add));
        rows.push(forms::labeled(look, "New group", field.into()));
    }
    rows.push(icon_row(ed, look));
    forms::section(look, "General", rows)
}

/// "Initials", the emoji quick picks and a field for any other symbol.
fn icon_row<'a>(ed: &ProfileEditor, look: Look) -> Row<'a> {
    let current = ed.draft.icon.as_deref().unwrap_or_default();
    let pick = |label: String, value: &str, on: bool, width: f32| {
        let value = value.to_owned();
        button(
            container(text(label).size(if width > 40.0 { 11.5 } else { 16.0 }))
                .center(Length::Fill),
        )
        .width(width)
        .height(32)
        .padding(0)
        .style(move |_theme: &iced::Theme, status| button::Style {
            background: (on || matches!(status, button::Status::Hovered))
                .then(|| alpha(look.fg, 0.08).into()),
            text_color: look.fg,
            border: iced::Border::default()
                .rounded(t::R_MD)
                .width(if on { 1.0 } else { 0.0 })
                .color(look.accent),
            ..button::Style::default()
        })
        .on_press(Field(FieldId::Icon, FieldValue::Text(value)))
    };
    let mut picks = iced::widget::Row::new()
        .spacing(4)
        .align_y(Alignment::Center)
        .push(pick("Initials".to_owned(), "", current.is_empty(), 56.0));
    for emoji in EMOJI {
        picks = picks.push(pick(emoji.to_owned(), emoji, current == emoji, 32.0));
    }
    let custom = !current.is_empty() && !EMOJI.contains(&current);
    let field = forms::input(look, FieldId::Icon, if custom { current } else { "" }, "…")
        .width(48)
        .align_x(Alignment::Center);
    let picks = picks.push(field).wrap().vertical_spacing(4);
    forms::labeled_hint(look, "Icon", "One symbol or emoji", picks.into())
}

fn appearance<'a>(ed: &'a ProfileEditor, config: &Config, look: Look) -> Row<'a> {
    let p = &ed.draft;
    let color_hex = shown(ed, FieldId::Color, p.color);
    let mut rows = vec![with_notes(
        ed,
        FieldId::Color,
        "color",
        forms::color(
            look,
            FieldId::Color,
            ("Tab colour", "Marks the tab, its panes and this card"),
            p.color,
            &color_hex,
            ed.expanded.contains(&FieldId::Color),
        ),
    )];
    rows.extend(background(ed, look));
    rows.extend([
        forms::toggle_desc(
            look,
            FieldId::DisableDynamicTitle,
            "Keep profile name as title",
            "Programs cannot rename the tab",
            p.disable_dynamic_title,
        ),
        forms::toggle_desc(
            look,
            FieldId::RestoreScrollback,
            "Save scrollback",
            "Restored with the tab after a restart",
            p.restore_scrollback
                .unwrap_or(config.recovery.restore_scrollback),
        ),
    ]);
    forms::section(look, "Appearance", rows)
}

fn session(ed: &ProfileEditor, look: Look) -> Row<'_> {
    let p = &ed.draft;
    let osc52 = match &p.kind {
        ProfileKind::Local(local) => local.osc52_write,
        ProfileKind::Ssh(ssh) => ssh.osc52_write,
    };
    forms::section(
        look,
        "Session",
        vec![
            forms::choice(
                look,
                FieldId::SessionEnd,
                "When the session ends",
                labels(&SESSION_END),
                index_of(&SESSION_END, &p.behavior_on_session_end),
            ),
            forms::choice(
                look,
                FieldId::Osc52,
                "Clipboard (OSC 52)",
                labels(&OSC52),
                index_of(&OSC52, &osc52),
            ),
        ],
    )
}

/// The profile background: none, a colour, or an image with fit and opacity.
fn background(ed: &ProfileEditor, look: Look) -> Vec<Row<'_>> {
    let bg = &ed.draft.background;
    let kind = match bg {
        None => 0,
        Some(Background::Color(_)) => 1,
        Some(Background::Image { .. }) => 2,
    };
    let kinds: Vec<String> = BG_KIND.iter().map(|k| (*k).to_owned()).collect();
    let segments = forms::segmented(look, &kinds, kind, |i| {
        Field(FieldId::BgKind, FieldValue::Choice(i))
    });
    let mut rows = vec![forms::labeled_hint(
        look,
        "Background",
        "Behind the terminal text",
        segments,
    )];
    match bg {
        None => {}
        Some(Background::Color(c)) => {
            let hex = shown(ed, FieldId::BgColor, Some(*c));
            let row = forms::color(
                look,
                FieldId::BgColor,
                ("Background colour", "Under the terminal text"),
                Some(*c),
                &hex,
                ed.expanded.contains(&FieldId::BgColor),
            );
            rows.push(with_notes(ed, FieldId::BgColor, "", row));
        }
        Some(Background::Image {
            path, opacity, fit, ..
        }) => {
            let path = path.to_string_lossy();
            let image = forms::path(
                look,
                FieldId::BgImage,
                "Background image",
                &path,
                "PNG, JPEG or WebP file",
                ed.picker,
            );
            rows.push(with_notes(ed, FieldId::BgImage, "", image));
            rows.push(forms::choice(
                look,
                FieldId::BgFit,
                "Image fit",
                labels(&BG_FIT),
                index_of(&BG_FIT, fit),
            ));
            let percent = (opacity * 100.0).round();
            let row = forms::number(
                look,
                FieldId::BgOpacity,
                "Image opacity (%)",
                &shown(ed, FieldId::BgOpacity, Some(percent)),
                "85",
            );
            rows.push(with_notes(ed, FieldId::BgOpacity, "", row));
        }
    }
    rows
}

fn local_sections<'a>(
    ed: &'a ProfileEditor,
    local: &'a LocalOpts,
    config: &Config,
    look: Look,
) -> Row<'a> {
    let d = &config.profile_defaults.local;
    let program_hint = d.program.clone().unwrap_or_else(|| "$SHELL".to_owned());
    let cwd = local.cwd.as_ref().map(|c| c.to_string_lossy().into_owned());
    let cwd_hint = d
        .cwd
        .as_ref()
        .map_or_else(|| "$HOME".to_owned(), |c| c.to_string_lossy().into_owned());
    let login = local.login.or(d.login).unwrap_or(fogu_core::DEFAULT_LOGIN);
    let command = vec![
        forms::text_field(
            look,
            FieldId::Program,
            "Program",
            local.program.as_deref().unwrap_or_default(),
            &program_hint,
        ),
        forms::list(
            look,
            FieldId::Args,
            "Arguments (one per row, no shell)",
            local.args.as_deref().unwrap_or_default(),
            "argument",
            None,
        ),
        forms::path(
            look,
            FieldId::LocalCwd,
            "Start directory",
            cwd.as_deref().unwrap_or_default(),
            &cwd_hint,
            ed.picker,
        ),
        forms::kv_list(
            look,
            FieldId::Env,
            "Environment (${VAR} expands)",
            &ed.env_rows,
        ),
        forms::toggle(look, FieldId::Login, "Login shell", login),
    ];
    forms::section(look, "Command", command)
}
