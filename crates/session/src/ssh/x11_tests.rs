#![expect(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

use super::*;

/// One `.Xauthority` record, encoded.
fn record(family: u16, address: &[u8], number: &str, name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = family.to_be_bytes().to_vec();
    for field in [address, number.as_bytes(), name.as_bytes(), data] {
        out.extend_from_slice(&u16::try_from(field.len()).unwrap().to_be_bytes());
        out.extend_from_slice(field);
    }
    out
}

/// An X11 client setup (protocol 11.0) with `name`/`data`, in either byte order.
fn setup(big: bool, name: &[u8], data: &[u8]) -> Vec<u8> {
    let enc = |n: usize| {
        let n = u16::try_from(n).unwrap();
        if big {
            n.to_be_bytes()
        } else {
            n.to_le_bytes()
        }
    };
    let mut out = vec![if big { b'B' } else { b'l' }, 0];
    out.extend_from_slice(&enc(11));
    out.extend_from_slice(&enc(0));
    out.extend_from_slice(&enc(name.len()));
    out.extend_from_slice(&enc(data.len()));
    out.extend_from_slice(&[0, 0]);
    for part in [name, data] {
        out.extend_from_slice(part);
        out.resize(pad4(out.len()), 0);
    }
    out
}

const FAKE: [u8; 16] = [7; 16];
const REAL: [u8; 16] = [0xA5; 16];

#[test]
fn displays_parse() {
    let d = |host: &str, number, screen| Display {
        host: host.to_owned(),
        number,
        screen,
    };
    assert_eq!(Display::parse(":0"), Some(d("", 0, 0)));
    assert_eq!(Display::parse(":1.2"), Some(d("", 1, 2)));
    assert_eq!(Display::parse("unix:10"), Some(d("", 10, 0)));
    assert_eq!(
        Display::parse("localhost:10.0"),
        Some(d("localhost", 10, 0))
    );
    assert_eq!(Display::parse("[::1]:3"), Some(d("::1", 3, 0)));
    for bad in ["", "0", ":x", ":1.y", "wayland-0"] {
        assert_eq!(Display::parse(bad), None, "{bad}");
    }
}

#[test]
fn xauthority_fixture_parses_and_picks_the_right_cookie() {
    let mut file = record(FAMILY_LOCAL, b"otherbox", "0", MIT_COOKIE, &[1; 16]);
    file.extend(record(FAMILY_LOCAL, b"mybox", "1", MIT_COOKIE, &[2; 16]));
    file.extend(record(
        FAMILY_LOCAL,
        b"mybox",
        "0",
        "XDM-AUTHORIZATION-1",
        &[3; 8],
    ));
    file.extend(record(FAMILY_LOCAL, b"mybox", "0", MIT_COOKIE, &REAL));
    file.extend(record(
        FAMILY_INTERNET,
        &[10, 0, 0, 5],
        "0",
        MIT_COOKIE,
        &[4; 16],
    ));
    file.extend([0, 1, 0]); // truncated tail
    let entries = parse_xauthority(&file);
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[1].address, b"mybox");
    let local = Display::parse(":0").unwrap();
    assert_eq!(find_cookie(&entries, &local, "mybox"), Some(REAL.to_vec()));
    let tcp = Display::parse("10.0.0.5:0").unwrap();
    assert_eq!(find_cookie(&entries, &tcp, "mybox"), Some(vec![4; 16]));
    // Never a cookie for a host it wasn't made for.
    let elsewhere = Display::parse("10.0.0.6:0").unwrap();
    assert_eq!(find_cookie(&entries, &elsewhere, "mybox"), None);
    assert_eq!(
        find_cookie(&entries, &Display::parse(":5").unwrap(), "mybox"),
        None
    );
    let wild = parse_xauthority(&record(FAMILY_WILD, b"", "5", MIT_COOKIE, &[9; 16]));
    assert_eq!(
        find_cookie(&wild, &Display::parse(":5").unwrap(), "x"),
        Some(vec![9; 16])
    );
}

#[test]
fn the_fake_cookie_is_swapped_for_the_real_one_in_both_byte_orders() {
    for big in [true, false] {
        let fake = setup(big, MIT_COOKIE.as_bytes(), &FAKE);
        let swapped = substitute(&fake, &FAKE, Some(&REAL)).unwrap();
        assert_eq!(
            swapped,
            setup(big, MIT_COOKIE.as_bytes(), &REAL),
            "big={big}"
        );
        // No real cookie: forwarded without authentication.
        let bare = substitute(&fake, &FAKE, None).unwrap();
        assert_eq!(bare, setup(big, b"", b""), "big={big}");
    }
}

#[test]
fn a_wrong_cookie_or_protocol_or_garbage_is_refused() {
    let wrong = setup(true, MIT_COOKIE.as_bytes(), &REAL);
    assert_eq!(
        substitute(&wrong, &FAKE, Some(&REAL)),
        Err(SetupError::BadCookie)
    );
    let short = setup(false, MIT_COOKIE.as_bytes(), &FAKE[..8]);
    assert_eq!(
        substitute(&short, &FAKE, Some(&REAL)),
        Err(SetupError::BadCookie)
    );
    let other = setup(true, b"XDM-AUTHORIZATION-1", &FAKE);
    assert_eq!(
        substitute(&other, &FAKE, Some(&REAL)),
        Err(SetupError::BadCookie)
    );
    let mut garbage = setup(true, MIT_COOKIE.as_bytes(), &FAKE);
    garbage[0] = b'X';
    assert_eq!(
        substitute(&garbage, &FAKE, None),
        Err(SetupError::Malformed)
    );
    assert_eq!(
        substitute(&garbage[..5], &FAKE, None),
        Err(SetupError::Malformed)
    );
}

#[tokio::test]
async fn read_setup_reads_exactly_one_setup() {
    let mut bytes = setup(false, MIT_COOKIE.as_bytes(), &FAKE);
    let len = bytes.len();
    bytes.extend_from_slice(b"next request");
    let mut reader = &bytes[..];
    let got = read_setup(&mut reader).await.unwrap();
    assert_eq!(got.len(), len);
    assert_eq!(reader, b"next request");
    let mut truncated = &bytes[..20];
    assert_eq!(read_setup(&mut truncated).await, Err(SetupError::Malformed));
}

#[test]
fn fake_cookie_hex_is_32_lowercase_digits() {
    let x11 = X11Fwd {
        display: Display::parse(":0").unwrap(),
        fake: [0xAB; 16],
    };
    assert_eq!(x11.fake_hex(), "ab".repeat(16));
}

#[cfg(target_os = "macos")]
#[test]
fn xquartz_display_is_its_socket_and_gets_the_local_cookie() {
    let text = "/private/tmp/com.apple.launchd.AbC/org.xquartz:0";
    let display = Display::parse(text).unwrap();
    assert!(display.is_socket() && display.is_local());
    assert_eq!(display.socket_path(), text);
    let file = [
        record(FAMILY_LOCAL, b"other", "0", MIT_COOKIE, b"wrong"),
        record(FAMILY_LOCAL, b"mac", "0", MIT_COOKIE, b"right"),
    ]
    .concat();
    let cookie = find_cookie(&parse_xauthority(&file), &display, "mac");
    assert_eq!(cookie.as_deref(), Some(&b"right"[..]));
    assert_eq!(
        Display::parse(":1").unwrap().socket_path(),
        "/tmp/.X11-unix/X1"
    );
}
