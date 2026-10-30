//! Application messages.

use fogu_core::{Action, NodePath, RgbColor};
use fogu_session::SessionOutput;
use fogu_term::input::KeyInput;
use fogu_term::{Scroll, SelectionType};
use iced::Point;

use crate::background_menu::BgMsg;
use crate::metrics::CellPos;
use crate::profile_editor::EditorMsg;
use crate::profile_picker::PickerMsg;
use crate::profiles::ProfilesMsg;
use crate::prompt::PromptMsg;
use crate::sftp::{DesktopMsg, SftpMsg};
use crate::tab::{PaneId, TabId};

/// A step of a mouse selection gesture, in viewport cells.
#[derive(Clone, Copy, Debug)]
pub enum SelectEvent {
    /// Start a new selection of `kind` anchored at `at`.
    Start {
        /// Simple / semantic (word) / lines / block.
        kind: SelectionType,
        /// Anchor cell.
        at: CellPos,
    },
    /// Move the free end of the selection.
    Update(CellPos),
    /// The button was released: copy-on-select.
    Finish,
    /// A plain click: drop the selection.
    Clear,
}

/// Which clipboard a paste reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteSource {
    /// The `paste` action (Ctrl+Shift+V / Shift+Insert by default).
    Clipboard,
    /// Middle-click.
    Primary,
}

