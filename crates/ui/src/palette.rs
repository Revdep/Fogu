//! Resolves the terminal's `Color::{Named, Indexed, Spec}` against the active [`ColorScheme`].
//!
//! The 269-entry table (16 ANSI, 6×6×6 cube, 24 grays, fg/bg/cursor, dim and bright variants) is
//! built once per scheme; the same table is handed to the `TermHandle` so OSC 4/10/11 queries
//! answer with the colours actually drawn.

use fogu_core::{ColorScheme, RgbColor};
use fogu_term::{Color, NamedColor, Palette, Rgb, default_palette, dim};

/// The resolved colour table of one scheme.
#[derive(Clone, Debug)]
pub struct Colors {
    table: Box<Palette>,
    /// Text drawn under a filled block cursor (the palette has no slot for it).
    cursor_text: Rgb,
}

impl Colors {
    /// Builds the full table for `scheme`.
    pub fn new(scheme: &ColorScheme) -> Self {
        // The default table already holds the computed 256-colour cube and gray ramp.
        let mut table = default_palette();
        let rgb = |c: RgbColor| Rgb {
            r: c.r,
            g: c.g,
            b: c.b,
        };
        for (i, color) in scheme.ansi.iter().enumerate() {
            set(&mut table, i, rgb(*color));
            if i < 8 {
                set(
                    &mut table,
                    NamedColor::DimBlack as usize + i,
                    dim(rgb(*color)),
                );
            }
        }
        let fg = rgb(scheme.fg);
        set(&mut table, NamedColor::Foreground as usize, fg);
        set(&mut table, NamedColor::Background as usize, rgb(scheme.bg));
        set(&mut table, NamedColor::Cursor as usize, rgb(scheme.cursor));
        set(&mut table, NamedColor::BrightForeground as usize, fg);
        set(&mut table, NamedColor::DimForeground as usize, dim(fg));
        Self {
            table,
            cursor_text: rgb(scheme.cursor_text),
        }
    }

    /// The table in the form `TermHandle::set_palette` takes.
    pub fn palette(&self) -> Box<Palette> {
        self.table.clone()
    }

    /// Resolves a cell colour; `bold` maps ANSI 0–7 to their bright variants (the usual
    /// "bold is bright" behaviour).
    pub fn resolve(&self, color: Color, bold: bool) -> Rgb {
        let index = match color {
            Color::Spec(rgb) => return rgb,
            Color::Named(named) => named as usize,
            Color::Indexed(i) => usize::from(i),
        };
        let index = match index {
            0..8 if bold => index + 8,
            i if i == NamedColor::Foreground as usize && bold => {
                NamedColor::BrightForeground as usize
            }
            i => i,
        };
        self.get(index)
    }

    /// Default background.
    pub fn background(&self) -> Rgb {
        self.get(NamedColor::Background as usize)
    }

    /// Default foreground.
    pub fn foreground(&self) -> Rgb {
        self.get(NamedColor::Foreground as usize)
    }

    /// Cursor colour.
    pub fn cursor(&self) -> Rgb {
        self.get(NamedColor::Cursor as usize)
    }

    /// Colour of the character under a filled block cursor.
    pub fn cursor_text(&self) -> Rgb {
        self.cursor_text
    }

    fn get(&self, index: usize) -> Rgb {
        self.table.get(index).copied().unwrap_or_default()
    }
}

fn set(table: &mut Palette, index: usize, value: Rgb) {
    if let Some(slot) = table.get_mut(index) {
        *slot = value;
    }
}

/// The window chrome's colours (menus, panels, title bar): the Nocturne tokens on dark schemes,
/// derived from the terminal scheme on light ones.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Look {
    /// Side panels: the sidebar, SFTP, editor columns.
    pub(crate) bg: iced::Color,
    /// Cards, menus, dialogs, inputs, the picker.
    pub(crate) surface: iced::Color,
    pub(crate) fg: iced::Color,
    /// Muted text.
    pub(crate) dim: iced::Color,
    /// Lines, dots, outlines, active toggles; never a large fill.
    pub(crate) accent: iced::Color,
    /// The accent for icons and text on the dark grounds (accent-400 in Nocturne).
    pub(crate) accent_hi: iced::Color,
    /// The terminal background (the active tab).
    pub(crate) base: iced::Color,
    /// The title / tab bar.
    pub(crate) raised: iced::Color,
    /// 1 px borders and the sidebar handle.
    pub(crate) border: iced::Color,
    /// Hovered rows and buttons.
    pub(crate) hover: iced::Color,
    /// Close-button hover, errors.
    pub(crate) danger: iced::Color,
    /// The Nocturne tokens are in use (a dark scheme).
    pub(crate) dark: bool,
}

