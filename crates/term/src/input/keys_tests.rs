#![expect(
    clippy::similar_names,
    reason = "`mode` and `mods` are the domain terms"
)]

use super::*;

const N: Mods = Mods::NONE;
const S: Mods = Mods {
    shift: true,
    ..Mods::NONE
};
const A: Mods = Mods {
    alt: true,
    ..Mods::NONE
};
const C: Mods = Mods {
    ctrl: true,
    ..Mods::NONE
};
const CS: Mods = Mods {
    ctrl: true,
    shift: true,
    ..Mods::NONE
};
const CA: Mods = Mods {
    ctrl: true,
    alt: true,
    ..Mods::NONE
};
const L: Mods = Mods {
    logo: true,
    ..Mods::NONE
};

const NORMAL: TermMode = TermMode::empty();
const CKM: TermMode = TermMode::APP_CURSOR;
const KPAM: TermMode = TermMode::APP_KEYPAD;
const LNM: TermMode = TermMode::LINE_FEED_NEW_LINE;
const KITTY1: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;
const KITTY9: TermMode = TermMode::DISAMBIGUATE_ESC_CODES.union(TermMode::REPORT_ALL_KEYS_AS_ESC);

struct Row {
    name: &'static str,
    key: KeyCode,
    text: Option<&'static str>,
    mods: Mods,
    mode: TermMode,
    meta: bool,
    want: Option<&'static [u8]>,
}

const fn row(
    name: &'static str,
    key: KeyCode,
    text: Option<&'static str>,
    mods: Mods,
    mode: TermMode,
    meta: bool,
    want: Option<&'static [u8]>,
) -> Row {
    Row {
        name,
        key,
        text,
        mods,
        mode,
        meta,
        want,
    }
}

use KeyCode::{
    Backspace, Char, Delete, Down, End, Enter, Escape, F, Home, Insert, Left, Numpad, NumpadEnter,
    PageDown, PageUp, Right, Tab, Up,
};

