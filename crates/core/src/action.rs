//! `Action`: the closed set of commands hotkeys (and later menus, the command palette and CLI
//! flags) resolve to. `Hotkeys` is the config's `BTreeMap<Action, Vec<KeyChord>>` plus a
//! precomputed reverse lookup.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::keychord::KeyChord;

/// A pane-navigation direction (`PaneNav`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dir {
    /// Toward the pane on the left.
    Left,
    /// Toward the pane on the right.
    Right,
    /// Toward the pane above.
    Up,
    /// Toward the pane below.
    Down,
}

impl Dir {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "left" => Self::Left,
            "right" => Self::Right,
            "up" => Self::Up,
            "down" => Self::Down,
            _ => return None,
        })
    }
}

/// One user-triggerable command. Closed set; there is no plugin system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// Opens a new tab with the default profile.
    NewTab,
    /// Closes the focused tab.
    CloseTab,
    /// Focuses the next tab.
    NextTab,
    /// Focuses the previous tab.
    PrevTab,
    /// Focuses tab number `n` (1-based).
    GoToTab(u8),
    /// Starts renaming the focused tab.
    RenameTab,
    /// Splits the focused pane, new pane to the right.
    SplitRight,
    /// Splits the focused pane, new pane below.
    SplitDown,
    /// Closes the focused pane (closes the tab if it is the last pane).
    ClosePane,
    /// Moves focus to the neighbouring pane in `Dir`.
    PaneNav(Dir),
    /// Copies the selection to the clipboard.
    Copy,
    /// Pastes from the clipboard.
    Paste,
    /// Pastes from the primary selection.
    PastePrimary,
    /// Selects the whole buffer.
    SelectAll,
    /// Opens the search bar of the focused pane.
    Find,
    /// Clears the screen and scrollback.
    Clear,
    /// Increases the font size.
    ZoomIn,
    /// Decreases the font size.
    ZoomOut,
    /// Resets the font size to the configured default.
    ZoomReset,
    /// Scrolls up one page.
    ScrollPageUp,
    /// Scrolls down one page.
    ScrollPageDown,
    /// Scrolls to the top of the scrollback.
    ScrollTop,
    /// Scrolls to the bottom (live screen).
    ScrollBottom,
    /// Toggles window fullscreen.
    ToggleFullscreen,
    /// Opens a new local tab in the focused pane's current directory.
    NewTabHere,
    /// Reconnects a disconnected session.
    Reconnect,
    /// Restarts the focused pane's process/connection.
    RestartTab,
    /// Opens the profile picker.
    OpenProfilePicker,
    /// Shows/hides the SFTP side panel of the active SSH tab.
    ToggleSftp,
    /// Shows/hides the profile manager sidebar.
    ToggleProfiles,
    /// Opens the settings tab.
    OpenSettings,
    /// Deletes the saved tab-recovery files (asks first).
    ClearSavedSessions,
    /// Opens the active tab's profile again in a new tab, in the same directory.
    DuplicateTab,
    /// Closes every tab but the active one.
    CloseOtherTabs,
    /// Closes the tabs to the right of the active one.
    CloseTabsRight,
    /// Copies the focused pane's current directory.
    CopyPath,
    /// Swaps the focused pane with its neighbour in the same split.
    SwapPanes,
    /// Gives every pane of the active tab the same share.
    EqualizePanes,
    /// Shows only the focused pane in its tab, or all of them again.
    MaximizePane,
    /// Grows the focused pane toward `Dir` by `terminal.pane_resize_step`.
    ResizePane(Dir),
}

