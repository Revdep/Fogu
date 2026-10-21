//! Generic form helpers: one labelled row per field, built once and reused by the profile editor
//! and, later, the settings pages. Every widget publishes a [`Field`]: the caller's field id plus a
//! [`FieldValue`]; the caller's reducer maps the id to the struct field it edits (one `match`).
//!
//! Stock iced widgets in the Nocturne styles (`style`): outlined fields, switches, segmented
//! controls for short choices, round colour swatches.

use std::path::PathBuf;

use fogu_core::RgbColor;
use iced::widget::{
    Column, Row, button, checkbox, column, container, pick_list, row, space, text, text_input,
    tooltip,
};
use iced::{Alignment, Color, Element, Length};

use crate::color_picker::{color_picker, preview};
use crate::context_menu::{PRESET_COLORS, to_iced};
use crate::sftp::panel::Look;
use crate::style::{self, Btn};
use crate::tokens::{self as t, alpha};

/// Width of the label column, logical px.
pub const LABEL_W: f32 = 180.0;
/// Width of a number field.
const NUMBER_W: f32 = 140.0;
/// Number inputs and their square stepper buttons share this visual height.
const NUMBER_H: f32 = 28.0;
const LABEL_SIZE: f32 = 13.0;
const INPUT_SIZE: f32 = 13.0;
const NOTE_SIZE: f32 = 11.0;
/// "Not supported yet" notes and warnings (amber, like the tab warning badge).
pub const WARN: Color = Color::from_rgb(0.96, 0.65, 0.14);
/// Validation errors.
pub const ERROR: Color = t::DANGER;
/// A choice with at most this many options, all short, is a segmented control.
const SEGMENTS: usize = 5;

/// What a widget reports; the caller's reducer interprets it for the field.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    /// A text or number field changed (numbers are parsed by the reducer).
    Text(String),
    /// A checkbox.
    Bool(bool),
    /// A slider moved.
    Number(f32),
    /// A number field's stepper moved by this many field-specific units.
    Step(i8),
    /// The index of the option picked in a [`choice`].
    Choice(usize),
    /// A colour swatch or "None".
    Color(Option<RgbColor>),
    /// "Browse…": open a file picker for this field.
    Browse,
    /// The picker returned this file.
    Picked(PathBuf),
    /// "+ Add" on a list.
    Add,
    /// "×" on list row `.0`.
    Remove(usize),
    /// "↑" on list row `.0` (moves it one up).
    Up(usize),
    /// List row `.0`'s text changed (a key-value list: its key).
    Item(usize, String),
    /// Key-value row `.0`'s value changed.
    Value(usize, String),
    /// A multi-select entry was ticked or unticked.
    Toggle(String),
    /// "Set…" on a secret.
    SetSecret,
    /// "Clear" on a secret.
    ClearSecret,
    /// Right-click on a text field at `.0` (window coordinates); `.1` is the text it shows.
    Menu(iced::Point, String),
    /// The colour card's "+": show or hide the full picker.
    Expand,
}

/// One widget event: which field, what happened.
#[derive(Clone, Debug, PartialEq)]
pub struct Field<Id>(pub Id, pub FieldValue);

/// `label` on the left, `control` on the right.
pub(crate) fn labeled<'a, M: 'a>(
    look: Look,
    label: &str,
    control: Element<'a, M>,
) -> Element<'a, M> {
    // Top-aligned, nudged to a one-line control's text: tall controls keep the label at the top.
    row![
        container(
            text(label.to_owned())
                .size(LABEL_SIZE)
                .color(crate::palette::mix(look.fg, look.dim, 0.35))
        )
        .padding(iced::Padding::ZERO.top(6))
        .width(LABEL_W),
        control,
    ]
    .spacing(12)
    .align_y(Alignment::Start)
    .into()
}

