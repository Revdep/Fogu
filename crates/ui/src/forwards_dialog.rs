//! The "Port forwarding" dialog of an SSH pane: its forwards with state and connection count,
//! Remove, and an "Add" form built with the generic form helpers (with an explicit "Save to
//! profile" toggle that appends the forward to the stored profile's `forwarded_ports`). Opened from
//! the tab menu and the tab bar's "Ports" button; the session does the work and reports
//! `ForwardStatus`, kept per pane in `Pane::forwards`.

use fogu_core::config::ProfileEdit;
use fogu_core::{ForwardKind, PortForward, ProfileKind};
use fogu_session::{
    ForwardCmd, ForwardId, ForwardState, ForwardStatus, SessionInput, sanitize_remote,
};
use iced::widget::{Column, button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Task};

use crate::app::App;
use crate::forms::{self, ERROR, Field, FieldValue, WARN};
use crate::icons::{Icon, icon};
use crate::message::Message;
use crate::notifications::{Level, Notification};
use crate::sftp::panel::Look;
use crate::style::{self, Btn};
use crate::tab::{Pane, PaneId};
use crate::tokens::{self as t, alpha};

const KINDS: [(&str, ForwardKind); 3] = [
    ("Local (-L)", ForwardKind::Local),
    ("Remote (-R)", ForwardKind::Remote),
    ("Dynamic SOCKS5 (-D)", ForwardKind::Dynamic),
];
/// Longest forward label / error shown, in characters.
const SHOWN_MAX_CHARS: usize = 120;

/// The open dialog: which pane, and the "Add" form.
#[derive(Debug)]
pub struct ForwardsDialog {
    pub pane: PaneId,
    pub draft: PortForward,
    /// Port fields as typed (parsed on Add).
    pub port: String,
    pub target_port: String,
    pub save: bool,
    /// Why the last Add was refused.
    pub error: Option<String>,
}

/// The form's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FwdField {
    Kind,
    Host,
    Port,
    Target,
    TargetPort,
    Description,
    Save,
}

/// Dialog input.
#[derive(Clone, Debug)]
pub enum ForwardsMsg {
    /// Open for the active tab's SSH pane.
    Open,
    Close,
    Field(Field<FwdField>),
    Add,
    Remove(ForwardId),
}

/// Applies a status report to a pane's list: `Stopped` removes the forward.
pub fn apply_status(list: &mut Vec<ForwardStatus>, status: ForwardStatus) {
    let at = list.iter().position(|s| s.id == status.id);
    match (at, status.state == ForwardState::Stopped) {
        (Some(at), true) => {
            list.remove(at);
        }
        (Some(at), false) => {
            if let Some(slot) = list.get_mut(at) {
                *slot = status;
            }
        }
        (None, false) => list.push(status),
        (None, true) => {}
    }
}

/// Forwards of the given SSH panes that are listening (the tab bar badge, the close
/// confirmation), each connection's forwards counted once (panes can share one).
pub fn listening<'a>(panes: impl Iterator<Item = &'a Pane>) -> usize {
    let mut seen = Vec::new();
    for pane in panes {
        for s in pane
            .forwards
            .iter()
            .filter(|s| s.state == ForwardState::Listening)
        {
            let key = (pane.connection.ok_or(pane.id), s.id);
            if !seen.contains(&key) {
                seen.push(key);
            }
        }
    }
    seen.len()
}

/// The form's draft as a forward, or why it isn't one.
fn parse(d: &ForwardsDialog) -> Result<PortForward, String> {
    let port = |text: &str, what: &str| {
        text.trim()
            .parse::<u16>()
            .map_err(|_| format!("the {what} must be a number 1–65535"))
    };
    let mut forward = d.draft.clone();
    forward.port = port(&d.port, "listen port")?;
    forward.target_port = match forward.kind {
        ForwardKind::Dynamic => 0,
        _ => port(&d.target_port, "target port")?,
    };
    if forward.kind == ForwardKind::Dynamic {
        forward.target_address.clear();
    }
    match forward.problem() {
        Some(problem) => Err(problem.to_owned()),
        None => Ok(forward),
    }
}