/// An `Action` string was not one of the known slugs (e.g. `"new-tab"`, `"go-to-tab-3"`,
/// `"pane-nav-left"`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown action {0:?}")]
pub struct ActionParseError(String);

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewTab => write!(f, "new-tab"),
            Self::CloseTab => write!(f, "close-tab"),
            Self::NextTab => write!(f, "next-tab"),
            Self::PrevTab => write!(f, "prev-tab"),
            Self::GoToTab(n) => write!(f, "go-to-tab-{n}"),
            Self::RenameTab => write!(f, "rename-tab"),
            Self::SplitRight => write!(f, "split-right"),
            Self::SplitDown => write!(f, "split-down"),
            Self::ClosePane => write!(f, "close-pane"),
            Self::PaneNav(dir) => write!(f, "pane-nav-{}", dir.as_str()),
            Self::ResizePane(dir) => write!(f, "resize-pane-{}", dir.as_str()),
            Self::MaximizePane => write!(f, "pane-maximize"),
            Self::Copy => write!(f, "copy"),
            Self::Paste => write!(f, "paste"),
            Self::PastePrimary => write!(f, "paste-primary"),
            Self::SelectAll => write!(f, "select-all"),
            Self::Find => write!(f, "find"),
            Self::Clear => write!(f, "clear"),
            Self::ZoomIn => write!(f, "zoom-in"),
            Self::ZoomOut => write!(f, "zoom-out"),
            Self::ZoomReset => write!(f, "zoom-reset"),
            Self::ScrollPageUp => write!(f, "scroll-page-up"),
            Self::ScrollPageDown => write!(f, "scroll-page-down"),
            Self::ScrollTop => write!(f, "scroll-top"),
            Self::ScrollBottom => write!(f, "scroll-bottom"),
            Self::ToggleFullscreen => write!(f, "toggle-fullscreen"),
            Self::NewTabHere => write!(f, "new-tab-here"),
            Self::Reconnect => write!(f, "reconnect"),
            Self::RestartTab => write!(f, "restart-tab"),
            Self::OpenProfilePicker => write!(f, "open-profile-picker"),
            Self::ToggleSftp => write!(f, "toggle-sftp"),
            Self::ToggleProfiles => write!(f, "toggle-profiles"),
            Self::OpenSettings => write!(f, "open-settings"),
            Self::ClearSavedSessions => write!(f, "clear-saved-sessions"),
            Self::DuplicateTab => write!(f, "duplicate-tab"),
            Self::CloseOtherTabs => write!(f, "close-other-tabs"),
            Self::CloseTabsRight => write!(f, "close-tabs-right"),
            Self::CopyPath => write!(f, "copy-current-path"),
            Self::SwapPanes => write!(f, "swap-panes"),
            Self::EqualizePanes => write!(f, "equalize-panes"),
        }
    }
}

impl FromStr for Action {
    type Err = ActionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(rest) = s.strip_prefix("go-to-tab-") {
            return rest
                .parse::<u8>()
                .map(Self::GoToTab)
                .map_err(|_| ActionParseError(s.to_owned()));
        }
        if let Some(rest) = s.strip_prefix("pane-nav-") {
            return Dir::parse(rest)
                .map(Self::PaneNav)
                .ok_or_else(|| ActionParseError(s.to_owned()));
        }
        if let Some(rest) = s.strip_prefix("resize-pane-") {
            return Dir::parse(rest)
                .map(Self::ResizePane)
                .ok_or_else(|| ActionParseError(s.to_owned()));
        }
        Ok(match s {
            "new-tab" => Self::NewTab,
            "close-tab" => Self::CloseTab,
            "next-tab" => Self::NextTab,
            "prev-tab" => Self::PrevTab,
            "rename-tab" => Self::RenameTab,
            "split-right" => Self::SplitRight,
            "split-down" => Self::SplitDown,
            "close-pane" => Self::ClosePane,
            "copy" => Self::Copy,
            "paste" => Self::Paste,
            "paste-primary" => Self::PastePrimary,
            "select-all" => Self::SelectAll,
            "find" => Self::Find,
            "clear" => Self::Clear,
            "zoom-in" => Self::ZoomIn,
            "zoom-out" => Self::ZoomOut,
            "zoom-reset" => Self::ZoomReset,
            "scroll-page-up" => Self::ScrollPageUp,
            "scroll-page-down" => Self::ScrollPageDown,
            "scroll-top" => Self::ScrollTop,
            "scroll-bottom" => Self::ScrollBottom,
            "toggle-fullscreen" => Self::ToggleFullscreen,
            "new-tab-here" => Self::NewTabHere,
            "reconnect" => Self::Reconnect,
            "restart-tab" => Self::RestartTab,
            "open-profile-picker" => Self::OpenProfilePicker,
            "toggle-sftp" => Self::ToggleSftp,
            "toggle-profiles" => Self::ToggleProfiles,
            "open-settings" => Self::OpenSettings,
            "clear-saved-sessions" => Self::ClearSavedSessions,
            "duplicate-tab" => Self::DuplicateTab,
            "close-other-tabs" => Self::CloseOtherTabs,
            "close-tabs-right" => Self::CloseTabsRight,
            "copy-current-path" => Self::CopyPath,
            "swap-panes" => Self::SwapPanes,
            "equalize-panes" => Self::EqualizePanes,
            "pane-maximize" => Self::MaximizePane,
            _ => return Err(ActionParseError(s.to_owned())),
        })
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(D::Error::custom)
    }
}

