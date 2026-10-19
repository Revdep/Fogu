//! `TerminalView`: one iced widget that draws a whole [`RenderSnapshot`] with `fill_quad` and
//! `fill_text` only, so it renders the same on wgpu and tiny-skia.
//!
//! Cells are grouped into style runs per row (`runs.rs`); every run is placed at
//! `x = col * cell_w`, never at the shaped advance.

use fogu_core::Hotkeys;
use fogu_term::{CellFlags, CursorShape, RenderSnapshot, Rgb, links};
use iced::advanced::input_method::{self, InputMethod};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, LineHeight, Shaping, Text, Wrapping};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, mouse};
use iced::alignment::Vertical;
use iced::{
    Border, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, font, keyboard, window,
};

use crate::keys;
use crate::message::Message;
use crate::metrics::{CellMetrics, cells_f32 as cells, grid_size, pixel_to_cell};
use crate::mouse::{Grid, MouseConfig, MouseState};
use crate::palette::{Colors, to_iced};
use crate::runs::{Style, Underline, cell_style, explicit_bg, row_runs};
use crate::tab::PaneId;

/// The terminal widget. Borrowed from the pane for one `view`.
#[derive(Debug)]
pub struct TerminalView<'a> {
    pane: PaneId,
    snapshot: &'a RenderSnapshot,
    colors: &'a Colors,
    metrics: CellMetrics,
    font: Font,
    focused: bool,
    overlay: Option<&'a str>,
    mouse_config: MouseConfig,
    hotkeys: Option<&'a Hotkeys>,
    /// Blink phase: `false` hides the focused pane's cursor.
    cursor_on: bool,
    /// Alpha of the scheme's default background (below 1 over a tab background).
    default_bg_alpha: f32,
}

impl<'a> TerminalView<'a> {
    /// Draws pane `pane`'s `snapshot`.
    pub fn new(
        pane: PaneId,
        snapshot: &'a RenderSnapshot,
        colors: &'a Colors,
        metrics: CellMetrics,
        font: Font,
        focused: bool,
        mouse_config: MouseConfig,
    ) -> Self {
        Self {
            pane,
            snapshot,
            colors,
            metrics,
            font,
            focused,
            overlay: None,
            mouse_config,
            hotkeys: None,
            cursor_on: true,
            default_bg_alpha: 1.0,
        }
    }

    /// A status line drawn below the cursor, such as "[process exited with code 0]".
    #[must_use]
    pub fn status(mut self, status: Option<&'a str>) -> Self {
        self.overlay = status;
        self
    }

    /// Blink phase of the focused pane's cursor (`false` = hidden this phase).
    #[must_use]
    pub fn cursor_on(mut self, on: bool) -> Self {
        self.cursor_on = on;
        self
    }

    /// Draws cells with the scheme's default background at `alpha` (0..=1) so a tab background
    /// shows through; explicit SGR backgrounds stay opaque.
    #[must_use]
    pub fn default_bg_alpha(mut self, alpha: f32) -> Self {
        self.default_bg_alpha = alpha;
        self
    }

    /// Key presses matching `hotkeys` publish [`Message::Action`] instead of reaching the shell.
    #[must_use]
    pub fn hotkeys(mut self, hotkeys: &'a Hotkeys) -> Self {
        self.hotkeys = Some(hotkeys);
        self
    }