impl Look {
    pub(crate) fn new(colors: &Colors) -> Self {
        use crate::tokens as t;
        let base = to_iced(colors.background());
        let fg = to_iced(colors.foreground());
        let dark = base.r * 0.299 + base.g * 0.587 + base.b * 0.114 < 0.5;
        if dark {
            return Self {
                bg: t::PANEL,
                surface: t::SURFACE,
                fg: t::TEXT,
                dim: t::N500,
                accent: t::ACCENT,
                accent_hi: t::A400,
                base,
                raised: t::BAR,
                // Opaque: on a see-through window a translucent line shows the desktop.
                border: mix(t::PANEL, t::TEXT, 0.16),
                hover: t::alpha(t::TEXT, 0.07),
                danger: t::DANGER,
                dark,
            };
        }
        let accent = to_iced(colors.resolve(Color::Named(NamedColor::BrightBlue), false));
        Self {
            bg: mix(base, fg, 0.06),
            surface: mix(base, fg, 0.03),
            fg,
            dim: mix(fg, base, 0.45),
            accent,
            accent_hi: accent,
            base,
            raised: mix(base, iced::Color::BLACK, 0.06),
            border: mix(base, fg, 0.18),
            hover: iced::Color { a: 0.10, ..fg },
            danger: t::DANGER,
            dark,
        }
    }

    /// The iced theme for stock widgets (text fields, sliders, scrollbars, tooltips), so none
    /// of them is drawn in iced's default light colours.
    pub(crate) fn theme(&self, colors: &Colors) -> iced::Theme {
        let ansi = |c| to_iced(colors.resolve(Color::Named(c), false));
        iced::Theme::custom(
            "fogu",
            iced::theme::Palette {
                background: self.surface,
                text: self.fg,
                primary: self.accent,
                success: ansi(NamedColor::Green),
                warning: ansi(NamedColor::Yellow),
                danger: self.danger,
            },
        )
    }
}

/// Linear mix of two colours (`t` = share of `b`).
pub(crate) fn mix(a: iced::Color, b: iced::Color, t: f32) -> iced::Color {
    iced::Color::from_rgb(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
    )
}

/// Converts to an iced colour.
pub fn to_iced(rgb: Rgb) -> iced::Color {
    iced::Color::from_rgb8(rgb.r, rgb.g, rgb.b)
}

/// Mixes `a` and `b` half and half (used for SGR 2 "dim").
pub fn blend_half(a: Rgb, b: Rgb) -> Rgb {
    let mix = u8::midpoint;
    Rgb {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: u32) -> Rgb {
        let c = RgbColor::from_hex(hex);
        Rgb {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }

    #[test]
    fn resolves_against_the_scheme() {
        let colors = Colors::new(&ColorScheme::default());
        let cases = [
            (Color::Named(NamedColor::Red), false, rgb(0xff_615a)),
            (Color::Named(NamedColor::Red), true, rgb(0xf5_8c80)),
            (Color::Indexed(1), true, rgb(0xf5_8c80)),
            (Color::Indexed(9), false, rgb(0xf5_8c80)),
            (Color::Named(NamedColor::Foreground), false, rgb(0xca_caca)),
            (Color::Named(NamedColor::Background), true, rgb(0x17_1717)),
            (Color::Named(NamedColor::Cursor), false, rgb(0xbb_bbbb)),
            // Cube: 16 + 36*1 + 6*2 + 3 → (95, 135, 175).
            (Color::Indexed(67), false, rgb(0x5f_87af)),
            // Gray ramp start/end.
            (Color::Indexed(232), false, rgb(0x08_0808)),
            (Color::Indexed(255), false, rgb(0xee_eeee)),
            (Color::Indexed(200), true, rgb(0xff_00d7)),
            (Color::Spec(rgb(0x12_3456)), true, rgb(0x12_3456)),
        ];
        for (color, bold, want) in cases {
            assert_eq!(colors.resolve(color, bold), want, "{color:?} bold={bold}");
        }
        assert_eq!(colors.cursor_text(), rgb(0x17_1717));
    }

    #[test]
    fn dim_variants_follow_the_scheme() {
        let colors = Colors::new(&ColorScheme::default());
        // 0xff_615a * 2/3 per channel.
        assert_eq!(
            colors.resolve(Color::Named(NamedColor::DimRed), false),
            rgb(0xaa_403c)
        );
        assert_eq!(colors.palette().len(), 269);
    }

    #[test]
    fn blend_half_is_the_midpoint() {
        assert_eq!(blend_half(rgb(0xff_ffff), rgb(0x00_0000)), rgb(0x7f_7f7f));
        assert_eq!(blend_half(rgb(0x10_2030), rgb(0x10_2030)), rgb(0x10_2030));
    }
}