/// The config's `hotkeys` table: one or more chords per action, plus a chord → action reverse
/// index built once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hotkeys {
    bindings: BTreeMap<Action, Vec<KeyChord>>,
    reverse: HashMap<KeyChord, Action>,
}

impl Hotkeys {
    /// Builds the reverse index from `bindings`. When the same chord is bound to more than one
    /// action, the later one (in `Action`'s declaration order) wins the lookup; [`validate`]
    /// flags the ambiguity so the user can fix it.
    ///
    /// [`validate`]: crate::config::validate
    #[must_use]
    pub fn new(bindings: BTreeMap<Action, Vec<KeyChord>>) -> Self {
        let mut reverse = HashMap::new();
        for (&action, chords) in &bindings {
            for &chord in chords {
                reverse.insert(chord, action);
            }
        }
        Self { bindings, reverse }
    }

    /// The default keymap. Actions that are rarely used or easy to trigger by accident (close pane,
    /// clear, reconnect, restart) default unbound; `PastePrimary`/`NewTabHere` are reached with the
    /// mouse/context menu, so they default unbound as well. Ctrl-PageUp/PageDown switch tabs like
    /// most Linux terminals; scrolling uses Shift with `PageUp`, `PageDown`, `Home` and `End` (plus
    /// Alt-PageUp/PageDown).
    #[must_use]
    pub fn default_keymap() -> Self {
        Self::default_keymap_for(cfg!(target_os = "macos"))
    }

