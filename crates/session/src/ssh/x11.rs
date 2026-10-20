//! X11 forwarding, the OpenSSH model: the server gets a random fake `MIT-MAGIC-COOKIE-1`; each
//! `x11` channel it opens must start with an X11 connection setup carrying that fake cookie, which
//! is replaced by the real one (from `$XAUTHORITY` / `~/.Xauthority`) before the bytes reach the
//! local display. A wrong cookie closes the channel. The real cookie never leaves this machine; a
//! forwarded X client can still read the local display's input (no SECURITY extension).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpStream, UnixStream};

use super::forward::pump;

/// The only auth protocol we forward.
pub(crate) const MIT_COOKIE: &str = "MIT-MAGIC-COOKIE-1";
/// Xauthority family codes we use.
const FAMILY_INTERNET: u16 = 0;
const FAMILY_LOCAL: u16 = 256;
const FAMILY_WILD: u16 = 65535;
/// How long a remote X client may take to send its connection setup.
const SETUP_DEADLINE: Duration = Duration::from_secs(10);
/// How long the local display may take to accept.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// A parsed `$DISPLAY`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Display {
    /// Empty or `unix` = the local socket; anything else = TCP to that host.
    pub(crate) host: String,
    pub(crate) number: u16,
    pub(crate) screen: u32,
}

impl Display {
    /// `:N[.S]`, `unix:N[.S]` or `host:N[.S]`; `None` for anything else.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (host, rest) = text.rsplit_once(':')?;
        let (number, screen) = rest.split_once('.').unwrap_or((rest, "0"));
        let host = host.trim_start_matches('[').trim_end_matches(']');
        Some(Self {
            host: if host == "unix" {
                String::new()
            } else {
                host.to_owned()
            },
            number: number.parse().ok()?,
            screen: screen.parse().ok()?,
        })
    }

    /// macOS (`XQuartz`): `$DISPLAY` is the socket path itself (`/private/tmp/…/org.xquartz:0`).
    fn is_path(&self) -> bool {
        cfg!(target_os = "macos") && self.host.starts_with('/')
    }

    fn is_socket(&self) -> bool {
        self.host.is_empty() || self.is_path()
    }

    fn is_local(&self) -> bool {
        self.is_socket() || self.host == "localhost"
    }

    /// The Unix socket of a local display.
    fn socket_path(&self) -> String {
        if self.is_path() {
            format!("{}:{}", self.host, self.number)
        } else {
            format!("/tmp/.X11-unix/X{}", self.number)
        }
    }
}

/// One `.Xauthority` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthEntry {
    pub(crate) family: u16,
    pub(crate) address: Vec<u8>,
    pub(crate) number: Vec<u8>,
    pub(crate) name: Vec<u8>,
    pub(crate) data: Vec<u8>,
}

/// Parses an `.Xauthority` file (big-endian `u16` family, then four `u16`-length-prefixed
/// fields per record). A truncated tail is ignored.
pub(crate) fn parse_xauthority(mut bytes: &[u8]) -> Vec<AuthEntry> {
    fn u16_be(bytes: &mut &[u8]) -> Option<u16> {
        let (head, rest) = bytes.split_first_chunk::<2>()?;
        *bytes = rest;
        Some(u16::from_be_bytes(*head))
    }
    fn field(bytes: &mut &[u8]) -> Option<Vec<u8>> {
        let len = usize::from(u16_be(bytes)?);
        let (head, rest) = bytes.split_at_checked(len)?;
        *bytes = rest;
        Some(head.to_vec())
    }
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let record = (|| {
            Some(AuthEntry {
                family: u16_be(&mut bytes)?,
                address: field(&mut bytes)?,
                number: field(&mut bytes)?,
                name: field(&mut bytes)?,
                data: field(&mut bytes)?,
            })
        })();
        match record {
            Some(entry) => out.push(entry),
            None => break,
        }
    }
    out
}

