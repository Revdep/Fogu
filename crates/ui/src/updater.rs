//! Optional, bounded release checks and signed archive installation.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::Task;
use ring::{
    digest,
    rand::{SecureRandom as _, SystemRandom},
};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::message::Message;
use crate::notifications::{Level, Notification, ToastAction};

const MAX_ASSET: u64 = 128 * 1024 * 1024;
const MAX_EXTRACT: u64 = 512 * 1024 * 1024;
const LATEST: &str = "https://api.github.com/repos/Revdep/Fogu/releases/latest";
const DOWNLOADS: &str = "https://github.com/Revdep/Fogu/releases/download";
const DAY: u64 = 24 * 3600;
type CheckedRelease = Option<Box<(Release, Policy, bool)>>;
type Result<T> = std::result::Result<T, String>;

fn failure(error: impl std::error::Error + 'static) -> String {
    tracing::debug!(%error, "updater operation failed");
    let mut cause = Some(&error as &(dyn std::error::Error + 'static));
    while let Some(error) = cause {
        if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
            matches!(
                e.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            )
        }) {
            return "Not enough disk space".into();
        }
        cause = error.source();
    }
    "Update failed; check your connection and installation permissions, then retry".into()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Saved policy and display cache; never authorizes installation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    schema: u8,
    prompt_version: Option<Version>,
    prompts_shown: u8,
    first_prompt_at: Option<u64>,
    cached_latest: Option<Version>,
    last_successful_check: Option<u64>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            schema: 1,
            prompt_version: None,
            prompts_shown: 0,
            first_prompt_at: None,
            cached_latest: None,
            last_successful_check: None,
        }
    }
}

/// Pure reminder policy. Session start must be strictly later than the first prompt.
fn prompt(version: &Version, saved: &Policy, time: u64, session_start: u64) -> (bool, Policy) {
    let mut next = saved.clone();
    if saved.prompt_version.as_ref().is_none_or(|v| version > v) {
        next.prompt_version = Some(version.clone());
        next.prompts_shown = 0;
        next.first_prompt_at = None;
    } else if saved.prompt_version.as_ref() != Some(version) {
        return (false, next);
    }
    next.prompts_shown = next.prompts_shown.min(2);
    if next.prompts_shown > 0 && next.first_prompt_at.is_none_or(|t| t > time) {
        next.first_prompt_at = Some(time);
    }
    let show = next.prompts_shown == 0
        || (next.prompts_shown == 1
            && next
                .first_prompt_at
                .is_some_and(|t| time.saturating_sub(t) >= 3 * DAY && session_start > t));
    if show {
        next.prompts_shown += 1;
        next.first_prompt_at.get_or_insert(time);
    }
    (show, next)
}

fn load_policy(dir: &Path) -> Policy {
    let path = dir.join("update.json");
    let read = || -> Option<Policy> {
        let file = File::open(&path).ok()?;
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes).ok()?;
        if bytes.len() > 4096 {
            return None;
        }
        let mut state: Policy = serde_json::from_slice(&bytes).ok()?;
        if state.schema != 1 {
            return None;
        }
        state.prompts_shown = state.prompts_shown.min(2);
        if state.last_successful_check.is_some_and(|t| t > now()) {
            state.last_successful_check = None;
            state.cached_latest = None;
        }
        Some(state)
    };
    read().unwrap_or_else(|| {
        if path.exists() {
            let _ = fs::rename(path, dir.join("update.corrupt.json"));
        }
        Policy::default()
    })
}

fn save_policy(dir: &Path, policy: &Policy) -> Result<()> {
    let bytes = serde_json::to_vec(policy).map_err(failure)?;
    fogu_core::fsutil::write_private_atomic(&dir.join("update.json"), &bytes).map_err(failure)
}

fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(failure)?;
    file.try_lock()
        .map_err(|_| "Another Fogu window is checking or installing".to_owned())?;
    Ok(file)
}

/// Validated release identity, kept unchanged between display and install approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    id: u64,
    version: Version,
    tag: String,
    body: String,
    asset: Asset,
    sums: Asset,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Asset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct Metadata {
    id: u64,
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<Asset>,
}

/// Exact canonical install identity, captured before any replacement.
#[derive(Clone, Debug)]
pub struct Target {
    destination: PathBuf,
    restart: PathBuf,
    asset: &'static str,
    entry: &'static str,
}

impl Target {
    fn detect() -> Result<Self> {
        let exe = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(failure)?;
        platform::detect(exe)
    }

    fn enabled(&self) -> Result<()> {
        if option_env!("FOGU_UPDATER").is_none()
            || std::env::var_os("FOGU_DISABLE_UPDATER").is_some_and(|v| v == "1")
            || self
                .destination
                .parent()
                .is_none_or(|p| p.join(".fogu-managed").exists())
        {
            return Err("Updates are managed by your installation method".into());
        }
        Ok(())
    }
}

/// UI state; only the message loop mutates it.
#[derive(Debug)]
pub struct Updater {
    target: Result<Target>,
    status: Status,
    release: Option<Release>,
    policy: Policy,
    session_start: u64,
    restart_args: Vec<std::ffi::OsString>,
    confirm_restart: bool,
}

#[derive(Debug, Default)]
enum Status {
    #[default]
    Idle,
    Checking,
    Installing(&'static str),
    Installed,
    Error(String),
}

impl Updater {
    /// The version the page offers: a candidate re-resolved at Install may be newer than the
    /// last saved check.
    fn latest(&self) -> Option<&Version> {
        self.release
            .as_ref()
            .map(|r| &r.version)
            .or(self.policy.cached_latest.as_ref())
    }
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            target: Target::detect(),
            status: Status::Idle,
            release: None,
            policy: Policy::default(),
            session_start: now(),
            restart_args: Vec::new(),
            confirm_restart: false,
        }
    }
}

/// Bounded worker messages, including phase changes from the install job.
#[derive(Clone, Debug)]
pub enum UpdateMsg {
    /// Start a check; true means automatic and silent on error.
    Check(bool),
    /// The check worker finished; the flag preserves its automatic/manual origin.
    Checked(bool, Result<CheckedRelease>),
    /// Re-resolve the displayed release before downloading.
    Install,
    /// One of the three bounded installation phase events.
    Phase(&'static str),
    /// The installation worker finished, including join failures.
    Finished(std::result::Result<InstallResult, String>),
    /// Open Settings › Updates without downloading.
    Open,
    /// Open the full changelog for the validated tag.
    Changelog,
    /// Open the manual download page for the validated tag.
    DownloadPage,
    /// Ask to restart, confirming any running terminals first.
    Restart,
    /// The restart confirmation was accepted or cancelled.
    ConfirmRestart(bool),
}

/// A changed candidate requires showing the new notes and a new click.
#[derive(Clone, Debug)]
pub enum InstallResult {
    /// The target already had this version, or replacement succeeded.
    Installed,
    /// Display the changed candidate and require another explicit click.
    Changed(Box<Release>),
}

/// Validate notes before rendering or enabling installation.
fn valid_notes(body: &str) -> bool {
    if body.len() > 16 * 1024
        || body.lines().count() > 120
        || body.chars().any(|c| {
            (c.is_control() && c != '\n' && c != '\t')
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return false;
    }
    let mut category = false;
    for line in body.lines() {
        if line.starts_with('#') {
            category = [
                "### Added",
                "### Changed",
                "### Fixed",
                "### Security",
                "### Upgrade notes",
            ]
            .contains(&line.trim());
        } else if category
            && line
                .trim()
                .strip_prefix("- ")
                .is_some_and(|v| !v.trim().is_empty())
        {
            return true;
        }
    }
    false
}

const COMPATIBILITY: &str = "<!-- fogu-update: ";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Compatibility {
    schema: u8,
    from: semver::VersionReq,
}

/// An explicit release contract gates skipped versions; unknown schemas require manual install.
fn compatibility(body: &str) -> Option<Compatibility> {
    let mut contracts = body
        .lines()
        .filter_map(|line| line.strip_prefix(COMPATIBILITY));
    let contract = contracts
        .next()
        .and_then(|line| line.strip_suffix(" -->"))
        .and_then(|json| serde_json::from_str::<Compatibility>(json).ok());
    if contracts.next().is_some() {
        return None;
    }
    contract.filter(|c| c.schema == 1)
}

fn compatible(body: &str, installed: &Version) -> bool {
    compatibility(body).is_some_and(|c| c.from.matches(installed))
}

fn shown_notes(body: &str) -> String {
    body.lines()
        .filter(|line| !line.starts_with(COMPATIBILITY))
        .collect::<Vec<_>>()
        .join("\n")
}

fn stable_tag(tag: &str) -> Result<Version> {
    let version =
        Version::parse(tag.strip_prefix('v').ok_or("Invalid release tag")?).map_err(failure)?;
    if !version.pre.is_empty() || !version.build.is_empty() || tag != format!("v{version}") {
        return Err("Invalid stable release tag".into());
    }
    Ok(version)
}

fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(failure)
}

fn url(tag: &str, name: &str) -> String {
    format!("{DOWNLOADS}/{tag}/{name}")
}

fn transport_url(original: &str) -> String {
    #[cfg(debug_assertions)]
    if let Ok(root) = std::env::var("FOGU_TEST_UPDATE_SERVER") {
        // Only loopback is permitted even in debug builds.
        if root
            .strip_prefix("http://127.0.0.1:")
            .is_some_and(|p| p.parse::<u16>().is_ok())
        {
            return if original == LATEST {
                format!("{root}/releases/latest")
            } else {
                original.replacen(DOWNLOADS, &format!("{root}/download"), 1)
            };
        }
    }
    original.into()
}

/// Rejects a write that would pass the cap, so an oversized body never lands on disk.
struct Limited<W> {
    inner: W,
    remaining: u64,
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other("Download exceeds size limit"));
        }
        let count = self.inner.write(bytes)?;
        self.remaining -= count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Shown when the system has no `curl` (the README lists it with the install commands).
