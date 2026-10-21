//! `SplitContainer`: lays a tab's panes out from its `SplitTree<PaneId>`, draws the dividers and
//! the focused pane's border, and turns divider drags into ratio changes. With [`tiles`]
//! (`SplitContainer::tiles`) the panes float as rounded tiles on the bar's ground, the focused
//! one ringed in the accent with a glow.
//! An optional side panel (the SFTP browser) sits right of the panes behind one more divider
//! with the same hit area, cursor and double-click behaviour.

use fogu_core::{Divider, Orientation, Rect, SplitTree};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Operation, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, mouse, overlay};
use iced::{Border, Color, Element, Event, Length, Point, Rectangle, Size, Vector};

use crate::background::Backdrop;
use crate::drag::{DragMsg, Zone, drop_zone};
use crate::message::Message;
use crate::sftp::{PanelMsg, SftpMsg};
use crate::tab::{PaneId, TabId};

/// Space between sibling panes; the divider line sits in its middle.
pub const GAP: f32 = 4.0;
/// Tiles: the space between and around them, and their corner radius.
const TILE_GAP: f32 = 6.0;
const TILE_R: f32 = 10.0;
/// Width of a divider's invisible mouse hit area.
const HIT: f32 = 8.0;
/// Room kept around the panes for the focused pane's border.
const BORDER: f32 = 1.0;

/// The split container of one tab. Children come in any order, each tagged with its pane.
pub struct SplitContainer<'a> {
    tab: TabId,
    tree: &'a SplitTree<PaneId>,
    ids: Vec<PaneId>,
    children: Vec<Element<'a, Message>>,
    background: Color,
    divider: Color,
    /// The pane outlined, and the outline colour.
    border: Option<(PaneId, Color)>,
    /// Width share of the side panel (the last child), if there is one.
    side: Option<f32>,
    /// The tab background under the panes.
    backdrop: Option<Backdrop>,
    /// Tiles on this ground, the unfocused ring colour and the glow colour.
    tiles: Option<(Color, Color, Color)>,
    /// A drag is under way: the current drop target and the highlight colour.
    drop: Option<(Option<(PaneId, Zone)>, Color)>,
}

impl std::fmt::Debug for SplitContainer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitContainer")
            .field("tab", &self.tab)
            .field("panes", &self.ids)
            .finish_non_exhaustive()
    }
}

/// Widget state: the divider being dragged and the last press (double-click detection).
#[derive(Debug, Default)]
struct State {
    drag: Option<Drag>,
    last_click: Option<mouse::Click>,
}

/// Which kind of divider is dragged.
#[derive(Clone, Debug)]
enum Drag {
    /// A split of the tree.
    Split(Divider),
    /// The side panel's divider (its `area` is the whole inner area).
    Side(Divider),
}

impl Drag {
    fn divider(&self) -> &Divider {
        match self {
            Self::Split(d) | Self::Side(d) => d,
        }
    }
}

