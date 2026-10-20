//! Child environment for local shells.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

/// Variables that must not leak from our process into user shells.
const REMOVED: &[&str] = &[
    "APPDIR",
    "APPIMAGE",
    "ARGV0",
    "DESKTOP_STARTUP_ID",
    "XDG_ACTIVATION_TOKEN",
    "RUST_LOG",
    "RUST_BACKTRACE",
];

/// Library overrides set by the `AppImage` runtime; removed only when running as an `AppImage`.
const APPIMAGE_LIB_VARS: &[&str] = &["LD_LIBRARY_PATH", "LD_PRELOAD"];

/// Builds the child environment: `parent` minus [`REMOVED`] (plus `LD_*` under an `AppImage`),
/// terminal identification variables, then `profile_env` with `${VAR}` expanded against that
/// result (unknown variables expand to nothing).
pub fn build_env<I>(
    parent: I,
    profile_env: &BTreeMap<String, String>,
) -> BTreeMap<OsString, OsString>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let mut env: BTreeMap<OsString, OsString> = parent.into_iter().collect();
    if env.contains_key(OsStr::new("APPIMAGE")) {
        for key in APPIMAGE_LIB_VARS {
            env.remove(OsStr::new(key));
        }
    }
    for key in REMOVED {
        env.remove(OsStr::new(key));
    }
    for (key, value) in [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("TERM_PROGRAM", "fogu"),
        ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
    ] {
        env.insert(key.into(), value.into());
    }
    if cfg!(target_os = "macos") && !LOCALE_VARS.iter().any(|k| env.contains_key(OsStr::new(k))) {
        env.insert("LANG".into(), system_lang().into());
    }
    let expanded: Vec<(OsString, OsString)> = profile_env
        .iter()
        .map(|(key, value)| (key.into(), expand(value, &env).into()))
        .collect();
    env.extend(expanded);
    env
}

/// Set by the user (any one is enough): then no `LANG` is added on macOS.
const LOCALE_VARS: &[&str] = &["LANG", "LC_ALL", "LC_CTYPE"];

/// macOS: `LANG` from the system language, as Terminal.app does (launchd gives an app started
/// from Finder no locale). Looked up once per run.
/// shortcut: runs `defaults` (~30 ms) on the first local shell; reading the preferences plist
/// directly needs a plist parser.
fn system_lang() -> &'static str {
    static LANG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LANG.get_or_init(|| {
        let locale = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleLocale"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok());
        lang_for(locale.as_deref(), |name| {
            std::path::Path::new("/usr/share/locale")
                .join(name)
                .is_dir()
        })
    })
}

/// `<locale>.UTF-8` for an `AppleLocale` value (`en_US@rg=itzzzz` gives `en_US.UTF-8`), else
/// `en_US.UTF-8` when that locale is not `installed` (`en_IT`).
fn lang_for(apple_locale: Option<&str>, installed: impl Fn(&str) -> bool) -> String {
    apple_locale
        .and_then(|l| l.trim().split('@').next())
        .filter(|l| !l.is_empty())
        .map(|l| format!("{l}.UTF-8"))
        .filter(|l| installed(l))
        .unwrap_or_else(|| "en_US.UTF-8".to_owned())
}

/// Replaces every `${NAME}` in `value` with `NAME` from `env`; an unterminated `${` is literal.
fn expand(value: &str, env: &BTreeMap<OsString, OsString>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let (before, tail) = rest.split_at(start);
        out.push_str(before);
        let Some(end) = tail.find('}') else {
            out.push_str(tail);
            return out;
        };
        let name = tail.get(2..end).unwrap_or_default();
        if let Some(found) = env.get(OsStr::new(name)) {
            out.push_str(&found.to_string_lossy());
        }
        rest = tail.get(end + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_follows_the_system_locale_if_installed() {
        let installed = |l: &str| ["it_IT.UTF-8", "en_US.UTF-8"].contains(&l);
        assert_eq!(lang_for(Some("it_IT\n"), installed), "it_IT.UTF-8");
        assert_eq!(lang_for(Some("en_US@rg=itzzzz"), installed), "en_US.UTF-8");
        assert_eq!(lang_for(Some("en_IT"), installed), "en_US.UTF-8");
        assert_eq!(lang_for(Some(""), installed), "en_US.UTF-8");
        assert_eq!(lang_for(None, installed), "en_US.UTF-8");
    }
}
