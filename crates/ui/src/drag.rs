//! Internal drag & drop: tab headers, pane grips and sidebar profiles are dragged onto the tab bar
//! (reorder) or onto a pane's edge or centre (split / move / swap). No OS drag and drop, so X11 and
//! Wayland behave the same.

use fogu_core::{ProfileId, Side, SplitTree};
use iced::{Point, Rectangle, Task};

use crate::app::App;
use crate::message::Message;
use crate::tab::{PaneId, TabId};
use crate::tab_bar::{BAR_H, STRIP_PAD, TAB_STRIDE};

/// Pixels the pointer moves before a press becomes a drag.
const THRESHOLD: f32 = 6.0;
/// Share of a pane's width / height that counts as its edge.
const EDGE: f32 = 0.25;

/// What is dragged.
#[derive(Clone, Debug)]
pub enum Payload {
    Tab(TabId),
    Pane(PaneId),
    /// A sidebar profile; `.1` is what a plain click on the row does (sent on release when the
    /// pointer did not move).
    Profile(ProfileId, Box<Message>),
}

/// Where on a pane a drop lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl Zone {
    /// The split side a drop on this zone makes (the centre of a pane splits right, except
    /// for a pane drag, where it swaps).
    fn side(self) -> Side {
        match self {
            Self::Left => Side::Left,
            Self::Right | Self::Center => Side::Right,
            Self::Top => Side::Top,
            Self::Bottom => Side::Bottom,
        }
    }

    /// The part of `pane` the drop highlight covers.
    pub fn highlight(self, pane: Rectangle) -> Rectangle {
        let (w, h) = (pane.width / 2.0, pane.height / 2.0);
        match self {
            Self::Left => Rectangle { width: w, ..pane },
            Self::Right => Rectangle {
                x: pane.x + w,
                width: w,
                ..pane
            },
            Self::Top => Rectangle { height: h, ..pane },
            Self::Bottom => Rectangle {
                y: pane.y + h,
                height: h,
                ..pane
            },
            Self::Center => pane,
        }
    }
}

/// Drag messages.
#[derive(Clone, Debug)]
pub enum DragMsg {
    /// A press on something draggable.
    Start(Payload),
    /// The pointer moved (window coordinates; only while a drag is pending).
    Move(Point),
    /// The split container's pane and zone under the pointer.
    Hover(Option<(PaneId, Zone)>),
    /// The left button went up.
    Release,
}

/// A press that may become a drag.
#[derive(Clone, Debug)]
pub struct Drag {
    pub payload: Payload,
    /// The first pointer position seen after the press.
    origin: Option<Point>,
    pub at: Point,
    /// Moved past [`THRESHOLD`].
    pub started: bool,
    pub target: Option<(PaneId, Zone)>,
}

/// The zone of `rect` under `at`: the nearest edge band (25 % of the dimension), else centre.
pub fn drop_zone(at: Point, rect: Rectangle) -> Zone {
    let fx = (at.x - rect.x) / rect.width.max(1.0);
    let fy = (at.y - rect.y) / rect.height.max(1.0);
    let bands = [
        (fx, Zone::Left),
        (1.0 - fx, Zone::Right),
        (fy, Zone::Top),
        (1.0 - fy, Zone::Bottom),
    ];
    bands
        .into_iter()
        .filter(|(d, _)| *d < EDGE)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Zone::Center, |(_, zone)| zone)
}

/// The gap between tab headers nearest to `x` (0 = before the first, `count` = after the last).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded and clamped to 0..=count first"
)]
pub fn insertion_index(x: f32, count: usize) -> usize {
    let slot = (x / TAB_STRIDE).round().max(0.0) as usize;
    slot.min(count)
}

/// The header under content coordinate `x`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "floored and non-negative"
)]
fn header_at(x: f32) -> usize {
    (x / TAB_STRIDE).floor().max(0.0) as usize
}