impl<'a> SplitContainer<'a> {
    /// Lays out `panes` (one per leaf of `tree`) of tab `tab` on a `background` with `divider`
    /// lines between them.
    pub fn new(
        tab: TabId,
        tree: &'a SplitTree<PaneId>,
        panes: Vec<(PaneId, Element<'a, Message>)>,
        background: Color,
        divider: Color,
    ) -> Self {
        let (ids, children) = panes.into_iter().unzip();
        Self {
            tab,
            tree,
            ids,
            children,
            background,
            divider,
            border: None,
            side: None,
            backdrop: None,
            drop: None,
            tiles: None,
        }
    }

    /// Floating tiles: `ground` around and between the panes, rings in `ring` (focused: the
    /// border colour, with a `glow`).
    #[must_use]
    pub fn tiles(mut self, ground: Color, ring: Color, glow: Color) -> Self {
        self.tiles = Some((ground, ring, glow));
        self
    }

    fn gap(&self) -> f32 {
        if self.tiles.is_some() { TILE_GAP } else { GAP }
    }

    /// The area the panes share: `bounds` minus room for the border (tiles: their margin).
    fn inner(&self, bounds: Rectangle) -> Rect {
        let pad = if self.tiles.is_some() {
            TILE_GAP
        } else {
            BORDER
        };
        Rect {
            x: bounds.x + pad,
            y: bounds.y + pad,
            w: (bounds.width - 2.0 * pad).max(0.0),
            h: (bounds.height - 2.0 * pad).max(0.0),
        }
    }

    /// Adds `panel` right of the panes, `ratio` of the width (the rest minus the gap goes to
    /// the panes); dragging its divider publishes the new ratio.
    #[must_use]
    pub fn side(mut self, panel: Element<'a, Message>, ratio: f32) -> Self {
        self.children.push(panel);
        self.side = Some(ratio);
        self
    }

    /// The panes' area, and the side panel's area and divider.
    fn areas(&self, bounds: Rectangle) -> (Rect, Option<(Rect, Divider)>) {
        let all = self.inner(bounds);
        let Some(ratio) = self.side else {
            return (all, None);
        };
        let gap_w = self.gap();
        let avail = (all.w - gap_w).max(0.0);
        let side_w = (avail * ratio).round();
        let panes = Rect {
            w: avail - side_w,
            ..all
        };
        let gap = Rect {
            x: all.x + panes.w,
            w: gap_w,
            ..all
        };
        let side = Rect {
            x: gap.x + gap_w,
            w: side_w,
            ..all
        };
        let divider = Divider {
            path: fogu_core::NodePath::default(),
            rect: gap,
            dir: Orientation::Horizontal,
            area: all,
        };
        (panes, Some((side, divider)))
    }

    /// Draws `backdrop` under the panes (not under the side panel).
    #[must_use]
    pub fn backdrop(mut self, backdrop: Option<Backdrop>) -> Self {
        self.backdrop = backdrop;
        self
    }

    /// A drag is under way: pointer moves over a pane publish its drop zone, and `target`
    /// is highlighted in `color`.
    #[must_use]
    pub fn dropping(mut self, target: Option<(PaneId, Zone)>, color: Color) -> Self {
        self.drop = Some((target, color));
        self
    }

    /// The pane and zone under `at`.
    fn drop_at(&self, layout: Layout<'_>, at: Point) -> Option<(PaneId, Zone)> {
        self.ids
            .iter()
            .zip(layout.children())
            .find(|(_, l)| l.bounds().contains(at))
            .map(|(id, l)| (*id, drop_zone(at, l.bounds())))
    }

    /// Outlines `pane` with a 1-px `color` border.
    #[must_use]
    pub fn border(mut self, pane: PaneId, color: Color) -> Self {
        self.border = Some((pane, color));
        self
    }

    /// Mouse handling for the dividers; `true` when the event was theirs.
    fn on_divider_event(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(at) = cursor.position() else {
                    return false;
                };
                let Some(drag) = self.divider_at(bounds, at) else {
                    return false;
                };
                let click = mouse::Click::new(at, mouse::Button::Left, state.last_click);
                state.last_click = Some(click);
                if click.kind() == mouse::click::Kind::Double {
                    state.drag = None;
                    shell.publish(match drag {
                        Drag::Split(_) => Message::Equalize(self.tab),
                        Drag::Side(_) => self.side_message(None),
                    });
                } else {
                    state.drag = Some(drag);
                }
                true
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let Some(drag) = &state.drag else {
                    return false;
                };
                let ratio = drag.divider().ratio_at(position.x, position.y, self.gap());
                shell.publish(match drag {
                    Drag::Split(d) => Message::SplitRatio(self.tab, d.path.clone(), ratio),
                    // `ratio` is the panes' share; the panel gets the rest.
                    Drag::Side(_) => self.side_message(Some(1.0 - ratio)),
                });
                true
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag.take().is_some()
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let Some(at) = cursor.position() else {
                    return false;
                };
                if !matches!(self.divider_at(bounds, at), Some(Drag::Split(_))) {
                    return false;
                }
                shell.publish(Message::DividerMenu(self.tab, at));
                true
            }
            _ => false,
        }
    }

    fn side_message(&self, ratio: Option<f32>) -> Message {
        self.panel_message(PanelMsg::Width(ratio))
    }

    fn panel_message(&self, message: PanelMsg) -> Message {
        Message::Sftp(SftpMsg::Panel(self.tab, message))
    }

    /// Every divider: the tree's, then the side panel's.
    fn dividers(&self, bounds: Rectangle) -> Vec<Drag> {
        let (panes, side) = self.areas(bounds);
        let splits = self
            .tree
            .dividers(panes, self.gap())
            .into_iter()
            .map(Drag::Split);
        splits.chain(side.map(|(_, d)| Drag::Side(d))).collect()
    }

    fn divider_at(&self, bounds: Rectangle, at: Point) -> Option<Drag> {
        self.dividers(bounds)
            .into_iter()
            .find(|d| hit_area(d.divider()).contains(at))
    }
}

