//! The byte stream SSH runs over: a TCP connection, a `direct-tcpip` channel of a jump host's
//! connection (same connect + host-key + auth code path, recursively), a proxy command's stdio, or
//! a TCP connection through a SOCKS5 / HTTP CONNECT proxy.
//!
//! Never a silent fallback: a route the profile asks for is followed exactly or the connection
//! fails. Proxy command tokens are substituted from the profile only (never server data) and
//! shell-quoted; its stderr is logged sanitized and only its last line is shown.

use std::net::IpAddr;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fogu_core::{EffectiveSsh, Jump, MAX_JUMP_HOPS};
#[cfg(unix)]
use rustix::process::{Pid, Signal};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::timeout;

use super::connect::{self, Connection};
use super::proxy::{http, socks5};
use super::{SshPaths, Stop, Ui, auth};
use crate::SessionError;

/// Any stream russh can run over.
pub(crate) trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// What keeps a transport alive besides the stream: the jump host's connection (itself with its
/// own route) or the proxy command. Dropped after the connection that uses it.
#[derive(Default)]
pub(crate) struct Route {
    hop: Option<Box<Connection>>,
    child: Option<ProxyChild>,
}

impl Route {
    /// Disconnects the jump hosts, innermost first (the connection over them is already
    /// closed); a proxy command is killed when dropped.
    pub(crate) async fn close(mut self) {
        while let Some(hop) = self.hop.take() {
            let Connection { handle, route, .. } = *hop;
            let _bye = handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
            self = route;
        }
    }

    /// When the proxy command has ended (waits up to [`PROXY_EXIT_WAIT`] for it): why the
    /// transport broke, with the last line it wrote to stderr.
    pub(crate) async fn proxy_failure(&mut self) -> Option<SessionError> {
        let proxy = self.child.as_mut()?;
        let status = timeout(PROXY_EXIT_WAIT, proxy.child.wait())
            .await
            .ok()?
            .ok()?;
        if let Some(task) = proxy.stderr_task.take() {
            let _late = timeout(PROXY_EXIT_WAIT, task).await;
        }
        let last = proxy.stderr.lock().map(|s| s.clone()).unwrap_or_default();
        let detail = if last.is_empty() {
            String::new()
        } else {
            format!(": {last}")
        };
        Some(SessionError::Proxy(format!(
            "the proxy command ended ({status}){detail}"
        )))
    }
}

/// How long a failed handshake waits for the proxy command to end (and its stderr to drain).
const PROXY_EXIT_WAIT: Duration = Duration::from_millis(500);

/// A running proxy command; its process group is killed when dropped.
struct ProxyChild {
    child: Child,
    /// Last line it wrote to stderr (sanitized, capped).
    stderr: Arc<Mutex<String>>,
    /// The task reading stderr (ends at EOF).
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for ProxyChild {
    fn drop(&mut self) {
        // Only while it still runs: after it was reaped its pid (= process group) may be reused.
        if let Ok(None) = self.child.try_wait() {
            #[cfg(unix)]
            {
                if let Some(pid) = self
                    .child
                    .id()
                    .and_then(|id| Pid::from_raw(id.cast_signed()))
                    && let Err(err) = rustix::process::kill_process_group(pid, Signal::KILL)
                {
                    tracing::debug!(error = %err, "could not kill the proxy command");
                }
            }
            #[cfg(windows)]
            {
                if let Err(err) = self.child.start_kill() {
                    tracing::debug!(error = %err, "could not kill the proxy command");
                }
            }
        }
    }
}

/// " via …" for the "Connecting to host:port…" line (empty for a direct connection).
pub(crate) fn describe(ssh: &EffectiveSsh) -> String {
    if let Some(cmd) = &ssh.proxy_command {
        format!(" via proxy command `{cmd}`")
    } else if let Jump::Via(hop) = &ssh.jump {
        format!(" via jump host {}", hop_label(hop))
    } else if let Some((host, port)) = &ssh.socks_proxy {
        format!(" via proxy socks5://{host}:{port}")
    } else if let Some((host, port)) = &ssh.http_proxy {
        format!(" via proxy http://{host}:{port}")
    } else {
        String::new()
    }
}

/// `user@host:port` of a jump host.
pub(crate) fn hop_label(hop: &EffectiveSsh) -> String {
    crate::sanitize_remote(
        &format!("{}@{}:{}", hop.user, hop.host, hop.port),
        200,
        false,
    )
}

/// Opens the stream to `ssh.host:ssh.port` along the profile's route. `depth` = jump hosts
/// already on the way (bounded by [`MAX_JUMP_HOPS`]).
pub(crate) async fn open(
    ssh: &EffectiveSsh,
    paths: &SshPaths,
    ui: &mut Ui,
    ready: Duration,
    depth: usize,
) -> Result<(Box<dyn Stream>, Route), Stop> {
    if let Some(template) = &ssh.proxy_command {
        let (stream, child) = spawn_proxy(template, ssh)?;
        let route = Route {
            hop: None,
            child: Some(child),
        };
        return Ok((stream, route));
    }
    if let Jump::Via(hop) = &ssh.jump {
        return via_jump(hop, ssh, paths, ui, (ready, depth)).await;
    }
    let stream: Box<dyn Stream> = if let Some((host, port)) = &ssh.socks_proxy {
        let mut tcp = proxy_tcp("SOCKS5", host, *port, ready).await?;
        let target = socks_target(ssh, ready).await?;
        let shake = timeout(ready, socks5::connect(&mut tcp, &target));
        proxy_result("SOCKS5", host, *port, shake.await)?;
        Box::new(tcp)
    } else if let Some((host, port)) = &ssh.http_proxy {
        let mut tcp = proxy_tcp("HTTP", host, *port, ready).await?;
        let shake = timeout(ready, http::connect(&mut tcp, &ssh.host, ssh.port));
        proxy_result("HTTP", host, *port, shake.await)?;
        Box::new(tcp)
    } else {
        Box::new(tcp(&ssh.host, ssh.port, ready).await.map_err(|err| {
            err.unwrap_or_else(|source| SessionError::Connect {
                target: crate::sanitize_remote(&format!("{}:{}", ssh.host, ssh.port), 300, false),
                source: Arc::new(source),
            })
        })?)
    };
    Ok((stream, Route::default()))
}

/// TCP connect within `ready`, `TCP_NODELAY` on. `Err(Ok(Timeout))` or `Err(Err(io))`.
async fn tcp(
    host: &str,
    port: u16,
    ready: Duration,
) -> Result<TcpStream, Result<SessionError, std::io::Error>> {
    let stream = timeout(ready, TcpStream::connect((host, port)))
        .await
        .map_err(|_| Ok(SessionError::Timeout))?
        .map_err(Err)?;
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!(error = %err, "TCP_NODELAY failed");
    }
    Ok(stream)
}

