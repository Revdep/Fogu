//! "Follow terminal cwd" and "cd terminal here" : the remote shell's OSC 7 reports (OSC
//! side-channel scanner, `SessionOutput::Cwd { remote: true }`) move the panel while following
//! is on; manual navigation turns it off until "resume following". "cd terminal here" types a
//! quoted `cd` into the panel's shell.
//!
//! The reported path is remote-shell output (attacker data): it is only ever sent back to the
//! server as an SFTP path and shown through `display_name`; a path typed into the shell is
//! single-quoted and refused if it holds control characters.

use std::path::Path;

use iced::clipboard;

use fogu_core::shellquote::cd_command;
use fogu_session::SftpRequest;
use iced::Task;
use tokio::sync::mpsc::Sender;

use super::SftpPanelState;
use crate::app::App;
use crate::message::Message;
use crate::notifications::{Level, Notification};
use crate::tab::PaneId;

impl SftpPanelState {
    /// The panel's shell reported `path` as its cwd: remembered, and listed when following
    /// and visible. Relative or non-UTF-8 paths are ignored.
    pub fn on_shell_cwd(&mut self, path: &str, tx: &Sender<SftpRequest>) -> Result<(), String> {
        if !path.starts_with('/') {
            return Ok(());
        }
        self.shell_cwd = Some(path.to_owned());
        if !self.follow || !self.visible || self.path.as_deref() == Some(path) {
            return Ok(());
        }
        self.list_with(tx, path.to_owned(), self.path.is_some(), true)
    }

    /// The "follow terminal" toggle; turning it on goes to the terminal's cwd when known.
    pub fn set_follow(&mut self, on: bool, tx: &Sender<SftpRequest>) -> Result<(), String> {
        self.follow = on;
        match self.shell_cwd.clone() {
            Some(cwd) if on && self.path.as_deref() != Some(cwd.as_str()) => {
                self.list_with(tx, cwd, self.path.is_some(), true)
            }
            _ => Ok(()),
        }
    }
}

/// bash/zsh hook that reports the cwd (OSC 7) at every prompt; the user adds it to the remote
/// rc file themselves (Fogu never types setup commands into a session). `$PWD` is percent-encoded
/// byte by byte: a directory name holding ESC/BEL must not end the sequence and inject another.
/// `& 255`: bash 3.2 (macOS /bin/bash) reads bytes >= 0x80 as negative (`è` -> %FFFFFFFFFFFFFFC3).
pub(crate) const OSC7_SNIPPET: &str = r#"__fogu_osc7() { local LC_ALL=C s="$PWD" o= c i; for ((i=0; i<${#s}; i++)); do c=${s:$i:1}; case $c in [-/._~A-Za-z0-9]) o+=$c;; *) printf -v c '%d' "'$c"; printf -v c '%%%02X' $((c & 255)); o+=$c;; esac; done; printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-$HOST}" "$o"; }
if [ -n "$ZSH_VERSION" ]; then precmd_functions+=(__fogu_osc7); else PROMPT_COMMAND="__fogu_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}"; fi
"#;

impl App {
    /// The setup dialog's answer: closes it; `copy` puts the snippet on the clipboard.
    pub(super) fn cwd_setup_done(&mut self, tab: crate::tab::TabId, copy: bool) -> Task<Message> {
        if let Some((panel, _)) = self.panel_of(tab) {
            panel.ops.cwd_setup = false;
        }
        if !copy {
            return Task::none();
        }
        let note = Notification::new(Level::Info, "Snippet copied")
            .detail("Paste it at the end of ~/.bashrc or ~/.zshrc on the server.");
        Task::batch([clipboard::write(OSC7_SNIPPET.to_owned()), self.notify(note)])
    }

    /// A remote cwd report from pane `pane`: kept on the pane (a panel opened later starts
    /// there) and passed to the panel bound to that pane.
    pub(crate) fn on_remote_cwd(&mut self, pane: PaneId, path: &Path) -> Task<Message> {
        let Some(path) = path.to_str() else {
            return Task::none();
        };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.panes.contains_key(&pane)) else {
            return Task::none();
        };
        if let Some(p) = tab.panes.get_mut(&pane) {
            p.remote_cwd = Some(path.to_owned());
        }
        let tab = tab.id;
        let Some((panel, tx)) = self.panel_of(tab).filter(|(p, _)| p.pane == pane) else {
            return Task::none();
        };
        match panel.on_shell_cwd(path, &tx) {
            Ok(()) => Task::none(),
            Err(err) => self.notify(Notification::new(Level::Error, "SFTP").detail(err)),
        }
    }

    /// "cd terminal here": the panel's directory into its shell.
    pub(super) fn cd_here(&mut self, tab: crate::tab::TabId) -> Task<Message> {
        let scroll = self.config.terminal.scroll_on_input;
        let Some((panel, _)) = self.panel_of(tab) else {
            return Task::none();
        };
        let (Some(path), pane) = (panel.path.clone(), panel.pane) else {
            return Task::none();
        };
        let bytes = match cd_command(&path) {
            Ok(bytes) => bytes,
            Err(why) => {
                let note = Notification::new(Level::Warn, "cd terminal here").detail(why);
                return self.notify(note);
            }
        };
        // The shell will report the new cwd: keep following from there.
        panel.follow = true;
        if let Some(pane) = self.pane_mut(pane).filter(|p| p.state.is_running()) {
            pane.send_input(bytes, scroll);
            return Task::none();
        }
        let note = Notification::new(Level::Warn, "cd terminal here")
            .detail("The terminal is not connected.");
        self.notify(note)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use fogu_term::{OscEvent, OscScanner};

    use super::OSC7_SNIPPET;

    /// The snippet, sourced in a real shell, reports a cwd the scanner reads back, even with
    /// escape sequences, `%` and non-ASCII in the name (shells not installed are skipped).
    #[test]
    fn osc7_snippet_reports_the_cwd() {
        let dir = std::env::temp_dir().join("fogu osc7 \x1b]52;c;eA==\x07 %41 è");
        std::fs::create_dir_all(&dir).unwrap();
        // the shell reports the path it was given ($PWD), so give it the real one: on macOS the
        // temp dir is under /var, a symlink to /private/var
        let dir = dir.canonicalize().unwrap();
        let prompt = [
            ("bash", r#"eval "$PROMPT_COMMAND""#),
            ("zsh", "for f in $precmd_functions; do $f; done"),
        ];
        for (shell, run_prompt) in prompt {
            let script = format!("{OSC7_SNIPPET}cd \"$1\" && {run_prompt}");
            let Ok(out) = Command::new(shell)
                .args(["-c", &script, shell])
                .arg(&dir)
                .output()
            else {
                continue;
            };
            let events = OscScanner::new().scan(&out.stdout);
            let paths: Vec<PathBuf> = events
                .into_iter()
                .filter_map(|e| match e {
                    OscEvent::Cwd { path, .. } => Some(path),
                    _ => None,
                })
                .collect();
            assert_eq!(
                paths,
                std::slice::from_ref(&dir),
                "{shell}: {:?}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
    }
}