/// Converts a window pointer into the tab strip's scrolled content coordinate.
fn strip_position(at: Point, viewport: Rectangle, offset: f32) -> Option<f32> {
    viewport.contains(at).then_some(at.x - viewport.x + offset)
}

impl App {
    pub(crate) fn on_drag(&mut self, message: DragMsg) -> Task<Message> {
        match message {
            DragMsg::Start(payload) => {
                if let Payload::Tab(tab) = &payload {
                    self.activate(*tab);
                }
                self.drag = Some(Drag {
                    payload,
                    origin: None,
                    at: Point::ORIGIN,
                    started: false,
                    target: None,
                });
            }
            DragMsg::Move(at) => self.drag_move(at),
            DragMsg::Hover(target) => {
                if let Some(drag) = &mut self.drag {
                    drag.target = target;
                }
            }
            DragMsg::Release => return self.drag_release(),
        }
        Task::none()
    }

    fn drag_move(&mut self, at: Point) {
        let started = {
            let Some(drag) = &mut self.drag else {
                return;
            };
            let origin = *drag.origin.get_or_insert(at);
            drag.at = at;
            drag.started |= origin.distance(at) >= THRESHOLD;
            drag.started
        };
        if !started {
            return;
        }
        let Some(x) = self.tab_strip_position(at) else {
            return;
        };
        if let Some(drag) = &mut self.drag {
            drag.target = None;
        }
        // Over a header: show that tab, so the drop can land on its panes.
        let hovered = self.tabs.get(header_at(x)).map(|t| t.id);
        if let Some(tab) = hovered.filter(|t| *t != self.active) {
            self.activate(tab);
        }
    }

    fn drag_release(&mut self) -> Task<Message> {
        let Some(drag) = self.drag.take() else {
            return Task::none();
        };
        if !drag.started {
            return match drag.payload {
                Payload::Profile(_, click) => self.update(*click),
                _ => Task::none(),
            };
        }
        if drag.at.y < BAR_H {
            if let Payload::Tab(tab) = drag.payload
                && let Some(x) = self.tab_strip_position(drag.at)
            {
                self.reorder_tab(tab, insertion_index(x, self.tabs.len()));
            }
            return Task::none();
        }
        match drag.target {
            Some((pane, zone)) => self.drop_on(drag.payload, pane, zone),
            None => Task::none(),
        }
    }

    /// The pointer's horizontal coordinate in the strip's content, when it is inside the
    /// visible strip rather than over `+` or the controls to its right.
    fn tab_strip_position(&self, at: Point) -> Option<f32> {
        let toggles = self.active_tab().map_or(2, |tab| {
            2 + usize::from(tab.panes.values().any(crate::tab::Pane::is_ssh))
                + usize::from(tab.panes.values().any(|pane| pane.sftp.is_some()))
        });
        let left = self.bar_inset() + STRIP_PAD;
        let width = crate::tab_bar::strip_width(
            self.window_size.width,
            self.bar_inset(),
            toggles,
            self.tabs.len(),
        );
        let viewport = Rectangle::new(Point::new(left, 0.0), iced::Size::new(width, BAR_H));
        let content_width = crate::tab_bar::headers_width(self.tabs.len());
        let offset = self
            .tab_strip
            .filter(|known| {
                (known.bounds().width - width).abs() < 0.5
                    && (known.content_bounds().width - content_width).abs() < 0.5
            })
            .map_or(0.0, |known| known.absolute_offset().x);
        strip_position(at, viewport, offset)
    }

    /// Moves tab `id` to the gap `slot` (an index into the bar before the move).
    pub(crate) fn reorder_tab(&mut self, id: TabId, slot: usize) {
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let tab = self.tabs.remove(from);
        let to = if slot > from { slot - 1 } else { slot };
        self.tabs.insert(to.min(self.tabs.len()), tab);
        self.activate(id);
    }