const NO_CURL: &str = "Updates need curl: install it, or download Fogu from the release page";

/// Fetches `url` into `out` with the system's curl, so Fogu carries no TLS stack and follows the
/// system's proxy and certificate settings. HTTPS only, at most `cap` bytes and `seconds` in all,
/// and no more than 30 s without data.
fn download(
    target: &Target,
    url: &str,
    accept: &str,
    cap: u64,
    seconds: u64,
    out: impl Write,
) -> Result<()> {
    target.enabled()?;
    let url = transport_url(url);
    // The loopback test server of debug builds speaks plain HTTP; `transport_url` allows nothing else.
    let proto = if url.starts_with("https://") {
        "=https"
    } else {
        "=http"
    };
    let mut command = Command::new(platform::curl().ok_or(NO_CURL)?);
    command
        // First: no `.curlrc`, whose `output` or `retry` would escape these limits.
        .arg("-q")
        .args(["--silent", "--fail", "--location", "--max-redirs", "5"])
        .args(["--proto", proto, "--proto-redir", proto])
        .args([
            "--connect-timeout",
            "15",
            "--speed-limit",
            "1",
            "--speed-time",
            "30",
        ])
        .arg("--max-time")
        .arg(seconds.to_string())
        .arg("--max-filesize")
        .arg(cap.to_string())
        .arg("--header")
        .arg(format!("Accept: {accept}"))
        .arg("--user-agent")
        .arg(concat!("fogu/", env!("CARGO_PKG_VERSION")))
        .arg("--")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    platform::quiet_probe(&mut command);
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            NO_CURL.to_owned()
        } else {
            failure(e)
        }
    })?;
    let copied = match child.stdout.take() {
        Some(mut body) => std::io::copy(
            &mut body,
            &mut Limited {
                inner: out,
                remaining: cap,
            },
        )
        .map(drop)
        .map_err(failure),
        None => Err("Could not read the download".to_owned()),
    };
    if copied.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(failure)?;
    copied?;
    if !status.success() {
        tracing::debug!(%status, "curl failed");
        return Err("Could not reach GitHub; check your connection and retry".into());
    }
    Ok(())
}

fn resolve(target: &Target) -> Result<Release> {
    let mut bytes = Vec::new();
    download(
        target,
        LATEST,
        "application/vnd.github+json",
        256 * 1024,
        15,
        &mut bytes,
    )?;
    let meta: Metadata = serde_json::from_slice(&bytes).map_err(failure)?;
    validate_metadata(meta, target)
}

fn validate_metadata(meta: Metadata, target: &Target) -> Result<Release> {
    let version = stable_tag(&meta.tag_name)?;
    if meta.draft || meta.prerelease {
        return Err("No stable public release is available".into());
    }
    let select = |name: &str, cap| -> Result<Asset> {
        let matching: Vec<_> = meta.assets.iter().filter(|a| a.name == name).collect();
        let [asset] = matching.as_slice() else {
            return Err("Release asset is missing or ambiguous".into());
        };
        if asset.size == 0
            || asset.size > cap
            || asset.browser_download_url != url(&meta.tag_name, name)
        {
            return Err("Release asset has invalid size or location".into());
        }
        Ok((*asset).clone())
    };
    let asset = select(target.asset, MAX_ASSET)?;
    parse_digest(
        asset
            .digest
            .as_deref()
            .ok_or("Release digest is missing")?
            .strip_prefix("sha256:")
            .ok_or("Invalid release digest")?,
    )?;
    let sums = select("SHA256SUMS", 64 * 1024)?;
    Ok(Release {
        id: meta.id,
        version,
        tag: meta.tag_name,
        // GitHub stores CRLF once a release body is edited on the web.
        body: meta.body.unwrap_or_default().replace("\r\n", "\n"),
        asset,
        sums,
    })
}

fn check(
    target: &Target,
    dir: &Path,
    automatic: bool,
    session_start: u64,
) -> Result<CheckedRelease> {
    target.enabled()?;
    if automatic {
        platform::preflight(target)?;
        let _probe = Stage::create(target.destination.parent().ok_or("Invalid installation")?)?;
    }
    let _lock = lock(&dir.join("update-policy.lock"))?;
    if automatic {
        // Recorded before the network: an offline start also waits a day for the next try.
        let (stamp, now) = (dir.join("update-check.stamp"), now());
        let last = fs::read_to_string(&stamp)
            .ok()
            .and_then(|t| t.trim().parse::<u64>().ok());
        // A stamp in the future (clock set back) is due, then rewritten with the current time.
        if last.is_some_and(|last| now.checked_sub(last).is_some_and(|age| age < DAY)) {
            return Ok(None);
        }
        fogu_core::fsutil::write_private_atomic(&stamp, now.to_string().as_bytes())
            .map_err(failure)?;
    }
    let release = resolve(target)?;
    let mut policy = load_policy(dir);
    policy.cached_latest = Some(release.version.clone());
    policy.last_successful_check = Some(now());
    let show = if automatic && release.version > current_version()? && valid_notes(&release.body) {
        let (show, next) = prompt(&release.version, &policy, now(), session_start);
        policy = next;
        show
    } else {
        false
    };
    // A toast that could not be counted must not show; a manual check still shows the release.
    if let Err(e) = save_policy(dir, &policy) {
        if automatic {
            return Err(e);
        }
        tracing::warn!(%e, "update state not saved");
    }
    Ok(Some(Box::new((release, policy, show))))
}

fn parse_digest(text: &str) -> Result<Vec<u8>> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid SHA-256".into());
    }
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(failure))
        .collect()
}

fn keys() -> Result<Vec<[u8; 32]>> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("FOGU_TEST_UPDATE_KEY") {
        let bytes: [u8; 32] = fs::read(path)
            .map_err(failure)?
            .try_into()
            .map_err(|_| "Invalid test key")?;
        return Ok(vec![bytes]);
    }
    [
        include_str!("../../../assets/updater/active.pub"),
        include_str!("../../../assets/updater/reserve.pub"),
    ]
    .into_iter()
    .map(|text| {
        let text = text
            .lines()
            .find(|l| !l.starts_with('#'))
            .ok_or("Missing public key")?;
        let bytes: [u8; 32] = parse_digest(text)?
            .try_into()
            .map_err(|_| "Invalid public key")?;
        Ok(bytes)
    })
    .collect()
}

