//! Chrome icons: Phosphor Regular glyphs from the bundled icon font (`assets/fonts/Phosphor.ttf`,
//! a subset; add a glyph by re-running `pyftsubset` with its codepoint from Phosphor's
//! `style.css`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use iced::widget::{container, image, text};
use iced::{Color, Element, Font, Length};

use crate::message::Message;

/// The icon font's family name.
pub const PHOSPHOR: Font = Font::with_name("Phosphor");

/// The icons the chrome uses (Phosphor names in the comments).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// `minus`
    Minimize,
    /// `square`
    Maximize,
    /// `x`
    Close,
    /// `plus`
    Plus,
    /// `caret-down`
    ChevronDown,
    /// `caret-right`
    ChevronRight,
    /// `sidebar-simple`
    Sidebar,
    /// `folder-simple`
    Folder,
    /// `arrows-left-right`
    Ports,
    /// `gear-six`
    Gear,
    /// `hard-drives`: an SSH tab.
    Ssh,
    /// `terminal-window`: a local tab.
    Local,
    /// `pencil-simple`
    Pencil,
    /// `square-split-horizontal`
    Split,
    /// `magnifying-glass`
    Search,
    /// `lightning`: quick connect.
    Lightning,
    /// `palette`
    Palette,
    /// `key`
    Key,
    /// `clock-counter-clockwise`
    History,
    /// `arrow-left`
    ArrowLeft,
    /// `arrow-up`
    ArrowUp,
    /// `house`
    Home,
    /// `arrow-clockwise`
    Refresh,
    /// `upload-simple`
    Upload,
    /// `download-simple`
    Download,
    /// `folder-plus`
    FolderPlus,
    /// `funnel-simple`
    Filter,
    /// `file-text`
    FileText,
    /// `file-zip`
    FileZip,
    /// `lock-key`
    Lock,
    /// `file-code`
    FileCode,
    /// `plugs-connected`
    Plugs,
    /// `identification-card`
    IdCard,
    /// `check-circle`
    CheckCircle,
    /// `arrows-out-simple`
    Expand,
}

impl Icon {
    pub(crate) const fn glyph(self) -> char {
        match self {
            Self::Minimize => '\u{e32a}',
            Self::Maximize => '\u{e45e}',
            Self::Close => '\u{e4f6}',
            Self::Plus => '\u{e3d4}',
            Self::ChevronDown => '\u{e136}',
            Self::ChevronRight => '\u{e13a}',
            Self::Sidebar => '\u{ec24}',
            Self::Folder => '\u{e25a}',
            Self::Ports => '\u{e0a0}',
            Self::Gear => '\u{e272}',
            Self::Ssh => '\u{e2a0}',
            Self::Local => '\u{eae8}',
            Self::Pencil => '\u{e3b4}',
            Self::Split => '\u{e870}',
            Self::Search => '\u{e30c}',
            Self::Lightning => '\u{e2de}',
            Self::Palette => '\u{e6c8}',
            Self::Key => '\u{e2d6}',
            Self::History => '\u{e1a0}',
            Self::ArrowLeft => '\u{e058}',
            Self::ArrowUp => '\u{e08e}',
            Self::Home => '\u{e2c2}',
            Self::Refresh => '\u{e036}',
            Self::Upload => '\u{e4c0}',
            Self::Download => '\u{e20c}',
            Self::FolderPlus => '\u{e258}',
            Self::Filter => '\u{e268}',
            Self::FileText => '\u{e23a}',
            Self::FileZip => '\u{e958}',
            Self::Lock => '\u{e2fe}',
            Self::FileCode => '\u{e914}',
            Self::Plugs => '\u{eb5a}',
            Self::IdCard => '\u{e2c8}',
            Self::CheckCircle => '\u{e184}',
            Self::Expand => '\u{e0a6}',
        }
    }
}

/// `icon`, `size` px in `color`, centred in all the space it is given.
pub fn icon<'a>(icon: Icon, size: f32, color: Color) -> Element<'a, Message> {
    container(glyph(icon, size, color))
        .center(Length::Fill)
        .into()
}

/// `icon` as a bare `size` px text element (no centring box), for rows.
pub fn glyph<'a>(icon: Icon, size: f32, color: Color) -> iced::widget::Text<'a> {
    text(icon.glyph())
        .font(PHOSPHOR)
        .size(size)
        .line_height(1.0)
        .color(color)
}

/// A `radius`-px square of `color` with a circle of that radius cut out, centred on its right
/// (`right`) or left edge and bottom (`bottom`) or top edge: a rounded-corner mask (the circle on
/// the inner corner) or a concave flare (on the outer one). Rasterized at 2×, cached.
pub fn notch(color: Color, radius: f32, right: bool, bottom: bool) -> image::Handle {
    type Key = ([u8; 4], u32, bool, bool);
    static CACHE: OnceLock<Mutex<HashMap<Key, image::Handle>>> = OnceLock::new();
    let key = (color.into_rgba8(), radius.to_bits(), right, bottom);
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_insert_with(|| notch_image(color, radius, right, bottom))
        .clone()
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "radii are small positive numbers of pixels"
)]
fn notch_image(color: Color, radius: f32, right: bool, bottom: bool) -> image::Handle {
    let r = (radius * 2.0).round().clamp(1.0, 128.0);
    let px = r as u32;
    let Some(mut pixmap) = tiny_skia::Pixmap::new(px, px) else {
        return image::Handle::from_rgba(1, 1, vec![0; 4]);
    };
    let [red, green, blue, a] = color.into_rgba8();
    pixmap.fill(tiny_skia::Color::from_rgba8(red, green, blue, a));
    let clear = tiny_skia::Paint {
        blend_mode: tiny_skia::BlendMode::Clear,
        anti_alias: true,
        ..tiny_skia::Paint::default()
    };
    let at = |far: bool| if far { r } else { 0.0 };
    if let Some(circle) = tiny_skia::PathBuilder::from_circle(at(right), at(bottom), r) {
        pixmap.fill_path(
            &circle,
            &clear,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
    }
    let rgba: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    image::Handle::from_rgba(px, px, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notch_keeps_the_corner_away_from_the_circle() {
        let red = Color::from_rgb8(0xff, 0, 0);
        // Circle on the bottom-right: the top-left pixel stays, the bottom-right one is cut.
        let handle = notch_image(red, 8.0, true, true);
        assert!(matches!(handle, image::Handle::Rgba { .. }));
        if let image::Handle::Rgba { pixels, .. } = handle {
            assert_eq!(pixels.len(), 16 * 16 * 4);
            assert_eq!(pixels.get(..4), Some(&[0xff, 0, 0, 0xff][..]));
            assert_eq!(pixels.last(), Some(&0));
        }
    }
}
