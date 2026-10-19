//! `Config`: the TOML config tree . Every field
//! has `#[serde(default)]`; `Default` impls are the single source of defaults (no separate
//! "defaults" file).

mod edit;
mod io;
pub mod route;
pub mod sftp;
pub mod ssh_opts;
mod validate;

pub use edit::{ProfileEdit, edit_profiles, slug, unique_group_id, unique_profile_id};
pub use io::{ConfigError, config_dir, load, set_profile_background};
pub use sftp::SftpCfg;
pub use validate::{ConfigIssue, validate};

use serde::{Deserialize, Serialize};

use crate::action::Hotkeys;
use crate::profile::{Group, Osc52Policy, Profile, ProfileDefaults, ProfileId};

/// The config schema version this binary writes; `migrate` (in `io.rs`) upgrades older files.
pub const CURRENT_VERSION: u32 = 1;

/// Cursor shape as stored in config; mapped to `fogu_term::CursorShape` by the UI
/// (`ui/src/appearance.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShapeCfg {
    /// Filled block.
    #[default]
    Block,
    /// A thin vertical bar.
    Beam,
    /// A line under the cell.
    Underline,
}

/// Where the tab bar sits. Only `Top` exists for now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabsLocation {
    /// Above the terminal.
    #[default]
    Top,
}

/// How many clicks open a profile row in the profile sidebar (the picker always opens on one).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileClick {
    /// One click opens it.
    #[default]
    Single,
    /// One click selects, a double click opens.
    Double,
}

/// What a right click does by default; a profile/session may
/// still show the menu via other means (e.g. a dedicated hotkey).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RightClickAction {
    /// Opens the context menu.
    #[default]
    Menu,
    /// Pastes the primary selection (Shift+right-click still opens the menu).
    Paste,
    /// Pastes the clipboard (Shift+right-click still opens the menu).
    Clipboard,
}

/// Appearance: font, cursor, active colour scheme. `tabs_location` is data only until a second
/// placement exists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Terminal font family; `"monospace"` resolves to the system monospace font.
    pub font_family: String,
    /// Font size in points.
    pub font_size: f32,
    /// Extra fonts tried (in order) for glyphs the main family lacks (ligatures/icons later).
    pub fallback_fonts: Vec<String>,
    /// Cursor shape.
    pub cursor_shape: CursorShapeCfg,
    /// Whether the cursor blinks while focused.
    pub cursor_blink: bool,
    /// Active colour scheme name (`ColorScheme::by_name`).
    pub color_scheme: String,
    /// Tab bar placement.
    pub tabs_location: TabsLocation,
    /// Alpha (0..=1) of the terminal's default background over a tab background; unset = 0.75
    /// over an image, 0 over a solid colour. Explicit SGR backgrounds always stay opaque.
    pub terminal_bg_alpha: Option<f32>,
    /// Show the generated "Default shell" + `/etc/shells` profiles (profile sidebar and picker).
    pub show_builtin_profiles: bool,
    /// How many recently opened profiles the sidebar's "Recent" section shows (0 hides it).
    pub recent_profiles: usize,
    /// Initial width of the profile sidebar, in logical px (drag its edge to resize).
    pub profiles_sidebar_width: f32,
    /// Clicks that open a profile row in the sidebar.
    pub profiles_click: ProfileClick,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            // Matches `crates/ui/src/metrics.rs` `FontSpec::default()`.
            font_family: "monospace".to_owned(),
            font_size: 12.0,
            fallback_fonts: Vec::new(),
            cursor_shape: CursorShapeCfg::default(),
            cursor_blink: true,
            color_scheme: "Fogu Dark".to_owned(),
            tabs_location: TabsLocation::default(),
            terminal_bg_alpha: None,
            show_builtin_profiles: true,
            recent_profiles: 3,
            profiles_sidebar_width: 280.0,
            profiles_click: ProfileClick::default(),
        }
    }
}