    /// The default keymap of Linux, or of macOS when `macos` (Cmd-based; actions only Fogu has
    /// move Ctrl-Shift to Cmd-Shift).
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "fixed table of valid chord strings; covered by \
                  `default_keymap_parses` below"
    )]
    pub fn default_keymap_for(macos: bool) -> Self {
        let chord = |s: &str| KeyChord::parse(s).expect("valid default chord");
        let chords = |strs: &[&str]| strs.iter().map(|s| chord(s)).collect::<Vec<_>>();

        let mut bindings = BTreeMap::new();
        bindings.insert(Action::NewTab, chords(&["Ctrl-Shift-T"]));
        bindings.insert(Action::CloseTab, chords(&["Ctrl-Shift-W"]));
        bindings.insert(
            Action::NextTab,
            chords(&["Ctrl-Shift-Right", "Ctrl-Tab", "Ctrl-PageDown"]),
        );
        bindings.insert(
            Action::PrevTab,
            chords(&["Ctrl-Shift-Left", "Ctrl-Shift-Tab", "Ctrl-PageUp"]),
        );
        for n in 1..=9u8 {
            bindings.insert(Action::GoToTab(n), chords(&[&format!("Alt-{n}")]));
        }
        bindings.insert(Action::GoToTab(10), chords(&["Alt-0"]));
        bindings.insert(Action::RenameTab, chords(&["Ctrl-Shift-R"]));
        bindings.insert(Action::SplitRight, chords(&["Ctrl-Shift-S"]));
        bindings.insert(Action::SplitDown, chords(&["Ctrl-Shift-D"]));
        bindings.insert(Action::ClosePane, chords(&[]));
        bindings.insert(Action::PaneNav(Dir::Right), chords(&["Ctrl-Alt-Right"]));
        bindings.insert(Action::PaneNav(Dir::Down), chords(&["Ctrl-Alt-Down"]));
        bindings.insert(Action::PaneNav(Dir::Up), chords(&["Ctrl-Alt-Up"]));
        bindings.insert(Action::PaneNav(Dir::Left), chords(&["Ctrl-Alt-Left"]));
        bindings.insert(Action::Copy, chords(&["Ctrl-Shift-C"]));
        bindings.insert(Action::Paste, chords(&["Ctrl-Shift-V", "Shift-Insert"]));
        bindings.insert(Action::PastePrimary, chords(&[]));
        bindings.insert(Action::SelectAll, chords(&["Ctrl-Shift-A"]));
        bindings.insert(Action::Find, chords(&["Ctrl-Shift-F"]));
        bindings.insert(Action::Clear, chords(&[]));
        bindings.insert(Action::ZoomIn, chords(&["Ctrl-=", "Ctrl-Shift-="]));
        bindings.insert(Action::ZoomOut, chords(&["Ctrl--", "Ctrl-Shift--"]));
        bindings.insert(Action::ZoomReset, chords(&["Ctrl-0"]));
        bindings.insert(
            Action::ScrollPageUp,
            chords(&["Alt-PageUp", "Shift-PageUp"]),
        );
        bindings.insert(
            Action::ScrollPageDown,
            chords(&["Alt-PageDown", "Shift-PageDown"]),
        );
        bindings.insert(Action::ScrollTop, chords(&["Shift-Home"]));
        bindings.insert(Action::ScrollBottom, chords(&["Shift-End"]));
        bindings.insert(Action::ToggleFullscreen, chords(&["F11"]));
        bindings.insert(Action::NewTabHere, chords(&[]));
        bindings.insert(Action::Reconnect, chords(&[]));
        bindings.insert(Action::RestartTab, chords(&[]));
        bindings.insert(Action::OpenProfilePicker, chords(&["Ctrl-Shift-E"]));
        bindings.insert(Action::ToggleSftp, chords(&["Ctrl-Shift-O"]));
        // M = manager.
        bindings.insert(Action::ToggleProfiles, chords(&["Ctrl-Shift-M"]));
        bindings.insert(Action::OpenSettings, chords(&["Ctrl-,"]));
        bindings.insert(Action::ClearSavedSessions, chords(&[]));
        bindings.insert(Action::DuplicateTab, chords(&[]));
        bindings.insert(Action::CloseOtherTabs, chords(&[]));
        bindings.insert(Action::CloseTabsRight, chords(&[]));
        bindings.insert(Action::CopyPath, chords(&[]));
        bindings.insert(Action::SwapPanes, chords(&[]));
        bindings.insert(Action::EqualizePanes, chords(&[]));
        bindings.insert(Action::MaximizePane, chords(&["Ctrl-Alt-Enter"]));
        // Ctrl+Alt+arrows already move focus, Shift makes them resize.
        for (dir, key) in [
            (Dir::Left, "Ctrl-Alt-Shift-Left"),
            (Dir::Right, "Ctrl-Alt-Shift-Right"),
            (Dir::Up, "Ctrl-Alt-Shift-Up"),
            (Dir::Down, "Ctrl-Alt-Shift-Down"),
        ] {
            bindings.insert(Action::ResizePane(dir), chords(&[key]));
        }
        if macos {
            macos_overrides(&mut bindings);
        }
        Self::new(bindings)
    }

    /// The action bound to `chord`, if any.
    #[must_use]
    pub fn lookup(&self, chord: KeyChord) -> Option<Action> {
        self.reverse.get(&chord).copied()
    }

    /// The chords bound to `action` (empty if unbound).
    #[must_use]
    pub fn chords_for(&self, action: Action) -> &[KeyChord] {
        self.bindings.get(&action).map_or(&[], Vec::as_slice)
    }

    /// The underlying table, e.g. for [`validate`](crate::config::validate).
    #[must_use]
    pub const fn bindings(&self) -> &BTreeMap<Action, Vec<KeyChord>> {
        &self.bindings
    }
}

