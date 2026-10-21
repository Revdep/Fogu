//! The list-like parts of the SSH form: the port forwards and login scripts tables and the
//! algorithm multi-selects (from russh's supported lists).

use fogu_core::{Algorithms, SshOpts};
use iced::widget::{Column, checkbox, row, text, text_input};
use iced::{Alignment, Length};

use super::ProfileEditor;
use super::fields::{AlgKind, FORWARD_KIND, FieldId, ForwardCol, ScriptCol};
use super::view::{Row, index_of, labels, shown, with_notes};
use crate::forms::{self, ERROR, Field, FieldValue, WARN};
use crate::sftp::panel::Look;

/// A plain line of text in the form (warnings, hints).
pub(super) fn line<'a>(color: iced::Color, words: &str) -> Row<'a> {
    text(words.to_owned()).size(12).color(color).into()
}

fn cell<'a>(id: FieldId, value: &str, placeholder: &str, width: Length) -> Row<'a> {
    text_input(placeholder, value)
        .on_input(move |s| Field(id, FieldValue::Text(s)))
        .size(13)
        .padding(4)
        .width(width)
        .into()
}

pub(super) fn forwards<'a>(ed: &'a ProfileEditor, ssh: &'a SshOpts, look: Look) -> Row<'a> {
    let mut rows = Column::new().spacing(4);
    for (i, f) in ssh.forwarded_ports.iter().flatten().enumerate() {
        let col = move |c| FieldId::Forward(i, c);
        let kind = iced::widget::pick_list(
            labels(&FORWARD_KIND),
            FORWARD_KIND
                .get(index_of(&FORWARD_KIND, &f.kind))
                .map(|(label, _)| (*label).to_owned()),
            move |picked: String| {
                let at = FORWARD_KIND
                    .iter()
                    .position(|(l, _)| *l == picked)
                    .unwrap_or(0);
                Field(col(ForwardCol::Kind), FieldValue::Choice(at))
            },
        )
        .text_size(13)
        .padding(4);
        let port = shown(ed, col(ForwardCol::Port), Some(f.port));
        let target_port = shown(ed, col(ForwardCol::TargetPort), Some(f.target_port));
        let cells = row![
            kind,
            cell(
                col(ForwardCol::Host),
                &f.host,
                "listen address",
                Length::FillPortion(3)
            ),
            cell(col(ForwardCol::Port), &port, "port", Length::FillPortion(2)),
            text("→").color(look.dim),
            cell(
                col(ForwardCol::Target),
                &f.target_address,
                "target",
                Length::FillPortion(3)
            ),
            cell(
                col(ForwardCol::TargetPort),
                &target_port,
                "port",
                Length::FillPortion(2)
            ),
            cell(
                col(ForwardCol::Description),
                &f.description,
                "description",
                Length::FillPortion(3)
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        let entry: Row<'a> = forms::list_row(look, FieldId::Forwards, i, cells.into());
        // Same widget tree with or without an error, so the port field keeps the keyboard.
        let error = [ForwardCol::Port, ForwardCol::TargetPort]
            .into_iter()
            .find_map(|c| ed.errors.get(&col(c)));
        let mut line = iced::widget::container(
            text(error.cloned().unwrap_or_default())
                .size(11)
                .color(ERROR),
        );
        if error.is_none() {
            line = line.height(0);
        }
        rows = rows.push(iced::widget::column![entry, line]);
    }
    let add = iced::widget::button(text("+ Add").size(13))
        .padding([2, 8])
        .style(iced::widget::button::secondary)
        .on_press(Field(FieldId::Forwards, FieldValue::Add));
    let table = row![
        text("Port forwards")
            .size(13)
            .color(look.fg)
            .width(forms::LABEL_W),
        rows.push(add)
    ]
    .spacing(8);
    with_notes(ed, FieldId::Forwards, "forwarded_ports", table.into())
}

pub(super) fn algorithms<'a>(ed: &'a ProfileEditor, ssh: &'a SshOpts, look: Look) -> Row<'a> {
    let chosen = ssh.algorithms.clone().unwrap_or_default();
    let [kex, cipher, hmac, host_key, compression] = supported();
    let kinds = [
        (AlgKind::Kex, "Key exchange", &chosen.kex, kex),
        (AlgKind::Cipher, "Ciphers", &chosen.cipher, cipher),
        (AlgKind::Hmac, "MACs", &chosen.hmac, hmac),
        (
            AlgKind::HostKey,
            "Host key types",
            &chosen.server_host_key,
            host_key,
        ),
        (
            AlgKind::Compression,
            "Compression",
            &chosen.compression,
            compression,
        ),
    ];
    let mut rows = vec![with_notes(
        ed,
        FieldId::Alg(AlgKind::Kex),
        "algorithms",
        line(
            look.dim,
            "Nothing ticked in a list = the built-in defaults (strict key exchange always on).",
        ),
    )];
    let weak = weak(&chosen);
    if !weak.is_empty() {
        let names: Vec<String> = weak.iter().map(|n| fogu_term::sanitize_title(n)).collect();
        rows.push(line(
            WARN,
            &format!("Weaker than the defaults: {}", names.join(", ")),
        ));
    }
    for (kind, label, chosen, supported) in kinds {
        let mut options: Vec<(String, String, bool)> = supported
            .iter()
            .map(|n| (n.clone(), n.clone(), chosen.contains(n)))
            .collect();
        // Names from config.toml this build doesn't know stay visible (and can be unticked).
        options.extend(chosen.iter().filter(|n| !supported.contains(n)).map(|n| {
            (
                n.clone(),
                format!("unknown: {}", fogu_term::sanitize_title(n)),
                true,
            )
        }));
        rows.push(forms::multi(look, FieldId::Alg(kind), label, &options));
    }
    forms::section(look, "Algorithms", rows)
}

/// russh's names per category (kex, cipher, mac, host key, compression), read once.
type Lists = [Vec<String>; 5];

#[cfg(feature = "ssh")]
fn supported() -> &'static Lists {
    static LISTS: std::sync::LazyLock<Lists> =
        std::sync::LazyLock::new(|| fogu_session::supported_algorithms().lists());
    &LISTS
}

#[cfg(not(feature = "ssh"))]
fn supported() -> &'static Lists {
    static EMPTY: Lists = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    &EMPTY
}