    /// Links: Ctrl+hover underlines the link under the pointer (found only then, one row at a
    /// time), Ctrl+click opens it, a right-press records it for the pane menu. `true` when the
    /// event was the link's (a Ctrl+click on a link).
    fn on_link_event(
        &self,
        state: &mut MouseState,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        // Ctrl, or Cmd on macOS.
        let ctrl = match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => m.command(),
            _ => state.modifiers.command(),
        };
        let link_under = || {
            let at = cursor.position_over(bounds)? - bounds.position();
            let cell = pixel_to_cell(at, &self.metrics, self.snapshot.cols, self.snapshot.rows);
            links::link_at(self.snapshot, cell.col, cell.row)
        };
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. })
            | Event::Keyboard(keyboard::Event::ModifiersChanged(_)) => {
                let link = if ctrl { link_under() } else { None };
                if link != state.link {
                    state.link.clone_from(&link);
                    shell.publish(Message::LinkHover(self.pane, link));
                }
                false
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if ctrl => {
                match state.link.clone().filter(|_| cursor.is_over(bounds)) {
                    Some(link) => {
                        shell.publish(Message::OpenLink(self.pane, link));
                        true
                    }
                    None => false,
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
                if cursor.is_over(bounds) =>
            {
                shell.publish(Message::LinkHover(self.pane, link_under()));
                false
            }
            _ => false,
        }
    }

    fn cell_rect(&self, origin: Point, col: usize, row: usize, cols: usize) -> Rectangle {
        let m = &self.metrics;
        Rectangle {
            x: origin.x + cells(col) * m.cell_w,
            y: origin.y + cells(row) * m.cell_h,
            width: cells(cols) * m.cell_w,
            height: m.cell_h,
        }
    }

    fn draw_text(
        &self,
        renderer: &mut iced::Renderer,
        content: String,
        rect: Rectangle,
        style: &Style,
        color: Rgb,
        clip: Rectangle,
    ) {
        let font = Font {
            weight: if style.bold {
                font::Weight::Bold
            } else {
                font::Weight::Normal
            },
            style: if style.italic {
                font::Style::Italic
            } else {
                font::Style::Normal
            },
            ..self.font
        };
        let text = Text {
            content,
            // A little slack so the last glyph of a run is never clipped by rounding.
            bounds: Size::new(rect.width + self.metrics.cell_w, rect.height),
            size: Pixels(self.metrics.font_size),
            line_height: LineHeight::Absolute(Pixels(self.metrics.cell_h)),
            font,
            align_x: text::Alignment::Left,
            align_y: Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::None,
        };
        text::Renderer::fill_text(renderer, text, rect.position(), to_iced(color), clip);
    }

    fn draw_runs(&self, renderer: &mut iced::Renderer, origin: Point, clip: Rectangle) {
        let default_bg = self.colors.background();
        let mut runs = Vec::new();
        for (row, cells) in self.snapshot.rows().enumerate() {
            row_runs(cells, self.colors, &mut runs);
            for run in &runs {
                let rect = self.cell_rect(origin, run.col, row, run.cols);
                if let Some(bg) = explicit_bg(&run.style, default_bg) {
                    fill(renderer, rect, bg);
                }
                if !run.style.hidden && !run.text.trim_end().is_empty() {
                    let color = run.style.fg;
                    self.draw_text(renderer, run.text.clone(), rect, &run.style, color, clip);
                }
                self.draw_decorations(renderer, rect, &run.style);
            }
        }
    }

    fn draw_decorations(&self, renderer: &mut iced::Renderer, rect: Rectangle, style: &Style) {
        let thickness = (self.metrics.cell_h / 16.0).max(1.0);
        let line = |y: f32| Rectangle {
            y,
            height: thickness,
            ..rect
        };
        let bottom = rect.y + rect.height - thickness;
        match style.underline {
            Some(Underline::Single) => fill(renderer, line(bottom), style.underline_color),
            Some(Underline::Double) => {
                fill(renderer, line(bottom), style.underline_color);
                fill(
                    renderer,
                    line(bottom - 2.0 * thickness),
                    style.underline_color,
                );
            }
            None => {}
        }
        if style.strike {
            fill(renderer, line(rect.y + rect.height / 2.0), style.fg);
        }
    }

    fn draw_cursor(&self, renderer: &mut iced::Renderer, origin: Point, clip: Rectangle) {
        let cursor = self.snapshot.cursor;
        if !cursor.visible || (self.focused && !self.cursor_on) {
            return;
        }
        let row = self.snapshot.rows().nth(cursor.row);
        let cell = row.and_then(|r| r.get(cursor.col));
        let wide = cell.is_some_and(|c| c.flags.contains(CellFlags::WIDE_CHAR));
        let rect = self.cell_rect(origin, cursor.col, cursor.row, if wide { 2 } else { 1 });
        let color = self.colors.cursor();
        let thickness = (self.metrics.cell_w / 6.0).max(1.0);
        let shape = if self.focused {
            cursor.shape
        } else {
            CursorShape::HollowBlock
        };
        match shape {
            CursorShape::Block => {
                fill(renderer, rect, color);
                // Redraw the character under the cursor in the scheme's cursor text colour.
                if let Some(cell) = cell.filter(|c| c.ch != ' ') {
                    let style = cell_style(cell, self.colors);
                    let mut text = String::from(cell.ch);
                    text.extend(cell.zerowidth.iter().flat_map(|z| z.iter()));
                    let fg = self.colors.cursor_text();
                    self.draw_text(renderer, text, rect, &style, fg, clip);
                }
            }
            CursorShape::Beam => fill(
                renderer,
                Rectangle {
                    width: thickness,
                    ..rect
                },
                color,
            ),
            CursorShape::Underline => {
                let y = rect.y + rect.height - thickness;
                fill(
                    renderer,
                    Rectangle {
                        y,
                        height: thickness,
                        ..rect
                    },
                    color,
                );
            }
            CursorShape::HollowBlock => {
                let quad = Quad {
                    bounds: rect,
                    border: Border {
                        color: to_iced(color),
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..Quad::default()
                };
                renderer::Renderer::fill_quad(renderer, quad, iced::Color::TRANSPARENT);
            }
            CursorShape::Hidden => {}
        }
    }

    /// Translucent quads over the selected cells; drawn after the cell backgrounds, so they sit
    /// under the text (iced draws a layer's quads before its text).
    fn draw_selection(&self, renderer: &mut iced::Renderer, origin: Point) {
        let mut color = to_iced(self.colors.foreground());
        color.a = 0.3;
        for (row, first, last) in self.snapshot.selection_spans() {
            let rect = self.cell_rect(origin, first, row, last + 1 - first);
            let quad = Quad {
                bounds: rect,
                ..Quad::default()
            };
            renderer::Renderer::fill_quad(renderer, quad, color);
        }
    }

    /// A 2 px bar at the right edge showing the viewport's place in history; only drawn while
    /// scrolled back.
    fn draw_scroll_indicator(&self, renderer: &mut iced::Renderer, bounds: Rectangle) {
        let snap = self.snapshot;
        if snap.display_offset == 0 {
            return;
        }
        let total = cells(snap.history_len + snap.rows).max(1.0);
        let top = cells(snap.history_len.saturating_sub(snap.display_offset)) / total;
        let height = (cells(snap.rows) / total * bounds.height).max(4.0);
        let mut color = to_iced(self.colors.foreground());
        color.a = 0.5;
        let quad = Quad {
            bounds: Rectangle {
                x: bounds.x + bounds.width - 2.0,
                y: bounds.y + top * bounds.height,
                width: 2.0,
                height,
            },
            ..Quad::default()
        };
        renderer::Renderer::fill_quad(renderer, quad, color);
    }

    fn draw_overlay(&self, renderer: &mut iced::Renderer, bounds: Rectangle) {
        let Some(label) = self.overlay else { return };
        let row = (self.snapshot.cursor.row + 1).min(self.snapshot.rows.saturating_sub(1));
        let cols = label.chars().count();
        let rect = self.cell_rect(bounds.position(), 0, row, cols);
        fill(renderer, rect, self.colors.foreground());
        let style = Style {
            fg: self.colors.background(),
            bg: self.colors.foreground(),
            underline_color: self.colors.background(),
            bold: true,
            italic: false,
            underline: None,
            strike: false,
            hidden: false,
        };
        let bg = self.colors.background();
        self.draw_text(renderer, label.to_owned(), rect, &style, bg, bounds);
    }
}

impl TerminalView<'_> {
    /// The hovered link: underlined, with its real target on the row below (above on the last
    /// row) so an OSC 8 text can't pass for another address.
    fn draw_link(&self, renderer: &mut iced::Renderer, bounds: Rectangle, link: &links::Link) {
        let origin = bounds.position();
        let cells_rect = self.cell_rect(origin, link.start, link.row, link.end - link.start);
        let underline = Rectangle {
            y: cells_rect.y + cells_rect.height - 1.0,
            height: 1.0,
            ..cells_rect
        };
        fill(renderer, underline, self.colors.foreground());
        let shown: String = link
            .uri
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .take(self.snapshot.cols)
            .collect();
        let width = shown.chars().count();
        let row = if link.row + 1 < self.snapshot.rows {
            link.row + 1
        } else {
            link.row.saturating_sub(1)
        };
        let col = link.start.min(self.snapshot.cols.saturating_sub(width));
        let rect = self.cell_rect(origin, col, row, width);
        fill(renderer, rect, self.colors.foreground());
        let style = Style {
            fg: self.colors.background(),
            bg: self.colors.foreground(),
            underline_color: self.colors.background(),
            bold: false,
            italic: false,
            underline: None,
            strike: false,
            hidden: false,
        };
        self.draw_text(
            renderer,
            shown,
            rect,
            &style,
            self.colors.background(),
            bounds,
        );
    }
}