/// Bounded direct probe; a reader thread drains at most 257 bytes while the parent polls.
pub fn probe_version(path: &Path) -> Result<Version> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped());
    platform::quiet_probe(&mut command);
    let mut child = command.spawn().map_err(failure)?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Could not read installed version")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(257).read_to_end(&mut bytes).map(|_| bytes)
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("Version probe timed out");
            }
        }
    };
    // A descendant inheriting stdout must not keep this operation waiting forever.
    while !reader.is_finished() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !reader.is_finished() {
        return Err("Version probe timed out".into());
    }
    let bytes = reader
        .join()
        .map_err(|_| "Version probe failed")?
        .map_err(failure)?;
    if !status?.success() || bytes.len() > 256 {
        return Err("Invalid version probe output".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(failure)?;
    let text = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    let version = Version::parse(
        text.strip_prefix("fogu ")
            .ok_or("Invalid version probe output")?,
    )
    .map_err(failure)?;
    if text != format!("fogu {version}") {
        return Err("Invalid version probe output".into());
    }
    Ok(version)
}

/// Private sibling directory. Retain a displaced destination if commit or rollback fails.
struct Stage {
    path: PathBuf,
    preserve: bool,
}
impl Stage {
    fn create(parent: &Path) -> Result<Self> {
        let mut random = [0; 16];
        SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| "Could not create staging directory")?;
        let suffix = random.iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        });
        let path = parent.join(format!(".fogu-update-{suffix}"));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt as _;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&path).map_err(failure)?;
        let stage = Self {
            path,
            preserve: false,
        };
        #[cfg(windows)]
        fogu_core::winsec::restrict_to_user(&stage.path).map_err(failure)?;
        Ok(stage)
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Remove only old, marked, incomplete stages; backups always need manual inspection.
fn clean_stale(parent: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(parent).map_err(failure)? {
        let entry = entry.map_err(failure)?;
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|n| n.strip_prefix(".fogu-update-")) else {
            continue;
        };
        if name.len() != 32
            || !name.bytes().all(|b| b.is_ascii_hexdigit())
            || !entry.file_type().map_err(failure)?.is_dir()
        {
            continue;
        }
        let path = entry.path();
        if path.join("previous").exists() {
            continue;
        }
        let marker = path.join("destination");
        if fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file() && m.len() <= 4096)
            && fs::read_to_string(&marker).is_ok_and(|text| text == destination.to_string_lossy())
            && entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > DAY)
        {
            fs::remove_dir_all(path).map_err(failure)?;
        }
    }
    Ok(())
}

fn safe_entry(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn install(
    target: &Target,
    approved: &Release,
    mut phase: impl FnMut(&'static str),
) -> Result<InstallResult> {
    target.enabled()?;
    let parent = target
        .destination
        .parent()
        .ok_or("Invalid installation path")?;
    let _lock = lock(&parent.join(".fogu-update.lock"))?;
    platform::preflight(target)?;
    clean_stale(parent, &target.destination)?;
    if probe_version(&target.restart)? >= approved.version {
        return Ok(InstallResult::Installed);
    }
    let fresh = resolve(target)?;
    if fresh != *approved {
        return Ok(InstallResult::Changed(Box::new(fresh)));
    }
    if approved.version <= current_version()? || !valid_notes(&approved.body) {
        return Err("Check the release notes before installing".into());
    }
    if !compatible(&approved.body, &current_version()?) {
        return Err(
            "This version needs a manual or intermediate upgrade; open the full changelog".into(),
        );
    }
    let mut stage = Stage::create(parent)?;
    fs::write(
        stage.path.join("destination"),
        target.destination.to_string_lossy().as_bytes(),
    )
    .map_err(failure)?;
    phase("Downloading");
    let archive = stage.path.join(&approved.asset.name);
    let mut file = File::create(&archive).map_err(failure)?;
    download(
        target,
        &approved.asset.browser_download_url,
        "application/octet-stream",
        MAX_ASSET,
        600,
        &mut file,
    )?;
    file.sync_all().map_err(failure)?;
    if file.metadata().map_err(failure)?.len() != approved.asset.size {
        return Err("Release asset size changed".into());
    }
    phase("Verifying");
    let hash = sha256(&archive)?;
    if hash.as_ref()
        != parse_digest(
            approved
                .asset
                .digest
                .as_deref()
                .ok_or("Missing digest")?
                .strip_prefix("sha256:")
                .ok_or("Invalid digest")?,
        )?
    {
        return Err("Security check failed: archive checksum".into());
    }
    let mut sums = Vec::new();
    download(
        target,
        &approved.sums.browser_download_url,
        "application/octet-stream",
        64 * 1024,
        15,
        &mut sums,
    )?;
    if hash.as_ref() != listed_sum(&sums, &approved.asset.name)? {
        return Err("Security check failed: SHA256SUMS".into());
    }
    let extracted = stage.path.join("extracted");
    let source = verify_payload(&archive, &extracted, target, &approved.version)?;
    if resolve(target)? != *approved {
        return Err("Release changed or was withdrawn; check again".into());
    }
    phase("Committing");
    stage.preserve = true;
    platform::commit(&source, target, &stage.path)?;
    stage.preserve = false;
    Ok(InstallResult::Installed)
}

fn sha256(path: &Path) -> Result<digest::Digest> {
    let mut context = digest::Context::new(&digest::SHA256);
    let mut file = File::open(path).map_err(failure)?;
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(failure)?;
        if n == 0 {
            return Ok(context.finish());
        }
        context.update(&buffer[..n]);
    }
}

/// The digest `SHA256SUMS` gives for `name`; exactly one entry must name it.
fn listed_sum(sums: &[u8], name: &str) -> Result<Vec<u8>> {
    let sums = std::str::from_utf8(sums).map_err(failure)?;
    let matching: Vec<_> = sums
        .lines()
        .filter_map(|l| l.split_once("  "))
        .filter(|(_, n)| *n == name)
        .collect();
    let [(expected, _)] = matching.as_slice() else {
        return Err("Security check failed: SHA256SUMS entry".into());
    };
    parse_digest(expected)
}

/// Shared by installation and the release pipeline's debug verification entry.
fn verify_payload(
    archive: &Path,
    extracted: &Path,
    target: &Target,
    version: &Version,
) -> Result<PathBuf> {
    verify_signature(archive, &keys()?).map_err(|_| "Security check failed: archive signature")?;
    platform::list_archive(archive)?;
    fs::create_dir(extracted).map_err(failure)?;
    extract(archive, extracted)?;
    let source = extracted.join(target.entry);
    platform::verify(&source, version)?;
    Ok(source)
}

// Platform strategies are deliberately opt-in, added only after their staging gate.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    use super::{Command, Path, PathBuf, Result, Target, Version};
    pub(super) fn detect(_: PathBuf) -> Result<Target> {
        Err("Update using your installation method".into())
    }
    /// Absolute `PATH` entries only: a `curl` in the working directory never runs.
    pub(super) fn curl() -> Option<PathBuf> {
        fogu_core::platform::find_in_path("curl")
    }
    pub(super) fn quiet_probe(_: &mut Command) {}
    pub(super) fn preflight(_: &Target) -> Result<()> {
        Err("Unsupported installation".into())
    }
    pub(super) fn list_archive(_: &Path) -> Result<()> {
        Err("Unsupported archive".into())
    }
    pub(super) fn verify(_: &Path, _: &Version) -> Result<()> {
        Err("Unsupported archive".into())
    }
    pub(super) fn commit(_: &Path, _: &Target, _: &Path) -> Result<()> {
        Err("Unsupported installation".into())
    }
}