/// The area the panes share in the classic look: `bounds` minus room for the border.
#[cfg(test)]
fn inner(bounds: Rectangle) -> Rect {
    Rect {
        x: bounds.x + BORDER,
        y: bounds.y + BORDER,
        w: (bounds.width - 2.0 * BORDER).max(0.0),
        h: (bounds.height - 2.0 * BORDER).max(0.0),
    }
}

/// Rounds each tile's corners (ground-coloured notches; images draw over the layer's text), then
/// rings them on a layer above: `border`'s pane in its colour, the others in `ring`.
fn draw_tiles(
    renderer: &mut iced::Renderer,
    ground: Color,
    ring: Color,
    border: Option<(PaneId, Color)>,
    tiles: &[(PaneId, Rectangle)],
) {
    use iced::advanced::image::{self, Renderer as _};
    let notches = [
        (
            (false, false),
            crate::icons::notch(ground, TILE_R, true, true),
        ),
        (
            (true, false),
            crate::icons::notch(ground, TILE_R, false, true),
        ),
        (
            (false, true),
            crate::icons::notch(ground, TILE_R, true, false),
        ),
        (
            (true, true),
            crate::icons::notch(ground, TILE_R, false, false),
        ),
    ];
    for &(_, b) in tiles {
        for &((right, bottom), ref handle) in &notches {
            let at = Rectangle {
                x: if right { b.x + b.width - TILE_R } else { b.x },
                y: if bottom { b.y + b.height - TILE_R } else { b.y },
                width: TILE_R,
                height: TILE_R,
            };
            // The circle sits on the tile's inner side of the notch.
            renderer.draw_image(image::Image::new(handle.clone()), at, at);
        }
    }
    let all = tiles
        .iter()
        .map(|&(_, b)| b)
        .reduce(|a, b| a.union(&b))
        .unwrap_or_default()
        .expand(2.0);
    renderer::Renderer::with_layer(renderer, all, |renderer| {
        for &(id, bounds) in tiles {
            let color = match border {
                Some((pane, color)) if pane == id => color,
                _ => ring,
            };
            let quad = Quad {
                bounds,
                border: Border::default().rounded(TILE_R).width(1).color(color),
                ..Quad::default()
            };
            renderer::Renderer::fill_quad(renderer, quad, Color::TRANSPARENT);
        }
    });
}

fn rectangle(r: Rect) -> Rectangle {
    Rectangle {
        x: r.x,
        y: r.y,
        width: r.w,
        height: r.h,
    }
}

/// A divider's gap widened (or narrowed) across its thin axis to `width`, centred.
fn across(d: &Divider, width: f32) -> Rectangle {
    let r = rectangle(d.rect);
    match d.dir {
        Orientation::Horizontal => Rectangle {
            x: r.x + (r.width - width) / 2.0,
            width,
            ..r
        },
        Orientation::Vertical => Rectangle {
            y: r.y + (r.height - width) / 2.0,
            height: width,
            ..r
        },
    }
}

fn hit_area(d: &Divider) -> Rectangle {
    across(d, HIT)
}

fn resize_cursor(dir: Orientation) -> mouse::Interaction {
    match dir {
        Orientation::Horizontal => mouse::Interaction::ResizingHorizontally,
        Orientation::Vertical => mouse::Interaction::ResizingVertically,
    }
}

fn fill(renderer: &mut iced::Renderer, bounds: Rectangle, color: Color) {
    let quad = Quad {
        bounds,
        ..Quad::default()
    };
    renderer::Renderer::fill_quad(renderer, quad, color);
}

