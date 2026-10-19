//! Keyboard → bytes: legacy xterm encoding honouring `TermMode` (DECCKM, DECKPAM, LNM), Alt as
//! Meta, `CSI 1;N` modifier forms, and the kitty keyboard protocol's "disambiguate escape codes"
//! and "report all keys as escape codes" flags.
//!
//! Pure and GUI-agnostic: the UI converts its key events into [`KeyInput`].

use alacritty_terminal::term::TermMode;

const ESC: u8 = 0x1b;

/// A key, independent of the GUI toolkit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    /// A character key, as the layout produces it with Shift applied (`'A'`, `'^'`, `' '`).
    Char(char),
    /// Return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Cursor up.
    Up,
    /// Cursor down.
    Down,
    /// Cursor left.
    Left,
    /// Cursor right.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Insert.
    Insert,
    /// Delete (forward).
    Delete,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Function key F1–F12 (other numbers encode to nothing).
    F(u8),
    /// A keypad character key with `NumLock` on: `'0'..='9'`, `'.'`, `'+'`, `'-'`, `'*'`, `'/'`.
    Numpad(char),
    /// The keypad's Enter.
    NumpadEnter,
}

/// Modifier keys held during a key press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent modifier keys"
)]
pub struct Mods {
    /// Shift.
    pub shift: bool,
    /// Alt.
    pub alt: bool,
    /// Control.
    pub ctrl: bool,
    /// Super / logo.
    pub logo: bool,
}

impl Mods {
    /// No modifier held.
    pub const NONE: Self = Self {
        shift: false,
        alt: false,
        ctrl: false,
        logo: false,
    };

    /// True when no modifier is held.
    pub fn is_empty(self) -> bool {
        self == Self::NONE
    }

    /// The xterm / kitty modifier parameter `1 + shift + 2·alt + 4·ctrl + 8·super`.
    pub fn param(self) -> u8 {
        1 + u8::from(self.shift)
            + 2 * u8::from(self.alt)
            + 4 * u8::from(self.ctrl)
            + 8 * u8::from(self.logo)
    }
}

/// One key press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInput {
    /// Which key.
    pub key: KeyCode,
    /// Text the key produced (layout, dead keys, compose), if any.
    pub text: Option<String>,
    /// Held modifiers.
    pub mods: Mods,
}

/// Encodes one key press for the program in the terminal, or `None` when the key sends nothing.
pub fn encode_key(k: &KeyInput, mode: TermMode, alt_is_meta: bool) -> Option<Vec<u8>> {
    if mode.intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC)
        && let Some(bytes) = encode_kitty(k, mode)
    {
        return Some(bytes);
    }
    encode_legacy(k, mode, alt_is_meta)
}

/// Committed text (IME, key text) as bytes. Control characters (C0, DEL, C1) are dropped so text
/// can never smuggle escape sequences such as the bracketed-paste end marker.
pub fn encode_text(text: &str) -> Option<Vec<u8>> {
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    (!clean.is_empty()).then(|| clean.into_bytes())
}

fn encode_legacy(k: &KeyInput, mode: TermMode, alt_is_meta: bool) -> Option<Vec<u8>> {
    let m = k.mods;
    let meta = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if m.alt {
            out.push(ESC);
        }
        out.extend_from_slice(bytes);
        out
    };
    let out = match k.key {
        KeyCode::Char(c) => return encode_char(c, k.text.as_deref(), m, alt_is_meta),
        KeyCode::NumpadEnter if mode.contains(TermMode::APP_KEYPAD) => ss3_or_csi(b'M', m),
        KeyCode::Enter | KeyCode::NumpadEnter => meta(newline(mode)),
        KeyCode::Tab if m.shift => csi_letter(b'Z', strip_shift(m)),
        KeyCode::Tab => meta(b"\t"),
        // ^W, not xterm's ^H: shells and vim read it as "delete the word before the cursor".
        KeyCode::Backspace if m.ctrl => meta(&[0x17]),
        KeyCode::Backspace => meta(&[0x7f]),
        KeyCode::Escape => meta(&[ESC]),
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Right
        | KeyCode::Left
        | KeyCode::Home
        | KeyCode::End => {
            let letter = cursor_letter(k.key)?;
            if m.is_empty() && mode.contains(TermMode::APP_CURSOR) {
                vec![ESC, b'O', letter]
            } else {
                csi_letter(letter, m)
            }
        }
        KeyCode::Insert => csi_tilde(2, m),
        KeyCode::Delete => csi_tilde(3, m),
        KeyCode::PageUp => csi_tilde(5, m),
        KeyCode::PageDown => csi_tilde(6, m),
        KeyCode::F(n @ 1..=4) => ss3_or_csi(b'P' + (n - 1), m),
        KeyCode::F(n) => csi_tilde(function_number(n)?, m),
        KeyCode::Numpad(c) => {
            match keypad_app_letter(c) {
                Some(letter) if mode.contains(TermMode::APP_KEYPAD) => ss3_or_csi(letter, m),
                // Digits and '.' with NumLock on stay digits even in DECKPAM (VTE behaviour).
                _ => return encode_char(c, k.text.as_deref(), m, alt_is_meta),
            }
        }
    };
    Some(out)
}

