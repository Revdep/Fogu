//! Terminal colour schemes. Six built-ins are embedded from `assets/schemes/*.toml`
//! (`ColorScheme::by_name`): schemes are data files, not code. Custom scheme files are not
//! supported yet.

use std::fmt;
use std::sync::LazyLock;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour, 8 bits per channel (`Profile.color`); (de)serialises as a `"#RRGGBB"` TOML
/// string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RgbColor {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl RgbColor {
    /// Builds a colour from a `0xRRGGBB` literal.
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: (hex >> 16) as u8,
            g: (hex >> 8) as u8,
            b: hex as u8,
        }
    }

    /// Parses `"#RRGGBB"` (case-insensitive hex digits, `#` required).
    pub fn parse_hex(s: &str) -> Result<Self, RgbColorError> {
        let digits = s
            .strip_prefix('#')
            .ok_or_else(|| RgbColorError(s.to_owned()))?;
        if digits.len() != 6 {
            return Err(RgbColorError(s.to_owned()));
        }
        let byte =
            |i: usize| u8::from_str_radix(digits.get(i..i + 2).ok_or(())?, 16).map_err(|_| ());
        let (r, g, b) = (byte(0), byte(2), byte(4));
        match (r, g, b) {
            (Ok(r), Ok(g), Ok(b)) => Ok(Self { r, g, b }),
            _ => Err(RgbColorError(s.to_owned())),
        }
    }
}

impl fmt::Display for RgbColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// `s` was not a valid `"#RRGGBB"` colour.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid colour {0:?}, expected \"#RRGGBB\"")]
pub struct RgbColorError(String);

impl Serialize for RgbColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RgbColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse_hex(&s).map_err(D::Error::custom)
    }
}

/// Foreground/background/cursor/selection plus the 16 ANSI colours (0–7 normal, 8–15 bright).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorScheme {
    /// Display name, matched by [`ColorScheme::by_name`] and `Config.appearance.color_scheme`.
    pub name: String,
    /// Default text colour.
    pub fg: RgbColor,
    /// Default background colour.
    pub bg: RgbColor,
    /// Cursor colour.
    pub cursor: RgbColor,
    /// Text colour drawn under a filled block cursor.
    pub cursor_text: RgbColor,
    /// Selection background (not drawn yet; the selection overlay is `fg` at low alpha,
    /// `crates/ui/src/terminal_view.rs` `draw_selection`).
    pub selection_bg: RgbColor,
    /// Selection text colour.
    pub selection_fg: RgbColor,
    /// ANSI colours 0–15.
    pub ansi: [RgbColor; 16],
}

/// The 6 built-in schemes, parsed once from the embedded TOML assets.
static BUILTINS: LazyLock<[ColorScheme; 6]> = LazyLock::new(load_builtins);

#[expect(
    clippy::expect_used,
    reason = "the asset strings are compile-time-embedded and fixed; a typo in one is a build-\
              time bug caught immediately by `six_builtins_with_distinct_names` and every other \
              test in this module, never a runtime failure mode for users"
)]
fn load_builtins() -> [ColorScheme; 6] {
    const SOURCES: [&str; 6] = [
        include_str!("../../../assets/schemes/fogu-dark.toml"),
        include_str!("../../../assets/schemes/fogu-light.toml"),
        include_str!("../../../assets/schemes/solarized-dark.toml"),
        include_str!("../../../assets/schemes/dracula.toml"),
        include_str!("../../../assets/schemes/nocturne.toml"),
        include_str!("../../../assets/schemes/catppuccin-mocha.toml"),
    ];
    SOURCES.map(|src| toml::from_str(src).expect("builtin colour scheme asset"))
}

impl ColorScheme {
    /// Looks up a built-in scheme by its exact (case-sensitive) `name`.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        BUILTINS.iter().find(|s| s.name == name).cloned()
    }

    /// All built-in schemes, default first.
    #[must_use]
    pub fn builtins() -> &'static [Self] {
        &*BUILTINS
    }
}

impl Default for ColorScheme {
    /// "Fogu Dark", the same values as `assets/schemes/fogu-dark.toml` (kept as a
    /// hardcoded fallback — unlike [`by_name`](Self::by_name) this can never fail to parse — and
    /// checked against the asset by the `default_matches_builtin_asset` test below).
    fn default() -> Self {
        const ANSI: [u32; 16] = [
            0x00_0000, 0xff_615a, 0xb1_e969, 0xeb_d99c, 0x5d_a9f6, 0xe8_6aff, 0x82_fff7, 0xde_dacf,
            0x76_7676, 0xf5_8c80, 0xdd_f88f, 0xee_e5b2, 0xa5_c7ff, 0xdd_aaff, 0xb7_fff9, 0xff_ffff,
        ];
        Self {
            name: "Fogu Dark".to_owned(),
            fg: RgbColor::from_hex(0xca_caca),
            bg: RgbColor::from_hex(0x17_1717),
            cursor: RgbColor::from_hex(0xbb_bbbb),
            cursor_text: RgbColor::from_hex(0x17_1717),
            selection_bg: RgbColor::from_hex(0xca_caca),
            selection_fg: RgbColor::from_hex(0x17_1717),
            ansi: ANSI.map(RgbColor::from_hex),
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn from_hex_splits_channels() {
        assert_eq!(
            RgbColor::from_hex(0x12_3456),
            RgbColor {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        );
    }

    #[test]
    fn parses_and_displays_hash_rrggbb() {
        assert_eq!(
            RgbColor::parse_hex("#ca1234").unwrap(),
            RgbColor {
                r: 0xca,
                g: 0x12,
                b: 0x34
            }
        );
        assert_eq!(RgbColor::from_hex(0x00_ff80).to_string(), "#00ff80");
        for bad in ["ca1234", "#ca123", "#ca12345", "#zzzzzz", ""] {
            assert!(RgbColor::parse_hex(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn default_matches_builtin_asset() {
        assert_eq!(
            ColorScheme::default(),
            ColorScheme::by_name("Fogu Dark").unwrap()
        );
    }

    #[test]
    fn six_builtins_with_distinct_names() {
        let builtins = ColorScheme::builtins();
        assert_eq!(builtins.len(), 6);
        let names: Vec<&str> = builtins.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names.iter().collect::<std::collections::HashSet<_>>().len(),
            6
        );
        assert!(ColorScheme::by_name("Dracula").is_some());
        assert!(ColorScheme::by_name("Nocturne").is_some());
        assert!(ColorScheme::by_name("Catppuccin Mocha").is_some());
        assert!(ColorScheme::by_name("nonexistent scheme").is_none());
    }
}
