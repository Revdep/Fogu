//! The editor page: header (title, read-only banner, issues, Save / Cancel / Delete), then the
//! General section, the kind's sections (`view_ssh.rs` for SSH) in a scrollable column.

use std::fmt::Display;

use fogu_core::config::ConfigIssue;
use fogu_core::config::ssh_opts::NOT_SUPPORTED_YET;
use fogu_core::{Background, Config, LocalOpts, ProfileKind};
use iced::widget::{Column, button, column, container, row, scrollable, space, text};
use iced::{Alignment, Element, Length};

use super::fields::{AlgKind, BG_FIT, BG_KIND, FieldId, OSC52, SESSION_END};
use super::handlers::EditorMsg;
use super::{ProfileEditor, Source};
use crate::forms::{self, ERROR, Field, FieldValue, WARN};
use crate::message::Message;
use crate::sftp::panel::Look;
use crate::tab::TabId;

/// One form row (or table) of the page.
pub(super) type Row<'a> = Element<'a, Field<FieldId>>;

/// Widest the form gets, logical px.
const MAX_W: f32 = 920.0;
/// Next to an option whose behaviour is not implemented yet.
pub(super) const NOT_YET: &str = "Not supported yet: saved, but has no effect for now.";

/// The page of editor tab `tab`.
pub fn page<'a>(
    tab: TabId,
    ed: &'a ProfileEditor,
    config: &Config,
    look: Look,
) -> Element<'a, Message> {
    let mut sections: Vec<Row<'a>> = vec![general(ed, config, look)];
    match &ed.draft.kind {
        ProfileKind::Local(local) => sections.extend(local_sections(ed, local, config, look)),
        ProfileKind::Ssh(ssh) => sections.extend(super::view_ssh::sections(ed, ssh, config, look)),
    }
    let form: Element<'a, Field<FieldId>> = Column::with_children(sections).spacing(12).into();
    let form = form.map(move |Field(id, value)| Message::Editor(tab, EditorMsg::Field(id, value)));
    let body = column![header(tab, ed, look), form]
        .spacing(12)
        .max_width(MAX_W)
        .padding(16);
    let bg = look.bg;
    container(scrollable(container(body).center_x(Length::Fill)))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_theme: &iced::Theme| container::background(bg))
        .into()
}

fn header(tab: TabId, ed: &ProfileEditor, look: Look) -> Element<'_, Message> {
    let msg = move |m: EditorMsg| Message::Editor(tab, m);
    let action = |label: &str, on: Option<Message>| {
        button(text(label.to_owned()).size(13))
            .padding([4, 12])
            .on_press_maybe(on)
    };
    let mut buttons = row![].spacing(8);
    if ed.read_only() {
        buttons = buttons.push(action(
            "Copy to my profiles",
            Some(msg(EditorMsg::CopyToMine)),
        ));
        buttons = buttons.push(action("Close", Some(msg(EditorMsg::Cancel))));
    } else {
        buttons = buttons.push(action("Save", ed.can_save().then(|| msg(EditorMsg::Save))));
        buttons = buttons.push(action("Cancel", Some(msg(EditorMsg::Cancel))));
        if matches!(ed.source, Source::Stored(_)) {
            buttons = buttons.push(action("Delete…", Some(msg(EditorMsg::Delete))));
        }
    }
    let title = text(ed.tab_title()).size(20).color(look.fg);
    let mut head =
        column![row![title, space().width(Length::Fill), buttons].align_y(Alignment::Center)]
            .spacing(6);
    if ed.read_only() {
        let banner = "From ~/.ssh/config. Changing a field copies it to your profiles first; \
                      the file itself is never changed.";
        head = head.push(text(banner).size(13).color(WARN));
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
        text("Save writes only the values you changed; empty fields use the default shown.")
            .color(look.dim)
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

/// The last Group choice: name a new group inline.
pub(crate) const NEW_GROUP: &str = "New group…";
/// Id of the new group's name field.
pub(crate) const NEW_GROUP_INPUT: &str = "fogu-editor-new-group";

fn general<'a>(ed: &'a ProfileEditor, config: &Config, look: Look) -> Row<'a> {
    let p = &ed.draft;
    let group = ed
        .groups
        .iter()
        .position(|(g, _)| g == &p.group)
        .unwrap_or(0);
    let osc52 = match &p.kind {
        ProfileKind::Local(local) => local.osc52_write,
        ProfileKind::Ssh(ssh) => ssh.osc52_write,
    };
    let color_hex = shown(ed, FieldId::Color, p.color);
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
            forms::choice(
                look,
                FieldId::Group,
                "Group",
                ed.groups
                    .iter()
                    .map(|(_, l)| l.clone())
                    .chain([NEW_GROUP.to_owned()])
                    .collect(),
                group,
            ),
        ),
    ];
    if let Some(name) = &ed.new_group {
        let field = forms::input(FieldId::NewGroup, name, "Group name (Enter or Save)")
            .id(iced::widget::Id::new(NEW_GROUP_INPUT))
            .on_submit(forms::Field(FieldId::NewGroup, FieldValue::Add));
        rows.push(forms::labeled(look, "New group", field.into()));
    }
    rows.extend([
        forms::text_field(
            look,
            FieldId::Icon,
            "Icon (one symbol or emoji)",
            p.icon.as_deref().unwrap_or_default(),
            "e.g. 🍓",
        ),
        with_notes(
            ed,
            FieldId::Color,
            "color",
            forms::color(look, FieldId::Color, "Tab colour", p.color, &color_hex),
        ),
        forms::toggle(
            look,
            FieldId::DisableDynamicTitle,
            "Keep the profile name as tab title",
            p.disable_dynamic_title,
        ),
        forms::toggle(
            look,
            FieldId::RestoreScrollback,
            "Save scrollback for tab recovery",
            p.restore_scrollback
                .unwrap_or(config.recovery.restore_scrollback),
        ),
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
            "Programs may copy to the clipboard (OSC 52)",
            labels(&OSC52),
            index_of(&OSC52, &osc52),
        ),
    ]);
    rows.extend(background(ed, look));
    forms::section(look, "General", rows)
}

/// The profile background: none, a colour, or an image with fit and opacity.
fn background(ed: &ProfileEditor, look: Look) -> Vec<Row<'_>> {
    let bg = &ed.draft.background;
    let kind = match bg {
        None => 0,
        Some(Background::Color(_)) => 1,
        Some(Background::Image { .. }) => 2,
    };
    let kinds = BG_KIND.iter().map(|k| (*k).to_owned()).collect();
    let mut rows = vec![forms::choice(
        look,
        FieldId::BgKind,
        "Background",
        kinds,
        kind,
    )];
    match bg {
        None => {}
        Some(Background::Color(c)) => {
            let hex = shown(ed, FieldId::BgColor, Some(*c));
            let row = forms::color(look, FieldId::BgColor, "Background colour", Some(*c), &hex);
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
) -> Vec<Row<'a>> {
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
    vec![forms::section(look, "Command", command)]
}