impl Serialize for Hotkeys {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.bindings.serialize(serializer)
    }
}

/// macOS replaces these defaults (see [`Hotkeys::default_keymap_for`]).
#[expect(
    clippy::expect_used,
    reason = "fixed table of valid chord strings; covered by `default_keymap_macos`"
)]
fn macos_overrides(bindings: &mut BTreeMap<Action, Vec<KeyChord>>) {
    let chords = |strs: &[&str]| {
        strs.iter()
            .map(|s| KeyChord::parse(s).expect("valid default chord"))
            .collect::<Vec<_>>()
    };
    let mac: &[(Action, &[&str])] = &[
        (Action::NewTab, &["Cmd-T"]),
        (Action::CloseTab, &["Cmd-W"]),
        (Action::NextTab, &["Ctrl-Tab"]),
        (Action::PrevTab, &["Ctrl-Shift-Tab"]),
        (Action::GoToTab(10), &[]),
        (Action::RenameTab, &["Cmd-R"]),
        (Action::SplitRight, &["Cmd-Shift-D"]),
        (Action::SplitDown, &["Cmd-D"]),
        (Action::ClosePane, &["Cmd-Shift-W"]),
        (Action::MaximizePane, &["Cmd-Alt-Enter"]),
        (Action::Copy, &["Cmd-C"]),
        (Action::Paste, &["Cmd-V"]),
        (Action::SelectAll, &["Cmd-A"]),
        (Action::Find, &["Cmd-F"]),
        (Action::Clear, &["Cmd-K"]),
        (Action::ZoomIn, &["Cmd-=", "Cmd-Shift-="]),
        (Action::ZoomOut, &["Cmd--", "Cmd-Shift--"]),
        (Action::ZoomReset, &["Cmd-0"]),
        (Action::ScrollPageUp, &["Alt-PageUp"]),
        (Action::ScrollPageDown, &["Alt-PageDown"]),
        (Action::ScrollTop, &["Shift-PageUp"]),
        (Action::ScrollBottom, &["Shift-PageDown"]),
        (Action::ToggleFullscreen, &["Ctrl-Cmd-F"]),
        (Action::OpenProfilePicker, &["Cmd-E"]),
        (Action::ToggleSftp, &["Cmd-Shift-O"]),
        (Action::ToggleProfiles, &["Cmd-Shift-M"]),
        (Action::OpenSettings, &["Cmd-,"]),
    ];
    for (action, keys) in mac {
        bindings.insert(*action, chords(keys));
    }
    for n in 1..=9u8 {
        bindings.insert(Action::GoToTab(n), chords(&[&format!("Cmd-{n}")]));
    }
    for (dir, key) in [
        (Dir::Left, "Left"),
        (Dir::Right, "Right"),
        (Dir::Up, "Up"),
        (Dir::Down, "Down"),
    ] {
        bindings.insert(Action::PaneNav(dir), chords(&[&format!("Cmd-Alt-{key}")]));
        let resize = format!("Cmd-Alt-Shift-{key}");
        bindings.insert(Action::ResizePane(dir), chords(&[&resize]));
    }
}