/// [`labeled`] with a small muted `hint` under the label.
pub(crate) fn labeled_hint<'a, M: 'a>(
    look: Look,
    label: &str,
    hint: &str,
    control: Element<'a, M>,
) -> Element<'a, M> {
    row![
        column![
            text(label.to_owned())
                .size(LABEL_SIZE)
                .color(crate::palette::mix(look.fg, look.dim, 0.35)),
            text(hint.to_owned()).size(11).color(look.dim),
        ]
        .spacing(2)
        .padding(iced::Padding::ZERO.top(4))
        .width(LABEL_W),
        control,
    ]
    .spacing(12)
    .align_y(Alignment::Start)
    .into()
}

pub(crate) fn input<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    value: &str,
    placeholder: &str,
) -> iced::widget::TextInput<'a, Field<Id>> {
    text_input(placeholder, value)
        .on_input(move |s| Field(id, FieldValue::Text(s)))
        .size(INPUT_SIZE)
        .padding([6, 10])
        .style(style::input(look))
}

/// A switch: a 28 × 16 track with a 12 px knob, right and lit when `on`.
pub(crate) fn switch<'a, M: Clone + 'a>(look: Look, on: bool, toggle: M) -> Element<'a, M> {
    let knob_color = if on { t::A200 } else { look.dim };
    let knob = container(space())
        .width(12)
        .height(12)
        .style(move |_theme: &iced::Theme| {
            container::background(knob_color).border(iced::Border::default().rounded(6))
        });
    let track = container(knob)
        .width(28)
        .height(16)
        .padding(2)
        .align_x(if on { Alignment::End } else { Alignment::Start })
        .align_y(Alignment::Center);
    let edge = look.border;
    button(track)
        .padding(0)
        .style(move |_theme: &iced::Theme, status| {
            let fill = if on {
                t::A700
            } else if matches!(status, button::Status::Hovered) {
                alpha(look.fg, 0.16)
            } else {
                alpha(look.fg, 0.10)
            };
            button::Style {
                background: Some(fill.into()),
                border: iced::Border::default()
                    .rounded(8)
                    .width(if on { 0.0 } else { 1.0 })
                    .color(edge),
                ..button::Style::default()
            }
        })
        .on_press(toggle)
        .into()
}

/// A segmented control: one option per segment, the chosen one ringed in the accent.
pub(crate) fn segmented<'a, M: Clone + 'a>(
    look: Look,
    options: &[String],
    selected: usize,
    pick: impl Fn(usize) -> M,
) -> Element<'a, M> {
    let mut segments = Row::new().spacing(2);
    for (i, name) in options.iter().enumerate() {
        let on = i == selected;
        segments = segments.push(
            button(text(name.clone()).size(12.5))
                .padding([5, 12])
                .style(move |_theme: &iced::Theme, status| {
                    let hover = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: (on || hover)
                            .then(|| alpha(if on { look.accent } else { look.fg }, 0.10).into()),
                        text_color: if on { look.accent_hi } else { look.fg },
                        border: iced::Border::default()
                            .rounded(6)
                            .width(if on { 1.0 } else { 0.0 })
                            .color(look.accent),
                        ..button::Style::default()
                    }
                })
                .on_press(pick(i)),
        );
    }
    let edge = look.border;
    container(segments)
        .padding(2)
        .style(move |_theme: &iced::Theme| {
            container::Style::default().border(
                iced::Border::default()
                    .rounded(t::R_MD)
                    .width(1)
                    .color(edge),
            )
        })
        .into()
}

/// `field` (showing `value`) with a right-click menu: reports [`FieldValue::Menu`].
fn with_menu<'a, Id: Copy + 'a>(
    id: Id,
    value: &str,
    field: impl Into<Element<'a, Field<Id>>>,
) -> Element<'a, Field<Id>> {
    let value = value.to_owned();
    crate::right_click::right_click(field, move |at| {
        Field(id, FieldValue::Menu(at, value.clone()))
    })
    .into()
}

fn small_button<'a, M: Clone + 'a>(look: Look, label: &str, on_press: Option<M>) -> Element<'a, M> {
    button(text(label.to_owned()).size(12.5))
        .padding([4, 10])
        .style(style::btn(look, Btn::Secondary))
        .on_press_maybe(on_press)
        .into()
}

/// A one-line text field; `placeholder` shows the inherited value.
pub fn text_field<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    value: &str,
    placeholder: &str,
) -> Element<'a, Field<Id>> {
    labeled(
        look,
        label,
        with_menu(id, value, input(look, id, value, placeholder)),
    )
}