#[rustfmt::skip]
const TABLE: &[Row] = &[
    // Printable text.
    row("text a", Char('a'), Some("a"), N, NORMAL, true, Some(b"a")),
    row("text shift A", Char('A'), Some("A"), S, NORMAL, true, Some(b"A")),
    row("text utf8", Char('é'), Some("é"), N, NORMAL, true, Some("é".as_bytes())),
    row("text dead-key compose", Char('e'), Some("ê"), N, NORMAL, true, Some("ê".as_bytes())),
    row("text missing uses key", Char('x'), None, N, NORMAL, true, Some(b"x")),
    row("text control chars dropped", Char('x'), Some("\x1b"), N, NORMAL, true, Some(b"x")),
    row("text space", Char(' '), Some(" "), N, NORMAL, true, Some(b" ")),
    // macOS: an unbound Cmd chord (Cmd-N, Cmd-S…) is an app shortcut, never text.
    row("text logo", Char('a'), Some("a"), L, NORMAL, true, if cfg!(target_os = "macos") { None } else { Some(b"a") }),
    row("text logo kitty", Char('a'), Some("a"), L, KITTY1, true, Some(b"\x1b[97;9u")),
    // Alt as meta.
    row("alt b meta", Char('b'), Some("b"), A, NORMAL, true, Some(b"\x1bb")),
    row("alt f meta", Char('f'), Some("f"), A, NORMAL, true, Some(b"\x1bf")),
    row("alt b no meta", Char('b'), Some("b"), A, NORMAL, false, Some(b"b")),
    row("alt altgr text no meta", Char('@'), Some("@"), A, NORMAL, false, Some(b"@")),
    // Ctrl → C0.
    row("ctrl c", Char('c'), Some("\x03"), C, NORMAL, true, Some(b"\x03")),
    row("ctrl d", Char('d'), None, C, NORMAL, true, Some(b"\x04")),
    row("ctrl shift A", Char('A'), None, CS, NORMAL, true, Some(b"\x01")),
    row("ctrl space", Char(' '), None, C, NORMAL, true, Some(b"\x00")),
    row("ctrl @", Char('@'), None, CS, NORMAL, true, Some(b"\x00")),
    row("ctrl [", Char('['), None, C, NORMAL, true, Some(b"\x1b")),
    row("ctrl backslash", Char('\\'), None, C, NORMAL, true, Some(b"\x1c")),
    row("ctrl ]", Char(']'), None, C, NORMAL, true, Some(b"\x1d")),
    row("ctrl ^", Char('^'), None, CS, NORMAL, true, Some(b"\x1e")),
    row("ctrl _", Char('_'), None, CS, NORMAL, true, Some(b"\x1f")),
    row("ctrl ?", Char('?'), None, CS, NORMAL, true, Some(b"\x7f")),
    row("ctrl 1 falls back to text", Char('1'), Some("1"), C, NORMAL, true, Some(b"1")),
    row("ctrl alt x meta", Char('x'), None, CA, NORMAL, true, Some(b"\x1b\x18")),
    row("ctrl alt x no meta", Char('x'), None, CA, NORMAL, false, Some(b"\x18")),
    // Enter / Tab / Backspace / Escape.
    row("enter", Enter, Some("\r"), N, NORMAL, true, Some(b"\r")),
    row("enter ckm", Enter, None, N, CKM, true, Some(b"\r")),
    row("enter lnm", Enter, None, N, LNM, true, Some(b"\r\n")),
    row("alt enter", Enter, None, A, NORMAL, true, Some(b"\x1b\r")),
    row("tab", Tab, Some("\t"), N, NORMAL, true, Some(b"\t")),
    row("shift tab", Tab, None, S, NORMAL, true, Some(b"\x1b[Z")),
    row("backspace", Backspace, None, N, NORMAL, true, Some(b"\x7f")),
    row("ctrl backspace", Backspace, None, C, NORMAL, true, Some(b"\x17")),
    row("alt backspace", Backspace, None, A, NORMAL, true, Some(b"\x1b\x7f")),
    row("escape", Escape, None, N, NORMAL, true, Some(b"\x1b")),
    row("alt escape", Escape, None, A, NORMAL, true, Some(b"\x1b\x1b")),
    // Cursor keys in both DECCKM states.
    row("up normal", Up, None, N, NORMAL, true, Some(b"\x1b[A")),
    row("up app", Up, None, N, CKM, true, Some(b"\x1bOA")),
    row("down normal", Down, None, N, NORMAL, true, Some(b"\x1b[B")),
    row("down app", Down, None, N, CKM, true, Some(b"\x1bOB")),
    row("right normal", Right, None, N, NORMAL, true, Some(b"\x1b[C")),
    row("right app", Right, None, N, CKM, true, Some(b"\x1bOC")),
    row("left normal", Left, None, N, NORMAL, true, Some(b"\x1b[D")),
    row("left app", Left, None, N, CKM, true, Some(b"\x1bOD")),
    row("home normal", Home, None, N, NORMAL, true, Some(b"\x1b[H")),
    row("home app", Home, None, N, CKM, true, Some(b"\x1bOH")),
    row("end normal", End, None, N, NORMAL, true, Some(b"\x1b[F")),
    row("end app", End, None, N, CKM, true, Some(b"\x1bOF")),
    row("shift up", Up, None, S, NORMAL, true, Some(b"\x1b[1;2A")),
    row("alt left normal", Left, None, A, NORMAL, true, Some(b"\x1b[1;3D")),
    row("ctrl right app", Right, None, C, CKM, true, Some(b"\x1b[1;5C")),
    row("ctrl shift home", Home, None, CS, NORMAL, true, Some(b"\x1b[1;6H")),
    row("logo end", End, None, L, NORMAL, true, Some(b"\x1b[1;9F")),
    // Editing keys.
    row("insert", Insert, None, N, NORMAL, true, Some(b"\x1b[2~")),
    row("delete", Delete, None, N, CKM, true, Some(b"\x1b[3~")),
    row("pageup", PageUp, None, N, NORMAL, true, Some(b"\x1b[5~")),
    row("pagedown", PageDown, None, N, NORMAL, true, Some(b"\x1b[6~")),
    row("shift pageup", PageUp, None, S, NORMAL, true, Some(b"\x1b[5;2~")),
    row("ctrl delete", Delete, None, C, NORMAL, true, Some(b"\x1b[3;5~")),
    // Function keys.
    row("f1", F(1), None, N, NORMAL, true, Some(b"\x1bOP")),
    row("f2", F(2), None, N, CKM, true, Some(b"\x1bOQ")),
    row("f3", F(3), None, N, NORMAL, true, Some(b"\x1bOR")),
    row("f4", F(4), None, N, NORMAL, true, Some(b"\x1bOS")),
    row("shift f1", F(1), None, S, NORMAL, true, Some(b"\x1b[1;2P")),
    row("ctrl f4", F(4), None, C, NORMAL, true, Some(b"\x1b[1;5S")),
    row("f5", F(5), None, N, NORMAL, true, Some(b"\x1b[15~")),
    row("f6", F(6), None, N, NORMAL, true, Some(b"\x1b[17~")),
    row("f10", F(10), None, N, NORMAL, true, Some(b"\x1b[21~")),
    row("f11", F(11), None, N, NORMAL, true, Some(b"\x1b[23~")),
    row("f12", F(12), None, N, NORMAL, true, Some(b"\x1b[24~")),
    row("alt f5", F(5), None, A, NORMAL, true, Some(b"\x1b[15;3~")),
    row("f13 unknown", F(13), None, N, NORMAL, true, None),
    row("f0 unknown", F(0), None, N, NORMAL, true, None),
    // Keypad.
    row("kp 5 normal", Numpad('5'), Some("5"), N, NORMAL, true, Some(b"5")),
    row("kp 5 deckpam stays digit", Numpad('5'), Some("5"), N, KPAM, true, Some(b"5")),
    row("kp + normal", Numpad('+'), Some("+"), N, NORMAL, true, Some(b"+")),
    row("kp + deckpam", Numpad('+'), Some("+"), N, KPAM, true, Some(b"\x1bOk")),
    row("kp - deckpam", Numpad('-'), Some("-"), N, KPAM, true, Some(b"\x1bOm")),
    row("kp * deckpam", Numpad('*'), Some("*"), N, KPAM, true, Some(b"\x1bOj")),
    row("kp / deckpam", Numpad('/'), Some("/"), N, KPAM, true, Some(b"\x1bOo")),
    row("kp enter normal", NumpadEnter, None, N, NORMAL, true, Some(b"\r")),
    row("kp enter deckpam", NumpadEnter, None, N, KPAM, true, Some(b"\x1bOM")),
    // Kitty keyboard protocol.
    row("kitty1 plain a stays text", Char('a'), Some("a"), N, KITTY1, true, Some(b"a")),
    row("kitty1 shift A stays text", Char('A'), Some("A"), S, KITTY1, true, Some(b"A")),
    row("kitty1 escape", Escape, None, N, KITTY1, true, Some(b"\x1b[27u")),
    row("kitty1 ctrl c", Char('c'), None, C, KITTY1, true, Some(b"\x1b[99;5u")),
    row("kitty1 alt b", Char('b'), Some("b"), A, KITTY1, true, Some(b"\x1b[98;3u")),
    row("kitty1 ctrl shift A", Char('A'), None, CS, KITTY1, true, Some(b"\x1b[97;6u")),
    row("kitty1 ctrl space", Char(' '), None, C, KITTY1, true, Some(b"\x1b[32;5u")),
    row("kitty1 enter legacy", Enter, None, N, KITTY1, true, Some(b"\r")),
    row("kitty1 shift enter", Enter, None, S, KITTY1, true, Some(b"\x1b[13;2u")),
    row("kitty1 ctrl backspace", Backspace, None, C, KITTY1, true, Some(b"\x1b[127;5u")),
    row("kitty1 shift tab", Tab, None, S, KITTY1, true, Some(b"\x1b[9;2u")),
    row("kitty1 up legacy", Up, None, N, KITTY1.union(CKM), true, Some(b"\x1bOA")),
    row("kitty1 ctrl f5 legacy", F(5), None, C, KITTY1, true, Some(b"\x1b[15;5~")),
    row("kitty9 plain a", Char('a'), Some("a"), N, KITTY9, true, Some(b"\x1b[97u")),
    row("kitty9 enter", Enter, None, N, KITTY9, true, Some(b"\x1b[13u")),
    row("kitty9 tab", Tab, None, N, KITTY9, true, Some(b"\x1b[9u")),
    row("kitty9 backspace", Backspace, None, N, KITTY9, true, Some(b"\x1b[127u")),
];