impl App {
    /// Capture the restart target and arguments before the executable can change.
    pub(crate) fn init_updater(&mut self, no_recovery: bool) {
        if let Some(dir) = &self.state_dir
            && let Ok(_policy_lock) = lock(&dir.join("update-policy.lock"))
        {
            self.updater.policy = load_policy(dir);
        }
        if let Some(dir) = &self.config_dir {
            self.updater
                .restart_args
                .extend(["--config-dir".into(), dir.as_os_str().to_owned()]);
        }
        if no_recovery {
            self.updater.restart_args.push("--no-recovery".into());
        }
    }

    pub(crate) fn update_busy(&self) -> bool {
        matches!(self.updater.status, Status::Installing(_))
    }

    /// Blocking work belongs to the session runtime, with join failures reported to the UI.
    #[expect(
        clippy::too_many_lines,
        reason = "one message dispatch with independent UI actions"
    )]
    pub(crate) fn on_update(&mut self, message: UpdateMsg) -> Task<Message> {
        match message {
            UpdateMsg::Open => {
                let task = self.open_settings();
                let page = self.on_settings(crate::settings::SettingsMsg::Page(
                    crate::settings::Page::Updates,
                ));
                Task::batch([task, page])
            }
            UpdateMsg::Check(automatic) => {
                if matches!(
                    self.updater.status,
                    Status::Checking | Status::Installing(_)
                ) {
                    return Task::none();
                }
                let target = self.updater.target.clone().and_then(|t| {
                    t.enabled()?;
                    Ok(t)
                });
                let target = match target {
                    Ok(t) => t,
                    Err(e) => {
                        if !automatic {
                            self.updater.status = Status::Error(e);
                        }
                        return Task::none();
                    }
                };
                let Some(dir) = self.state_dir.clone() else {
                    self.updater.status = Status::Error("Updates need a state directory".into());
                    return Task::none();
                };
                self.updater.status = Status::Checking;
                let session = self.updater.session_start;
                let job = self
                    .runtime
                    .handle()
                    .spawn_blocking(move || check(&target, &dir, automatic, session));
                Task::perform(
                    async move { job.await.map_err(failure).and_then(|r| r) },
                    move |r| Message::Update(UpdateMsg::Checked(automatic, r)),
                )
            }
            UpdateMsg::Checked(automatic, result) => {
                self.updater.status = Status::Idle;
                match result {
                    Ok(Some(checked)) => {
                        let (release, policy, show) = *checked;
                        let title = format!("Fogu {} is available", release.version);
                        self.updater.release = Some(release);
                        self.updater.policy = policy;
                        if show {
                            return self.notify(
                                Notification::new(Level::Info, title)
                                    .detail("The update will only be installed when you choose.")
                                    .action(ToastAction::Update),
                            );
                        }
                    }
                    Err(e) if !automatic => self.updater.status = Status::Error(e),
                    Err(e) => tracing::debug!(error = %e, "automatic update check skipped"),
                    Ok(None) => {}
                }
                Task::none()
            }
            UpdateMsg::Install => {
                if matches!(
                    self.updater.status,
                    Status::Checking | Status::Installing(_)
                ) || self.state_dir.is_none()
                {
                    return Task::none();
                }
                let (Ok(target), Some(release)) =
                    (self.updater.target.clone(), self.updater.release.clone())
                else {
                    return Task::none();
                };
                self.updater.status = Status::Installing("Downloading");
                let (tx, rx) = tokio::sync::mpsc::channel(4);
                let job = self.runtime.handle().spawn_blocking(move || {
                    install(&target, &release, |phase| {
                        let _ = tx.blocking_send(Message::Update(UpdateMsg::Phase(phase)));
                    })
                });
                let phases = iced::futures::stream::unfold(rx, |mut rx| async {
                    rx.recv().await.map(|m| (m, rx))
                });
                Task::batch([
                    Task::run(phases, |m| m),
                    Task::perform(
                        async move { job.await.map_err(failure).and_then(|r| r) },
                        |r| Message::Update(UpdateMsg::Finished(r)),
                    ),
                ])
            }
            UpdateMsg::Phase(phase) => {
                if self.update_busy() {
                    self.updater.status = Status::Installing(phase);
                }
                Task::none()
            }
            UpdateMsg::Finished(result) => {
                match result {
                    Ok(InstallResult::Installed) => self.updater.status = Status::Installed,
                    Ok(InstallResult::Changed(release)) => {
                        self.updater.release = Some(*release);
                        self.updater.status = Status::Error(
                            "Release changed; review the notes and choose Install again".into(),
                        );
                    }
                    Err(e) => self.updater.status = Status::Error(e),
                }
                Task::none()
            }
            UpdateMsg::Changelog => {
                let url = self.updater.release.as_ref().map_or_else(
                    || "https://github.com/Revdep/Fogu/blob/main/CHANGELOG.md".to_owned(),
                    |r| format!("https://github.com/Revdep/Fogu/blob/{}/CHANGELOG.md", r.tag),
                );
                self.open_url(url)
            }
            UpdateMsg::DownloadPage => {
                let url = self.updater.release.as_ref().map_or_else(
                    || "https://github.com/Revdep/Fogu/releases".to_owned(),
                    |r| format!("https://github.com/Revdep/Fogu/releases/tag/{}", r.tag),
                );
                self.open_url(url)
            }
            UpdateMsg::Restart => {
                if !matches!(self.updater.status, Status::Installed) {
                    return Task::none();
                }
                if self
                    .tabs
                    .iter()
                    .flat_map(|t| t.panes.values())
                    .any(|p| p.state.is_running())
                {
                    self.updater.confirm_restart = true;
                    Task::none()
                } else {
                    self.restart_updated()
                }
            }
            UpdateMsg::ConfirmRestart(yes) => {
                self.updater.confirm_restart = false;
                if yes {
                    self.restart_updated()
                } else {
                    Task::none()
                }
            }
        }
    }

    fn restart_updated(&mut self) -> Task<Message> {
        let Ok(target) = self.updater.target.clone() else {
            return Task::none();
        };
        self.save_on_exit();
        let mut command = Command::new(target.restart);
        // Back on Wayland if Fogu runs on XWayland: the new Fogu decides again from its settings.
        fogu_core::platform::give_back_wayland(
            &mut command,
            fogu_core::platform::kept_wayland_display().as_deref(),
        );
        // The new AppImage's AppRun decides again which libraries it needs.
        fogu_core::platform::drop_appimage_libs(&mut command, fogu_core::platform::in_appimage());
        command
            .arg("--after-update-of")
            .arg(std::process::id().to_string())
            .args(&self.updater.restart_args);
        // A secondary window must not take over the primary window's recovery files.
        if self.instance_lock.is_none() {
            command.arg("--no-recovery");
        }
        // shortcut: spawn plus a bounded parent-exit barrier, add a ready handshake after native restart fault tests.
        match command.spawn() {
            Ok(_) => {
                self.recovery = None;
                iced::exit()
            }
            Err(e) => {
                self.updater.status = Status::Error(failure(e));
                Task::none()
            }
        }
    }

    pub(crate) fn update_modal(&self) -> Option<crate::modal::Modal> {
        self.updater.confirm_restart.then(|| {
            let count = self
                .tabs
                .iter()
                .flat_map(|t| t.panes.values())
                .filter(|p| p.state.is_running())
                .count();
            crate::modal::Modal {
                title: "Restart Fogu?",
                body: format!("Restarting closes {count} terminals; running programs will stop."),
                buttons: vec![
                    crate::modal::ModalButton {
                        label: "Restart",
                        on_press: Message::Update(UpdateMsg::ConfirmRestart(true)),
                    },
                    crate::modal::ModalButton {
                        label: "Cancel",
                        on_press: Message::Update(UpdateMsg::ConfirmRestart(false)),
                    },
                ],
            }
        })
    }
}

/// Native release installs remain manual until their real-machine update gate passes.
fn ota_supported() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        cfg!(debug_assertions) || platform::OTA_VALIDATED
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        cfg!(target_os = "linux")
    }
}

