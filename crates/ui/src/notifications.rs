//! Toasts: a short list of messages drawn top-right. Info and warnings dismiss themselves after
//! [`AUTO_DISMISS`]; errors and anything with buttons stay until the user acts. Nothing the app
//! tells the user is silent.

use std::time::{Duration, Instant};

use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Color, Element, Length, Padding, Theme};

use crate::message::Message;
use crate::palette::Look;

/// Toast width in logical px.
const TOAST_W: f32 = 360.0;

/// How long an info/warning toast stays.
pub const AUTO_DISMISS: Duration = Duration::from_secs(8);
/// Toasts kept at once; the oldest auto-dismissing one makes room.
const MAX_TOASTS: usize = 6;
/// Longest detail text shown (attacker-influenced strings are cut, not wrapped forever).
const MAX_DETAIL_CHARS: usize = 300;

/// Severity of a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Informational; auto-dismisses.
    Info,
    /// Something was ignored or replaced by a default; auto-dismisses.
    Warn,
    /// Something failed; sticky until dismissed.
    Error,
}

/// Buttons a toast can carry besides the dismiss `×`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToastAction {
    /// OSC 52 write waiting for the user's decision: "Copy" applies it, "Deny" drops it.
    ClipboardWrite,
    /// A quick connect just opened: "Save as profile…" opens the editor on it.
    SaveAsProfile(fogu_core::ProfileId),
    /// A finished download: "Open" (`file`, single-entry downloads) and "Show in file
    /// manager" (`dir`), both through the desktop's opener and only on click.
    Downloaded {
        file: Option<std::path::PathBuf>,
        dir: std::path::PathBuf,
    },
}

/// One toast.
#[derive(Clone, Debug)]
pub struct Notification {
    /// Assigned by [`Toasts::push`]; 0 before.
    pub id: u64,
    /// Severity.
    pub level: Level,
    /// Headline.
    pub title: String,
    /// Optional second line.
    pub detail: String,
    /// When it was created.
    pub created: Instant,
    /// Extra buttons; a toast waiting for a decision (OSC 52) never auto-dismisses.
    pub action: Option<ToastAction>,
}

impl Notification {
    /// A toast without a detail line.
    pub fn new(level: Level, title: impl Into<String>) -> Self {
        Self {
            id: 0,
            level,
            title: title.into(),
            detail: String::new(),
            created: Instant::now(),
            action: None,
        }
    }

    /// Adds a detail line (control characters removed, length capped).
    #[must_use]
    pub fn detail(mut self, detail: impl AsRef<str>) -> Self {
        self.detail = detail
            .as_ref()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_DETAIL_CHARS)
            .collect();
        self
    }

    /// Adds buttons.
    #[must_use]
    pub fn action(mut self, action: ToastAction) -> Self {
        self.action = Some(action);
        self
    }

    /// Whether the toast removes itself after [`AUTO_DISMISS`].
    pub fn auto_dismisses(&self) -> bool {
        self.level != Level::Error && self.action != Some(ToastAction::ClipboardWrite)
    }

    /// Whether its time is up at `now`.
    pub fn expired(&self, now: Instant) -> bool {
        self.auto_dismisses() && now.saturating_duration_since(self.created) >= AUTO_DISMISS
    }
}

/// The visible toasts, oldest first.
#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Notification>,
    next_id: u64,
}

impl Toasts {
    /// Adds `note`; returns its id. Over capacity, the oldest auto-dismissing toast goes first
    /// (else the oldest).
    pub fn push(&mut self, mut note: Notification) -> u64 {
        self.next_id += 1;
        note.id = self.next_id;
        if self.items.len() >= MAX_TOASTS {
            let victim = self
                .items
                .iter()
                .position(Notification::auto_dismisses)
                .unwrap_or(0);
            self.items.remove(victim);
        }
        self.items.push(note);
        self.next_id
    }

    /// Removes toast `id`; `false` when it was already gone.
    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|n| n.id != id);
        self.items.len() != before
    }

    /// Removes every toast whose time is up at `now`; returns their ids.
    pub fn expire(&mut self, now: Instant) -> Vec<u64> {
        let gone: Vec<u64> = self
            .items
            .iter()
            .filter(|n| n.expired(now))
            .map(|n| n.id)
            .collect();
        self.items.retain(|n| !n.expired(now));
        gone
    }

    /// The toast `id`, if still shown.
    pub fn get(&self, id: u64) -> Option<&Notification> {
        self.items.iter().find(|n| n.id == id)
    }

    /// Whether nothing is shown.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The toasts, oldest first.
    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &Notification> {
        self.items.iter()
    }

    /// The stack pinned to the top-right of the area it is layered over, `top` px down so it
    /// clears the first terminal row; `None` when empty.
    pub fn view(&self, top: f32, look: Look) -> Option<Element<'_, Message>> {
        if self.items.is_empty() {
            return None;
        }
        let stack = column(self.items.iter().map(|note| toast_view(note, look))).spacing(8);
        Some(
            container(stack)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::Start)
                .padding(Padding::new(12.0).top(top + 12.0))
                .into(),
        )
    }
}