#[test]
fn input_key_table() {
    assert!(TABLE.len() >= 40);
    let mut failures = Vec::new();
    for r in TABLE {
        let k = KeyInput {
            key: r.key,
            text: r.text.map(str::to_owned),
            mods: r.mods,
        };
        let got = encode_key(&k, r.mode, r.meta);
        if got.as_deref() != r.want {
            failures.push(format!("{}: got {got:?}, want {:?}", r.name, r.want));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn input_mods_param() {
    assert_eq!(N.param(), 1);
    assert_eq!(S.param(), 2);
    assert_eq!(A.param(), 3);
    assert_eq!(C.param(), 5);
    assert_eq!(CA.param(), 7);
    let all = Mods {
        shift: true,
        alt: true,
        ctrl: true,
        logo: true,
    };
    assert_eq!(all.param(), 16);
}

#[test]
fn input_encode_text_strips_controls() {
    assert_eq!(encode_text("héllo"), Some("héllo".as_bytes().to_vec()));
    assert_eq!(encode_text("a\x1b[201~b"), Some(b"a[201~b".to_vec()));
    assert_eq!(encode_text("\u{9b}201~"), Some(b"201~".to_vec()));
    assert_eq!(encode_text("\x07\x7f"), None);
    assert_eq!(encode_text(""), None);
}

/// Exhaustive over every key, modifier combination and relevant mode: no encoding may contain the
/// bracketed-paste end marker, even when the key's text tries to inject it.
#[test]
fn input_never_emits_paste_end() {
    const PASTE_END: &[u8] = b"\x1b[201~";
    let mut keys = vec![
        Enter,
        Tab,
        Backspace,
        Escape,
        Up,
        Down,
        Left,
        Right,
        Home,
        End,
        Insert,
        Delete,
        PageUp,
        PageDown,
        NumpadEnter,
    ];
    keys.extend((0..=30).map(F));
    keys.extend("0123456789.+-*/".chars().map(Numpad));
    keys.extend((' '..='~').chain("éß€\u{9b}\u{1b}".chars()).map(Char));
    let modes = [
        NORMAL,
        CKM,
        KPAM,
        LNM,
        CKM | KPAM,
        KITTY1,
        KITTY9,
        TermMode::BRACKETED_PASTE,
    ];
    let texts = [None, Some("\x1b[201~"), Some("x\u{9b}201~"), Some("201~")];
    for key in keys {
        for bits in 0u8..16 {
            let mods = Mods {
                shift: bits & 1 != 0,
                alt: bits & 2 != 0,
                ctrl: bits & 4 != 0,
                logo: bits & 8 != 0,
            };
            for mode in modes {
                for text in texts {
                    for meta in [false, true] {
                        let k = KeyInput {
                            key,
                            text: text.map(str::to_owned),
                            mods,
                        };
                        if let Some(bytes) = encode_key(&k, mode, meta) {
                            assert!(
                                !bytes.windows(PASTE_END.len()).any(|w| w == PASTE_END),
                                "{k:?} {mode:?} → {bytes:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