/// The `MIT-MAGIC-COOKIE-1` for `display`: a local display matches `FamilyLocal` records of
/// this host (or wildcard ones); a TCP display to an IPv4 literal matches that address. Other
/// hosts get none (the cookie is never sent to a server it wasn't made for).
pub(crate) fn find_cookie(
    entries: &[AuthEntry],
    display: &Display,
    hostname: &str,
) -> Option<Vec<u8>> {
    let number = display.number.to_string();
    let ipv4 = display.host.parse::<std::net::Ipv4Addr>().ok();
    entries
        .iter()
        .filter(|e| e.name == MIT_COOKIE.as_bytes() && e.number == number.as_bytes())
        .find(|e| match e.family {
            FAMILY_LOCAL => display.is_local() && e.address == hostname.as_bytes(),
            FAMILY_WILD => display.is_local(),
            FAMILY_INTERNET => ipv4.is_some_and(|ip| e.address == ip.octets()),
            _ => false,
        })
        .map(|e| e.data.clone())
}

/// The real cookie for `display` from `$XAUTHORITY` or `~/.Xauthority` (`None` = none found;
/// the setup is then forwarded without authentication).
async fn real_cookie(display: &Display) -> Option<Vec<u8>> {
    let path = std::env::var_os("XAUTHORITY")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".Xauthority")))?;
    let bytes = tokio::fs::read(&path).await.ok()?;
    let hostname = crate::pipeline::local_hostname();
    find_cookie(&parse_xauthority(&bytes), display, &hostname)
}

/// What the session sends with `x11-req` and checks on each `x11` channel.
#[derive(Clone, Debug)]
pub(crate) struct X11Fwd {
    pub(crate) display: Display,
    pub(crate) fake: [u8; 16],
}

impl X11Fwd {
    /// From `$DISPLAY` with a fresh random fake cookie; `Err` says why X11 can't be forwarded.
    pub(crate) fn from_env() -> Result<Self, &'static str> {
        let text = std::env::var("DISPLAY").map_err(|_| "DISPLAY is not set")?;
        let display = Display::parse(&text).ok_or("DISPLAY is not understood")?;
        let mut fake = [0_u8; 16];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut fake)
            .map_err(|_| "no randomness for the cookie")?;
        Ok(Self { display, fake })
    }

    /// The fake cookie as the hex text `x11-req` carries.
    pub(crate) fn fake_hex(&self) -> String {
        self.fake.iter().fold(String::new(), |mut hex, b| {
            let _infallible = write!(hex, "{b:02x}");
            hex
        })
    }
}

/// Why a setup was refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SetupError {
    /// Not an X11 setup (bad byte order byte, truncated).
    Malformed,
    /// Wrong protocol or cookie.
    BadCookie,
}

/// Reads the client's connection setup from `channel` (12-byte header, padded name and data).
async fn read_setup<R: AsyncRead + Unpin>(channel: &mut R) -> Result<Vec<u8>, SetupError> {
    let mut setup = vec![0_u8; 12];
    channel
        .read_exact(&mut setup)
        .await
        .map_err(|_| SetupError::Malformed)?;
    let (name_len, data_len) = lengths(&setup)?;
    let rest = pad4(name_len) + pad4(data_len);
    setup.resize(12 + rest, 0);
    let body = setup.get_mut(12..).ok_or(SetupError::Malformed)?;
    channel
        .read_exact(body)
        .await
        .map_err(|_| SetupError::Malformed)?;
    Ok(setup)
}

fn pad4(n: usize) -> usize {
    n.div_ceil(4) * 4
}

fn u16_at(setup: &[u8], at: usize, big: bool) -> Result<usize, SetupError> {
    let bytes: [u8; 2] = setup
        .get(at..at + 2)
        .and_then(|b| b.try_into().ok())
        .ok_or(SetupError::Malformed)?;
    Ok(usize::from(if big {
        u16::from_be_bytes(bytes)
    } else {
        u16::from_le_bytes(bytes)
    }))
}

