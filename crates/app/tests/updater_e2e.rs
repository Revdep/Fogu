//! End to end: the debug `fogu` binary updates itself (a plain install and an `AppImage`) from a
//! local HTTP server, through the system's curl, and every bad release leaves it untouched.
//!
//! Needs the updater compiled in and curl installed:
//! `FOGU_UPDATER=manual cargo test -p fogu --test updater_e2e -- --ignored`
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ring::rand::{SecureRandom as _, SystemRandom};

/// Bigger than the client accepts (`MAX_ASSET`).
const TOO_BIG: u64 = 128 * 1024 * 1024 + 1;

/// Serves files under `root` over HTTP on loopback; counts the requests.
fn serve(root: PathBuf) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            // The headers end with an empty line.
            let mut header = String::new();
            while reader.read_line(&mut header).unwrap() > 2 {
                header.clear();
            }
            let path = Path::new(
                line.split(' ')
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/'),
            );
            // Files under `root` only: no `..` or absolute path from another local user.
            let inside = path
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
            let mut stream = &stream;
            match inside.then(|| fs::read(root.join(path)).ok()).flatten() {
                Some(body) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream
                        .write_all(head.as_bytes())
                        .and_then(|()| stream.write_all(&body));
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), requests)
}

/// A stand-in Fogu: all the updater runs of the payload is `--version`.
fn payload(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho fogu {version}\n").into_bytes()
}

/// `entry` = `bytes` in a `.tar.gz` at `path`, zipsign-signed with `key` (context: the file name).
fn pack(path: &Path, entry: &str, bytes: &[u8], key: &zipsign_api::SigningKey) {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_ustar();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, entry, bytes).unwrap();
    let unsigned = tar.into_inner().unwrap().finish().unwrap();
    let mut signed = std::io::Cursor::new(Vec::new());
    let context = path.file_name().unwrap().to_str().unwrap().as_bytes();
    zipsign_api::sign::copy_and_sign_tar(
        &mut std::io::Cursor::new(unsigned),
        &mut signed,
        std::slice::from_ref(key),
        Some(context),
    )
    .unwrap();
    fs::write(path, signed.into_inner()).unwrap();
}

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
}

fn key() -> zipsign_api::SigningKey {
    let mut seed = [0; 32];
    SystemRandom::new().fill(&mut seed).unwrap();
    zipsign_api::SigningKey::from_bytes(&seed)
}

/// Runs the hidden headless install of `runner` with `env`.
fn install(runner: &Path, env: &[(&str, &str)], remove: &[&str]) -> Output {
    let mut command = Command::new(runner);
    command
        .arg("--updater-test-install")
        .envs(env.iter().copied());
    for name in remove {
        command.env_remove(name);
    }
    command.output().unwrap()
}

