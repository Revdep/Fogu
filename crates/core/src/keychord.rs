//! Hotkey chord parsing: dash-separated string syntax (`Ctrl-Shift-T`, `Cmd-Option-Left`),
//! case-insensitive; no `⌘` glyphs. Multi-chord sequences (`Ctrl-K Ctrl-C`) are out of scope;
//! they fail to parse as a single chord.

use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Modifier keys held together with the base key.
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent modifier flags, not a state machine"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    /// Control.
    pub ctrl: bool,
    /// Alt (Option on macOS, not relevant here).
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// The "meta"/Super/Windows key.
    pub meta: bool,
}

/// Named (non-printable) keys recognised in a chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamedKey {
    /// `Tab`.
    Tab,
    /// `Enter`/`Return`.
    Enter,
    /// `Escape`/`Esc`.
    Escape,
    /// `Backspace`.
    Backspace,
    /// `Delete`/`Del`.
    Delete,
    /// `Insert`/`Ins`.
    Insert,
    /// `Space`.
    Space,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// `Home`.
    Home,
    /// `End`.
    End,
    /// `PageUp`/`PgUp`.
    PageUp,
    /// `PageDown`/`PgDn`.
    PageDown,
}

impl NamedKey {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "tab" => Self::Tab,
            "enter" | "return" => Self::Enter,
            "escape" | "esc" => Self::Escape,
            "backspace" => Self::Backspace,
            "delete" | "del" => Self::Delete,
            "insert" | "ins" => Self::Insert,
            "space" => Self::Space,
            "left" => Self::Left,
            "right" => Self::Right,
            "up" => Self::Up,
            "down" => Self::Down,
            "home" => Self::Home,
            "end" => Self::End,
            "pageup" | "pgup" => Self::PageUp,
            "pagedown" | "pgdn" => Self::PageDown,
            _ => return None,
        })
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Tab => "Tab",
            Self::Enter => "Enter",
            Self::Escape => "Escape",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Insert => "Insert",
            Self::Space => "Space",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
        }
    }
}

/// The non-modifier key of a chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyName {
    /// A single printable character; letters are canonicalised to uppercase (Shift is a separate
    /// modifier bit, not a case distinction).
    Char(char),
    /// `F1`..`F24`.
    Function(u8),
    /// A named, non-printable key.
    Named(NamedKey),
}

/// A hotkey: zero or more modifiers plus one key, e.g. `Ctrl-Shift-T`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// Held modifiers.
    pub mods: Mods,
    /// The base key.
    pub key: KeyName,
}

/// Why a hotkey string failed to parse.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyChordError {
    /// The string was empty (after trimming).
    #[error("empty hotkey")]
    Empty,
    /// A `-`-separated segment before the key was not a known modifier name.
    #[error("unknown modifier {0:?}")]
    UnknownModifier(String),
    /// The final segment was not a recognised key name, single character, or `F<n>`.
    #[error("unknown key {0:?}")]
    UnknownKey(String),
    /// `F<n>` with `n` outside `1..=24`.
    #[error("function key out of range: {0:?}")]
    BadFunctionKey(String),
}

impl KeyChord {
    /// Parses the hotkey syntax, e.g. `"Ctrl-Shift-T"`, `"F11"`, `"Ctrl--"` (the literal `-`
    /// key). Case-insensitive for modifier and named-key tokens.
    pub fn parse(s: &str) -> Result<Self, KeyChordError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(KeyChordError::Empty);
        }
        let (mods_str, key_str) = split_mods_and_key(s);
        let mut mods = Mods::default();
        if !mods_str.is_empty() {
            for part in mods_str.split('-') {
                match part.to_ascii_lowercase().as_str() {
                    "ctrl" | "control" => mods.ctrl = true,
                    "alt" | "option" | "opt" => mods.alt = true,
                    "shift" => mods.shift = true,
                    "meta" | "super" | "win" | "cmd" | "command" => mods.meta = true,
                    _ => return Err(KeyChordError::UnknownModifier(part.to_owned())),
                }
            }
        }
        let key = parse_key(key_str)?;
        Ok(Self { mods, key })
    }
}