/// Terminal behaviour: scrollback, paste policy, mouse wheel, word selection... Defaults are
/// the usual Linux terminal behaviour.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flat, directly-TOML-mapped settings record, not a state machine"
)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalCfg {
    /// Scrollback lines kept per pane.
    pub scrollback_lines: usize,
    /// Copies the selection to PRIMARY when a mouse selection finishes.
    pub copy_on_select: bool,
    /// Middle-click pastes from PRIMARY.
    pub paste_on_middle_click: bool,
    /// Confirms before pasting text with more than one line.
    pub warn_on_multiline_paste: bool,
    /// Trims leading/trailing whitespace from pasted text.
    pub trim_whitespace_on_paste: bool,
    /// Replaces newlines in pasted text with spaces.
    pub replace_newlines_on_paste: bool,
    /// Honours the application's bracketed-paste request; `false` forces plain paste even if the
    /// application asked for bracketed paste.
    pub bracketed_paste: bool,
    /// Characters that end a double-click (semantic) selection.
    pub word_separators: String,
    /// Sends an ESC prefix for Alt-modified keys.
    pub alt_is_meta: bool,
    /// Scrolls to the bottom when the user types.
    pub scroll_on_input: bool,
    /// Lines scrolled per wheel notch.
    pub wheel_lines: u32,
    /// Share of the split a resize-pane hotkey moves.
    pub pane_resize_step: f32,
    /// Default right-click behaviour.
    pub right_click: RightClickAction,
    /// Default OSC 52 (clipboard write) policy; profiles may override it
    /// (`LocalOpts`/`SshOpts::osc52_write`).
    pub osc52_write: Osc52Policy,
    /// Restart (local) / reconnect (SSH) of an ended pane keeps its screen and scrollback, below
    /// a dim "── restarted ──" / "── reconnected ──" line; `false` starts from a
    /// blank terminal. Terminal modes the old session set are reset either way. Default `true`.
    pub keep_history_on_restart: bool,
}

impl Default for TerminalCfg {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            copy_on_select: true,
            paste_on_middle_click: true,
            warn_on_multiline_paste: true,
            trim_whitespace_on_paste: true,
            replace_newlines_on_paste: false,
            bracketed_paste: true,
            word_separators: fogu_word_separators().to_owned(),
            alt_is_meta: true,
            scroll_on_input: true,
            wheel_lines: 3,
            pane_resize_step: 0.1,
            right_click: RightClickAction::default(),
            osc52_write: Osc52Policy::default(),
            keep_history_on_restart: true,
        }
    }
}

/// The default word separators, duplicated here
/// (rather than depending on `fogu_term`, which depends on this crate) — `fogu-term`'s
/// own `WORD_SEPARATORS` constant carries the same value for its unit tests/fallback.
const fn fogu_word_separators() -> &'static str {
    " ()[]{}'\""
}

/// The whole config file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema version (see `migrate` in `io.rs`).
    pub version: u32,
    /// Font/cursor/colour scheme.
    pub appearance: Appearance,
    /// Scrollback/paste/mouse behaviour.
    pub terminal: TerminalCfg,
    /// Hotkey bindings.
    pub hotkeys: Hotkeys,
    /// Defaults applied under every profile of a kind.
    pub profile_defaults: ProfileDefaults,
    /// Stored profiles (the built-in "Default shell" is not among them; see
    /// `Profile::default_shell`).
    pub profiles: Vec<Profile>,
    /// Profile groups.
    pub groups: Vec<Group>,
    /// Profile opened for new tabs when set; falls back to "Default shell".
    pub default_profile: Option<ProfileId>,
    /// SFTP panel and transfers.
    pub sftp: SftpCfg,
    /// SSH settings that are not per-profile.
    pub ssh: SshCfg,
    /// Tab recovery on restart.
    pub recovery: RecoveryCfg,
}

