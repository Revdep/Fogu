//! iced key events → [`KeyChord`]s for the keymap, or GUI-agnostic [`KeyInput`]s for
//! `fogu_term::input::encode_key`.

use fogu_core::{Action, Hotkeys, KeyChord, KeyName, Mods as ChordMods, NamedKey};
use fogu_term::input::{KeyCode, KeyInput, Mods};
use iced::keyboard::key::Named;
use iced::keyboard::{Key, Location, Modifiers};

/// The action `hotkeys` binds to a key press, if any.
///
/// Looks up the chord of the layout key without modifiers first (`Ctrl-Shift-T` is `t` + Ctrl +
/// Shift), then the Shift-applied key, so a chord whose character needs Shift on the user's layout
/// (e.g. `Ctrl-Shift-=` on layouts where `=` is Shift+0) still matches.
pub fn hotkey(
    hotkeys: &Hotkeys,
    key: &Key,
    modified_key: &Key,
    modifiers: Modifiers,
) -> Option<Action> {
    let lookup = |k: &Key| chord(k, modifiers).and_then(|c| hotkeys.lookup(c));
    lookup(key).or_else(|| {
        (modified_key != key)
            .then(|| lookup(modified_key))
            .flatten()
    })
}

/// An iced key + modifiers as a [`KeyChord`]; `None` for keys a chord cannot name.
pub fn chord(key: &Key, modifiers: Modifiers) -> Option<KeyChord> {
    let key = match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyName::Char(c.to_ascii_uppercase()),
                _ => return None,
            }
        }
        Key::Named(named) => chord_key(*named)?,
        Key::Unidentified => return None,
    };
    let mods = ChordMods {
        ctrl: modifiers.control(),
        alt: modifiers.alt(),
        shift: modifiers.shift(),
        meta: modifiers.logo(),
    };
    Some(KeyChord { mods, key })
}

fn chord_key(named: Named) -> Option<KeyName> {
    let n = |k| Some(KeyName::Named(k));
    match named {
        Named::Tab => n(NamedKey::Tab),
        Named::Enter => n(NamedKey::Enter),
        Named::Escape => n(NamedKey::Escape),
        Named::Backspace => n(NamedKey::Backspace),
        Named::Delete => n(NamedKey::Delete),
        Named::Insert => n(NamedKey::Insert),
        Named::Space => n(NamedKey::Space),
        Named::ArrowLeft => n(NamedKey::Left),
        Named::ArrowRight => n(NamedKey::Right),
        Named::ArrowUp => n(NamedKey::Up),
        Named::ArrowDown => n(NamedKey::Down),
        Named::Home => n(NamedKey::Home),
        Named::End => n(NamedKey::End),
        Named::PageUp => n(NamedKey::PageUp),
        Named::PageDown => n(NamedKey::PageDown),
        _ => function_key(named).map(KeyName::Function),
    }
}

/// `F1`..`F24` as their number.
const fn function_key(named: Named) -> Option<u8> {
    Some(match named {
        Named::F1 => 1,
        Named::F2 => 2,
        Named::F3 => 3,
        Named::F4 => 4,
        Named::F5 => 5,
        Named::F6 => 6,
        Named::F7 => 7,
        Named::F8 => 8,
        Named::F9 => 9,
        Named::F10 => 10,
        Named::F11 => 11,
        Named::F12 => 12,
        Named::F13 => 13,
        Named::F14 => 14,
        Named::F15 => 15,
        Named::F16 => 16,
        Named::F17 => 17,
        Named::F18 => 18,
        Named::F19 => 19,
        Named::F20 => 20,
        Named::F21 => 21,
        Named::F22 => 22,
        Named::F23 => 23,
        Named::F24 => 24,
        _ => return None,
    })
}

/// Converts a key press. `modified_key` is the layout's key with Shift applied (iced's
/// `KeyPressed::modified_key`); keys the terminal has no encoding for give `None`.
pub fn key_input(
    modified_key: &Key,
    location: Location,
    modifiers: Modifiers,
    text: Option<&str>,
) -> Option<KeyInput> {
    let numpad = location == Location::Numpad;
    let key = match modified_key {
        // Multi-char keysyms carry their full content in `text`; the first char names the key.
        Key::Character(s) => {
            let c = s.chars().next()?;
            if numpad {
                KeyCode::Numpad(c)
            } else {
                KeyCode::Char(c)
            }
        }
        Key::Named(named) => named_key(*named, numpad)?,
        Key::Unidentified => return None,
    };
    Some(KeyInput {
        key,
        text: text.map(str::to_owned),
        mods: mods(modifiers),
    })
}

