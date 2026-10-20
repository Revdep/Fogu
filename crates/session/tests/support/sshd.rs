//! A throwaway OpenSSH `sshd` (and `ssh-agent`) for the SSH integration tests.
//!
//! Everything is hermetic: a private temp dir holds the host key, the client keys, the
//! `authorized_keys`, the config, the logs and the `known_hosts` the session under test uses; the
//! daemon listens on a free port of 127.0.0.1 and runs as the *current user* (no root, no sudo,
//! no system user is created: key auth only; password and keyboard-interactive need PAM or
//! `/etc/shadow` and are covered by [`super::fake_server`] instead). The daemon's whole process
//! group is killed on drop, so a panicking test leaves nothing behind.
//!
//! Missing binaries, or an `sshd` that will not start, are a *skip*: a clear line on stderr and
//! `None`. With `FOGU_REQUIRE_SSHD=1` (CI) a skip is a failure instead.

use std::fmt::Write as _;
use std::fs;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fogu_core::{EffectiveSsh, SshAuth};
use fogu_session::SshPaths;
use rustix::process::{Pid, Signal};

use super::tmp::TempDir;

/// Set to a non-empty value other than `0` to turn "sshd not available" into a failure.
pub const REQUIRE_ENV: &str = "FOGU_REQUIRE_SSHD";
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const SFTP_SERVERS: [&str; 4] = [
    "/usr/lib/openssh/sftp-server",
    "/usr/lib/ssh/sftp-server",
    "/usr/libexec/openssh/sftp-server",
    "/usr/libexec/sftp-server",
];

/// Reports a skipped test (stderr), or panics when [`REQUIRE_ENV`] demands the test to run.
pub fn skip(reason: &str) {
    assert!(
        !std::env::var_os(REQUIRE_ENV).is_some_and(|v| !v.is_empty() && v != "0"),
        "{REQUIRE_ENV} is set but the SSH test cannot run: {reason}"
    );
    eprintln!("SKIPPED (SSH test): {reason}; set {REQUIRE_ENV}=1 to make this an error");
}

/// An OpenSSH tool by name: `$PATH` first, then the usual sbin directories. Absolute, because
/// `sshd` insists on being started by its absolute path.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(
            ["/usr/sbin", "/usr/local/sbin", "/sbin", "/usr/bin"]
                .into_iter()
                .map(PathBuf::from),
        )
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// [`find_tool`] or a skip.
pub fn require_tools(names: &[&str]) -> Option<Vec<PathBuf>> {
    let mut found = Vec::new();
    for name in names {
        let Some(path) = find_tool(name) else {
            skip(&format!(
                "`{name}` not found (install openssh-server / openssh-client)"
            ));
            return None;
        };
        found.push(path);
    }
    Some(found)
}

/// Kills a child's whole process group (and the child) on drop.
struct Reaper(Child);