/// [`text_field`] in the monospace font (hosts, paths, TERM).
pub fn mono_field<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    value: &str,
    placeholder: &str,
) -> Element<'a, Field<Id>> {
    let field = input(look, id, value, placeholder).font(iced::Font::MONOSPACE);
    labeled(look, label, with_menu(id, value, field))
}

/// A number field (the reducer parses it and checks the range).
pub fn number<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    value: &str,
    placeholder: &str,
) -> Element<'a, Field<Id>> {
    let step = |label: &'static str, by| {
        button(text(label).size(14))
            .width(NUMBER_H)
            .height(NUMBER_H)
            .padding(0)
            .style(style::btn(look, Btn::Secondary))
            .on_press(Field(id, FieldValue::Step(by)))
    };
    let input = with_menu(
        id,
        value,
        input(look, id, value, placeholder)
            .font(iced::Font::MONOSPACE)
            .width(NUMBER_W),
    );
    labeled(
        look,
        label,
        row![step("−", -1), input, step("+", 1)]
            .spacing(4)
            .align_y(Alignment::Center)
            .into(),
    )
}

/// A checkbox.
pub fn toggle<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    value: bool,
) -> Element<'a, Field<Id>> {
    labeled(
        look,
        label,
        switch(look, value, Field(id, FieldValue::Bool(!value))),
    )
}

/// A switch with a muted line of what it does next to it.
pub fn toggle_desc<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    desc: &str,
    value: bool,
) -> Element<'a, Field<Id>> {
    let control = row![
        switch(look, value, Field(id, FieldValue::Bool(!value))),
        text(desc.to_owned()).size(12).color(look.dim),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    labeled(look, label, control.into())
}

/// Pills, one per option (the chosen one in the accent); clicking the chosen one again
/// publishes `again`. `extra` is a ghost pill after them (e.g. "+ New group").
pub(crate) fn pills<'a, M: Clone + 'a>(
    look: Look,
    options: &[String],
    selected: Option<usize>,
    pick: impl Fn(usize) -> M,
    again: Option<&M>,
    extra: Option<(&str, M)>,
) -> Element<'a, M> {
    let mut row = Row::new().spacing(6).align_y(Alignment::Center);
    for (i, name) in options.iter().enumerate() {
        let on = selected == Some(i);
        let press = if on { again.cloned() } else { Some(pick(i)) };
        row = row.push(
            button(text(name.to_uppercase()).size(11.5))
                .padding([4, 11])
                .style(move |_theme: &iced::Theme, status| {
                    let hover = matches!(status, button::Status::Hovered);
                    let fill = if on {
                        t::A800
                    } else if hover {
                        alpha(look.fg, 0.12)
                    } else {
                        alpha(look.fg, 0.07)
                    };
                    button::Style {
                        background: Some(fill.into()),
                        text_color: if on { t::A200 } else { look.fg },
                        border: iced::Border::default()
                            .rounded(999)
                            .width(if on { 1.0 } else { 0.0 })
                            .color(t::A500),
                        ..button::Style::default()
                    }
                })
                .on_press_maybe(press),
        );
    }
    if let Some((label, message)) = extra {
        // iced has no dashed border: a faint ring stands in for it.
        row = row.push(
            button(text(format!("+ {label}")).size(11.5))
                .padding([4, 11])
                .style(move |_theme: &iced::Theme, status| button::Style {
                    background: matches!(status, button::Status::Hovered)
                        .then(|| alpha(look.fg, 0.07).into()),
                    text_color: look.dim,
                    border: iced::Border::default()
                        .rounded(999)
                        .width(1)
                        .color(alpha(look.fg, 0.28)),
                    ..button::Style::default()
                })
                .on_press(message),
        );
    }
    row.wrap().vertical_spacing(6).into()
}

