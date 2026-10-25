//! The settings tab: every `[appearance]`, `[terminal]`, `[sftp]`, `[ssh]` and `[recovery]` key
//! of config.toml as a form, one page per category (a nav column of colour-barred rows on the
//! left, like the profile sidebar). There is no Save: each valid change is written at once, in
//! place (`config::edit_settings`, only the changed keys), and applied to the running app
//! (`handlers.rs`). Hotkeys and the profile defaults are not here yet.

mod handlers;
mod view;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt::Display;
use std::str::FromStr;

use fogu_core::config::{CursorShapeCfg, ProfileClick, RightClickAction};
use fogu_core::{ColorScheme, Config, Osc52Policy};
use iced::widget::text_editor;

use crate::forms::FieldValue;

pub use handlers::SettingsMsg;
pub use view::{Ctx, page};

/// One page of the nav column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Appearance,
    Terminal,
    Sidebar,
    Sftp,
    Ssh,
    Recovery,
    /// `config.toml` as text.
    File,
    Updates,
    Licenses,
}

impl Page {
    pub const ALL: [Self; 9] = [
        Self::Appearance,
        Self::Terminal,
        Self::Sidebar,
        Self::Sftp,
        Self::Ssh,
        Self::Recovery,
        Self::File,
        Self::Updates,
        Self::Licenses,
    ];

    /// Title and the one-line summary under it in the nav column.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            Self::Appearance => ("Appearance", "Font, theme, cursor, transparency"),
            Self::Terminal => ("Terminal", "Scrollback, paste, mouse, keys"),
            Self::Sidebar => ("Profile sidebar", "Rows, recent, clicks"),
            Self::Sftp => ("SFTP", "Panel, downloads, limits"),
            Self::Ssh => ("SSH", "~/.ssh/config"),
            Self::Licenses => ("About / Licenses", "Bundled third-party licenses"),
            Self::Updates => ("Updates", "Optional updates and release notes"),
            Self::Recovery => ("Tab recovery", "Restore tabs on restart"),
            Self::File => ("config.toml", "Every key, as text"),
        }
    }
}

/// Every setting the form edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldId {
    FontFamily,
    FontSize,
    Scheme,
    CursorShape,
    CursorBlink,
    WindowOpacity,
    WindowBlur,
    RoundedCorners,
    TerminalBgAuto,
    TerminalBgAlpha,
    ShowBuiltins,
    RecentProfiles,
    SidebarWidth,
    ProfilesClick,
    Scrollback,
    CopyOnSelect,
    PasteOnMiddleClick,
    WarnMultilinePaste,
    TrimPaste,
    ReplaceNewlines,
    BracketedPaste,
    WordSeparators,
    AltIsMeta,
    ScrollOnInput,
    WheelLines,
    PaneResizeStep,
    RightClick,
    Osc52,
    KeepHistory,
    DownloadDir,
    ShowHidden,
    PanelWidth,
    FollowCwd,
    TransferLinger,
    MaxParallel,
    KeepPartial,
    RequestTimeout,
    MaxListing,
    MaxWalkEntries,
    MaxWalkDepth,
    AllowGrowing,
    MaxDownload,
    FreeSpace,
    KeepExecutable,
    WaylandDragDrop,
    ReadSshConfig,
    RecoveryEnabled,
    RecoveryScrollback,
    RecoverySshScrollback,
    RecoveryLines,
}

pub const CURSOR: [(CursorShapeCfg, &str); 3] = [
    (CursorShapeCfg::Block, "Block"),
    (CursorShapeCfg::Beam, "Beam"),
    (CursorShapeCfg::Underline, "Underline"),
];
pub const CLICKS: [(ProfileClick, &str); 2] = [
    (ProfileClick::Single, "Single click opens"),
    (ProfileClick::Double, "Double click opens"),
];
pub const RIGHT_CLICK: [(RightClickAction, &str); 3] = [
    (RightClickAction::Menu, "Context menu"),
    (RightClickAction::Paste, "Paste the selection"),
    (RightClickAction::Clipboard, "Paste the clipboard"),
];
pub const OSC52: [(Osc52Policy, &str); 3] = [
    (Osc52Policy::Allow, "Allow"),
    (Osc52Policy::Ask, "Ask each time"),
    (Osc52Policy::Deny, "Deny"),
];
/// Lowest window opacity: below it the terminal text floats on the desktop.
pub const MIN_OPACITY: f32 = 0.2;

/// The state of the settings tab.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub page: Page,
    /// What the user typed into number fields (shown instead of the stored value).
    pub texts: BTreeMap<FieldId, String>,
    /// Number fields whose text does not parse, with the reason (nothing is written).
    pub errors: BTreeMap<FieldId, String>,
    /// The installed monospace families, "monospace" first.
    pub fonts: Vec<String>,
    /// What resolving the font found missing (shown under the font fields).
    pub font_notes: Vec<String>,
    /// Why "Browse…" can't open a folder picker (`None` = it can).
    pub no_picker: Option<&'static str>,
    /// The SSH scrollback toggle waits for its explanation modal.
    pub pending_ssh_recovery: bool,
    /// The config.toml page: the text being edited, as read when the page opened.
    pub file: text_editor::Content,
    /// The file's text when it was read or saved (the editor differs = unsaved changes).
    pub file_saved: String,
    /// Why the last Save was refused.
    pub file_error: Option<String>,
}