async fn proxy_tcp(
    kind: &str,
    host: &str,
    port: u16,
    ready: Duration,
) -> Result<TcpStream, SessionError> {
    tcp(host, port, ready).await.map_err(|err| match err {
        Ok(timed_out) => timed_out,
        Err(io) => proxy_error(kind, host, port, &format!("could not connect: {io}")),
    })
}

fn proxy_result(
    kind: &str,
    host: &str,
    port: u16,
    shake: Result<Result<(), String>, tokio::time::error::Elapsed>,
) -> Result<(), SessionError> {
    match shake {
        Err(_) => Err(SessionError::Timeout),
        Ok(Err(why)) => Err(proxy_error(kind, host, port, &why)),
        Ok(Ok(())) => Ok(()),
    }
}

fn proxy_error(kind: &str, host: &str, port: u16, why: &str) -> SessionError {
    let text = format!("{kind} proxy {host}:{port}: {why}");
    SessionError::Proxy(crate::sanitize_remote(&text, 300, false))
}

/// The SOCKS5 target: the name (the proxy resolves it) unless `socks_proxy_remote_dns` is off;
/// IP literals are always sent as addresses.
async fn socks_target(ssh: &EffectiveSsh, ready: Duration) -> Result<socks5::Target, SessionError> {
    if let Ok(ip) = ssh.host.parse::<IpAddr>() {
        return Ok(socks5::Target::Addr(ip, ssh.port));
    }
    if ssh.socks_remote_dns {
        return Ok(socks5::Target::Name(ssh.host.clone(), ssh.port));
    }
    let found = timeout(
        ready,
        tokio::net::lookup_host((ssh.host.as_str(), ssh.port)),
    )
    .await
    .map_err(|_| SessionError::Timeout)?
    .map_err(|source| SessionError::Connect {
        target: crate::sanitize_remote(&format!("{}:{}", ssh.host, ssh.port), 300, false),
        source: Arc::new(source),
    })?
    .next();
    let addr = found.ok_or_else(|| SessionError::Connect {
        target: crate::sanitize_remote(&ssh.host, 300, false),
        source: Arc::new(std::io::Error::from(std::io::ErrorKind::NotFound)),
    })?;
    Ok(socks5::Target::Addr(addr.ip(), ssh.port))
}