/// Radio buttons, one per line; `selected` is an index into `options`.
pub fn radios<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    options: &[String],
    selected: usize,
) -> Element<'a, Field<Id>> {
    let mut list = Column::new().spacing(6);
    for (i, name) in options.iter().enumerate() {
        let on = i == selected;
        let ring = if on { look.accent } else { look.border };
        let inner = container(space().width(6).height(6)).style(move |_theme: &iced::Theme| {
            container::Style {
                background: on.then(|| look.accent.into()),
                border: iced::Border::default().rounded(3),
                ..container::Style::default()
            }
        });
        let dot = container(inner)
            .center(16)
            .style(move |_theme: &iced::Theme| {
                container::Style::default()
                    .border(iced::Border::default().rounded(8).width(1.5).color(ring))
            });
        list = list.push(
            button(
                row![dot, text(name.clone()).size(13).color(look.fg)]
                    .spacing(10)
                    .align_y(Alignment::Center),
            )
            .padding([2, 0])
            .style(|_theme: &iced::Theme, _status| button::Style::default())
            .on_press(Field(id, FieldValue::Choice(i))),
        );
    }
    labeled(look, label, list.into())
}

/// A drop-down of `options`; `selected` is an index into them.
pub fn choice<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    options: Vec<String>,
    selected: usize,
) -> Element<'a, Field<Id>> {
    let short =
        options.len() <= SEGMENTS && options.iter().map(|o| o.chars().count()).sum::<usize>() <= 56;
    if short {
        let control = segmented(look, &options, selected, move |i| {
            Field(id, FieldValue::Choice(i))
        });
        return labeled(look, label, control);
    }
    let current = options.get(selected).cloned();
    let lookup = options.clone();
    let list = pick_list(options, current, move |picked: String| {
        let index = lookup.iter().position(|o| *o == picked).unwrap_or(0);
        Field(id, FieldValue::Choice(index))
    })
    .text_size(INPUT_SIZE)
    .padding([6, 10])
    .style(move |_theme: &iced::Theme, status| pick_list::Style {
        text_color: look.fg,
        placeholder_color: look.dim,
        handle_color: look.dim,
        background: look.surface.into(),
        border: iced::Border::default().rounded(t::R_MD).width(1).color(
            if matches!(status, pick_list::Status::Opened { .. }) {
                look.accent
            } else {
                look.border
            },
        ),
    })
    .menu_style(move |_theme: &iced::Theme| iced::overlay::menu::Style {
        background: look.surface.into(),
        border: iced::Border::default()
            .rounded(t::R_MD)
            .width(1)
            .color(look.border),
        text_color: look.fg,
        selected_text_color: look.fg,
        selected_background: alpha(look.accent, 0.16).into(),
        shadow: t::shadow_lg(),
    });
    labeled(look, label, list.into())
}

/// A slider over `range`, with `shown` (the value as text) after it.
pub fn slider<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    shown: String,
) -> Element<'a, Field<Id>> {
    let slider = iced::widget::slider(range, value, move |v| Field(id, FieldValue::Number(v)))
        .step(0.01_f32)
        .width(260);
    let row = row![slider, text(shown).size(LABEL_SIZE).color(look.dim)]
        .spacing(10)
        .align_y(Alignment::Center);
    labeled(look, label, row.into())
}