/// Splits `"Ctrl-Shift-T"` into `("Ctrl-Shift", "T")`. The literal `-` key is written with an
/// extra separator dash (`"Ctrl--"`): a trailing `-` is treated as a separator before a `-` key,
/// never as part of a modifier name.
fn split_mods_and_key(s: &str) -> (&str, &str) {
    if s == "-" {
        return ("", "-");
    }
    if s.len() > 1
        && let Some(without_key) = s.strip_suffix('-')
    {
        let mods_str = without_key.strip_suffix('-').unwrap_or(without_key);
        return (mods_str, "-");
    }
    s.rsplit_once('-').unwrap_or(("", s))
}

fn parse_key(s: &str) -> Result<KeyName, KeyChordError> {
    if s.is_empty() {
        return Err(KeyChordError::UnknownKey(s.to_owned()));
    }
    if let Some(rest) = s.strip_prefix(['f', 'F'])
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return match rest.parse::<u8>() {
            Ok(n @ 1..=24) => Ok(KeyName::Function(n)),
            _ => Err(KeyChordError::BadFunctionKey(s.to_owned())),
        };
    }
    if let Some(named) = NamedKey::parse(s) {
        return Ok(KeyName::Named(named));
    }
    let mut chars = s.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return Err(KeyChordError::UnknownKey(s.to_owned()));
    };
    Ok(KeyName::Char(ch.to_ascii_uppercase()))
}

impl FromStr for KeyChord {
    type Err = KeyChordError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for KeyChord {
    /// Canonical order `Ctrl-Alt-Shift-Meta-<key>`; parsing (above) accepts any modifier
    /// order, but this picks one so round-tripping through config is deterministic.
    /// macOS names Alt and Meta `Option-` and `Cmd-`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (alt, meta) = if cfg!(target_os = "macos") {
            ("Option-", "Cmd-")
        } else {
            ("Alt-", "Meta-")
        };
        if self.mods.ctrl {
            write!(f, "Ctrl-")?;
        }
        if self.mods.alt {
            write!(f, "{alt}")?;
        }
        if self.mods.shift {
            write!(f, "Shift-")?;
        }
        if self.mods.meta {
            write!(f, "{meta}")?;
        }
        match self.key {
            KeyName::Char(c) => write!(f, "{c}"),
            KeyName::Function(n) => write!(f, "F{n}"),
            KeyName::Named(n) => write!(f, "{}", n.as_str()),
        }
    }
}