/// Chosen names outside russh's defaults.
#[cfg(feature = "ssh")]
fn weak(chosen: &Algorithms) -> Vec<String> {
    fogu_session::non_default_algorithms(chosen)
}

#[cfg(not(feature = "ssh"))]
fn weak(_: &Algorithms) -> Vec<String> {
    Vec::new()
}

/// Names in `algorithms` this build doesn't know (sanitized); none without SSH support.
pub(super) fn unknown(algorithms: &Algorithms) -> Vec<String> {
    if cfg!(not(feature = "ssh")) {
        return Vec::new();
    }
    algorithms
        .unknown(supported())
        .iter()
        .map(|n| fogu_term::sanitize_title(n))
        .collect()
}

pub(super) fn scripts<'a>(ed: &'a ProfileEditor, ssh: &'a SshOpts, look: Look) -> Row<'a> {
    let mut rows = Column::new().spacing(4);
    for (i, s) in ssh.scripts.iter().flatten().enumerate() {
        let col = move |c| FieldId::Script(i, c);
        let flag = |c, label: &str, on: bool| {
            row![
                checkbox(on)
                    .on_toggle(move |b| Field(col(c), FieldValue::Bool(b)))
                    .size(14),
                text(label.to_owned()).size(12).color(look.fg)
            ]
            .spacing(4)
            .align_y(Alignment::Center)
        };
        let cells = row![
            cell(
                col(ScriptCol::Expect),
                &s.expect,
                "expect (output to wait for)",
                Length::FillPortion(3)
            ),
            cell(
                col(ScriptCol::Send),
                &s.send,
                "send (\\n = Enter)",
                Length::FillPortion(3)
            ),
            flag(ScriptCol::IsRegex, "regex", s.is_regex),
            flag(ScriptCol::Optional, "optional", s.optional),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        rows = rows.push(forms::list_row(look, FieldId::Scripts, i, cells.into()));
    }
    let add = iced::widget::button(text("+ Add").size(13))
        .padding([2, 8])
        .style(iced::widget::button::secondary)
        .on_press(Field(FieldId::Scripts, FieldValue::Add));
    let table = row![
        text("When the output shows…, send…")
            .size(13)
            .color(look.fg)
            .width(forms::LABEL_W),
        rows.push(add)
    ]
    .spacing(8);
    let table = with_notes(ed, FieldId::Scripts, "scripts", table.into());
    let hint = line(
        WARN,
        "Sent text is stored in config.toml as written: don't put passwords here.",
    );
    forms::section(look, "Login scripts", vec![table, hint])
}