/// Connects and logs in to `hop` (the same code path as any profile, so its host key is checked
/// against `known_hosts` with the same prompts, labelled with the hop), then opens a
/// `direct-tcpip` channel to `target`.
async fn via_jump(
    hop: &EffectiveSsh,
    target: &EffectiveSsh,
    paths: &SshPaths,
    ui: &mut Ui,
    (ready, depth): (Duration, usize),
) -> Result<(Box<dyn Stream>, Route), Stop> {
    let label = hop_label(hop);
    if depth >= MAX_JUMP_HOPS {
        let why = format!("more than {MAX_JUMP_HOPS} jump hosts");
        return Err(SessionError::Route(why).into());
    }
    let outer = ui.via.replace(label.clone());
    let result = async {
        let mut conn = connect::connect_boxed(hop, paths, ui, ready, depth + 1).await?;
        auth::authenticate(&mut conn, hop, ui, ready).await?;
        conn.drain_events(hop, ui).await?;
        let open = conn.handle.channel_open_direct_tcpip(
            target.host.clone(),
            u32::from(target.port),
            "127.0.0.1",
            0,
        );
        let channel = timeout(ready, open)
            .await
            .map_err(|_| SessionError::Timeout)?
            .map_err(|err| {
                connect::ssh_error(&format!(
                    "could not open a tunnel to {}:{}: {err}",
                    target.host, target.port
                ))
            })?;
        let stream: Box<dyn Stream> = Box::new(channel.into_stream());
        let route = Route {
            hop: Some(Box::new(conn)),
            child: None,
        };
        Ok((stream, route))
    }
    .await;
    ui.via = outer;
    result.map_err(|stop| match stop {
        Stop::Failed(error) => Stop::Failed(SessionError::Jump {
            hop: label,
            error: Box::new(error),
        }),
        other => other,
    })
}

/// Starts the proxy command through the user's shell; its stdin/stdout are the stream.
fn spawn_proxy(
    template: &str,
    ssh: &EffectiveSsh,
) -> Result<(Box<dyn Stream>, ProxyChild), SessionError> {
    let command =
        fogu_core::shellquote::expand_proxy_command(template, &ssh.host, ssh.port, &ssh.user)
            .map_err(|why| SessionError::Proxy(crate::sanitize_remote(&why, 300, false)))?;
    let mut process = {
        #[cfg(unix)]
        {
            let shell = std::env::var_os("SHELL")
                .filter(|s| std::path::Path::new(s).is_absolute())
                .unwrap_or_else(|| "/bin/sh".into());
            // Inherits Fogu's environment: no `WAYLAND_DISPLAY` when Fogu runs on XWayland, which
            // only matters for a GUI askpass/pinentry started by the command.
            let mut process = Command::new(shell);
            process
                .arg("-c")
                .arg(format!("exec {command}"))
                .process_group(0);
            process
        }
        #[cfg(windows)]
        {
            // Windows has no user shell variable with POSIX `-c` semantics. `cmd /S /C`
            // understands the same redirections and pipes; host/user substitution is already
            // restricted to characters with no special meaning. The command line is passed raw:
            // std would quote it and escape inner `"` as `\"`, which cmd does not understand.
            // With `/S`, cmd strips exactly the outer pair of quotes and runs the rest as typed.
            let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
            let mut process = Command::new(shell);
            process.raw_arg(format!("/S /C \"{command}\""));
            process
        }
    };
    let mut child = process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| SessionError::Proxy(format!("could not start the proxy command: {err}")))?;
    let (Some(stdout), Some(stdin), Some(stderr)) =
        (child.stdout.take(), child.stdin.take(), child.stderr.take())
    else {
        return Err(SessionError::Proxy("proxy command has no pipes".to_owned()));
    };
    let last = Arc::new(Mutex::new(String::new()));
    let stderr_task = tokio::spawn(log_stderr(stderr, Arc::clone(&last)));
    let stream: Box<dyn Stream> = Box::new(tokio::io::join(stdout, stdin));
    Ok((
        stream,
        ProxyChild {
            child,
            stderr: last,
            stderr_task: Some(stderr_task),
        },
    ))
}

/// Lines the proxy command's stderr gets logged (the rest is read and dropped).
const STDERR_LOG_LINES: usize = 50;
/// Longest stderr line kept, in bytes (the rest of the line is dropped).
const STDERR_LINE_MAX: usize = 512;

/// Logs the proxy command's stderr lines (sanitized, capped) and keeps the last one.
async fn log_stderr(mut stderr: tokio::process::ChildStderr, last: Arc<Mutex<String>>) {
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::new();
    let mut logged = 0;
    let mut finish = |line: &mut Vec<u8>| {
        let text = crate::sanitize_remote(&String::from_utf8_lossy(line), 200, false);
        line.clear();
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if logged < STDERR_LOG_LINES {
            logged += 1;
            tracing::warn!(line = %text, "proxy command stderr");
        }
        if let Ok(mut keep) = last.lock() {
            *keep = text;
        }
    };
    while let Ok(n) = stderr.read(&mut buf).await {
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            if byte == b'\n' {
                finish(&mut line);
            } else if line.len() < STDERR_LINE_MAX {
                line.push(byte);
            }
        }
    }
    finish(&mut line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_names_the_route() {
        let mut ssh = EffectiveSsh::new("t");
        assert_eq!(describe(&ssh), "");
        ssh.socks_proxy = Some(("p".into(), 1080));
        assert_eq!(describe(&ssh), " via proxy socks5://p:1080");
        let mut hop = EffectiveSsh::new("gw\x1b[31m");
        hop.user = "ops".into();
        ssh.jump = Jump::Via(Box::new(hop));
        assert_eq!(describe(&ssh), " via jump host ops@gw[31m:22");
    }
}