impl Settings {
    /// Applies one widget event to `c` (a copy of the running config). `Err` = the typed text
    /// does not parse; the error is kept for the field and `c` is unchanged.
    pub fn reduce(&mut self, c: &mut Config, id: FieldId, value: &FieldValue) -> Result<(), ()> {
        self.errors.remove(&id);
        if let FieldValue::Text(text) = value {
            self.texts.insert(id, text.clone());
        } else if matches!(value, FieldValue::Step(_)) {
            // A step starts from the last valid stored value, not invalid text left in the field.
            self.texts.remove(&id);
        }
        // An empty reason = an event the field doesn't take (nothing to show).
        self.apply(c, id, value).map_err(|why| {
            if !why.is_empty() {
                self.errors.insert(id, why);
            }
        })
    }

    #[expect(clippy::too_many_lines, reason = "one reducer arm per settings field")]
    fn apply(&self, cfg: &mut Config, id: FieldId, v: &FieldValue) -> Result<(), String> {
        use FieldId as F;
        let (a, t, s) = (&mut cfg.appearance, &mut cfg.terminal, &mut cfg.sftp);
        match id {
            F::FontFamily => a.font_family = pick(v, &self.font_choices(&a.font_family))?,
            F::FontSize => a.font_size = number(v, a.font_size, 6.0, 40.0, 1.0)?,
            F::Scheme => {
                let names: Vec<String> = ColorScheme::builtins()
                    .iter()
                    .map(|s| s.name.clone())
                    .collect();
                a.color_scheme = pick(v, &names)?;
            }
            F::CursorShape => a.cursor_shape = choice(v, &CURSOR)?,
            F::CursorBlink => a.cursor_blink = on(v)?,
            F::WindowOpacity => a.window_opacity = slide(v)?.clamp(MIN_OPACITY, 1.0),
            F::WindowBlur => a.window_blur = on(v)?,
            F::RoundedCorners => a.rounded_corners = on(v)?,
            F::TerminalBgAuto => a.terminal_bg_alpha = (!on(v)?).then_some(0.75),
            F::TerminalBgAlpha => a.terminal_bg_alpha = Some(slide(v)?.clamp(0.0, 1.0)),
            F::ShowBuiltins => a.show_builtin_profiles = on(v)?,
            F::RecentProfiles => {
                a.recent_profiles = number(v, a.recent_profiles, 0, 50, 1)?;
            }
            F::SidebarWidth => {
                a.profiles_sidebar_width = number(
                    v,
                    a.profiles_sidebar_width,
                    crate::profiles::MIN_WIDTH,
                    800.0,
                    1.0,
                )?;
            }
            F::ProfilesClick => a.profiles_click = choice(v, &CLICKS)?,
            F::Scrollback => {
                t.scrollback_lines = number(v, t.scrollback_lines, 0, 1_000_000, 1)?;
            }
            F::CopyOnSelect => t.copy_on_select = on(v)?,
            F::PasteOnMiddleClick => t.paste_on_middle_click = on(v)?,
            F::WarnMultilinePaste => t.warn_on_multiline_paste = on(v)?,
            F::TrimPaste => t.trim_whitespace_on_paste = on(v)?,
            F::ReplaceNewlines => t.replace_newlines_on_paste = on(v)?,
            F::BracketedPaste => t.bracketed_paste = on(v)?,
            F::WordSeparators => text(v)?.clone_into(&mut t.word_separators),
            F::AltIsMeta => t.alt_is_meta = on(v)?,
            F::ScrollOnInput => t.scroll_on_input = on(v)?,
            F::WheelLines => t.wheel_lines = number(v, t.wheel_lines, 1, 100, 1)?,
            F::PaneResizeStep => {
                t.pane_resize_step = number(v, t.pane_resize_step, 0.01, 0.5, 0.01)?;
            }
            F::RightClick => t.right_click = choice(v, &RIGHT_CLICK)?,
            F::Osc52 => t.osc52_write = choice(v, &OSC52)?,
            F::KeepHistory => t.keep_history_on_restart = on(v)?,
            F::DownloadDir => {
                s.download_dir = match v {
                    FieldValue::Picked(path) => Some(path.clone()),
                    _ => Some(text(v)?.trim())
                        .filter(|p| !p.is_empty())
                        .map(Into::into),
                };
            }
            F::ShowHidden => s.show_hidden = on(v)?,
            F::PanelWidth => s.panel_width = number(v, s.panel_width, 0.15, 0.85, 0.01)?,
            F::FollowCwd => s.follow_cwd = on(v)?,
            F::TransferLinger => {
                s.transfer_linger_secs = number(v, s.transfer_linger_secs, 0, 3600, 1)?;
            }
            F::MaxParallel => {
                s.max_parallel_files = number(v, s.max_parallel_files, 1, 64, 1)?;
            }
            F::KeepPartial => s.keep_partial = on(v)?,
            F::RequestTimeout => {
                s.request_timeout_secs = number(v, s.request_timeout_secs, 1, 3600, 1)?;
            }
            F::MaxListing => {
                s.max_listing_entries = number(v, s.max_listing_entries, 100, 10_000_000, 1)?;
            }
            F::MaxWalkEntries => {
                s.max_walk_entries = number(v, s.max_walk_entries, 100, 10_000_000, 1)?;
            }
            F::MaxWalkDepth => {
                s.max_walk_depth = number(v, s.max_walk_depth, 1, 1024, 1)?;
            }
            F::AllowGrowing => s.allow_growing_files = on(v)?,
            F::MaxDownload => {
                let empty = matches!(v, FieldValue::Text(text) if text.trim().is_empty());
                s.max_download_bytes = if empty {
                    None
                } else {
                    Some(number(
                        v,
                        s.max_download_bytes.unwrap_or(1),
                        1,
                        u64::MAX,
                        1,
                    )?)
                };
            }
            F::FreeSpace => {
                s.free_space_reserve_bytes = number(v, s.free_space_reserve_bytes, 0, u64::MAX, 1)?;
            }
            F::KeepExecutable => s.keep_executable_bit = on(v)?,
            F::WaylandDragDrop => s.wayland_drag_and_drop = on(v)?,
            F::ReadSshConfig => cfg.ssh.read_ssh_config = on(v)?,
            F::RecoveryEnabled => cfg.recovery.enabled = on(v)?,
            F::RecoveryScrollback => cfg.recovery.restore_scrollback = on(v)?,
            F::RecoverySshScrollback => cfg.recovery.restore_ssh_scrollback = on(v)?,
            F::RecoveryLines => {
                cfg.recovery.scrollback_lines =
                    number(v, cfg.recovery.scrollback_lines, 0, 1_000_000, 1)?;
            }
        }
        Ok(())
    }