impl Widget<Message, iced::Theme, iced::Renderer> for SplitContainer<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.max();
        let (pane_area, side_area) = self.areas(Rectangle::with_size(size));
        let rects = self.tree.layout(pane_area, self.gap());
        // Children are the panes (tagged by `ids`), then the side panel if any.
        let side_rect = side_area.map(|(r, _)| r);
        let nodes = self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .enumerate()
            .map(|(i, (child, state))| {
                let rect = match self.ids.get(i) {
                    Some(id) => rects
                        .iter()
                        .find(|(pane, _)| pane == id)
                        .map_or(Rect::default(), |&(_, r)| r),
                    None => side_rect.unwrap_or_default(),
                };
                let limits = layout::Limits::new(Size::ZERO, Size::new(rect.w, rect.h));
                child
                    .as_widget_mut()
                    .layout(state, renderer, &limits)
                    .move_to(Point::new(rect.x, rect.y))
            })
            .collect();
        layout::Node::with_children(size, nodes)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((child, state), layout) in self
                .children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
            {
                child
                    .as_widget_mut()
                    .operate(state, layout, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let (Some((current, _)), Event::Mouse(mouse::Event::CursorMoved { .. })) =
            (self.drop, event)
        {
            let target = cursor.position().and_then(|at| self.drop_at(layout, at));
            if target != current {
                shell.publish(Message::Drag(DragMsg::Hover(target)));
            }
        }
        let state = tree.state.downcast_mut::<State>();
        if self.on_divider_event(state, event, layout.bounds(), cursor, shell) {
            shell.capture_event();
            return;
        }
        for ((child, state), layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                state, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
        // A right-press in the side panel opens its menu at the pointer. Published after the
        // children, so an entry's own right-press (which picks the entry) arrives first.
        if self.side.is_some()
            && let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) = event
            && let Some(at) = cursor.position()
            && layout
                .children()
                .last()
                .is_some_and(|l| l.bounds().contains(at))
        {
            let menu = PanelMsg::MenuAt(at);
            shell.publish(Message::Sftp(SftpMsg::Panel(self.tab, menu)));
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if let Some(drag) = &tree.state.downcast_ref::<State>().drag {
            return resize_cursor(drag.divider().dir);
        }
        if let Some(drag) = cursor
            .position()
            .and_then(|at| self.divider_at(layout.bounds(), at))
        {
            return resize_cursor(drag.divider().dir);
        }
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, state), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .find(|&interaction| interaction != mouse::Interaction::None)
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let panes = || self.ids.iter().zip(layout.children());
        match self.tiles {
            Some((ground, _, glow)) => {
                fill(renderer, bounds, ground);
                // The focused tile's glow, under the tiles.
                if let Some((pane, _)) = self.border
                    && let Some((_, l)) = panes().find(|(id, _)| **id == pane)
                {
                    let quad = Quad {
                        bounds: l.bounds(),
                        border: Border::default().rounded(TILE_R),
                        shadow: iced::Shadow {
                            color: glow,
                            offset: Vector::ZERO,
                            blur_radius: 28.0,
                        },
                        ..Quad::default()
                    };
                    renderer::Renderer::fill_quad(renderer, quad, self.background);
                }
                for (_, l) in panes() {
                    fill(renderer, l.bounds(), self.background);
                }
            }
            None => fill(renderer, bounds, self.background),
        }
        // Children, dividers and the focused pane's border.
        let content = |renderer: &mut iced::Renderer| {
            for ((child, state), layout) in self
                .children
                .iter()
                .zip(&tree.children)
                .zip(layout.children())
            {
                child
                    .as_widget()
                    .draw(state, renderer, theme, style, layout, cursor, viewport);
            }
            if self.tiles.is_none() {
                for drag in self.dividers(bounds) {
                    fill(renderer, across(drag.divider(), 1.0), self.divider);
                }
            }
            if let Some((Some((pane, zone)), color)) = self.drop
                && let Some((_, l)) = self
                    .ids
                    .iter()
                    .zip(layout.children())
                    .find(|(id, _)| **id == pane)
            {
                fill(
                    renderer,
                    zone.highlight(l.bounds()),
                    Color { a: 0.25, ..color },
                );
            }
            if let Some((ground, ring, _)) = self.tiles {
                let tiles: Vec<(PaneId, Rectangle)> =
                    panes().map(|(id, l)| (*id, l.bounds())).collect();
                draw_tiles(renderer, ground, ring, self.border, &tiles);
                return;
            }
            let Some((pane, color)) = self.border else {
                return;
            };
            let focused = self
                .ids
                .iter()
                .zip(layout.children())
                .find(|(id, _)| **id == pane);
            if let Some((_, layout)) = focused {
                let quad = Quad {
                    bounds: layout.bounds().expand(BORDER),
                    border: Border {
                        color,
                        width: BORDER,
                        radius: 0.0.into(),
                    },
                    ..Quad::default()
                };
                renderer::Renderer::fill_quad(renderer, quad, Color::TRANSPARENT);
            }
        };
        let Some(backdrop) = &self.backdrop else {
            content(renderer);
            return;
        };
        // A layer of its own: it clips (a covering image overhangs the area) and iced draws a
        // layer's images after its quads and text, so the panes go on a layer above.
        let area = rectangle(self.areas(bounds).0);
        renderer::Renderer::with_layer(renderer, area, |renderer| backdrop.draw(renderer, area));
        renderer::Renderer::with_layer(renderer, bounds, content);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a> From<SplitContainer<'a>> for Element<'a, Message> {
    fn from(container: SplitContainer<'a>) -> Self {
        Element::new(container)
    }
}

#[cfg(test)]
#[path = "split_container_tests.rs"]
mod tests;