/// `[recovery]`: rebuild the tabs on restart. Saved scrollback may contain passwords or
/// other secrets shown in the terminal. It is stored unencrypted, readable only by your user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecoveryCfg {
    /// Save and restore the tabs, splits and their profiles. `false` also deletes saved files.
    pub enabled: bool,
    /// Also save each pane's scrollback (profiles may override with `restore_scrollback`).
    pub restore_scrollback: bool,
    /// Lines of scrollback saved per pane.
    pub scrollback_lines: usize,
}

impl Default for RecoveryCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            restore_scrollback: true,
            scrollback_lines: 1000,
        }
    }
}

/// `[ssh]`: SSH settings that are not per-profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshCfg {
    /// Show every plain `Host` of `~/.ssh/config` as a read-only profile ("SSH config" group of
    /// the profile sidebar and picker). Default `true`.
    pub read_ssh_config: bool,
}

impl Default for SshCfg {
    fn default() -> Self {
        Self {
            read_ssh_config: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            appearance: Appearance::default(),
            terminal: TerminalCfg::default(),
            hotkeys: Hotkeys::default_keymap(),
            profile_defaults: ProfileDefaults::default(),
            profiles: Vec::new(),
            groups: Vec::new(),
            default_profile: None,
            sftp: SftpCfg::default(),
            ssh: SshCfg::default(),
            recovery: RecoveryCfg::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests"
    )]

    use super::*;
    use crate::action::Action;
    use crate::keychord::KeyChord;

    #[test]
    fn default_config_round_trips_through_toml() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn default_config_has_the_default_keymap() {
        let config = Config::default();
        let new_tab = if cfg!(target_os = "macos") {
            "Cmd-T"
        } else {
            "Ctrl-Shift-T"
        };
        assert_eq!(
            config.hotkeys.lookup(KeyChord::parse(new_tab).unwrap()),
            Some(Action::NewTab)
        );
    }

    #[test]
    fn unknown_toml_keys_are_tolerated() {
        let text = r"
            totally_unknown_top_level_key = true

            [appearance]
            font_size = 20
            unknown_appearance_key = 1
        ";
        let config: Config = toml::from_str(text).unwrap();
        assert!((config.appearance.font_size - 20.0).abs() < f32::EPSILON);
        // Everything else still gets its default.
        assert_eq!(config.terminal.scrollback_lines, 10_000);
        assert!(config.terminal.keep_history_on_restart);
    }

    #[test]
    fn read_ssh_config_defaults_on_and_can_be_turned_off() {
        assert!(Config::default().ssh.read_ssh_config);
        let config: Config = toml::from_str("[ssh]\nread_ssh_config = false\n").unwrap();
        assert!(!config.ssh.read_ssh_config);
    }

    #[test]
    fn keep_history_on_restart_can_be_turned_off() {
        let text = "[terminal]\nkeep_history_on_restart = false\n";
        let config: Config = toml::from_str(text).unwrap();
        assert!(!config.terminal.keep_history_on_restart);
    }

    #[test]
    fn a_hand_written_fixture_with_two_profiles_parses_and_resolves() {
        let text = r#"
            version = 1

            [[profiles]]
            id = "local:work"
            name = "Work shell"
            type = "local"
            program = "/usr/bin/fish"

            [[profiles]]
            id = "ssh:example"
            name = "Example box"
            type = "ssh"
            host = "example.com"
            port = 2222
            user = "alice"
        "#;
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.profiles.len(), 2);
        let local = crate::profile::resolve(&config.profiles[0], &config.profile_defaults);
        let crate::profile::EffectiveProfile::Local(local) = local else {
            panic!("expected a local profile");
        };
        assert_eq!(local.program, Some("/usr/bin/fish".to_owned()));
        let ssh = crate::profile::resolve(&config.profiles[1], &config.profile_defaults);
        let crate::profile::EffectiveProfile::Ssh(ssh) = ssh else {
            panic!("expected an ssh profile");
        };
        assert_eq!(ssh.host, "example.com");
        assert_eq!(ssh.port, 2222);
        assert_eq!(ssh.user, "alice");
    }
}