/// Everything `update` reacts to. Pane-level input without a [`PaneId`] (keymap actions) targets
/// the active tab's focused pane; input from a pane widget arrives wrapped in [`Message::Pane`].
#[derive(Clone, Debug)]
pub enum Message {
    /// An event from pane `.0`'s session.
    Session(PaneId, SessionOutput),
    /// Pane-level input (keys, mouse, paste, scroll) produced by pane `.0`'s widget.
    Pane(PaneId, Box<Message>),
    /// Pane `pane`'s widget now fits a different grid.
    GridResized {
        /// The pane whose rectangle changed.
        pane: PaneId,
        /// Columns that fit.
        cols: u16,
        /// Rows that fit.
        rows: u16,
    },
    /// Pane `.0`'s 50 ms resize debounce for generation `.1` expired.
    ResizeTimer(PaneId, u64),
    /// The window's scale factor (device px per logical px).
    ScaleFactor(f32),
    /// The window's new logical size (keeps context menus inside it).
    WindowResized(iced::Size),
    /// The window's mode, queried after a resize on macOS (full screen can start from the green
    /// button, not only from the action).
    WindowMode(iced::window::Mode),
    /// Ask for [`Message::WindowMode`] (after a full-screen animation settled).
    QueryWindowMode,
    /// Window focus gained or lost.
    Focused(bool),
    /// A click into pane `.0`: focus it (and the window's terminal area).
    FocusPane(PaneId),
    /// Divider drag: set the ratio of the split at `.1` in tab `.0`.
    SplitRatio(TabId, NodePath, f32),
    /// Double-click on a divider: equalize tab `.0`'s splits.
    Equalize(TabId),
    /// A split's cwd query to pane `.0` got no answer in time: split without a cwd.
    CwdTimeout(PaneId),
    /// A key press for the focused pane (no hotkey matched).
    Key(KeyInput),
    /// Text committed by the input method for the focused pane.
    Text(String),
    /// Bytes the widget generated for the application: mouse reports and alternate-scroll arrow
    /// keys. Written as-is, without scrolling the view to the bottom.
    MouseInput(Vec<u8>),
    /// Mouse selection gesture.
    Select(SelectEvent),
    /// Scroll the view through history.
    Scroll(Scroll),
    /// Read `source`'s current text and maybe paste it (paste action, middle-click).
    Paste(PasteSource),
    /// The clipboard read for a [`Message::Paste`] completed.
    PasteReady(Option<String>),
    /// The user confirmed the pending multi-line/large paste in the modal.
    ConfirmPaste,
    /// The user cancelled it.
    CancelPaste,
    /// A keymap/menu action.
    Action(Action),
    /// Middle-click or `×` on a tab header.
    CloseTab(TabId),
    /// The pointer entered header slot `.0` (a position in the bar, as iced tracks hover: a tab
    /// sliding under a still pointer keeps the slot's hover without a new enter).
    HoverTab(usize),
    /// The pointer left header slot `.0` (clears the hover only if it is still that slot's:
    /// moving left, the next header's enter arrives before this one's leave).
    LeaveTab(usize),
    /// The pointer moved over the tab bar (window coordinates); anchors the tab menu.
    TabBarCursor(Point),
    /// The tab strip scrolled or resized (only reported while it overflows).
    TabStripViewport(iced::widget::scrollable::Viewport),
    /// A wheel turn over the tab bar the strip itself ignored (vertical): scroll it sideways.
    TabBarWheel(iced::mouse::ScrollDelta),
    /// Double-click on a tab title: start inline rename.
    StartRename(TabId),
    /// The inline rename field changed.
    RenameInput(String),
    /// Enter in the rename field: commit (empty clears the override).
    CommitRename,
    /// Right-click on a tab header: open its context menu.
    OpenTabMenu(TabId),
    /// Click-away / Esc / an item was chosen: close the context menu.
    CloseMenu,
    /// "Color ▸": swap the tab menu for the colour palette.
    MenuColors,
    /// The menu's text field changed.
    MenuInput(String),
    /// Enter in the menu's text field ("Custom hex…").
    MenuSubmit,
    /// Set (or clear) the active tab's colour.
    SetTabColor(Option<RgbColor>),
    /// The tab background menu, and image decodes finishing.
    Background(BgMsg),
    /// `×` on toast `.0` (or "Deny" on an OSC 52 toast).
    DismissToast(u64),
    /// "Copy" on the OSC 52 toast `.0`.
    ToastAccept(u64),
    /// A toast's display time may be up: drop the expired ones.
    ToastExpire,
    /// Right-click on the focused pane at this window point (`terminal.right_click = menu`, or
    /// Shift held).
    PaneMenu(iced::Point),
    /// The menu's "Split ▸".
    MenuSplit,
    /// The menu's "New terminal with ▸" / "New tab ▸": the profile list.
    MenuNewWith,
    /// The pane menu's notification toggles.
    PaneNotify(PaneId, crate::pane_menu::NotifyKind),
    /// The pane menu's "Save as profile…".
    PaneSaveAsProfile(PaneId),
    /// The pane menu's "Export to file…".
    PaneExport(PaneId),
    /// The export's save dialog answered (`None`: cancelled).
    PaneExportTo(PaneId, Option<std::path::PathBuf>),
    /// Right-click on a split divider of this tab.
    DividerMenu(TabId, iced::Point),
    /// Right-click on the tab bar's empty area.
    TabBarMenu,
    /// The answer to "Discard changes?" for a profile editor tab.
    ConfirmDiscard(bool),
    /// The tab menu's "Save to profile": the tab's colour and background into its profile.
    TabSaveToProfile,
    /// Tab recovery: compare the layout with the last save (every second).
    RecoveryTick,
    /// Poll `config.toml` and apply an external edit when its file stamp changed.
    ConfigTick,
    /// The window's close button: save the session, then exit.
    CloseRequested,
    /// A title bar button or drag (the window has no OS decorations).
    Window(WindowCmd),
    /// Dragging tabs, panes and sidebar profiles.
    Drag(crate::drag::DragMsg),
    /// The link under the pointer of a pane changed (Ctrl/Cmd held, or a right-press).
    LinkHover(PaneId, Option<fogu_term::links::Link>),
    /// Ctrl+click (Cmd+click on macOS) or "Open link".
    OpenLink(PaneId, fogu_term::links::Link),
    /// "Copy link".
    CopyLink(String),
    /// The answer to "Clear saved sessions?".
    ConfirmClearSessions(bool),
    /// One second of pane `.0`'s automatic reconnect countdown passed.
    ReconnectTick(PaneId),
    /// The cursor blink timer fired.
    BlinkTick,
    /// A frame was presented (`--exit-after-first-frame`).
    Frame,
    /// Escape pressed anywhere: close the innermost overlay (picker, confirmation, prompt,
    /// menu, rename).
    Escape,
    /// Enter that no widget took (host-key dialogs have no text field).
    Enter,
    /// The SSH prompt dialog of a pane (typed text only as a redacting `PromptText`).
    Prompt(PromptMsg),
    /// The profile picker.
    Picker(PickerMsg),
    /// The profile manager sidebar.
    Profiles(ProfilesMsg),
    /// The profile editor of tab `.0`.
    Editor(TabId, EditorMsg),
    /// The settings tab.
    Settings(crate::settings::SettingsMsg),
    /// Optional updater actions and worker results.
    Update(crate::updater::UpdateMsg),
    /// A start page tab.
    Welcome(crate::tab::TabId, crate::welcome::WelcomeMsg),
    /// The "close live SSH session?" modal: `true` = close, `false` = keep it.
    ConfirmClose(bool),
    /// SFTP engine events and side-panel input.
    Sftp(SftpMsg),
    /// The port forwarding dialog.
    Forwards(crate::forwards_dialog::ForwardsMsg),
    /// The editor's "Set stored password / passphrase" dialog.
    SecretDialog(crate::secret_dialog::SecretMsg),
    /// The search bar.
    Search(crate::search::SearchMsg),
    /// A key press no widget took (unmodified key, key with modifiers applied, modifiers).
    KeyIgnored(
        iced::keyboard::Key,
        iced::keyboard::Key,
        iced::keyboard::Modifiers,
    ),
    /// The modifier keys changed (Ctrl/Shift-click selection in the SFTP list).
    Modifiers(iced::keyboard::Modifiers),
    /// A mouse button was pressed anywhere (keyboard focus between terminal and SFTP panel).
    MousePressed,
    /// Dropped files, opening local files (SFTP panel).
    Desktop(DesktopMsg),
}

impl Message {
    /// Wraps pane-level input coming from pane `pane`'s widget in [`Message::Pane`]; other
    /// messages pass through unchanged.
    pub fn for_pane(self, pane: PaneId) -> Self {
        match self {
            Self::Key(_)
            | Self::Text(_)
            | Self::MouseInput(_)
            | Self::Select(_)
            | Self::Scroll(_)
            | Self::Paste(_) => Self::Pane(pane, Box::new(self)),
            other => other,
        }
    }
}

/// What the title bar asks of the window.
#[derive(Clone, Copy, Debug)]
pub enum WindowCmd {
    /// A press on the bar's empty area: the window moves once the pointer does (so a
    /// double-click only maximizes).
    Press,
    /// The press ended without moving.
    Release,
    /// Resize from an edge or corner.
    Resize(iced::window::Direction),
    Minimize,
    ToggleMaximize,
}