fn big_endian(setup: &[u8]) -> Result<bool, SetupError> {
    match setup.first() {
        Some(b'B') => Ok(true),
        Some(b'l') => Ok(false),
        _ => Err(SetupError::Malformed),
    }
}

/// `(auth name length, auth data length)` from the header.
fn lengths(setup: &[u8]) -> Result<(usize, usize), SetupError> {
    let big = big_endian(setup)?;
    Ok((u16_at(setup, 6, big)?, u16_at(setup, 8, big)?))
}

/// Checks that `setup` carries `MIT-MAGIC-COOKIE-1` with `fake`, and returns it rewritten to
/// carry `real` instead (no auth at all when `real` is `None`), in the client's byte order.
pub(crate) fn substitute(
    setup: &[u8],
    fake: &[u8],
    real: Option<&[u8]>,
) -> Result<Vec<u8>, SetupError> {
    let big = big_endian(setup)?;
    let (name_len, data_len) = lengths(setup)?;
    let name = setup.get(12..12 + name_len).ok_or(SetupError::Malformed)?;
    let data_at = 12 + pad4(name_len);
    let data = setup
        .get(data_at..data_at + data_len)
        .ok_or(SetupError::Malformed)?;
    if name != MIT_COOKIE.as_bytes() || !same_secret(data, fake) {
        return Err(SetupError::BadCookie);
    }
    let (name, data): (&[u8], &[u8]) = match real {
        Some(real) => (MIT_COOKIE.as_bytes(), real),
        None => (&[], &[]),
    };
    let encode = |n: usize| {
        let n = u16::try_from(n).unwrap_or(u16::MAX);
        if big {
            n.to_be_bytes()
        } else {
            n.to_le_bytes()
        }
    };
    let mut out = setup.get(..6).ok_or(SetupError::Malformed)?.to_vec();
    out.extend_from_slice(&encode(name.len()));
    out.extend_from_slice(&encode(data.len()));
    out.extend_from_slice(&[0, 0]);
    for part in [name, data] {
        out.extend_from_slice(part);
        out.resize(pad4(out.len()), 0);
    }
    Ok(out)
}

/// Compares without an early exit on the first differing byte (the length is not secret).
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Serves one `x11` channel: setup check + cookie swap, then [`pump`] to the local display.
pub(crate) async fn serve<C>(mut channel: C, x11: &X11Fwd)
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    let real = real_cookie(&x11.display).await;
    let setup = match tokio::time::timeout(SETUP_DEADLINE, read_setup(&mut channel)).await {
        Ok(Ok(setup)) => setup,
        Ok(Err(err)) => return tracing::warn!(?err, "X11 channel refused"),
        Err(_elapsed) => return tracing::warn!("X11 channel refused: no setup in time"),
    };
    let setup = match substitute(&setup, &x11.fake, real.as_deref()) {
        Ok(setup) => setup,
        Err(err) => return tracing::warn!(?err, "X11 channel refused"),
    };
    let display = &x11.display;
    let connected = if display.is_socket() {
        let path = display.socket_path();
        match tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(path)).await {
            Ok(Ok(mut local)) => send_and_pump(&mut channel, &mut local, &setup).await,
            Ok(Err(err)) => Err(err),
            Err(_elapsed) => Err(std::io::ErrorKind::TimedOut.into()),
        }
    } else {
        let addr = (
            display.host.as_str(),
            6000_u16.saturating_add(display.number),
        );
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(mut local)) => send_and_pump(&mut channel, &mut local, &setup).await,
            Ok(Err(err)) => Err(err),
            Err(_elapsed) => Err(std::io::ErrorKind::TimedOut.into()),
        }
    };
    if let Err(err) = connected {
        tracing::debug!(error = %err, "X11 channel ended");
    }
}

async fn send_and_pump<C, L>(channel: &mut C, local: &mut L, setup: &[u8]) -> std::io::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
    L: AsyncRead + AsyncWrite + Unpin,
{
    local.write_all(setup).await?;
    pump(channel, local).await.map(|_| ())
}

#[cfg(test)]
#[path = "x11_tests.rs"]
mod tests;
