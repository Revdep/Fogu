//! Built-in local profiles:
//! the default shell (named after the computer) plus one profile per installed shell listed in
//! `/etc/shells`, generated at runtime and never stored. Hidden with `appearance.show_builtin_profiles = false`.

use std::collections::HashSet;
use std::path::Path;

use crate::profile::{LocalOpts, Profile, ProfileId, ProfileKind};

/// The system's list of login shells (Unix).
#[cfg(unix)]
pub const ETC_SHELLS: &str = "/etc/shells";
/// Id prefix of every built-in profile (they can't be edited, moved or deleted).
pub const BUILTIN_PREFIX: &str = "builtin:";
/// `/etc/shells` entries that are not shells to open a terminal with: login blockers and the
/// restricted shells packages add (git's `git-shell`, systemd-homed's fallback, a `homectl`
/// symlink target).
const NOT_INTERACTIVE: [&str; 5] = [
    "git-shell",
    "systemd-home-fallback-shell",
    "homectl",
    "nologin",
    "false",
];

/// "Default shell", then one login-shell profile per existing entry of the `/etc/shells`-style
/// file at `shells` (`#` comments and blank lines skipped). Entries are de-duplicated by
/// canonical path (`/bin/bash`, `/usr/bin/bash` and `/bin/sh` → bash are one shell) and named
/// after the canonical file (`bash`), which is also what runs. A missing or unreadable file
/// gives just "Default shell".
#[must_use]
pub fn builtin_profiles(shells: &Path) -> Vec<Profile> {
    let text = std::fs::read_to_string(shells).unwrap_or_default();
    let mut seen = HashSet::new();
    let mut out = vec![Profile::default_shell()];
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let listed = Path::new(line).file_name().and_then(|n| n.to_str());
        if listed.is_some_and(|n| NOT_INTERACTIVE.contains(&n)) {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(line) else {
            continue;
        };
        if real
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| NOT_INTERACTIVE.contains(&n))
        {
            continue;
        }
        if !real.is_file() || !seen.insert(real.clone()) {
            continue;
        }
        let (Some(program), Some(name)) =
            (real.to_str(), real.file_name().and_then(|n| n.to_str()))
        else {
            continue;
        };
        out.push(Profile {
            id: ProfileId(format!("{BUILTIN_PREFIX}local:{program}")),
            name: name.to_owned(),
            kind: ProfileKind::Local(LocalOpts {
                program: Some(program.to_owned()),
                // Login shells; a `-` argv0 is the portable form of `-l`.
                login: Some(true),
                ..LocalOpts::default()
            }),
            ..Profile::default_shell()
        });
    }
    out
}

/// The built-in local profiles for this platform: `/etc/shells` on Unix, the usual Windows
/// shells discovered from the system on Windows. The default shell is named after the computer
/// ("Default shell" when it has no name).
#[must_use]
pub fn builtin_shell_profiles() -> Vec<Profile> {
    #[cfg(unix)]
    let mut out = builtin_profiles(Path::new(ETC_SHELLS));
    #[cfg(windows)]
    let mut out = windows_profiles();
    if let (Some(first), Some(name)) = (out.first_mut(), crate::platform::host_name()) {
        first.name = name;
    }
    out
}

#[cfg(windows)]
fn windows_profiles() -> Vec<Profile> {
    use std::path::PathBuf;

    let mut out = vec![Profile::default_shell()];
    let mut add = |name: &str, program: PathBuf| {
        if !program.is_file() {
            return;
        }
        let program = program.to_string_lossy().into_owned();
        out.push(Profile {
            id: ProfileId(format!("{BUILTIN_PREFIX}local:{program}")),
            name: name.to_owned(),
            kind: ProfileKind::Local(LocalOpts {
                program: Some(program),
                ..LocalOpts::default()
            }),
            ..Profile::default_shell()
        });
    };

    let path = crate::platform::find_in_path;
    let system_root = std::env::var_os("SystemRoot").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);

    if let Some(pwsh) = path("pwsh.exe") {
        add("PowerShell 7", pwsh);
    }
    if let Some(root) = system_root {
        add(
            "Windows PowerShell",
            root.join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        );
        add("Command Prompt", root.join("System32/cmd.exe"));
    }
    if let Some(files) = program_files {
        add("Git Bash", files.join("Git/bin/bash.exe"));
    }
    out
}

#[cfg(all(test, unix))]
mod tests {

    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn dedupes_symlinks_and_skips_missing_entries_and_comments() {
        let dir = std::env::temp_dir().join(format!("fogu-shells-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let real = |n: &str| {
            let p = dir.join("bin").join(n);
            std::fs::write(&p, "").unwrap();
            p
        };
        let (bash, zsh) = (real("bash"), real("zsh"));
        let sh = dir.join("bin/sh");
        let _ = std::fs::remove_file(&sh);
        symlink(&bash, &sh).unwrap();
        // Arch: git adds git-shell, systemd-homed a fallback shell linked to homectl.
        let git_shell = real("git-shell");
        let homectl = real("homectl");
        let fallback = dir.join("bin/systemd-home-fallback-shell");
        let _ = std::fs::remove_file(&fallback);
        symlink(&homectl, &fallback).unwrap();
        let list = format!(
            "# comment\n\n{sh}\n{bash}\n  {zsh}  \n{gone}\n{dir}\n{bash}\n{git}\n{fallback}\n",
            git = git_shell.display(),
            fallback = fallback.display(),
            sh = sh.display(),
            bash = bash.display(),
            zsh = zsh.display(),
            gone = dir.join("bin/nope").display(),
            dir = dir.join("bin").display(),
        );
        let file = dir.join("shells");
        std::fs::write(&file, list).unwrap();

        let profiles = builtin_profiles(&file);
        let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Default shell", "bash", "zsh"]);
        let ProfileKind::Local(opts) = &profiles[1].kind else {
            panic!("local")
        };
        let canon = std::fs::canonicalize(&bash).unwrap();
        assert_eq!(opts.program.as_deref().map(PathBuf::from), Some(canon));
        assert_eq!(opts.login, Some(true));
        assert!(profiles.iter().all(|p| p.id.0.starts_with(BUILTIN_PREFIX)));

        assert_eq!(builtin_profiles(&dir.join("missing")).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
