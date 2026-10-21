//! Nocturne widget styles shared by the chrome's screens: inputs, the button variants (primary
//! is an accent outline, never a fill), icon buttons, cards and dialogs.

use iced::widget::{button, container, scrollable, text_input};
use iced::{Background, Border, Color, Shadow, Theme};

use crate::palette::Look;
use crate::tokens::{self as t, alpha};

/// A text field: surface fill, 1 px divider ring, accent ring when focused, accent caret.
pub(crate) fn input(look: Look) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_theme, status| {
        let ring = match status {
            text_input::Status::Focused { .. } => look.accent,
            text_input::Status::Hovered => alpha(look.fg, 0.45),
            _ => look.border,
        };
        text_input::Style {
            background: Background::Color(look.surface),
            border: Border::default().rounded(t::R_MD).width(1).color(ring),
            icon: look.dim,
            placeholder: look.dim,
            value: look.fg,
            selection: alpha(look.accent, 0.3),
        }
    }
}

/// A borderless field (the picker's filter): no fill, no ring.
pub(crate) fn bare_input(look: Look) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |theme, status| text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        ..input(look)(theme, status)
    }
}

/// The button variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Btn {
    /// Accent outline, accent text; hover fills accent @ 12 %.
    Primary,
    /// Divider outline, text colour; hover fills text @ 7 %.
    Secondary,
    /// No outline, accent text; hover fills accent @ 10 %.
    Ghost,
    /// Ghost in the danger colour (Delete…).
    Danger,
    /// No outline, muted text (icon buttons, rows); hover fills text @ 7 %.
    Flat,
}

/// `kind`'s button style; disabled buttons drop to 45 % opacity.
pub(crate) fn btn(look: Look, kind: Btn) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let (color, ring) = match kind {
            Btn::Primary => (look.accent_hi, Some(look.accent)),
            Btn::Secondary => (look.fg, Some(look.border)),
            Btn::Ghost => (look.accent_hi, None),
            Btn::Danger => (look.danger, None),
            Btn::Flat => (look.fg, None),
        };
        let tint = match kind {
            Btn::Secondary | Btn::Flat => look.fg,
            Btn::Danger => look.danger,
            Btn::Primary | Btn::Ghost => look.accent,
        };
        let fill = match (status, kind) {
            (button::Status::Hovered, Btn::Primary) => Some(alpha(tint, 0.12)),
            (button::Status::Hovered, _) => Some(alpha(tint, 0.08)),
            (button::Status::Pressed, _) => Some(alpha(tint, 0.18)),
            _ => None,
        };
        let disabled = status == button::Status::Disabled;
        let fade = |c: Color| if disabled { alpha(c, c.a * 0.45) } else { c };
        button::Style {
            background: fill.map(Background::Color),
            text_color: fade(color),
            border: Border::default()
                .rounded(t::R_MD)
                .width(if ring.is_some() { 1.0 } else { 0.0 })
                .color(fade(ring.unwrap_or(Color::TRANSPARENT))),
            ..button::Style::default()
        }
    }
}

/// [`Btn::Primary`] from the theme alone (dialogs that have no [`Look`]).
pub(crate) fn primary(theme: &Theme, status: button::Status) -> button::Style {
    themed(theme, status, true)
}

/// [`Btn::Secondary`] from the theme alone.
pub(crate) fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    themed(theme, status, false)
}

fn themed(theme: &Theme, status: button::Status, primary: bool) -> button::Style {
    let p = theme.palette();
    let (color, ring) = if primary {
        (p.primary, p.primary)
    } else {
        (p.text, alpha(p.text, 0.16))
    };
    let fill = match status {
        button::Status::Hovered => Some(alpha(if primary { p.primary } else { p.text }, 0.1)),
        button::Status::Pressed => Some(alpha(if primary { p.primary } else { p.text }, 0.18)),
        _ => None,
    };
    let fade = |c: Color| {
        if status == button::Status::Disabled {
            alpha(c, c.a * 0.45)
        } else {
            c
        }
    };
    button::Style {
        background: fill.map(Background::Color),
        text_color: fade(color),
        border: Border::default()
            .rounded(t::R_MD)
            .width(1)
            .color(fade(ring)),
        ..button::Style::default()
    }
}

/// A dialog frame from the theme alone: its background, the large elevation.
pub(crate) fn dialog(theme: &Theme) -> container::Style {
    let p = theme.palette();
    container::Style {
        background: Some(Background::Color(p.background)),
        border: t::ring_lg(t::R_DIALOG),
        shadow: t::shadow_lg(),
        text_color: Some(p.text),
        ..container::Style::default()
    }
}

/// A square icon button, radius 7; `on` = accent icon on accent @ 14 %.
pub(crate) fn icon_btn(look: Look, on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let mut style = btn(look, Btn::Flat)(theme, status);
        style.border = Border::default().rounded(t::R_BTN);
        if on {
            style.background = Some(Background::Color(alpha(look.accent, 0.14)));
            style.text_color = look.accent_hi;
        }
        style
    }
}

/// A row-like card: mix(bg, surface) fill, 1 px divider ring.
pub(crate) fn card(look: Look, radius: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(crate::palette::mix(
            look.bg,
            look.surface,
            0.5,
        ))),
        border: Border::default()
            .rounded(radius)
            .width(1)
            .color(look.border),
        text_color: Some(look.fg),
        ..container::Style::default()
    }
}

/// A floating panel (picker, dialogs, menus): surface fill, the large elevation.
pub(crate) fn floating(look: Look, radius: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(look.surface)),
        border: t::ring_lg(radius),
        shadow: t::shadow_lg(),
        text_color: Some(look.fg),
        ..container::Style::default()
    }
}

/// A small tag: `bg` fill, radius 6.
pub(crate) fn tag(bg: Color) -> container::Style {
    container::Style {
        background: Some(Background::Color(bg)),
        border: Border::default().rounded(6),
        ..container::Style::default()
    }
}

/// A glow around a focused element (focused pane, connected dot).
pub(crate) fn glow(color: Color, a: f32, blur: f32) -> Shadow {
    Shadow {
        color: alpha(color, a),
        offset: iced::Vector::ZERO,
        blur_radius: blur,
    }
}

/// A quiet scrollbar: no rail, a thin neutral thumb that lightens under the pointer.
pub(crate) fn scrollbar(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let mut style = scrollable::default(theme, status);
    let hot = matches!(
        status,
        scrollable::Status::Hovered { .. } | scrollable::Status::Dragged { .. }
    );
    let thumb = if hot { t::N600 } else { t::N800 };
    for rail in [&mut style.vertical_rail, &mut style.horizontal_rail] {
        rail.background = None;
        rail.border = Border::default();
        rail.scroller.background = Background::Color(thumb);
        rail.scroller.border = Border::default().rounded(4);
    }
    style
}