/// Text keys: Ctrl → C0, Alt → ESC prefix when `alt_is_meta`, else the key's text.
///
/// The Meta prefix is only added to a single character: prefixing multi-character text could
/// build an arbitrary escape sequence (e.g. the bracketed-paste end marker).
fn encode_char(c: char, text: Option<&str>, m: Mods, alt_is_meta: bool) -> Option<Vec<u8>> {
    // macOS: `text` is Option's composed character (`ƒ`); as Meta the key itself is meant.
    let text = text.filter(|_| !(cfg!(target_os = "macos") && m.alt && alt_is_meta));
    let body = if m.ctrl {
        match ctrl_byte(c) {
            Some(b) => vec![b],
            None => plain_text(c, text)?,
        }
    } else {
        plain_text(c, text)?
    };
    let single_char = std::str::from_utf8(&body).map_or(true, |s| s.chars().count() == 1);
    if m.alt && alt_is_meta && single_char {
        let mut out = Vec::with_capacity(body.len() + 1);
        out.push(ESC);
        out.extend(body);
        return Some(out);
    }
    Some(body)
}

/// The key's text, or the key's character when the text is missing or only control characters.
fn plain_text(c: char, text: Option<&str>) -> Option<Vec<u8>> {
    text.and_then(encode_text)
        .or_else(|| encode_text(c.encode_utf8(&mut [0; 4])))
}

/// xterm's Ctrl table: letters → 0x01..0x1a, plus `@ space 2`, `[ 3`, `\ 4`, `] 5`, `^ 6`,
/// `_ 7 /`, `? 8`.
fn ctrl_byte(c: char) -> Option<u8> {
    let b = match c {
        'a'..='z' | 'A'..='Z' => u8::try_from(c).ok()? & 0x1f,
        '@' | ' ' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    };
    Some(b)
}

fn newline(mode: TermMode) -> &'static [u8] {
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        b"\r\n"
    } else {
        b"\r"
    }
}

fn strip_shift(m: Mods) -> Mods {
    Mods { shift: false, ..m }
}

fn cursor_letter(key: KeyCode) -> Option<u8> {
    Some(match key {
        KeyCode::Up => b'A',
        KeyCode::Down => b'B',
        KeyCode::Right => b'C',
        KeyCode::Left => b'D',
        KeyCode::Home => b'H',
        KeyCode::End => b'F',
        _ => return None,
    })
}

/// F5–F12 → the `CSI n ~` number.
fn function_number(n: u8) -> Option<u8> {
    Some(match n {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => return None,
    })
}

/// DECKPAM final bytes for the keypad operators (digits and '.' are sent as text).
fn keypad_app_letter(c: char) -> Option<u8> {
    Some(match c {
        '+' => b'k',
        '-' => b'm',
        '*' => b'j',
        '/' => b'o',
        _ => return None,
    })
}

/// `CSI <letter>` without modifiers, `CSI 1;N <letter>` with.
fn csi_letter(letter: u8, m: Mods) -> Vec<u8> {
    if m.is_empty() {
        vec![ESC, b'[', letter]
    } else {
        format!("\x1b[1;{}", m.param())
            .into_bytes()
            .into_iter()
            .chain([letter])
            .collect()
    }
}

/// `SS3 <letter>` without modifiers, `CSI 1;N <letter>` with.
fn ss3_or_csi(letter: u8, m: Mods) -> Vec<u8> {
    if m.is_empty() {
        vec![ESC, b'O', letter]
    } else {
        csi_letter(letter, m)
    }
}

/// `CSI n ~` / `CSI n;N ~`.
fn csi_tilde(n: u8, m: Mods) -> Vec<u8> {
    if m.is_empty() {
        format!("\x1b[{n}~").into_bytes()
    } else {
        format!("\x1b[{n};{}~", m.param()).into_bytes()
    }
}

/// Kitty keyboard protocol, flags 1 (disambiguate) and 8 (all keys as escape codes). Returns
/// `None` for keys that keep their legacy encoding (cursor, editing, function and keypad keys).
fn encode_kitty(k: &KeyInput, mode: TermMode) -> Option<Vec<u8>> {
    let all_keys = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let m = k.mods;
    let code = match k.key {
        KeyCode::Escape => 27,
        KeyCode::Enter if all_keys || !m.is_empty() => 13,
        KeyCode::Tab if all_keys || !m.is_empty() => 9,
        KeyCode::Backspace if all_keys || !m.is_empty() => 127,
        KeyCode::Char(c) => {
            let text_only = !m.ctrl && !m.alt && !m.logo;
            if text_only && !all_keys {
                return None;
            }
            // Kitty reports the unshifted key; for letters that is the lower-case form.
            u32::from(c.to_lowercase().next().unwrap_or(c))
        }
        _ => return None,
    };
    let out = if m.is_empty() {
        format!("\x1b[{code}u")
    } else {
        format!("\x1b[{code};{}u", m.param())
    };
    Some(out.into_bytes())
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