fn toast_view(note: &Notification, look: Look) -> Element<'_, Message> {
    let id = note.id;
    let flat = |label: &'static str, message| crate::sftp::panel::flat(label, look, Some(message));
    let mut body = column![
        text(&note.title)
            .size(14)
            .font(crate::UI_MEDIUM)
            .color(look.fg)
    ]
    .spacing(4);
    if !note.detail.is_empty() {
        body = body.push(text(&note.detail).size(13).color(look.dim));
    }
    match &note.action {
        Some(ToastAction::ClipboardWrite) => {
            body = body.push(
                row![
                    flat("Copy", Message::ToastAccept(id)),
                    flat("Deny", Message::DismissToast(id)),
                ]
                .spacing(8),
            );
        }
        Some(ToastAction::Downloaded { file, dir }) => {
            let open = |path: &std::path::Path| {
                Message::Desktop(crate::sftp::DesktopMsg::Open(path.to_path_buf()))
            };
            let mut buttons = row![].spacing(8);
            if let Some(file) = file {
                buttons = buttons.push(flat("Open", open(file)));
            }
            body = body.push(buttons.push(flat("Show in file manager", open(dir))));
        }
        Some(ToastAction::SaveAsProfile(_)) => {
            body = body.push(flat("Save as profile…", Message::ToastAccept(id)));
        }
        None => {}
    }
    let close = button(crate::icons::icon(
        crate::icons::Icon::Close,
        13.0,
        look.dim,
    ))
    .width(20)
    .height(20)
    .padding(0)
    .style(move |_theme: &Theme, status| button::Style {
        background: matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then_some(look.hover.into()),
        border: iced::Border::default().rounded(4),
        ..button::Style::default()
    })
    .on_press(Message::DismissToast(id));
    let level = match note.level {
        Level::Info => look.accent,
        Level::Warn => Color::from_rgb8(0xf5, 0xa6, 0x24),
        Level::Error => look.danger,
    };
    let dot = container(iced::widget::space().width(8).height(8))
        .padding(iced::Padding::ZERO)
        .style(move |_theme: &Theme| container::Style {
            background: Some(level.into()),
            border: iced::Border::default().rounded(4),
            ..container::Style::default()
        });
    let dot = container(dot).padding(Padding::ZERO.top(6));
    container(
        row![dot, body.width(Length::Fill), close]
            .spacing(10)
            .align_y(Alignment::Start),
    )
    .padding([12, 14])
    // Fixed width: a Fill child would stretch the column and defeat the right alignment.
    .width(TOAST_W)
    .style(move |_theme: &Theme| {
        let mut style = crate::context_menu::panel_style(look);
        style.border.radius = 8.0.into();
        style
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(level: Level, created: Instant) -> Notification {
        Notification {
            created,
            ..Notification::new(level, "t")
        }
    }

    #[test]
    fn auto_dismiss_table() {
        let t0 = Instant::now();
        let just_before = t0 + Duration::from_millis(7_999);
        let due = t0 + AUTO_DISMISS;
        let clip = Notification {
            created: t0,
            ..Notification::new(Level::Info, "t").action(ToastAction::ClipboardWrite)
        };
        let hour = Duration::from_secs(3600);
        // (notification, now, expired)
        let rows = [
            (at(Level::Info, t0), just_before, false),
            (at(Level::Info, t0), due, true),
            (at(Level::Warn, t0), due, true),
            (at(Level::Error, t0), due + hour, false),
            (clip, due + hour, false),
        ];
        for (note, now, want) in rows {
            assert_eq!(note.expired(now), want, "{:?}", note.level);
        }
    }

    #[test]
    fn expire_removes_only_due_toasts_and_reports_ids() {
        let t0 = Instant::now();
        let mut toasts = Toasts::default();
        let info = toasts.push(at(Level::Info, t0));
        let error = toasts.push(at(Level::Error, t0));
        let late = toasts.push(at(Level::Warn, t0 + Duration::from_secs(5)));
        assert_eq!(toasts.expire(t0 + AUTO_DISMISS), vec![info]);
        assert!(toasts.get(error).is_some() && toasts.get(late).is_some());
        assert!(toasts.dismiss(error));
        assert!(!toasts.dismiss(error), "already gone");
    }

    #[test]
    fn capacity_evicts_auto_dismissing_toasts_before_errors() {
        let mut toasts = Toasts::default();
        let error = toasts.push(Notification::new(Level::Error, "first"));
        for _ in 0..MAX_TOASTS {
            toasts.push(Notification::new(Level::Info, "x"));
        }
        assert_eq!(toasts.iter().count(), MAX_TOASTS);
        assert!(toasts.get(error).is_some(), "the sticky error survives");
    }

    #[test]
    fn detail_is_sanitized_and_capped() {
        let raw = format!("a\x1b[31mb{}", "x".repeat(500));
        let note = Notification::new(Level::Error, "t").detail(raw);
        assert!(!note.detail.contains('\x1b'));
        assert_eq!(note.detail.chars().count(), MAX_DETAIL_CHARS);
    }
}