/// A colour card: the current colour, the preset swatches, "None", "+" (the full picker,
/// shown when `expanded`) and a `#rrggbb` field (`hex`).
#[expect(clippy::too_many_lines, reason = "one widget tree, left to right")]
pub fn color<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    (label, hint): (&str, &str),
    value: Option<RgbColor>,
    hex: &str,
    expanded: bool,
) -> Element<'a, Field<Id>> {
    let mut swatches = Row::new().spacing(6).align_y(Alignment::Center);
    let gap = crate::palette::mix(look.bg, look.surface, 0.5);
    for (name, rgb) in PRESET_COLORS {
        let rgb = RgbColor::from_hex(rgb);
        let fill = to_iced(rgb);
        let chosen = value == Some(rgb);
        // Chosen: a 2 px gap in the card colour, then a ring in the swatch colour.
        let dot = container(space().width(20).height(20)).style(move |_theme: &iced::Theme| {
            container::background(fill).border(
                iced::Border::default()
                    .rounded(10)
                    .width(if chosen { 2.0 } else { 0.0 })
                    .color(gap),
            )
        });
        let swatch = button(dot)
            .padding(1.5)
            .style(move |_theme: &iced::Theme, _status| button::Style {
                border: iced::Border::default()
                    .rounded(12)
                    .width(if chosen { 1.5 } else { 0.0 })
                    .color(fill),
                ..button::Style::default()
            })
            .on_press(Field(id, FieldValue::Color(Some(rgb))));
        swatches = swatches.push(tooltip(
            swatch,
            container(text(name).size(NOTE_SIZE)).padding(4),
            tooltip::Position::Top,
        ));
    }
    let none = button(text("None").size(11.5))
        .padding([2, 9])
        .style(move |theme: &iced::Theme, status| {
            let mut style = style::btn(look, Btn::Secondary)(theme, status);
            style.border.radius = 999.0.into();
            if value.is_none() {
                style.border.color = look.accent;
                style.text_color = look.accent_hi;
            }
            style
        })
        .on_press(Field(id, FieldValue::Color(None)));
    // "+": a coloured ring around a plus, opens the full picker.
    let ring = if expanded { look.accent_hi } else { t::A600 };
    let more = button(
        container(crate::icons::glyph(crate::icons::Icon::Plus, 11.0, look.fg))
            .center(20)
            .style(move |_theme: &iced::Theme| {
                container::Style::default()
                    .border(iced::Border::default().rounded(10).width(2).color(ring))
            }),
    )
    .padding(1.5)
    .style(|_theme: &iced::Theme, _status| button::Style::default())
    .on_press(Field(id, FieldValue::Expand));
    let hex_field = with_menu(
        id,
        hex,
        input(look, id, hex, "rrggbb")
            .font(iced::Font::MONOSPACE)
            .icon(text_input::Icon {
                font: iced::Font::MONOSPACE,
                code_point: '#',
                size: Some(12.into()),
                spacing: 4.0,
                side: text_input::Side::Left,
            })
            .width(104),
    );
    let current =
        container(preview(value, 38.0, look.border)).style(move |_theme: &iced::Theme| {
            container::Style {
                shadow: value.map_or(iced::Shadow::default(), |c| {
                    style::glow(to_iced(c), 0.45, 20.0)
                }),
                ..container::Style::default()
            }
        });
    let mut body = column![
        row![
            current,
            // Wraps when the card is narrow; the hex field stays at the right end.
            swatches
                .push(none)
                .push(more)
                .spacing(8)
                .width(Length::Fill)
                .wrap()
                .vertical_spacing(6),
            hex_field
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    ]
    .spacing(12);
    if expanded {
        let picker = color_picker(value, 260.0, move |c| Field(id, FieldValue::Color(Some(c))));
        body = body.push(Element::from(picker));
    }
    let card = container(body)
        .padding(12)
        .style(move |_theme: &iced::Theme| style::card(look, t::R_PANE));
    labeled_hint(look, label, hint, card.into())
}

/// A file path with a "Browse…" button (disabled with `why` when no picker is available).
pub fn path<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    value: &str,
    placeholder: &str,
    picker: Result<(), &'static str>,
) -> Element<'a, Field<Id>> {
    let field = with_menu(
        id,
        value,
        input(look, id, value, placeholder).width(Length::Fill),
    );
    let browse = browse_button(look, id, picker);
    labeled(
        look,
        label,
        row![field, browse]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
    )
}

fn browse_button<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    picker: Result<(), &'static str>,
) -> Element<'a, Field<Id>> {
    let button = small_button(
        look,
        "Browse…",
        picker.is_ok().then_some(Field(id, FieldValue::Browse)),
    );
    match picker {
        Ok(()) => button,
        Err(why) => tooltip(
            button,
            container(text(why).size(NOTE_SIZE)).padding(4),
            tooltip::Position::Top,
        )
        .into(),
    }
}