impl<'de> Deserialize<'de> for Hotkeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `[hotkeys]` overrides the defaults per action (`[]` unbinds); a chord the user assigns is
        // taken away from whichever default action had it.
        let user = BTreeMap::<Action, Vec<KeyChord>>::deserialize(deserializer)?;
        let claimed: HashSet<KeyChord> = user.values().flatten().copied().collect();
        let mut bindings = Self::default_keymap().bindings;
        for chords in bindings.values_mut() {
            chords.retain(|chord| !claimed.contains(chord));
        }
        bindings.extend(user);
        Ok(Self::new(bindings))
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn user_hotkeys_merge_over_the_defaults() {
        // The platform's defaults: Ctrl-Shift-T / Ctrl-Shift-W, or Cmd-T / Cmd-W on macOS.
        let defaults = Hotkeys::default_keymap();
        let first = |a| defaults.chords_for(a).first().copied().unwrap();
        let (new_tab, close_tab) = (first(Action::NewTab), first(Action::CloseTab));
        let keymap: Hotkeys =
            toml::from_str(&format!("copy = [\"{new_tab}\"]\npaste = []")).unwrap();
        assert_eq!(keymap.lookup(new_tab), Some(Action::Copy));
        assert_eq!(keymap.lookup(close_tab), Some(Action::CloseTab));
        assert_eq!(keymap.bindings()[&Action::NewTab], Vec::<KeyChord>::new());
        assert_eq!(keymap.bindings()[&Action::Paste], Vec::<KeyChord>::new());
    }

    #[test]
    fn default_keymap_macos() {
        let keymap = Hotkeys::default_keymap_for(true);
        let chord = |s| KeyChord::parse(s).unwrap();
        assert_eq!(keymap.lookup(chord("Cmd-T")), Some(Action::NewTab));
        assert_eq!(keymap.lookup(chord("Cmd-D")), Some(Action::SplitDown));
        assert_eq!(
            keymap.lookup(chord("Cmd-Shift-D")),
            Some(Action::SplitRight)
        );
        assert_eq!(keymap.lookup(chord("Cmd-9")), Some(Action::GoToTab(9)));
        assert_eq!(keymap.lookup(chord("Cmd-F")), Some(Action::Find));
        assert_eq!(
            keymap.lookup(chord("Cmd-Option-Left")),
            Some(Action::PaneNav(Dir::Left))
        );
        assert_eq!(
            keymap.lookup(chord("Ctrl-Cmd-F")),
            Some(Action::ToggleFullscreen)
        );
        // Linux chords are gone, so the shell gets them.
        assert_eq!(keymap.lookup(chord("Ctrl-Shift-T")), None);
        assert_eq!(keymap.lookup(chord("Alt-1")), None);
    }

    #[test]
    fn default_keymap_parses() {
        let keymap = Hotkeys::default_keymap_for(false);
        assert_eq!(
            keymap.lookup(KeyChord::parse("Ctrl-Shift-T").unwrap()),
            Some(Action::NewTab)
        );
        assert_eq!(
            keymap.lookup(KeyChord::parse("Alt-1").unwrap()),
            Some(Action::GoToTab(1))
        );
        assert_eq!(
            keymap.lookup(KeyChord::parse("Alt-0").unwrap()),
            Some(Action::GoToTab(10))
        );
        assert_eq!(keymap.chords_for(Action::ClosePane), []);
    }

    #[test]
    fn action_strings_round_trip() {
        let actions = [
            Action::NewTab,
            Action::GoToTab(3),
            Action::PaneNav(Dir::Left),
            Action::OpenProfilePicker,
            Action::ToggleSftp,
            Action::ToggleProfiles,
            Action::OpenSettings,
            Action::ClearSavedSessions,
            Action::DuplicateTab,
            Action::CloseOtherTabs,
            Action::CloseTabsRight,
            Action::CopyPath,
            Action::SwapPanes,
            Action::EqualizePanes,
            Action::MaximizePane,
            Action::ResizePane(Dir::Up),
            Action::Find,
        ];
        for action in actions {
            let s = action.to_string();
            assert_eq!(s.parse::<Action>().unwrap(), action, "{s:?}");
        }
        assert!("not-an-action".parse::<Action>().is_err());
        assert!("go-to-tab-not-a-number".parse::<Action>().is_err());
        assert!("pane-nav-sideways".parse::<Action>().is_err());
    }

    #[test]
    fn hotkeys_serde_round_trips_through_toml() {
        let keymap = Hotkeys::default_keymap();
        let text = toml::to_string(&keymap).unwrap();
        let back: Hotkeys = toml::from_str(&text).unwrap();
        assert_eq!(back.bindings(), keymap.bindings());
    }
}