/// Plain-text update page, with an explicit action for every network/install operation.
pub(crate) fn view(updater: &Updater, look: crate::palette::Look) -> iced::Element<'_, Message> {
    use iced::widget::{button, column, row, scrollable, text};
    let busy = matches!(updater.status, Status::Checking | Status::Installing(_));
    let install_supported = ota_supported();
    let enabled = updater
        .target
        .as_ref()
        .map_err(Clone::clone)
        .and_then(Target::enabled);
    let mut body = column![text(format!(
        "Installed version   {}",
        env!("CARGO_PKG_VERSION")
    ))]
    .spacing(14);
    if let Some(version) = updater.latest() {
        body = body.push(text(format!("Latest version   {version}")));
    }
    body = body.push(text(updater.policy.last_successful_check.map_or_else(
        || "Not checked yet".into(),
        |t| {
            format!(
                "Last checked   {} minutes ago",
                now().saturating_sub(t) / 60
            )
        },
    )));
    if let Err(reason) = &enabled {
        body = body.push(text(reason.clone()));
    } else if !install_supported {
        body = body.push(text(
            "Use the manual download page to update Fogu on this platform.",
        ));
    }
    match &updater.status {
        Status::Checking => body = body.push(text("Checking…")),
        Status::Installing(phase) => {
            body = body.push(text(format!("{phase}… Please wait before closing Fogu.")));
        }
        Status::Error(e) => body = body.push(text(e)),
        Status::Installed => {
            body = body.push(button("Restart Fogu").on_press(Message::Update(UpdateMsg::Restart)));
        }
        Status::Idle => {}
    }
    if let Some(release) = &updater.release {
        if current_version().is_ok_and(|v| release.version <= v) {
            body = body.push(text("Fogu is up to date"));
        } else {
            let notes = valid_notes(&release.body);
            let compatible = current_version().is_ok_and(|v| compatible(&release.body, &v));
            if !compatible {
                body = body.push(text(
                    "This release needs a manual or intermediate upgrade. See the full changelog.",
                ));
            }
            if notes {
                body = body.push(scrollable(text(shown_notes(&release.body))).height(280));
            } else {
                body = body.push(text(
                    "Release notes are unavailable or invalid. Retry before installing.",
                ));
            }
            // shortcut: latest notes only, replace with bounded cumulative history when a release contract exists.
            if current_version().is_ok_and(|v| {
                v.major != release.version.major
                    || v.minor != release.version.minor
                    || release.version.patch.saturating_sub(v.patch) > 1
            }) {
                body = body.push(text("You are skipping versions: see the full changelog"));
            }
            let install =
                button("Install update").style(crate::style::btn(look, crate::style::Btn::Primary));
            body = body.push(
                install.on_press_maybe(
                    (notes
                        && compatible
                        && !busy
                        && enabled.is_ok()
                        && install_supported
                        && !matches!(updater.status, Status::Installed))
                    .then_some(Message::Update(UpdateMsg::Install)),
                ),
            );
        }
    }
    body.push(
        row![
            button("Full changelog").on_press(Message::Update(UpdateMsg::Changelog)),
            button("Open download page").on_press(Message::Update(UpdateMsg::DownloadPage)),
            button("Check now / Retry").on_press_maybe(
                (!busy && enabled.is_ok()).then_some(Message::Update(UpdateMsg::Check(false)))
            ),
        ]
        .spacing(12),
    )
    .into()
}

/// Headless debug driver uses the same resolver, verifier and installer as Settings.
#[cfg(debug_assertions)]
pub fn debug_install() -> Result<()> {
    let target = Target::detect()?;
    target.enabled()?;
    let release = resolve(&target)?;
    install(&target, &release, |_| {}).map(|_| ())
}

/// The release pipeline's gate, run on a debug build: the notes, `SHA256SUMS` and the signature of
/// every release file in `dir`, and for the Linux archives the whole client path (listing,
/// extraction, version probe). The archives for other systems get their own checks on install.
#[cfg(all(debug_assertions, target_os = "linux"))]
pub fn debug_verify_release(dir: &Path, version: &str) -> Result<()> {
    let version = Version::parse(version).map_err(failure)?;
    let mut notes = Vec::new();
    File::open(dir.join("release-notes.md"))
        .map_err(failure)?
        .take(16 * 1024 + 1)
        .read_to_end(&mut notes)
        .map_err(failure)?;
    let notes = std::str::from_utf8(&notes)
        .map_err(failure)?
        .replace("\r\n", "\n");
    if !valid_notes(&notes) || compatibility(&notes).is_none() {
        return Err("Invalid release notes or compatibility contract".into());
    }
    let sums = fs::read(dir.join("SHA256SUMS")).map_err(failure)?;
    if sums.len() > 64 * 1024
        || std::str::from_utf8(&sums).map_err(failure)?.lines().count() != RELEASE.len()
    {
        return Err("SHA256SUMS must list exactly the release files".into());
    }
    let stage = Stage::create(dir)?;
    for name in RELEASE {
        let path = dir.join(name);
        if fs::metadata(&path).map_err(failure)?.len() > MAX_ASSET {
            return Err(format!("{name} is too large"));
        }
        if sha256(&path)?.as_ref() != listed_sum(&sums, name)? {
            return Err(format!("{name}: SHA256SUMS mismatch"));
        }
        let entry = match name {
            "Fogu-linux-x86_64.tar.gz" => "Fogu-linux-x86_64/fogu",
            "Fogu-linux-appimage-x86_64.tar.gz" => "Fogu-x86_64.AppImage",
            "Fogu-macos-aarch64.zip" | "Fogu-macos-x86_64.zip" | "Fogu-windows-x86_64.zip" => {
                zip_signed_by(&path, &keys()?).map_err(|_| format!("{name}: bad signature"))?;
                let entries = list_zip(&path).map_err(|e| format!("{name}: {e}"))?;
                let exe = if name.starts_with("Fogu-macos") {
                    "Fogu.app/Contents/MacOS/fogu"
                } else {
                    "Fogu-windows-x86_64/fogu.exe"
                };
                if !entries.contains(Path::new(exe)) {
                    return Err(format!("{name}: no {exe}"));
                }
                continue;
            }
            _ => continue,
        };
        let target = Target {
            destination: path.clone(),
            restart: path.clone(),
            asset: "",
            entry,
        };
        verify_payload(&path, &stage.path.join(name), &target, &version)
            .map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(())
}

/// Every file of a release besides its notes.
#[cfg(all(debug_assertions, target_os = "linux"))]
const RELEASE: [&str; 6] = [
    "Fogu-linux-x86_64.tar.gz",
    "Fogu-linux-appimage-x86_64.tar.gz",
    "Fogu-macos-aarch64.zip",
    "Fogu-macos-x86_64.zip",
    "Fogu-windows-x86_64.zip",
    "Fogu-x86_64.AppImage",
];

#[cfg(target_os = "linux")]
mod platform {
    use super::{
        Command, File, MAX_EXTRACT, Path, PathBuf, Result, Target, Version, failure, fs,
        probe_version, safe_entry, swap,
    };
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    pub(super) fn detect(exe: PathBuf) -> Result<Target> {
        if std::env::consts::ARCH != "x86_64" {
            return Err("Use your installation method on this architecture".into());
        }
        if let (Some(image), Some(dir)) = (std::env::var_os("APPIMAGE"), std::env::var_os("APPDIR"))
            && fs::symlink_metadata(&image).is_ok_and(|m| m.is_file())
            && let (Ok(dir), Ok(image)) = (fs::canonicalize(dir), fs::canonicalize(image))
            && dir.is_dir()
            && exe.starts_with(dir)
        {
            return Ok(Target {
                destination: image.clone(),
                restart: image,
                asset: "Fogu-linux-appimage-x86_64.tar.gz",
                entry: "Fogu-x86_64.AppImage",
            });
        }
        if !fs::symlink_metadata(&exe).map_err(failure)?.is_file() {
            return Err("Unsupported installation".into());
        }
        Ok(Target {
            destination: exe.clone(),
            restart: exe,
            asset: "Fogu-linux-x86_64.tar.gz",
            entry: "Fogu-linux-x86_64/fogu",
        })
    }
    /// Absolute `PATH` entries only: a `curl` in the working directory never runs.
    pub(super) fn curl() -> Option<PathBuf> {
        fogu_core::platform::find_in_path("curl")
    }
    pub(super) fn quiet_probe(_: &mut Command) {}
    pub(super) fn preflight(target: &Target) -> Result<()> {
        let parent = target.destination.parent().ok_or("Invalid installation")?;
        let meta = fs::metadata(parent).map_err(failure)?;
        let file = fs::metadata(&target.destination).map_err(failure)?;
        if meta.mode() & 0o222 == 0 || file.uid() == 0 && !cfg!(debug_assertions) {
            return Err(
                "Update using your installation method; this directory is not writable".into(),
            );
        }
        Ok(())
    }
    pub(super) fn list_archive(path: &Path) -> Result<()> {
        let input = flate2::read::GzDecoder::new(File::open(path).map_err(failure)?);
        let mut archive = tar::Archive::new(input);
        let mut total = 0u64;
        let mut count = 0u32;
        let mut names = std::collections::HashSet::new();
        // shortcut: Linux rejects extended TAR headers, add bounded metadata parsing if the package layout needs them.
        for entry in archive.entries().map_err(failure)?.raw(true) {
            let entry = entry.map_err(failure)?;
            count += 1;
            total = total
                .checked_add(entry.size())
                .ok_or("Archive is too large")?;
            let path = entry.path().map_err(failure)?;
            let kind = entry.header().entry_type();
            if count > 4096
                || total > MAX_EXTRACT
                || !safe_entry(&path)
                || !(kind.is_file() || kind.is_dir())
                || !names.insert(path.into_owned())
            {
                return Err("Unsafe or oversized archive".into());
            }
        }
        Ok(())
    }
    pub(super) fn verify(source: &Path, expected: &Version) -> Result<()> {
        if !fs::symlink_metadata(source).map_err(failure)?.is_file()
            || fs::metadata(source).map_err(failure)?.permissions().mode() & 0o111 == 0
        {
            return Err("Archive does not contain an executable".into());
        }
        if probe_version(source)? != *expected {
            return Err("Security check failed: executable version".into());
        }
        File::open(source)
            .and_then(|f| f.sync_all())
            .map_err(failure)
    }
    pub(super) fn commit(source: &Path, target: &Target, stage: &Path) -> Result<()> {
        // shortcut: two same-filesystem renames leave a recoverable backup after a kill, use an upstream atomic exchange when available.
        swap(source, &target.destination, &stage.join("previous")).map_err(|e| {
                tracing::error!(%e, backup = %stage.display(), "update commit failed; keep staging for recovery");
                "Replacement failed; close other windows and retry. A recoverable copy is kept in the .fogu-update directory beside Fogu".to_owned()
            })?;
        File::open(target.destination.parent().ok_or("Invalid installation")?)
            .and_then(|f| f.sync_all())
            .map_err(failure)
    }
}

/// Moves `dest` to `backup` and `source` into its place, putting `dest` back if the second
/// rename fails. All three share a filesystem (the stage is a sibling of `dest`); a kill between
/// the renames leaves the old copy at `backup`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn swap(source: &Path, dest: &Path, backup: &Path) -> std::io::Result<()> {
    fs::rename(dest, backup)?;
    if let Err(e) = fs::rename(source, dest) {
        fs::rename(backup, dest)?;
        return Err(e);
    }
    Ok(())
}