impl App {
    pub(crate) fn on_forwards(&mut self, message: ForwardsMsg) -> Task<Message> {
        match message {
            ForwardsMsg::Open => {
                self.menu = None;
                let tab = self.active_tab();
                let pane = tab
                    .and_then(|t| t.focused_pane())
                    .filter(|p| p.is_ssh())
                    .or_else(|| tab.and_then(|t| t.panes.values().find(|p| p.is_ssh())));
                self.forwards = pane.map(|p| ForwardsDialog {
                    pane: p.id,
                    draft: PortForward::default(),
                    port: String::new(),
                    target_port: String::new(),
                    save: false,
                    error: None,
                });
            }
            ForwardsMsg::Close => self.forwards = None,
            ForwardsMsg::Field(Field(field, value)) => {
                if let Some(d) = self.forwards.as_mut() {
                    edit(d, field, value);
                }
            }
            ForwardsMsg::Add => return self.add_forward(),
            ForwardsMsg::Remove(id) => {
                let pane = self.forwards.as_ref().map(|d| d.pane);
                if let Some(pane) = pane.and_then(|p| self.pane(p)) {
                    pane.control(SessionInput::Forward(ForwardCmd::Remove(id)));
                }
            }
        }
        Task::none()
    }

    fn add_forward(&mut self) -> Task<Message> {
        let Some(d) = self.forwards.as_mut() else {
            return Task::none();
        };
        let forward = match parse(d) {
            Ok(forward) => forward,
            Err(why) => {
                d.error = Some(why);
                return Task::none();
            }
        };
        let (pane_id, save) = (d.pane, d.save);
        let sent = self.pane(pane_id).is_some_and(|p| {
            p.state.is_running()
                && p.control(SessionInput::Forward(ForwardCmd::Add(forward.clone())))
        });
        let Some(d) = self.forwards.as_mut() else {
            return Task::none();
        };
        if !sent {
            d.error = Some("the session is not running".to_owned());
            return Task::none();
        }
        d.error = None;
        d.port.clear();
        d.target_port.clear();
        d.draft = PortForward {
            kind: d.draft.kind,
            ..PortForward::default()
        };
        if save {
            return self.save_forward(pane_id, forward);
        }
        Task::none()
    }

    /// Appends `forward` to the pane's stored profile (`forwarded_ports`, in place).
    fn save_forward(&mut self, pane: PaneId, forward: PortForward) -> Task<Message> {
        let Some(id) = self.pane(pane).map(|p| p.profile.id.clone()) else {
            return Task::none();
        };
        let Some(old) = self.config.profiles.iter().find(|p| p.id == id).cloned() else {
            return Task::none();
        };
        let mut new = old.clone();
        if let ProfileKind::Ssh(ssh) = &mut new.kind {
            let inherited = &self.config.profile_defaults.ssh.forwarded_ports;
            let list = ssh
                .forwarded_ports
                .get_or_insert_with(|| inherited.clone().unwrap_or_default());
            list.push(forward);
        }
        let name = new.name.clone();
        let edit = ProfileEdit::Update {
            old: Box::new(old),
            new: Box::new(new.clone()),
        };
        if let Err(task) = self.write_edits(&[edit]) {
            return task;
        }
        // Panes of this profile restart/reconnect with the saved list (like the editor's Save).
        for pane in self.tabs.iter_mut().flat_map(|t| t.panes.values_mut()) {
            if pane.profile.id == id {
                pane.profile = new.clone();
            }
        }
        self.notify(Notification::new(Level::Info, "Forward saved to the profile").detail(name))
    }
}

fn edit(d: &mut ForwardsDialog, field: FwdField, value: FieldValue) {
    d.error = None;
    match (field, value) {
        (FwdField::Kind, FieldValue::Choice(i)) => {
            d.draft.kind = KINDS.get(i).map_or(ForwardKind::Local, |k| k.1);
        }
        (FwdField::Host, FieldValue::Text(t)) => d.draft.host = t,
        (FwdField::Port, FieldValue::Text(t)) => d.port = t,
        (FwdField::Target, FieldValue::Text(t)) => d.draft.target_address = t,
        (FwdField::TargetPort, FieldValue::Text(t)) => d.target_port = t,
        (FwdField::Description, FieldValue::Text(t)) => d.draft.description = t,
        (FwdField::Save, FieldValue::Bool(b)) => d.save = b,
        _ => {}
    }
}