#[test]
#[ignore = "needs FOGU_UPDATER=manual at build time and curl: see the module docs"]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario table for both install kinds"
)]
fn updates_and_refuses_bad_releases() {
    assert!(
        option_env!("FOGU_UPDATER").is_some(),
        "build with FOGU_UPDATER=manual"
    );
    let fogu = Path::new(env!("CARGO_BIN_EXE_fogu"));
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("updater-e2e");
    let _ = fs::remove_dir_all(&root);
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let next = semver::Version::new(current.major, current.minor, current.patch + 1);
    let tag = format!("v{next}");
    let server = root.join("server");
    let assets = server.join("download").join(&tag);
    fs::create_dir_all(&assets).unwrap();
    fs::create_dir_all(server.join("releases")).unwrap();
    let (url, requests) = serve(server.clone());
    let (good, wrong) = (key(), key());
    let key_file = root.join("key.pub");
    fs::write(&key_file, good.verifying_key().to_bytes()).unwrap();

    for appimage in [false, true] {
        let (name, entry) = if appimage {
            ("Fogu-linux-appimage-x86_64.tar.gz", "Fogu-x86_64.AppImage")
        } else {
            ("Fogu-linux-x86_64.tar.gz", "Fogu-linux-x86_64/fogu")
        };
        let kind = if appimage { "AppImage" } else { "binary" };
        // (case, what the refusal says)
        for (case, refusal) in [
            ("opt-out", "managed by your installation method"),
            ("managed", "managed by your installation method"),
            ("no-curl", "need curl"),
            ("good", ""),
            ("bad-signature", "archive signature"),
            ("wrong-checksum", "SHA256SUMS"),
            ("oversized", "invalid size"),
            ("wrong-version", "executable version"),
            ("read-only", "installation permissions"),
        ] {
            let dir = root.join(kind).join(case);
            fs::create_dir_all(&dir).unwrap();
            // The installed copy: a hard link to the test binary (an update renames over it).
            let (runner, target) = if appimage {
                let app_dir = dir.join("AppDir");
                fs::create_dir_all(&app_dir).unwrap();
                fs::hard_link(fogu, app_dir.join("fogu")).unwrap();
                let image = dir.join("Fogu.AppImage");
                fs::write(&image, payload(&current.to_string())).unwrap();
                fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
                (app_dir.join("fogu"), image)
            } else {
                fs::hard_link(fogu, dir.join("fogu")).unwrap();
                (dir.join("fogu"), dir.join("fogu"))
            };
            let before = fs::read(&target).unwrap();

            let version = if case == "wrong-version" {
                current.to_string()
            } else {
                next.to_string()
            };
            let signer = if case == "bad-signature" {
                &wrong
            } else {
                &good
            };
            let archive = assets.join(name);
            pack(&archive, entry, &payload(&version), signer);
            let bytes = fs::read(&archive).unwrap();
            let sum = if case == "wrong-checksum" {
                "0".repeat(64)
            } else {
                sha256(&bytes)
            };
            fs::write(assets.join("SHA256SUMS"), format!("{sum}  {name}\n")).unwrap();
            let size = if case == "oversized" {
                TOO_BIG
            } else {
                bytes.len() as u64
            };
            let asset = |name: &str, size: u64, digest: Option<String>| {
                serde_json::json!({
                    "name": name, "size": size, "digest": digest,
                    "browser_download_url": format!("https://github.com/Revdep/Fogu/releases/download/{tag}/{name}"),
                })
            };
            let sums_size = fs::metadata(assets.join("SHA256SUMS")).unwrap().len();
            let metadata = serde_json::json!({
                "id": 1, "tag_name": tag, "draft": false, "prerelease": false,
                "body": "### Fixed\n- The updater test fixture.\n<!-- fogu-update: {\"schema\":1,\"from\":\">=0.0.0\"} -->",
                "assets": [asset(name, size, Some(format!("sha256:{}", sha256(&bytes)))), asset("SHA256SUMS", sums_size, None)],
            });
            fs::write(server.join("releases/latest"), metadata.to_string()).unwrap();

            let image = target.to_str().unwrap();
            let app_dir = runner.parent().unwrap().to_str().unwrap();
            let mut env = vec![
                ("FOGU_TEST_UPDATE_SERVER", url.as_str()),
                ("FOGU_TEST_UPDATE_KEY", key_file.to_str().unwrap()),
            ];
            if appimage {
                env.extend([("APPIMAGE", image), ("APPDIR", app_dir)]);
            }
            // An AppImage variable inherited from a terminal must not turn the binary into one.
            let remove: &[&str] = if appimage {
                &[]
            } else {
                &["APPIMAGE", "APPDIR"]
            };
            let empty = dir.join("empty-path");
            fs::create_dir_all(&empty).unwrap();
            match case {
                "opt-out" => env.push(("FOGU_DISABLE_UPDATER", "1")),
                "managed" => fs::write(target.parent().unwrap().join(".fogu-managed"), "").unwrap(),
                "no-curl" => env.push(("PATH", empty.to_str().unwrap())),
                "read-only" => {
                    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
                }
                _ => {}
            }
            let requests_before = requests.load(Ordering::SeqCst);
            let out = install(&runner, &env, remove);
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            let why = String::from_utf8_lossy(&out.stderr);
            if case == "good" {
                assert!(out.status.success(), "{kind} {case}: {why}");
                assert_eq!(
                    fs::read(&target).unwrap(),
                    payload(&next.to_string()),
                    "{kind}: replaced"
                );
                let mut probe = String::new();
                Command::new(&target)
                    .arg("--version")
                    .output()
                    .unwrap()
                    .stdout
                    .as_slice()
                    .read_to_string(&mut probe)
                    .unwrap();
                assert_eq!(probe.trim(), format!("fogu {next}"));
            } else {
                assert!(!out.status.success(), "{kind} {case} installed");
                assert_eq!(
                    fs::read(&target).unwrap(),
                    before,
                    "{kind} {case}: install changed"
                );
            }
            if matches!(case, "opt-out" | "managed" | "no-curl") {
                assert_eq!(
                    requests.load(Ordering::SeqCst),
                    requests_before,
                    "{kind} {case}: network used"
                );
            }
            assert!(why.contains(refusal), "{kind} {case}: {why}");
        }
    }
}