    /// The font family choices: the installed ones, plus `current` when it is not installed.
    pub fn font_choices(&self, current: &str) -> Vec<String> {
        let mut fonts = self.fonts.clone();
        if !fonts.iter().any(|f| f == current) {
            fonts.push(current.to_owned());
        }
        fonts
    }

    /// The value shown in number field `id`: what was typed, else `value`.
    pub fn shown(&self, id: FieldId, value: impl Display) -> String {
        self.texts
            .get(&id)
            .cloned()
            .unwrap_or_else(|| value.to_string())
    }
}

fn on(v: &FieldValue) -> Result<bool, String> {
    match v {
        FieldValue::Bool(b) => Ok(*b),
        _ => Err(String::new()),
    }
}

fn text(v: &FieldValue) -> Result<&str, String> {
    match v {
        FieldValue::Text(t) => Ok(t),
        _ => Err(String::new()),
    }
}

fn slide(v: &FieldValue) -> Result<f32, String> {
    match v {
        FieldValue::Number(n) => Ok(*n),
        _ => Err(String::new()),
    }
}

fn number<T: FromStr + PartialOrd + Display + Copy + Step>(
    v: &FieldValue,
    current: T,
    lo: T,
    hi: T,
    unit: T,
) -> Result<T, String> {
    let value = match v {
        FieldValue::Text(text) => {
            return text
                .trim()
                .parse()
                .ok()
                .filter(|n| (lo..=hi).contains(n))
                .ok_or_else(|| format!("a number from {lo} to {hi}"));
        }
        FieldValue::Step(by) => current.stepped(*by, unit),
        _ => return Err(String::new()),
    };
    Ok(if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    })
}

/// Adds or subtracts a field's natural unit without overflowing integer settings.
trait Step {
    fn stepped(self, by: i8, unit: Self) -> Self;
}

macro_rules! unsigned_step {
    ($($ty:ty),+ $(,)?) => {$(
        impl Step for $ty {
            fn stepped(mut self, by: i8, unit: Self) -> Self {
                for _ in 0..by.unsigned_abs() {
                    self = if by < 0 {
                        self.saturating_sub(unit)
                    } else {
                        self.saturating_add(unit)
                    };
                }
                self
            }
        }
    )+};
}

unsigned_step!(usize, u32, u64);

impl Step for f32 {
    fn stepped(self, by: i8, unit: Self) -> Self {
        self + f32::from(by) * unit
    }
}

fn choice<T: Copy>(v: &FieldValue, options: &[(T, &str)]) -> Result<T, String> {
    match v {
        FieldValue::Choice(i) => options.get(*i).map(|(t, _)| *t).ok_or_else(String::new),
        _ => Err(String::new()),
    }
}

fn pick(v: &FieldValue, options: &[String]) -> Result<String, String> {
    match v {
        FieldValue::Choice(i) => options.get(*i).cloned().ok_or_else(String::new),
        _ => Err(String::new()),
    }
}