/// macOS: Option composes characters (Option+f = `ƒ`, Option+ò = `@` on an Italian layout).
/// Option + ASCII letter stays Meta: the key becomes the letter itself, sent as ESC + letter
/// when `alt_is_meta` (else the composed text). Option + anything else is just its character.
/// `key` is the layout key without modifiers.
pub fn macos_option(mut input: KeyInput, key: &Key) -> KeyInput {
    if !input.mods.alt || !matches!(input.key, KeyCode::Char(_)) {
        return input;
    }
    match key {
        Key::Character(s) => match s.as_bytes() {
            [b] if b.is_ascii_alphabetic() => {
                let b = if input.mods.shift {
                    b.to_ascii_uppercase()
                } else {
                    b.to_ascii_lowercase()
                };
                input.key = KeyCode::Char(char::from(b));
            }
            _ => input.mods.alt = false,
        },
        _ => input.mods.alt = false,
    }
    input
}

/// iced modifiers as the encoder's [`Mods`].
pub fn mods(modifiers: Modifiers) -> Mods {
    Mods {
        shift: modifiers.shift(),
        alt: modifiers.alt(),
        ctrl: modifiers.control(),
        logo: modifiers.logo(),
    }
}

fn named_key(named: Named, numpad: bool) -> Option<KeyCode> {
    Some(match named {
        Named::Enter if numpad => KeyCode::NumpadEnter,
        Named::Enter => KeyCode::Enter,
        Named::Tab => KeyCode::Tab,
        Named::Backspace => KeyCode::Backspace,
        Named::Escape => KeyCode::Escape,
        Named::Space => KeyCode::Char(' '),
        Named::ArrowUp => KeyCode::Up,
        Named::ArrowDown => KeyCode::Down,
        Named::ArrowLeft => KeyCode::Left,
        Named::ArrowRight => KeyCode::Right,
        Named::Home => KeyCode::Home,
        Named::End => KeyCode::End,
        Named::Insert => KeyCode::Insert,
        Named::Delete => KeyCode::Delete,
        Named::PageUp => KeyCode::PageUp,
        Named::PageDown => KeyCode::PageDown,
        Named::F1 => KeyCode::F(1),
        Named::F2 => KeyCode::F(2),
        Named::F3 => KeyCode::F(3),
        Named::F4 => KeyCode::F(4),
        Named::F5 => KeyCode::F(5),
        Named::F6 => KeyCode::F(6),
        Named::F7 => KeyCode::F(7),
        Named::F8 => KeyCode::F(8),
        Named::F9 => KeyCode::F(9),
        Named::F10 => KeyCode::F(10),
        Named::F11 => KeyCode::F(11),
        Named::F12 => KeyCode::F(12),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn converts_characters_named_and_numpad() {
        let ctrl = Modifiers::CTRL;
        let k = key_input(
            &Key::Character("c".into()),
            Location::Standard,
            ctrl,
            Some("\x03"),
        );
        assert_eq!(
            k.map(|k| (k.key, k.mods.ctrl)),
            Some((KeyCode::Char('c'), true))
        );

        let k = key_input(
            &Key::Named(Named::Enter),
            Location::Numpad,
            Modifiers::empty(),
            None,
        );
        assert_eq!(k.map(|k| k.key), Some(KeyCode::NumpadEnter));

        let k = key_input(
            &Key::Character("5".into()),
            Location::Numpad,
            Modifiers::empty(),
            None,
        );
        assert_eq!(k.map(|k| k.key), Some(KeyCode::Numpad('5')));

        let k = key_input(&Key::Named(Named::Space), Location::Standard, ctrl, None);
        assert_eq!(k.map(|k| k.key), Some(KeyCode::Char(' ')));

        let none = key_input(
            &Key::Named(Named::Shift),
            Location::Left,
            Modifiers::SHIFT,
            None,
        );
        assert_eq!(none, None);
    }

    /// What the shell receives for Option + `key` (the layout key without modifiers) when the
    /// layout composes `composed`.
    #[cfg(target_os = "macos")]
    fn option_bytes(key: &str, composed: &str, mods: Modifiers, alt_is_meta: bool) -> Vec<u8> {
        let ch = |c: &str| Key::Character(c.into());
        let input = key_input(&ch(composed), Location::Standard, mods, Some(composed)).unwrap();
        let input = macos_option(input, &ch(key));
        fogu_term::input::encode_key(&input, fogu_term::TermMode::empty(), alt_is_meta).unwrap()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_option_letters_are_meta_other_keys_type_their_character() {
        let alt = Modifiers::ALT;
        let shift = Modifiers::ALT | Modifiers::SHIFT;
        let rows: [(&str, &str, Modifiers, bool, &[u8]); 9] = [
            ("f", "ƒ", alt, true, b"\x1bf"),
            ("b", "∫", alt, true, b"\x1bb"),
            ("f", "Ï", shift, true, b"\x1bF"),
            ("f", "ƒ", alt | Modifiers::CTRL, true, b"\x1b\x06"),
            ("f", "ƒ", alt, false, "ƒ".as_bytes()),
            // Italian layout: @ # [ { ~ need Option and must arrive bare.
            ("ò", "@", alt, true, b"@"),
            ("à", "#", alt, true, b"#"),
            ("è", "{", shift, true, b"{"),
            ("5", "~", alt, true, b"~"),
        ];
        for (key, composed, mods, alt_is_meta, want) in rows {
            assert_eq!(
                option_bytes(key, composed, mods, alt_is_meta),
                want,
                "{key} {composed} {mods:?} alt_is_meta={alt_is_meta}"
            );
        }
        // Named keys keep Option (word moves on Option+arrows).
        let left = key_input(&Key::Named(Named::ArrowLeft), Location::Standard, alt, None);
        let left = macos_option(left.unwrap(), &Key::Named(Named::ArrowLeft));
        assert!(left.mods.alt);
    }

    #[test]
    fn chord_from_iced_table() {
        let ch = |c: &str| Key::Character(c.into());
        let nk = Key::Named;
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let rows: [(Key, Modifiers, Option<&str>); 13] = [
            (ch("t"), ctrl_shift, Some("Ctrl-Shift-T")),
            (ch("T"), ctrl_shift, Some("Ctrl-Shift-T")),
            (ch("2"), Modifiers::ALT, Some("Alt-2")),
            (ch("="), Modifiers::CTRL, Some("Ctrl-=")),
            (ch("-"), Modifiers::CTRL, Some("Ctrl--")),
            (nk(Named::PageDown), Modifiers::CTRL, Some("Ctrl-PageDown")),
            (nk(Named::Tab), ctrl_shift, Some("Ctrl-Shift-Tab")),
            (nk(Named::Insert), Modifiers::SHIFT, Some("Shift-Insert")),
            (nk(Named::F11), Modifiers::empty(), Some("F11")),
            (nk(Named::Space), Modifiers::LOGO, Some("Meta-Space")),
            (nk(Named::Shift), Modifiers::SHIFT, None),
            (ch("ab"), Modifiers::CTRL, None),
            (Key::Unidentified, Modifiers::CTRL, None),
        ];
        for (key, mods, want) in rows {
            let want = want.map(|s| KeyChord::parse(s).unwrap());
            assert_eq!(chord(&key, mods), want, "{key:?} {mods:?}");
        }
    }

    #[test]
    fn hotkey_lookup_falls_back_to_the_shifted_key() {
        let hotkeys = Hotkeys::default_keymap();
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let ch = |c: &str| Key::Character(c.into());
        assert_eq!(
            hotkey(&hotkeys, &ch("t"), &ch("T"), ctrl_shift),
            Some(Action::NewTab)
        );
        // `=` typed as Shift+0 (e.g. Italian layout): unmodified `0` is unbound with Ctrl+Shift.
        assert_eq!(
            hotkey(&hotkeys, &ch("0"), &ch("="), ctrl_shift),
            Some(Action::ZoomIn)
        );
        // Plain typing and Ctrl+C go to the shell.
        assert_eq!(hotkey(&hotkeys, &ch("c"), &ch("c"), Modifiers::CTRL), None);
        assert_eq!(
            hotkey(&hotkeys, &ch("a"), &ch("a"), Modifiers::empty()),
            None
        );
    }
}