impl Reaper {
    fn spawn(command: &mut Command) -> std::io::Result<Self> {
        command.process_group(0).spawn().map(Self)
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Some(pid) = i32::try_from(self.0.id()).ok().and_then(Pid::from_raw) {
            let _already_gone = rustix::process::kill_process_group(pid, Signal::KILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Server-side knobs of [`TestSshd::start_with`].
#[derive(Default)]
pub struct Options {
    /// Pre-authentication banner (`Banner`), sent verbatim.
    pub banner: Option<Vec<u8>>,
}

/// A running `sshd` on 127.0.0.1.
pub struct TestSshd {
    _daemon: Reaper,
    /// The daemon's private directory (keys, config, logs, `known_hosts`).
    pub dir: TempDir,
    pub port: u16,
    /// The account the daemon runs as: the only user it can authenticate.
    pub user: String,
    /// Unencrypted ed25519 client key, authorized.
    pub client_key: PathBuf,
    /// `ssh-ed25519 AAAA…` of the host key (no comment).
    pub host_pub: String,
    keygen: PathBuf,
}

fn run_ok(command: &mut Command) -> String {
    let out = command.output().expect("run command");
    assert!(
        out.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn free_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind a free port");
    listener.local_addr().expect("local addr").port()
}

impl TestSshd {
    /// [`Self::start_with`] defaults.
    pub fn start() -> Option<Self> {
        Self::start_with(&Options::default())
    }

    /// Starts the daemon, or skips (see the module docs).
    pub fn start_with(options: &Options) -> Option<Self> {
        let tools = require_tools(&["sshd", "ssh-keygen"])?;
        let (sshd, keygen) = (&tools[0], tools[1].clone());
        let dir = TempDir::new("sshd");
        let host_key = dir.join("host_ed25519");
        let client_key = dir.join("client_ed25519");
        for key in [&host_key, &client_key] {
            keygen_new(&keygen, key, "ed25519", "");
        }
        let host_pub = pub_without_comment(&dir.join("host_ed25519.pub"));
        fs::copy(dir.join("client_ed25519.pub"), dir.join("authorized_keys")).expect("authorize");
        if let Some(banner) = &options.banner {
            fs::write(dir.join("banner"), banner).expect("write banner");
        }
        // sshd as root insists on its privilege-separation directory.
        let _already_there = fs::create_dir_all("/run/sshd");
        let user = run_ok(Command::new("id").arg("-un"));
        for attempt in 1..=5 {
            let port = free_port();
            let config = config_text(&dir, port, options);
            fs::write(dir.join("sshd_config"), config).expect("write sshd_config");
            let log = fs::File::create(dir.join("sshd.log")).expect("sshd log");
            let mut command = Command::new(sshd);
            command
                .args(["-D", "-e", "-f"])
                .arg(dir.join("sshd_config"))
                // Nothing of the real user's configuration leaks into the daemon.
                .env("HOME", dir.path())
                .env("XDG_CONFIG_HOME", dir.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log);
            let mut daemon = Reaper::spawn(&mut command).expect("spawn sshd");
            match wait_listening(&mut daemon.0, port) {
                Ok(()) => {
                    return Some(Self {
                        _daemon: daemon,
                        dir,
                        port,
                        user,
                        client_key,
                        host_pub,
                        keygen,
                    });
                }
                Err(why) => eprintln!("sshd start attempt {attempt} on port {port} failed: {why}"),
            }
        }
        // A skip (a failure under `FOGU_REQUIRE_SSHD`): some hosts, e.g. macOS CI runners, have
        // an `sshd` that will not run unprivileged.
        skip(&format!(
            "sshd did not start; log:\n{}",
            fs::read_to_string(dir.join("sshd.log")).unwrap_or_default()
        ));
        None
    }

    /// A profile for this daemon: key auth with [`Self::client_key`], nothing else configured.
    pub fn profile(&self) -> EffectiveSsh {
        let mut ssh = EffectiveSsh::new("127.0.0.1");
        ssh.port = self.port;
        ssh.user.clone_from(&self.user);
        ssh.auth = SshAuth::PublicKey;
        ssh.private_keys = vec![self.client_key.clone()];
        ssh.ready_timeout = Some(Duration::from_secs(10));
        ssh.keepalive_interval = None;
        ssh
    }

    /// The `known_hosts` the session under test reads and writes (nothing else is consulted).
    pub fn paths(&self) -> SshPaths {
        SshPaths {
            known_hosts: self.known_hosts(),
            system_known_hosts: None,
        }
    }

    pub fn known_hosts(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }

    /// Writes [`Self::known_hosts_line`] into the session's `known_hosts`: the host is Known.
    pub fn trust(&self) {
        fs::write(self.known_hosts(), self.known_hosts_line()).expect("write known_hosts");
    }

    /// The `known_hosts` line that trusts this daemon's real host key.
    pub fn known_hosts_line(&self) -> String {
        format!("[127.0.0.1]:{} {}\n", self.port, self.host_pub)
    }

    /// `SHA256:…` of the host key as `ssh-keygen -l` prints it (an independent computation).
    pub fn host_key_fingerprint(&self) -> String {
        let out = run_ok(
            Command::new(&self.keygen)
                .arg("-lf")
                .arg(self.dir.join("host_ed25519.pub")),
        );
        out.split_whitespace()
            .nth(1)
            .expect("fingerprint field")
            .to_owned()
    }

    /// Generates a key pair named `name` in the daemon's directory (`kind` is an `ssh-keygen -t`
    /// type) and, when `authorize`, lets it log in. Returns the private key path.
    pub fn new_key(&self, name: &str, kind: &str, passphrase: &str, authorize: bool) -> PathBuf {
        let path = self.dir.join(name);
        keygen_new(&self.keygen, &path, kind, passphrase);
        if authorize {
            let line = fs::read_to_string(self.dir.join(&format!("{name}.pub"))).expect("read pub");
            let file = self.dir.join("authorized_keys");
            let mut all = fs::read_to_string(&file).unwrap_or_default();
            all.push_str(&line);
            fs::write(file, all).expect("authorize key");
        }
        path
    }

    /// The `ssh-keygen` binary, for tests that need other key material.
    pub fn keygen(&self) -> &Path {
        &self.keygen
    }
}

fn keygen_new(keygen: &Path, path: &Path, kind: &str, passphrase: &str) {
    let mut command = Command::new(keygen);
    command.args(["-q", "-t", kind, "-C", "fogu-test", "-N", passphrase, "-f"]);
    command.arg(path);
    if kind == "rsa" {
        command.args(["-b", "2048"]);
    }
    run_ok(&mut command);
}

/// `ssh-ed25519 AAAA…` without the trailing comment.
pub fn pub_without_comment(path: &Path) -> String {
    let text = fs::read_to_string(path).expect("read public key");
    text.split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

fn config_text(dir: &TempDir, port: u16, options: &Options) -> String {
    let d = dir.path().display();
    let mut config = format!(
        "ListenAddress 127.0.0.1\nPort {port}\nHostKey {d}/host_ed25519\nPidFile {d}/sshd.pid\n\
         AuthorizedKeysFile {d}/authorized_keys\nPasswordAuthentication no\n\
         KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nUsePAM no\nStrictModes no\n\
         PermitRootLogin prohibit-password\nPrintMotd no\nPrintLastLog no\nLogLevel ERROR\n\
         AllowTcpForwarding yes\n"
    );
    if options.banner.is_some() {
        let _infallible = writeln!(config, "Banner {d}/banner");
    }
    if let Some(sftp) = SFTP_SERVERS.iter().find(|p| Path::new(p).is_file()) {
        let _infallible = writeln!(config, "Subsystem sftp {sftp}");
    }
    config
}

/// Waits until `port` accepts connections; fails early when the daemon exits.
fn wait_listening(daemon: &mut Child, port: u16) -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(status) = daemon.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("exited early: {status}"));
        }
        if TcpStream::connect_timeout(
            &(Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("not listening in time".to_owned())
}

/// A private `ssh-agent` holding keys added with [`Self::add`].
pub struct TestAgent {
    _proc: Reaper,
    pub socket: PathBuf,
    ssh_add: PathBuf,
}

impl TestAgent {
    /// Starts `ssh-agent` with its socket in `dir`, or skips.
    pub fn start(dir: &TempDir) -> Option<Self> {
        let tools = require_tools(&["ssh-agent", "ssh-add"])?;
        let socket = dir.join("agent.sock");
        let mut command = Command::new(&tools[0]);
        command
            .arg("-D")
            .arg("-a")
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let proc = Reaper::spawn(&mut command).expect("spawn ssh-agent");
        let deadline = Instant::now() + READY_TIMEOUT;
        while !socket.exists() {
            assert!(Instant::now() < deadline, "ssh-agent socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
        Some(Self {
            _proc: proc,
            socket,
            ssh_add: tools[1].clone(),
        })
    }

    /// `ssh-add key` (an unencrypted private key).
    pub fn add(&self, key: &Path) {
        run_ok(
            Command::new(&self.ssh_add)
                .env("SSH_AUTH_SOCK", &self.socket)
                .arg(key),
        );
    }
}