fn fill(renderer: &mut iced::Renderer, bounds: Rectangle, color: Rgb) {
    let quad = Quad {
        bounds,
        ..Quad::default()
    };
    renderer::Renderer::fill_quad(renderer, quad, to_iced(color));
}

impl Widget<Message, iced::Theme, iced::Renderer> for TerminalView<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<MouseState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(MouseState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        // Any event (incl. the RedrawRequested sent before every frame) checks whether the
        // widget's size now holds a different grid.
        let (cols, rows) = grid_size(bounds.size(), &self.metrics);
        if (usize::from(cols), usize::from(rows)) != (self.snapshot.cols, self.snapshot.rows) {
            shell.publish(Message::GridResized {
                pane: self.pane,
                cols,
                rows,
            });
        }
        if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event
            && cursor.is_over(bounds)
            && !self.focused
        {
            shell.publish(Message::FocusPane(self.pane));
        }
        if self.on_link_event(
            tree.state.downcast_mut::<MouseState>(),
            event,
            bounds,
            cursor,
            shell,
        ) {
            shell.capture_event();
            return;
        }
        // Order: hotkeys → mouse → encoder (one dispatch path for keys).
        if self.focused
            && let Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modified_key,
                modifiers,
                ..
            }) = event
            && let Some(action) = self
                .hotkeys
                .and_then(|h| keys::hotkey(h, key, modified_key, *modifiers))
        {
            shell.publish(Message::Action(action));
            shell.capture_event();
            return;
        }
        let grid = Grid {
            bounds,
            metrics: self.metrics,
            cols: self.snapshot.cols,
            rows: self.snapshot.rows,
            mode: self.snapshot.mode,
            config: self.mouse_config,
        };
        if tree
            .state
            .downcast_mut::<MouseState>()
            .update(event, cursor, &grid, shell)
        {
            shell.capture_event();
            return;
        }
        match event {
            _ if !self.focused => {}
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: plain_key,
                modified_key,
                location,
                modifiers,
                text,
                ..
            }) => {
                let input = keys::key_input(modified_key, *location, *modifiers, text.as_deref());
                if let Some(key) = input.map(|k| {
                    if cfg!(target_os = "macos") {
                        keys::macos_option(k, plain_key)
                    } else {
                        k
                    }
                }) {
                    shell.publish(Message::Key(key));
                    shell.capture_event();
                }
            }
            Event::InputMethod(input_method::Event::Commit(text)) => {
                shell.publish(Message::Text(text.clone()));
                shell.capture_event();
            }
            Event::Window(window::Event::RedrawRequested(_)) => {
                let c = &self.snapshot.cursor;
                shell.request_input_method::<&str>(&InputMethod::Enabled {
                    cursor: self.cell_rect(bounds.position(), c.col, c.row, 1),
                    purpose: input_method::Purpose::Terminal,
                    preedit: None,
                });
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if !cursor.is_over(layout.bounds()) {
            mouse::Interaction::None
        } else if tree.state.downcast_ref::<MouseState>().link.is_some() {
            mouse::Interaction::Pointer
        } else if tree
            .state
            .downcast_ref::<MouseState>()
            .reporting(self.snapshot.mode)
        {
            mouse::Interaction::Idle
        } else {
            mouse::Interaction::Text
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if self.default_bg_alpha > 0.0 {
            let mut bg = to_iced(self.colors.background());
            bg.a = self.default_bg_alpha;
            let quad = Quad {
                bounds,
                ..Quad::default()
            };
            renderer::Renderer::fill_quad(renderer, quad, bg);
        }
        let origin = bounds.position();
        self.draw_runs(renderer, origin, bounds);
        self.draw_selection(renderer, origin);
        self.draw_cursor(renderer, origin, bounds);
        self.draw_scroll_indicator(renderer, bounds);
        self.draw_overlay(renderer, bounds);
        if let Some(link) = &tree.state.downcast_ref::<MouseState>().link {
            self.draw_link(renderer, bounds, link);
        }
    }
}

impl<'a> From<TerminalView<'a>> for Element<'a, Message> {
    fn from(view: TerminalView<'a>) -> Self {
        Element::new(view)
    }
}