/// Checks the zipsign signature of `archive` against `keys`, its file name as the context.
fn signed_by(
    archive: &Path,
    keys: &[[u8; 32]],
    verify: impl FnOnce(&mut File, &[zipsign_api::VerifyingKey], &[u8]) -> Result<()>,
) -> Result<()> {
    let context = archive
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid archive name")?;
    let keys = zipsign_api::verify::collect_keys(keys.iter().copied().map(Ok)).map_err(failure)?;
    verify(
        &mut File::open(archive).map_err(failure)?,
        &keys,
        context.as_bytes(),
    )
}

#[cfg(target_os = "linux")]
fn verify_signature(archive: &Path, keys: &[[u8; 32]]) -> Result<()> {
    signed_by(archive, keys, |file, keys, context| {
        zipsign_api::verify::verify_tar(file, keys, Some(context))
            .map(drop)
            .map_err(failure)
    })
}

#[cfg(target_os = "linux")]
fn extract(archive: &Path, into: &Path) -> Result<()> {
    let gzip = flate2::read::GzDecoder::new(File::open(archive).map_err(failure)?);
    tar::Archive::new(gzip).unpack(into).map_err(failure)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn verify_signature(archive: &Path, keys: &[[u8; 32]]) -> Result<()> {
    zip_signed_by(archive, keys)
}

#[cfg(any(target_os = "macos", target_os = "windows", debug_assertions))]
fn zip_signed_by(archive: &Path, keys: &[[u8; 32]]) -> Result<()> {
    signed_by(archive, keys, |file, keys, context| {
        zipsign_api::verify::verify_zip(file, keys, Some(context))
            .map(drop)
            .map_err(failure)
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn extract(archive: &Path, into: &Path) -> Result<()> {
    zip::ZipArchive::new(File::open(archive).map_err(failure)?)
        .and_then(|mut zip| zip.extract(into))
        .map_err(failure)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn verify_signature(_: &Path, _: &[[u8; 32]]) -> Result<()> {
    Err("Unsupported archive".into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn extract(_: &Path, _: &Path) -> Result<()> {
    Err("Unsupported archive".into())
}

/// The regular files of a safe zip: no links or special files, bounded count and size.
#[cfg(any(target_os = "macos", target_os = "windows", debug_assertions))]
fn list_zip(path: &Path) -> Result<std::collections::HashSet<PathBuf>> {
    let mut archive = zip::ZipArchive::new(File::open(path).map_err(failure)?).map_err(failure)?;
    if archive.len() > 4096 {
        return Err("Archive has too many entries".into());
    }
    let mut total = 0u64;
    let (mut names, mut files) = (
        std::collections::HashSet::new(),
        std::collections::HashSet::new(),
    );
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(failure)?;
        total = total
            .checked_add(entry.size())
            .ok_or("Archive is too large")?;
        let path = entry.enclosed_name().ok_or("Unsafe archive path")?;
        let kind = entry.unix_mode().unwrap_or(0) & 0o170_000;
        if total > MAX_EXTRACT
            || !safe_entry(&path)
            || !names.insert(path.clone())
            || !matches!(kind, 0 | 0o100_000 | 0o040_000)
        {
            return Err("Unsafe or oversized archive".into());
        }
        if entry.is_file() {
            files.insert(path);
        }
    }
    Ok(files)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{
        Command, Path, PathBuf, Result, Target, Version, failure, fs, list_zip, ota_supported,
        probe_version, swap,
    };
    // shortcut: macOS OTA is untested on real machines, enable after real N-1 to N runs on ARM and Intel Macs.
    pub(super) const OTA_VALIDATED: bool = false;

    pub(super) fn detect(exe: PathBuf) -> Result<Target> {
        if exe.file_name().is_none_or(|name| name != "fogu") {
            return Err("Unsupported application layout".into());
        }
        if exe.to_string_lossy().contains("/AppTranslocation/") {
            return Err("Move Fogu to Applications before updating".into());
        }
        let bundle = exe
            .parent()
            .filter(|p| p.file_name().is_some_and(|n| n == "MacOS"))
            .and_then(Path::parent)
            .filter(|p| p.file_name().is_some_and(|n| n == "Contents"))
            .and_then(Path::parent)
            .ok_or("Unsupported application layout")?;
        if bundle.file_name().is_none_or(|n| n != "Fogu.app") {
            return Err("Update the app using your installation method".into());
        }
        let asset = match std::env::consts::ARCH {
            "aarch64" => "Fogu-macos-aarch64.zip",
            "x86_64" => "Fogu-macos-x86_64.zip",
            _ => return Err("Unsupported architecture".into()),
        };
        Ok(Target {
            destination: bundle.to_path_buf(),
            restart: exe,
            asset,
            entry: "Fogu.app",
        })
    }
    fn identity(bundle: &Path) -> Result<()> {
        let output = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleIdentifier"])
            .arg(bundle.join("Contents/Info.plist"))
            .output()
            .map_err(failure)?;
        if !output.status.success() || output.stdout != b"io.github.revdep.fogu\n" {
            return Err("Unrecognized application bundle".into());
        }
        Ok(())
    }
    /// Shipped with macOS.
    pub(super) fn curl() -> Option<PathBuf> {
        Some(PathBuf::from("/usr/bin/curl")).filter(|p| p.is_file())
    }
    pub(super) fn quiet_probe(_: &mut Command) {}
    pub(super) fn preflight(target: &Target) -> Result<()> {
        identity(&target.destination)?;
        if !ota_supported() {
            return Err("Download the app and replace Fogu in Applications".into());
        }
        Ok(())
    }
    pub(super) fn list_archive(path: &Path) -> Result<()> {
        list_zip(path).map(drop)
    }
    pub(super) fn verify(source: &Path, expected: &Version) -> Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        identity(source)?;
        let exe = source.join("Contents/MacOS/fogu");
        if fs::metadata(&exe).map_err(failure)?.permissions().mode() & 0o111 == 0 {
            return Err("Bundle executable is not executable".into());
        }
        let output = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleShortVersionString"])
            .arg(source.join("Contents/Info.plist"))
            .output()
            .map_err(failure)?;
        if !output.status.success() || output.stdout != format!("{expected}\n").as_bytes() {
            return Err("Bundle version does not match release".into());
        }
        if probe_version(&exe)? != *expected {
            return Err("Bundle version does not match release".into());
        }
        Ok(())
    }
    pub(super) fn commit(source: &Path, target: &Target, stage: &Path) -> Result<()> {
        swap(source, &target.destination, &stage.join("previous")).map_err(|e| {
            tracing::error!(%e, backup = %stage.display(), "bundle replacement failed; preserve backup");
            "Bundle replacement failed; recover the previous bundle from the sibling .fogu-update directory".into()
        })
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{
        Command, Path, PathBuf, Result, Target, Version, failure, fs, list_zip, ota_supported,
        probe_version,
    };
    use std::os::windows::process::CommandExt as _;
    // shortcut: Windows OTA is untested on a real machine, enable after a real N-1 to N run including Defender.
    pub(super) const OTA_VALIDATED: bool = false;

    pub(super) fn detect(exe: PathBuf) -> Result<Target> {
        if std::env::consts::ARCH != "x86_64" || exe.file_name().is_none_or(|n| n != "fogu.exe") {
            return Err("Unsupported installation".into());
        }
        for key in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
            // `exe` is canonical (`\\?\C:\...`): compare canonical paths, component by component.
            if let Some(dir) = std::env::var_os(key)
                && fs::canonicalize(dir).is_ok_and(|dir| exe.starts_with(dir))
            {
                return Err("Update Fogu using your installation method in Program Files".into());
            }
        }
        Ok(Target {
            destination: exe.clone(),
            restart: exe,
            asset: "Fogu-windows-x86_64.zip",
            entry: "Fogu-windows-x86_64/fogu.exe",
        })
    }
    /// Shipped with Windows 10 1803 and later; the full path skips a `curl.exe` beside Fogu.
    pub(super) fn curl() -> Option<PathBuf> {
        Some(
            std::env::var_os("SystemRoot")
                .map_or_else(|| r"C:\Windows".into(), PathBuf::from)
                .join(r"System32\curl.exe"),
        )
        .filter(|p| p.is_file())
    }
    pub(super) fn quiet_probe(command: &mut Command) {
        command.creation_flags(0x0800_0000);
    }
    pub(super) fn preflight(target: &Target) -> Result<()> {
        if !fs::metadata(&target.destination)
            .map_err(failure)?
            .is_file()
        {
            return Err("Unsupported installation".into());
        }
        if !ota_supported() {
            return Err("Download the portable release to update Fogu".into());
        }
        Ok(())
    }
    pub(super) fn list_archive(path: &Path) -> Result<()> {
        list_zip(path).map(drop)
    }
    pub(super) fn verify(source: &Path, expected: &Version) -> Result<()> {
        if probe_version(source)? != *expected {
            return Err("Executable version does not match release".into());
        }
        Ok(())
    }
    pub(super) fn commit(source: &Path, target: &Target, stage: &Path) -> Result<()> {
        // `self_replace` moves the running exe aside before copying the new one in: a copy kept in
        // the stage puts it back if that copy fails (disk full, antivirus).
        let backup = stage.join("previous");
        fs::copy(&target.destination, &backup).map_err(failure)?;
        if let Err(e) = self_replace::self_replace(source) {
            tracing::debug!(%e, "Windows replacement failed");
            if !target.destination.exists() && fs::copy(&backup, &target.destination).is_err() {
                return Err(format!(
                    "Replacement failed; copy {} back to {}",
                    backup.display(),
                    target.destination.display()
                ));
            }
            return Err("Replacement failed; close other Fogu windows and retry".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_files_and_locks_fail_closed() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/updater-e2e/unit");
        fs::create_dir_all(&parent).unwrap();
        let stage = Stage::create(&parent).unwrap();
        assert_eq!(load_policy(&stage.path).prompts_shown, 0);
        fs::write(stage.path.join("update.json"), b"{").unwrap();
        assert_eq!(load_policy(&stage.path).prompts_shown, 0);
        assert!(stage.path.join("update.corrupt.json").exists());
        fs::write(stage.path.join("update.json"), vec![b' '; 4097]).unwrap();
        assert_eq!(load_policy(&stage.path).prompts_shown, 0);
        let (_, first) = prompt(&Version::new(0, 1, 7), &Policy::default(), now(), 0);
        save_policy(&stage.path, &first).unwrap();
        assert_eq!(load_policy(&stage.path).prompts_shown, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(stage.path.join("update.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let path = stage.path.join("update-policy.lock");
        let first = lock(&path).unwrap();
        assert!(lock(&path).is_err());
        drop(first);
        assert!(lock(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn probe_rejects_extra_output_and_terminates_hangs() {
        use std::os::unix::fs::PermissionsExt as _;
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/updater-e2e/unit");
        fs::create_dir_all(&parent).unwrap();
        let stage = Stage::create(&parent).unwrap();
        let binary = stage.path.join("probe");
        fs::write(&binary, "#!/bin/sh\nprintf 'fogu 0.1.7\\r\\n'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(probe_version(&binary).unwrap(), Version::new(0, 1, 7));
        fs::write(&binary, "#!/bin/sh\nprintf 'fogu 0.1.7\\nextra'\n").unwrap();
        assert!(probe_version(&binary).is_err());
        fs::write(&binary, "#!/bin/sh\nexec sleep 20\n").unwrap();
        let start = Instant::now();
        assert!(probe_version(&binary).is_err());
        assert!(start.elapsed() < Duration::from_secs(8));
    }

    #[cfg(unix)]
    #[test]
    fn clean_stale_removes_only_old_marked_stages_without_backups() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/updater-e2e/stale");
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir_all(&parent).unwrap();
        let destination = parent.join("fogu");
        let old = std::time::SystemTime::now() - Duration::from_secs(2 * DAY);
        let stage = |hex: char, age: Option<std::time::SystemTime>, backup: bool| {
            let path = parent.join(format!(".fogu-update-{}", hex.to_string().repeat(32)));
            fs::create_dir(&path).unwrap();
            fs::write(
                path.join("destination"),
                destination.to_string_lossy().as_bytes(),
            )
            .unwrap();
            if backup {
                fs::write(path.join("previous"), b"old fogu").unwrap();
            }
            if let Some(age) = age {
                File::open(&path)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(age))
                    .unwrap();
            }
            path
        };
        let fresh = stage('a', None, false);
        let expired = stage('b', Some(old), false);
        let backup = stage('c', Some(old), true);
        // A link named like a stale stage, pointing at a directory that looks like one.
        let outside = parent.join("outside");
        fs::rename(stage('d', Some(old), false), &outside).unwrap();
        let link = parent.join(format!(".fogu-update-{}", "e".repeat(32)));
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        clean_stale(&parent, &destination).unwrap();
        assert!(fresh.exists() && backup.exists() && link.exists() && outside.exists());
        assert!(!expired.exists());
        fs::remove_dir_all(&parent).unwrap();
    }

    #[test]
    fn metadata_rejects_untrusted_locations_and_ambiguous_assets() {
        let target = Target {
            destination: "fogu".into(),
            restart: "fogu".into(),
            asset: "Fogu-linux-x86_64.tar.gz",
            entry: "Fogu-linux-x86_64/fogu",
        };
        let mut metadata = serde_json::json!({
            "id": 7, "tag_name": "v0.1.7", "draft": false, "prerelease": false,
            "body": "### Fixed\n- Fixed scrolling.",
            "assets": [
                { "name": target.asset, "size": 100, "digest": format!("sha256:{}", "a".repeat(64)), "browser_download_url": url("v0.1.7", target.asset) },
                { "name": "SHA256SUMS", "size": 100, "digest": null, "browser_download_url": url("v0.1.7", "SHA256SUMS") }
            ]
        });
        let validate = |value| validate_metadata(serde_json::from_value(value).unwrap(), &target);
        assert!(validate(metadata.clone()).is_ok());
        metadata["body"] = "### Fixed\r\n- Fixed scrolling.\r\n".into();
        assert!(valid_notes(&validate(metadata.clone()).unwrap().body));
        // The page names the candidate it would install, not the version the last check saved.
        let mut updater = Updater::default();
        updater.policy.cached_latest = Some(Version::new(0, 1, 6));
        updater.release = Some(validate(metadata.clone()).unwrap());
        assert_eq!(updater.latest(), Some(&Version::new(0, 1, 7)));
        metadata["draft"] = true.into();
        assert!(validate(metadata.clone()).is_err());
        metadata["draft"] = false.into();
        metadata["prerelease"] = true.into();
        assert!(validate(metadata.clone()).is_err());
        metadata["prerelease"] = false.into();
        let duplicate = metadata["assets"][0].clone();
        metadata["assets"].as_array_mut().unwrap().push(duplicate);
        assert!(validate(metadata.clone()).is_err());
        metadata["assets"].as_array_mut().unwrap().pop();
        metadata["assets"][0]["browser_download_url"] = "https://evil.example/fogu".into();
        assert!(validate(metadata.clone()).is_err());
        metadata["assets"][0]["browser_download_url"] = url("v0.1.7", target.asset).into();
        metadata["assets"][0]["size"] = (MAX_ASSET + 1).into();
        assert!(validate(metadata).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn archive_listing_rejects_links_and_expansion_before_extraction() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/updater-e2e/unit");
        fs::create_dir_all(&parent).unwrap();
        let stage = Stage::create(&parent).unwrap();
        let archive = stage.path.join("archive.tar.gz");
        for link in [false, true] {
            let mut header = tar::Header::new_gnu();
            header.set_path("Fogu-linux-x86_64/fogu").unwrap();
            header.set_size(if link { 0 } else { MAX_EXTRACT + 1 });
            header.set_mode(0o755);
            if link {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("../../outside").unwrap();
            }
            header.set_cksum();
            let mut gzip = flate2::write::GzEncoder::new(
                File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            gzip.write_all(header.as_bytes()).unwrap();
            gzip.finish().unwrap();
            assert!(platform::list_archive(&archive).is_err());
        }
    }

    #[test]
    fn release_contract_allows_skips_and_requires_intermediate_versions() {
        let body = "### Fixed\n- Fixed scrolling.\n<!-- fogu-update: {\"schema\":1,\"from\":\">=0.1.0, <0.2.0\"} -->";
        assert!(compatible(body, &Version::new(0, 1, 2)));
        assert!(!compatible(body, &Version::new(0, 2, 0)));
        let intermediate = body.replace(">=0.1.0", ">=0.1.6");
        assert!(!compatible(&intermediate, &Version::new(0, 1, 5)));
        assert!(compatible(&intermediate, &Version::new(0, 1, 6)));
        assert!(!compatible(
            &body.replace("schema\":1", "schema\":2"),
            &Version::new(0, 1, 6)
        ));
        assert!(!compatible(
            &format!("{body}\n{body}"),
            &Version::new(0, 1, 6)
        ));
        assert!(!shown_notes(body).contains("fogu-update"));
    }

    #[test]
    fn installing_holds_window_and_last_tab_close() {
        let temp = crate::profiles::tests::tmp();
        let mut app = crate::profiles::tests::app_in(&temp.0);
        let _ = app.open_settings();
        let id = app.active;
        app.tabs.retain(|t| t.id == id);
        app.updater.status = Status::Installing("Verifying");
        let _ = app.update(Message::CloseRequested);
        assert!(app.update_busy());
        let _ = app.close_tab(id);
        assert!(app.tabs.iter().any(|t| t.id == id));
        assert!(app.update_busy());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_commit_restores_the_displaced_binary() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/updater-e2e/unit");
        fs::create_dir_all(&parent).unwrap();
        let mut stage = Stage::create(&parent).unwrap();
        let destination = stage.path.join("fogu");
        fs::write(&destination, b"previous executable").unwrap();
        let target = Target {
            destination: destination.clone(),
            restart: destination.clone(),
            asset: "",
            entry: "",
        };
        stage.preserve = true;
        assert!(platform::commit(&stage.path.join("missing"), &target, &stage.path).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"previous executable");
        assert!(!stage.path.join("previous").exists());
        stage.preserve = false;
    }

    #[test]
    fn disk_full_errors_are_actionable_through_the_dependency() {
        let error = std::io::Error::from(std::io::ErrorKind::StorageFull);
        assert_eq!(failure(error), "Not enough disk space");
    }

    #[test]
    fn reminder_budget_and_sessions() {
        let v = Version::new(0, 1, 7);
        let (show, first) = prompt(&v, &Policy::default(), 100, 90);
        assert!(show);
        assert!(!prompt(&v, &first, 100 + 3 * DAY, 90).0);
        assert!(!prompt(&v, &first, 100 + 3 * DAY - 1, 101).0);
        let (show, second) = prompt(&v, &first, 100 + 3 * DAY, 101);
        assert!(show);
        assert!(!prompt(&v, &second, 100 + 9 * DAY, 200).0);
        assert!(prompt(&Version::new(0, 1, 8), &second, 500, 400).0);
        assert!(!prompt(&Version::new(0, 1, 6), &second, 500, 400).0);
        let corrupt = Policy {
            first_prompt_at: Some(u64::MAX),
            ..first
        };
        assert!(!prompt(&v, &corrupt, 900, 800).0);
    }

    #[test]
    fn notes_and_tags_are_strict() {
        assert!(valid_notes("### Fixed\n- Fixed trackpad scrolling."));
        for body in [
            "",
            "### Fixed\n",
            "### Fixed\n- \n",
            "### Other\n- Test",
            "### Fixed\n- bad\u{202e}",
            "### Fixed\n- bad\r",
        ] {
            assert!(!valid_notes(body));
        }
        assert!(!valid_notes(&format!("### Fixed\n- {}", "x".repeat(16384))));
        for tag in ["0.1.7", "v0.1.7-beta", "v0.1.7+build", "v01.1.7"] {
            assert!(stable_tag(tag).is_err());
        }
        assert!(stable_tag("v0.1.7").is_ok());
    }

    #[test]
    fn boundary_writer_does_not_write_excess_bytes() {
        let mut bytes = Vec::new();
        let mut out = Limited {
            inner: &mut bytes,
            remaining: 3,
        };
        assert!(out.write_all(b"four").is_err());
        assert_eq!(bytes, [] as [u8; 0]);
        assert!(!safe_entry(Path::new("../fogu")));
        assert!(!safe_entry(Path::new("/fogu")));
        assert!(safe_entry(Path::new("Fogu-linux-x86_64/fogu")));
    }
}