/// An ordered list of strings: each row editable, movable up and removable; "+ Add" appends an
/// empty row, `browse` (path lists) adds "Browse…".
pub fn list<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    items: &[String],
    placeholder: &str,
    browse: Option<Result<(), &'static str>>,
) -> Element<'a, Field<Id>> {
    let mut rows = Column::new().spacing(4);
    for (i, item) in items.iter().enumerate() {
        let field = text_input(placeholder, item)
            .on_input(move |s| Field(id, FieldValue::Item(i, s)))
            .size(INPUT_SIZE)
            .padding([6, 10])
            .style(style::input(look));
        rows = rows.push(list_row(look, id, i, field.into()));
    }
    let mut add = row![small_button(
        look,
        "+ Add",
        Some(Field(id, FieldValue::Add))
    )]
    .spacing(6);
    if let Some(picker) = browse {
        add = add.push(browse_button(look, id, picker));
    }
    labeled(look, label, rows.push(add).into())
}

/// `content` followed by "↑" and "×" for list row `i`.
pub fn list_row<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    i: usize,
    content: Element<'a, Field<Id>>,
) -> Element<'a, Field<Id>> {
    let up = small_button(look, "↑", (i > 0).then_some(Field(id, FieldValue::Up(i))));
    let remove = small_button(look, "×", Some(Field(id, FieldValue::Remove(i))));
    row![container(content).width(Length::Fill), up, remove]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
}

/// Key = value rows (environment variables).
pub fn kv_list<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    items: &[(String, String)],
) -> Element<'a, Field<Id>> {
    let mut rows = Column::new().spacing(4);
    for (i, (key, value)) in items.iter().enumerate() {
        let key = text_input("NAME", key)
            .on_input(move |s| Field(id, FieldValue::Item(i, s)))
            .size(INPUT_SIZE)
            .padding([6, 10])
            .font(iced::Font::MONOSPACE)
            .style(style::input(look))
            .width(Length::FillPortion(1));
        let value = text_input("value", value)
            .on_input(move |s| Field(id, FieldValue::Value(i, s)))
            .size(INPUT_SIZE)
            .padding([6, 10])
            .font(iced::Font::MONOSPACE)
            .style(style::input(look))
            .width(Length::FillPortion(2));
        let pair = row![key, text("=").color(look.dim), value]
            .spacing(4)
            .align_y(Alignment::Center);
        rows = rows.push(list_row(look, id, i, pair.into()));
    }
    let add = small_button(look, "+ Add", Some(Field(id, FieldValue::Add)));
    labeled(look, label, rows.push(add).into())
}

/// A multi-select: one checkbox per `(value, label, ticked)`; a click reports
/// [`FieldValue::Toggle`] with the value.
pub fn multi<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    options: &[(String, String, bool)],
) -> Element<'a, Field<Id>> {
    let mut boxes = Column::new().spacing(3);
    for (value, name, ticked) in options {
        let toggled = value.clone();
        let check = checkbox(*ticked)
            .on_toggle(move |_| Field(id, FieldValue::Toggle(toggled.clone())))
            .size(14)
            .style(move |_theme: &iced::Theme, status| {
                let checked = match status {
                    checkbox::Status::Active { is_checked }
                    | checkbox::Status::Hovered { is_checked }
                    | checkbox::Status::Disabled { is_checked } => is_checked,
                };
                checkbox::Style {
                    background: if checked {
                        alpha(look.accent, 0.16)
                    } else {
                        look.surface
                    }
                    .into(),
                    icon_color: look.accent_hi,
                    border: iced::Border::default()
                        .rounded(4)
                        .width(1)
                        .color(if checked { look.accent } else { look.border }),
                    text_color: Some(look.fg),
                }
            });
        boxes = boxes.push(
            row![check, text(name.clone()).size(LABEL_SIZE).color(look.fg)]
                .spacing(6)
                .align_y(Alignment::Center),
        );
    }
    labeled(look, label, boxes.into())
}