/// The dialog for `pane`, if it is open and the pane still exists.
pub fn view<'a>(
    d: &'a ForwardsDialog,
    pane: &'a Pane,
    stored: bool,
    look: Look,
) -> Element<'a, Message> {
    let host = match &pane.profile.kind {
        ProfileKind::Ssh(ssh) => ssh
            .host
            .clone()
            .unwrap_or_else(|| pane.profile.name.clone()),
        ProfileKind::Local(_) => pane.profile.name.clone(),
    };
    let mut list = Column::new().spacing(8);
    if pane.forwards.is_empty() {
        list = list.push(text("No forwards yet.").size(13).color(look.dim));
    }
    for status in &pane.forwards {
        list = list.push(forward_row(status, &host, look));
    }
    let listening = pane
        .forwards
        .iter()
        .filter(|f| f.state == ForwardState::Listening)
        .count();
    let close = button(icon(Icon::Close, 14.0, look.dim))
        .width(28)
        .height(28)
        .padding(0)
        .style(style::icon_btn(look, false))
        .on_press(Message::Forwards(ForwardsMsg::Close));
    let title = row![
        column![
            text("Port forwarding")
                .size(19)
                .font(crate::UI_MEDIUM)
                .color(look.fg),
            text(format!(
                "{host} · {listening} listening · live, no reconnect"
            ))
            .size(12)
            .color(look.dim),
        ]
        .spacing(3)
        .width(Length::Fill),
        close,
    ]
    .align_y(Alignment::Start);
    let content = column![
        title,
        scrollable(list)
            .height(Length::Shrink)
            .style(crate::style::scrollbar),
        text("ADD A FORWARD").size(10.5).color(look.dim),
        form(d, stored, look),
        row![
            iced::widget::space().width(Length::Fill),
            button(text("Close").size(13))
                .padding([6, 14])
                .style(style::btn(look, Btn::Secondary))
                .on_press(Message::Forwards(ForwardsMsg::Close)),
            button(text("Add").size(13))
                .padding([6, 14])
                .style(style::btn(look, Btn::Primary))
                .on_press(Message::Forwards(ForwardsMsg::Add)),
        ]
        .spacing(8),
    ]
    .spacing(16);
    container(content)
        .padding([20, 22])
        .max_width(720)
        .style(move |_theme: &iced::Theme| style::floating(look, t::R_DIALOG))
        .into()
}

/// A small chip: mono text on `bg`, radius 6.
fn chip<'a>(label: String, fg: iced::Color, bg: iced::Color) -> Element<'a, Message> {
    container(text(label).size(11.5).font(iced::Font::MONOSPACE).color(fg))
        .padding([3, 8])
        .style(move |_theme: &iced::Theme| style::tag(bg))
        .into()
}

/// The route's flow line: accent while listening, faint otherwise.
fn flow<'a>(on: bool) -> Element<'a, Message> {
    let color = if on { t::A500 } else { alpha(t::A500, 0.3) };
    container(iced::widget::space())
        .width(Length::Fill)
        .height(2)
        .style(move |_theme: &iced::Theme| container::background(color))
        .into()
}