impl Serialize for KeyChord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeyChord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::panic,
        clippy::fn_params_excessive_bools,
        reason = "tests"
    )]

    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Mods {
        Mods {
            ctrl,
            alt,
            shift,
            meta,
        }
    }

    #[test]
    fn parses_valid_chords() {
        let cases: &[(&str, Mods, KeyName)] = &[
            ("T", mods(false, false, false, false), KeyName::Char('T')),
            ("t", mods(false, false, false, false), KeyName::Char('T')),
            (
                "Ctrl-Shift-T",
                mods(true, false, true, false),
                KeyName::Char('T'),
            ),
            (
                "ctrl-shift-t",
                mods(true, false, true, false),
                KeyName::Char('T'),
            ),
            (
                "Shift-Ctrl-T",
                mods(true, false, true, false),
                KeyName::Char('T'),
            ),
            (
                "Alt-F4",
                mods(false, true, false, false),
                KeyName::Function(4),
            ),
            ("F1", mods(false, false, false, false), KeyName::Function(1)),
            (
                "F24",
                mods(false, false, false, false),
                KeyName::Function(24),
            ),
            (
                "Ctrl-=",
                mods(true, false, false, false),
                KeyName::Char('='),
            ),
            (
                "Ctrl-Shift-=",
                mods(true, false, true, false),
                KeyName::Char('='),
            ),
            (
                "Ctrl--",
                mods(true, false, false, false),
                KeyName::Char('-'),
            ),
            (
                "Ctrl-Shift--",
                mods(true, false, true, false),
                KeyName::Char('-'),
            ),
            ("-", mods(false, false, false, false), KeyName::Char('-')),
            (
                "Ctrl-0",
                mods(true, false, false, false),
                KeyName::Char('0'),
            ),
            (
                "Tab",
                mods(false, false, false, false),
                KeyName::Named(NamedKey::Tab),
            ),
            (
                "Ctrl-Tab",
                mods(true, false, false, false),
                KeyName::Named(NamedKey::Tab),
            ),
            (
                "PageUp",
                mods(false, false, false, false),
                KeyName::Named(NamedKey::PageUp),
            ),
            (
                "Alt-PageDown",
                mods(false, true, false, false),
                KeyName::Named(NamedKey::PageDown),
            ),
            (
                "Meta-C",
                mods(false, false, false, true),
                KeyName::Char('C'),
            ),
            (
                "Super-C",
                mods(false, false, false, true),
                KeyName::Char('C'),
            ),
            (
                "Ctrl-Alt-Shift-Meta-I",
                mods(true, true, true, true),
                KeyName::Char('I'),
            ),
        ];
        for (input, want_mods, want_key) in cases {
            let got = KeyChord::parse(input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(got.mods, *want_mods, "{input:?} mods");
            assert_eq!(got.key, *want_key, "{input:?} key");
        }
    }

    #[test]
    fn rejects_invalid_chords() {
        let cases: &[&str] = &[
            "",
            "   ",
            "Foo-T",
            "Ctrl-Shift-Unknown",
            "F0",
            "F25",
            "F100",
        ];
        for input in cases {
            assert!(KeyChord::parse(input).is_err(), "{input:?} should fail");
        }
    }

    #[test]
    fn display_round_trips_in_canonical_order() {
        let cases = ["T", "Ctrl-Shift-T", "Alt-F4", "Ctrl--", "Tab", "Ctrl-Tab"];
        for input in cases {
            let chord = KeyChord::parse(input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            let displayed = chord.to_string();
            let reparsed =
                KeyChord::parse(&displayed).unwrap_or_else(|e| panic!("{displayed:?}: {e}"));
            assert_eq!(chord, reparsed, "{input:?} -> {displayed:?}");
        }
        // Normalises modifier order regardless of how the user wrote it.
        assert_eq!(
            KeyChord::parse("Shift-Ctrl-T").unwrap().to_string(),
            "Ctrl-Shift-T"
        );
    }

    /// macOS shows Alt and Meta as `Option-` and `Cmd-` (what the keyboard says); both
    /// spellings parse everywhere, so a config written on one OS loads on the other.
    #[test]
    fn display_names_the_platform_modifiers() {
        let chord = KeyChord::parse("Meta-Alt-Left").unwrap();
        let want = if cfg!(target_os = "macos") {
            "Option-Cmd-Left"
        } else {
            "Alt-Meta-Left"
        };
        assert_eq!(chord.to_string(), want);
        for spelling in ["Option-Cmd-Left", "Alt-Meta-Left", "opt-command-left"] {
            assert_eq!(KeyChord::parse(spelling).unwrap(), chord, "{spelling}");
        }
    }

    #[test]
    fn serde_round_trips_through_toml() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            v: KeyChord,
        }
        let chord = KeyChord::parse("Ctrl-Shift-T").unwrap();
        let text = toml::to_string(&Wrapper { v: chord }).unwrap();
        assert_eq!(text.trim(), "v = \"Ctrl-Shift-T\"");
        let back: Wrapper = toml::from_str(&text).unwrap();
        assert_eq!(back.v, chord);
    }
}
