//! Finding an SSH agent when Fogu is started from the desktop menu (no `SSH_AUTH_SOCK`), and
//! a private agent for RSA keys when there is none: RSA signatures are always made by an OpenSSH
//! agent process, never in Fogu (no `rsa` crate, RUSTSEC-2023-0071).

use std::ffi::OsString;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use russh::keys::agent::client::AgentClient;
use tokio::net::UnixStream;

/// How long one candidate socket may take to accept.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// How long a freshly started private agent may take to create its socket.
const START_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) type Agent = AgentClient<UnixStream>;

/// Agent sockets to try, in order: `$SSH_AUTH_SOCK`, systemd's `ssh-agent.socket`, gcr's agent,
/// then the sockets OpenSSH ≥ 10.1 keeps in `~/.ssh/agent/` (newest first). `dir_sockets` are
/// those, already filtered by [`owned_sockets`].
pub(crate) fn candidates(
    env: Option<OsString>,
    runtime_dir: Option<&Path>,
    dir_sockets: Vec<PathBuf>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = env
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if let Some(run) = runtime_dir {
        out.push(run.join("ssh-agent.socket"));
        out.push(run.join("gcr/ssh"));
    }
    out.extend(dir_sockets);
    out.dedup();
    out
}

/// Sockets in `dir` (newest first) when `dir` and each socket belong to `uid` and `dir` is not
/// writable by others: another user must not be able to plant an agent we would trust.
pub(crate) fn owned_sockets(dir: &Path, uid: u32) -> Vec<PathBuf> {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return Vec::new();
    };
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o022 != 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            (m.file_type().is_socket() && m.uid() == uid)
                .then(|| (m.modified().unwrap_or(std::time::UNIX_EPOCH), e.path()))
        })
        .collect();
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found.into_iter().map(|(_, p)| p).collect()
}

/// The configured agent (`agent_path`), else the first live candidate.
pub(crate) async fn connect(configured: Option<&Path>) -> Option<Agent> {
    find(configured).await.map(|(_, agent)| agent)
}

/// [`connect`], also returning the socket path (agent forwarding connects to it per channel).
pub(crate) async fn find(configured: Option<&Path>) -> Option<(PathBuf, Agent)> {
    if let Some(path) = configured {
        return try_connect(path)
            .await
            .map(|agent| (path.to_owned(), agent));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let uid = home
        .as_deref()
        .and_then(|h| std::fs::metadata(h).ok())
        .map(|m| m.uid());
    let dir_sockets = match (&home, uid) {
        (Some(home), Some(uid)) => owned_sockets(&home.join(".ssh/agent"), uid),
        _ => Vec::new(),
    };
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    for path in candidates(
        std::env::var_os("SSH_AUTH_SOCK"),
        runtime.as_deref(),
        dir_sockets,
    ) {
        if let Some(agent) = try_connect(&path).await {
            return Some((path, agent));
        }
    }
    None
}

/// Serves one `auth-agent@openssh.com` channel: a fresh connection to the agent at `socket`,
/// then [`pump`](super::forward::pump). The channel is accepted only once the agent answered.
pub(crate) async fn serve_forward(
    channel: russh::Channel<russh::client::Msg>,
    reply: russh::client::ChannelOpenHandle,
    socket: &Path,
) {
    match tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(socket)).await {
        Ok(Ok(mut agent)) => {
            reply.accept().await;
            let mut stream = channel.into_stream();
            let _totals = super::forward::pump(&mut stream, &mut agent).await;
        }
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "forwarded agent request refused: agent unreachable");
        }
        Err(_elapsed) => tracing::warn!("forwarded agent request refused: agent timed out"),
    }
}

async fn try_connect(path: &Path) -> Option<Agent> {
    match tokio::time::timeout(CONNECT_TIMEOUT, AgentClient::connect_uds(path)).await {
        Ok(Ok(agent)) => {
            tracing::info!(path = %path.display(), "using SSH agent");
            Some(agent)
        }
        Ok(Err(err)) => {
            tracing::debug!(path = %path.display(), error = %err, "no SSH agent here");
            None
        }
        Err(_elapsed) => None,
    }
}

/// A discovered agent, else Fogu's own `ssh-agent` (started once, stops when Fogu exits).
pub(crate) async fn connect_or_start(configured: Option<&Path>) -> Option<Agent> {
    static PRIVATE: OnceLock<Option<PathBuf>> = OnceLock::new();
    if let Some(agent) = connect(configured).await {
        return Some(agent);
    }
    if configured.is_some() {
        return None;
    }
    let socket = PRIVATE.get_or_init(start_private).clone()?;
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Some(agent) = try_connect(&socket).await {
            return Some(agent);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tracing::warn!("private ssh-agent did not start");
    None
}

/// macOS stand-in for `tail --pid` (GNU-only; BSD tail exits at once and takes the agent with
/// it): lives while the pid in `$1` does.
const WAIT_FOR_PID: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 5; done"#;

/// Starts `ssh-agent -a <socket> tail --pid=<us> -f /dev/null` ([`WAIT_FOR_PID`] on macOS):
/// OpenSSH's agent runs the command as its child and exits with it, so it never outlives Fogu.
fn start_private() -> Option<PathBuf> {
    // Never under a shared /tmp: another user could pre-create the directory and swap the
    // socket before a decrypted RSA key is handed to it. macOS has no XDG_RUNTIME_DIR, but its
    // TMPDIR is a per-user 0700 directory under /var/folders.
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .or_else(|| std::env::var_os("TMPDIR").filter(|_| cfg!(target_os = "macos")));
    let Some(base) = base.map(PathBuf::from) else {
        tracing::warn!("no XDG_RUNTIME_DIR: not starting a private ssh-agent");
        return None;
    };
    let dir = base.join("fogu");
    if let Err(err) = fogu_core::fsutil::create_private_dir(&dir) {
        tracing::warn!(error = %err, "cannot create the private agent directory");
        return None;
    }
    let ours = std::fs::symlink_metadata(&dir).is_ok_and(|m| {
        m.is_dir()
            && m.uid() == rustix::process::getuid().as_raw()
            && m.permissions().mode() & 0o777 == 0o700
    });
    if !ours {
        tracing::warn!(path = %dir.display(), "private agent directory is not ours or not 0700");
        return None;
    }
    let socket = dir.join(format!("agent-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let pid = std::process::id().to_string();
    let tail_pid = format!("--pid={pid}");
    let keep_alive: &[&str] = if cfg!(target_os = "macos") {
        &["sh", "-c", WAIT_FOR_PID, "_", &pid]
    } else {
        &["tail", &tail_pid, "-f", "/dev/null"]
    };
    let spawned = std::process::Command::new("ssh-agent")
        .arg("-a")
        .arg(&socket)
        .args(keep_alive)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(_child) => {
            tracing::info!(path = %socket.display(), "started a private ssh-agent");
            Some(socket)
        }
        Err(err) => {
            tracing::warn!(error = %err, "cannot start ssh-agent");
            None
        }
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