/// One route: type tag · listen · flow · host · flow · destination · connections · remove.
fn forward_row<'a>(status: &'a ForwardStatus, host: &str, look: Look) -> Element<'a, Message> {
    let f = &status.forward;
    let on = status.state == ForwardState::Listening;
    let (kind, tag_text, tag_fill) = match f.kind {
        ForwardKind::Local => ("Local", t::A100, t::A800),
        ForwardKind::Remote => ("Remote", t::N100, t::N800),
        ForwardKind::Dynamic => ("Dynamic", look.accent_hi, iced::Color::TRANSPARENT),
    };
    let tag = container(text(kind).size(11).color(tag_text))
        .padding([2, 7])
        .style(move |_theme: &iced::Theme| {
            let mut s = style::tag(tag_fill);
            if tag_fill == iced::Color::TRANSPARENT {
                s.border = s.border.width(1).color(t::A600);
            }
            s
        });
    let clip = |s: String| sanitize_remote(&s, SHOWN_MAX_CHARS, false);
    let listen = chip(
        clip(format!("{}:{}", f.host, f.port)),
        look.fg,
        look.surface,
    );
    let host_pill = container(text(host.to_owned()).size(11.5).color(t::A200))
        .padding([3, 9])
        .style(move |_theme: &iced::Theme| container::Style {
            background: Some(t::A900.into()),
            border: iced::Border::default().rounded(999),
            shadow: if on {
                style::glow(t::ACCENT, 0.45, 12.0)
            } else {
                iced::Shadow::default()
            },
            ..container::Style::default()
        });
    let mut route = row![tag, listen, flow(on), host_pill]
        .spacing(8)
        .align_y(Alignment::Center);
    if f.kind != ForwardKind::Dynamic {
        let target = chip(
            clip(format!("{}:{}", f.target_address, f.target_port)),
            look.fg,
            look.surface,
        );
        route = route.push(flow(on)).push(target);
    }
    let n = status.active_connections;
    let count = match &status.state {
        ForwardState::Listening => format!("{n} conn{}", if n == 1 { "" } else { "s" }),
        ForwardState::Error(_) => "error".to_owned(),
        ForwardState::Stopped => "stopped".to_owned(),
    };
    let remove = button(icon(Icon::Close, 13.0, look.dim))
        .width(24)
        .height(24)
        .padding(0)
        .style(style::icon_btn(look, false))
        .on_press(Message::Forwards(ForwardsMsg::Remove(status.id)));
    let route = route
        .push(text(count).size(11.5).color(look.dim))
        .push(remove);
    let mut body = column![route].spacing(6);
    if !f.description.is_empty() {
        body = body.push(text(clip(f.description.clone())).size(12).color(look.dim));
    }
    if let ForwardState::Error(why) = &status.state {
        body = body.push(text(clip(why.clone())).size(12).color(ERROR));
    }
    if f.is_exposed() {
        body = body.push(
            text("Reachable from other machines (not a loopback address)")
                .size(12)
                .color(WARN),
        );
    }
    // Stopped routes leave the list (`apply_status`), so there is none to dim.
    container(body)
        .padding([12, 14])
        .width(Length::Fill)
        .style(move |_theme: &iced::Theme| style::card(look, t::R_PANE))
        .into()
}

fn form<'a>(d: &'a ForwardsDialog, stored: bool, look: Look) -> Element<'a, Message> {
    let kind = KINDS.iter().position(|k| k.1 == d.draft.kind).unwrap_or(0);
    let labels = KINDS.iter().map(|k| k.0.to_owned()).collect();
    let listen = if d.draft.kind == ForwardKind::Remote {
        "Listen address (server)"
    } else {
        "Listen address"
    };
    let mut rows: Vec<Element<'a, Field<FwdField>>> = vec![
        forms::choice(look, FwdField::Kind, "Type", labels, kind),
        forms::text_field(look, FwdField::Host, listen, &d.draft.host, "127.0.0.1"),
        forms::number(look, FwdField::Port, "Listen port", &d.port, "8080"),
    ];
    if d.draft.kind != ForwardKind::Dynamic {
        let (target, hint) = if d.draft.kind == ForwardKind::Local {
            ("Target (from the server)", "127.0.0.1")
        } else {
            ("Target (from this computer)", "127.0.0.1")
        };
        rows.push(forms::text_field(
            look,
            FwdField::Target,
            target,
            &d.draft.target_address,
            hint,
        ));
        rows.push(forms::number(
            look,
            FwdField::TargetPort,
            "Target port",
            &d.target_port,
            "80",
        ));
    }
    rows.push(forms::text_field(
        look,
        FwdField::Description,
        "Description",
        &d.draft.description,
        "",
    ));
    let save = forms::toggle(look, FwdField::Save, "Save to profile", d.save && stored);
    let save_note = (!stored).then_some((
        "Only stored profiles can be changed (save this connection as a profile first).",
        look.dim,
    ));
    rows.push(forms::note_maybe(save, save_note));
    let exposed = d.draft.is_exposed().then_some((
        "Not a loopback address: other machines will be able to connect.",
        WARN,
    ));
    let error = d.error.as_deref().map(|e| (e, ERROR));
    let mut fields = Column::with_children(rows).spacing(6);
    for (line, color) in exposed.into_iter().chain(error) {
        fields = fields.push(text(line.to_owned()).size(12).color(color));
    }
    Element::from(fields).map(|f| Message::Forwards(ForwardsMsg::Field(f)))
}

#[cfg(test)]
#[path = "forwards_dialog_tests.rs"]
mod tests;