/// A secret that is never shown: its `status` ("stored" / "not stored") with Set… / Clear.
/// `disabled` = why the buttons are off (shown as their tooltip).
pub fn secret_hint<'a, Id: Copy + 'a>(
    look: Look,
    id: Id,
    label: &str,
    status: &str,
    disabled: Option<&'static str>,
) -> Element<'a, Field<Id>> {
    let on = |value| disabled.is_none().then_some(Field(id, value));
    let buttons = row![
        small_button(look, "Set…", on(FieldValue::SetSecret)),
        small_button(look, "Clear", on(FieldValue::ClearSecret)),
    ]
    .spacing(6);
    let buttons: Element<'a, Field<Id>> = match disabled {
        Some(why) => tooltip(
            buttons,
            container(text(why).size(NOTE_SIZE)).padding(4),
            tooltip::Position::Top,
        )
        .into(),
        None => buttons.into(),
    };
    let row = row![
        text(status.to_owned()).size(LABEL_SIZE).color(look.dim),
        buttons
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    labeled(look, label, row.into())
}

/// `field` with a small line under it (aligned with the controls), in `color`.
pub fn note<'a, M: 'a>(field: Element<'a, M>, line: &str, color: Color) -> Element<'a, M> {
    note_maybe(field, Some((line, color)))
}

/// [`note`] when `line` is set, else `field` alone. The widget tree is the same either way
/// (the line is just 0 px high), so a text field keeps the keyboard when a note comes or goes
/// while the user types.
pub fn note_maybe<'a, M: 'a>(field: Element<'a, M>, line: Option<(&str, Color)>) -> Element<'a, M> {
    let below: Element<'a, M> = match line {
        Some((words, color)) => row![
            space().width(LABEL_W + 12.0),
            text(words.to_owned())
                .size(NOTE_SIZE)
                .color(color)
                .width(Length::Fill)
        ]
        .into(),
        None => space().height(0).into(),
    };
    column![field, below]
        .spacing(if line.is_some() { 2 } else { 0 })
        .into()
}

/// A page's left-nav entry: icon and label; the current one is tinted, `dirty` adds a 6 px
/// accent dot (unsaved changes).
pub(crate) fn nav_item<'a, M: Clone + 'a>(
    look: Look,
    glyph: crate::icons::Icon,
    label: &str,
    current: bool,
    dirty: bool,
    on_press: M,
) -> Element<'a, M> {
    let tint = if current { look.accent_hi } else { look.dim };
    let mut line = row![
        crate::icons::glyph(glyph, 15.0, tint),
        text(label.to_owned())
            .size(13)
            .color(if current {
                look.fg
            } else {
                crate::palette::mix(look.fg, look.dim, 0.35)
            })
            .width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if dirty {
        line = line.push(container(space()).width(6).height(6).style(
            move |_theme: &iced::Theme| {
                container::background(look.accent).border(iced::Border::default().rounded(3))
            },
        ));
    }
    button(line)
        .width(Length::Fill)
        .padding([7, 10])
        .style(move |_theme: &iced::Theme, status| button::Style {
            background: if current {
                Some(alpha(look.accent, 0.16).into())
            } else {
                matches!(status, button::Status::Hovered).then(|| alpha(look.fg, 0.06).into())
            },
            border: iced::Border::default().rounded(t::R_BTN),
            ..button::Style::default()
        })
        .on_press(on_press)
        .into()
}

/// A page heading: an optional accent kicker, the 25 px title and a muted hint.
pub(crate) fn page_head<'a, M: 'a>(
    look: Look,
    kicker: Option<&str>,
    title: &str,
    hint: &str,
) -> Column<'a, M> {
    let mut head = Column::new().spacing(4);
    if let Some(kicker) = kicker {
        head = head.push(text(kicker.to_uppercase()).size(10.5).color(look.accent_hi));
    }
    head.push(
        text(title.to_owned())
            .size(25)
            .font(crate::UI_MEDIUM)
            .color(look.fg),
    )
    .push(text(hint.to_owned()).size(13).color(look.dim))
}

/// A titled group of rows: a small uppercase label, then the rows (whitespace, not rules,
/// separates the groups).
pub fn section<'a, M: 'a>(look: Look, title: &str, rows: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut body = column![text(title.to_uppercase()).size(10.5).color(look.dim)].spacing(14);
    for r in rows {
        body = body.push(r);
    }
    body.padding(iced::Padding {
        top: 10.0,
        right: 0.0,
        bottom: 18.0,
        left: 0.0,
    })
    .into()
}