    /// A drag released over `zone` of `pane` (a pane of the active tab).
    pub(crate) fn drop_on(&mut self, payload: Payload, pane: PaneId, zone: Zone) -> Task<Message> {
        let Some(dst) = self.tab_of(pane) else {
            return Task::none();
        };
        let dst_id = self.tabs.get(dst).map(|t| t.id);
        if let Some(tab) = self.tabs.get_mut(dst) {
            tab.solo = None;
        }
        match payload {
            Payload::Tab(src) if Some(src) != dst_id => self.drop_tab(src, pane, zone.side()),
            Payload::Pane(moved) if moved != pane => self.drop_pane(moved, pane, zone),
            Payload::Profile(id, _) => {
                let profile = self
                    .all_entries()
                    .into_iter()
                    .find(|e| e.profile.id == id)
                    .map(|e| e.profile);
                if let Some(profile) = profile {
                    self.remember(&profile.id);
                    self.split_now(pane, zone.side(), None, Some(profile));
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// Tab `src`'s whole layout goes beside `pane`; `src` closes (its sessions live on).
    fn drop_tab(&mut self, src: TabId, pane: PaneId, side: Side) -> Task<Message> {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == src && !t.is_page()) else {
            return Task::none();
        };
        let panes = std::mem::take(&mut tab.panes);
        let layout = std::mem::replace(&mut tab.layout, SplitTree::new(pane));
        let focused = tab.focused;
        tab.sftp = None;
        let Some(dst) = self.tab_of(pane).and_then(|i| self.tabs.get_mut(i)) else {
            return Task::none();
        };
        if !dst.layout.graft(&pane, side, layout) {
            return Task::none();
        }
        dst.panes.extend(panes);
        let dst = dst.id;
        let closed = self.remove_tab(src);
        self.activate(dst);
        self.focus_pane(focused);
        closed
    }

    /// Pane `moved` goes to `zone` of `pane` (centre: they swap), in its tab or another one.
    fn drop_pane(&mut self, moved: PaneId, pane: PaneId, zone: Zone) -> Task<Message> {
        let (Some(src), Some(dst)) = (self.tab_of(moved), self.tab_of(pane)) else {
            return Task::none();
        };
        if src == dst {
            if let Some(tab) = self.tabs.get_mut(dst) {
                if zone == Zone::Center {
                    tab.layout.swap(&moved, &pane);
                } else {
                    tab.layout.move_leaf(&moved, &pane, zone.side());
                }
            }
            self.focus_pane(moved);
            return Task::none();
        }
        let Some(from) = self.tabs.get_mut(src) else {
            return Task::none();
        };
        let Some(value) = from.panes.remove(&moved) else {
            return Task::none();
        };
        from.layout.remove(&moved);
        from.solo = None;
        if from.sftp.as_ref().is_some_and(|p| p.pane == moved) {
            from.sftp = None;
        }
        let emptied = from.layout.is_empty().then_some(from.id);
        if let Some(next) = from.layout.leaves().first().map(|p| **p)
            && from.focused == moved
        {
            from.focused = next;
        }
        if let Some(to) = self.tabs.get_mut(dst) {
            to.layout.split(&pane, zone.side(), moved);
            to.panes.insert(moved, value);
        }
        let closed = emptied.map_or_else(Task::none, |tab| self.remove_tab(tab));
        self.focus_pane(moved);
        closed
    }

    /// The floating label next to the pointer while dragging.
    pub(crate) fn drag_label(&self) -> Option<(Point, String)> {
        let drag = self.drag.as_ref().filter(|d| d.started)?;
        let label = match &drag.payload {
            Payload::Tab(id) => self.tabs.iter().find(|t| t.id == *id)?.title().to_owned(),
            Payload::Pane(id) => self.pane(*id)?.profile.name.clone(),
            Payload::Profile(id, _) => {
                self.all_entries()
                    .into_iter()
                    .find(|e| e.profile.id == *id)?
                    .profile
                    .name
            }
        };
        Some((drag.at, label))
    }
}

#[cfg(test)]
#[path = "drag_tests.rs"]
mod tests;
