//! Local mouse gestures of the terminal widget: selection (simple / word / line / block), wheel
//! scrollback and auto-scroll while drag-selecting past an edge — plus mouse reporting to the
//! application: every mouse event first goes through `mouse_disposition` (Shift → local).

use std::time::{Duration, Instant};

use fogu_term::input::{self, Direction, Disposition, MouseAction, mouse_disposition};
use fogu_term::{Scroll, SelectionType, TermMode};
use iced::advanced::Shell;
use iced::advanced::mouse::{self, Button, Click, ScrollDelta, click};
use iced::keyboard::{self, Modifiers};
use iced::{Event, Point, Rectangle, window};

use crate::keys;
use crate::message::{Message, PasteSource, SelectEvent};
use crate::metrics::{CellMetrics, CellPos, grid_cell};

/// Auto-scroll pace while drag-selecting past the top/bottom edge.
const AUTOSCROLL_EVERY: Duration = Duration::from_millis(50);

/// The mouse-related config values the terminal widget needs
/// (`Config.terminal.{wheel_lines,paste_on_middle_click}`).
#[derive(Clone, Copy, Debug)]
pub struct MouseConfig {
    /// Scrollback lines per wheel notch.
    pub wheel_lines: u32,
    /// Middle-click pastes from PRIMARY.
    pub paste_on_middle_click: bool,
    /// What a right-click does (`terminal.right_click`).
    pub right_click: fogu_core::config::RightClickAction,
}

/// Where the grid sits on screen.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    /// Widget bounds; the grid starts [`crate::metrics::GRID_PAD`] in from the top-left corner.
    pub bounds: Rectangle,
    /// Cell size.
    pub metrics: CellMetrics,
    /// Columns.
    pub cols: usize,
    /// Visible rows.
    pub rows: usize,
    /// Terminal modes (mouse tracking, alternate screen, ...).
    pub mode: TermMode,
    /// Mouse-related config (wheel lines, middle-click paste).
    pub config: MouseConfig,
}

/// `grid.config.wheel_lines` as `f32` (the accumulator below is fractional: trackpads send
/// pixel deltas).
fn wheel_lines(grid: &Grid) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a wheel-notch line count; always tiny relative to f32's 24-bit mantissa"
    )]
    let lines = grid.config.wheel_lines as f32;
    lines
}

impl Grid {
    fn cell_at(&self, p: Point) -> CellPos {
        grid_cell(self.bounds, p, &self.metrics, self.cols, self.rows)
    }

    /// `(col, row)` under `p`, clamped to the grid.
    fn cell(&self, p: Point) -> (usize, usize) {
        let at = self.cell_at(p);
        (at.col, at.row)
    }
}

/// A selection gesture in progress (left button held).
#[derive(Clone, Copy, Debug)]
struct Drag {
    kind: SelectionType,
    anchor: CellPos,
    /// The term has a selection for this gesture (a plain click starts none until it moves).
    started: bool,
    /// Last known pointer position, possibly outside the widget.
    last: Point,
}

/// Per-widget mouse state, kept in the widget tree across `view`s.
#[derive(Debug, Default)]
pub struct MouseState {
    pub(crate) modifiers: Modifiers,
    /// The link drawn underlined (Ctrl held over it).
    pub(crate) link: Option<fogu_term::links::Link>,
    last_click: Option<Click>,
    drag: Option<Drag>,
    /// Fractional wheel lines not yet scrolled (trackpads send pixels).
    wheel_lines: f32,
    last_autoscroll: Option<Instant>,
    /// Button whose press was reported; its release and drag motion are reported too.
    report_held: Option<input::Button>,
    /// Cell of the last report: motion is reported only when the pointer enters a new cell.
    report_cell: Option<(usize, usize)>,
    /// Fractional wheel notches not yet reported / sent as arrow keys.
    report_wheel: f32,
}

impl MouseState {
    /// Handles one event; returns `true` when it was consumed.
    pub fn update(
        &mut self,
        event: &Event,
        cursor: mouse::Cursor,
        grid: &Grid,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        if let Event::Mouse(mouse_event) = event
            && self.report(mouse_event, cursor, grid, shell)
        {
            return true;
        }
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => self.modifiers = *m,
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) => {
                if let Some(p) = cursor.position_over(grid.bounds) {
                    self.press(p, grid, shell);
                    return true;
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if self.drag.is_some() {
                    self.motion(*position, grid, shell);
                    return true;
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => {
                if let Some(drag) = self.drag.take() {
                    self.last_autoscroll = None;
                    if drag.started {
                        shell.publish(Message::Select(SelectEvent::Finish));
                    }
                    return true;
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(grid.bounds) => {
                self.wheel(*delta, grid, shell);
                return true;
            }
            Event::Mouse(mouse::Event::ButtonPressed(Button::Right)) => {
                if let Some(p) = cursor.position_over(grid.bounds) {
                    let shift = self.modifiers.shift();
                    shell.publish(crate::pane_menu::right_click(
                        grid.config.right_click,
                        shift,
                        p,
                    ));
                    return true;
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(Button::Middle))
                if grid.config.paste_on_middle_click && cursor.is_over(grid.bounds) =>
            {
                shell.publish(Message::Paste(PasteSource::Primary));
                return true;
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                self.autoscroll(*now, grid, shell);
            }
            _ => {}
        }
        false
    }

    fn press(&mut self, p: Point, grid: &Grid, shell: &mut Shell<'_, Message>) {
        let click = Click::new(p, Button::Left, self.last_click);
        self.last_click = Some(click);
        let at = grid.cell_at(p);
        let block = self.modifiers.alt() || self.modifiers.control();
        let kind = match click.kind() {
            click::Kind::Single if block => SelectionType::Block,
            click::Kind::Single => SelectionType::Simple,
            click::Kind::Double => SelectionType::Semantic,
            click::Kind::Triple => SelectionType::Lines,
        };
        // A plain click clears; the selection starts once the pointer leaves the anchor.
        let started = !matches!(kind, SelectionType::Simple | SelectionType::Block);
        let event = if started {
            SelectEvent::Start { kind, at }
        } else {
            SelectEvent::Clear
        };
        shell.publish(Message::Select(event));
        self.drag = Some(Drag {
            kind,
            anchor: at,
            started,
            last: p,
        });
    }

    fn motion(&mut self, p: Point, grid: &Grid, shell: &mut Shell<'_, Message>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        drag.last = p;
        let at = grid.cell_at(p);
        if !drag.started {
            if at == drag.anchor {
                return;
            }
            drag.started = true;
            let (kind, at) = (drag.kind, drag.anchor);
            shell.publish(Message::Select(SelectEvent::Start { kind, at }));
        }
        shell.publish(Message::Select(SelectEvent::Update(at)));
        self.autoscroll(Instant::now(), grid, shell);
    }

    fn wheel(&mut self, delta: ScrollDelta, grid: &Grid, shell: &mut Shell<'_, Message>) {
        self.wheel_lines += match delta {
            ScrollDelta::Lines { y, .. } => y * wheel_lines(grid),
            ScrollDelta::Pixels { y, .. } => y / grid.metrics.cell_h,
        };
        if !self.wheel_lines.is_finite() {
            self.wheel_lines = 0.0;
        }
        let whole = self.wheel_lines.trunc();
        self.wheel_lines -= whole;
        // Positive (wheel up) scrolls back into history.
        let lines = whole as i32;
        if lines != 0 {
            shell.publish(Message::Scroll(Scroll::Delta(lines)));
        }
    }

    /// While a started drag sits above/below the widget, scrolls one line per 50 ms and keeps
    /// the selection's free end on the edge row.
    fn autoscroll(&mut self, now: Instant, grid: &Grid, shell: &mut Shell<'_, Message>) {
        let Some(drag) = self.drag.filter(|d| d.started) else {
            return;
        };
        let b = grid.bounds;
        let dir = if drag.last.y < b.y {
            1
        } else if drag.last.y >= b.y + b.height {
            -1
        } else {
            self.last_autoscroll = None;
            return;
        };
        let due = self
            .last_autoscroll
            .is_none_or(|t| now.saturating_duration_since(t) >= AUTOSCROLL_EVERY);
        if due {
            shell.publish(Message::Scroll(Scroll::Delta(dir)));
            shell.publish(Message::Select(SelectEvent::Update(
                grid.cell_at(drag.last),
            )));
            self.last_autoscroll = Some(now);
        }
        let last = self.last_autoscroll.unwrap_or(now);
        shell.request_redraw_at(last + AUTOSCROLL_EVERY);
    }
}

fn report_button(button: Button) -> Option<input::Button> {
    Some(match button {
        Button::Left => input::Button::Left,
        Button::Middle => input::Button::Middle,
        Button::Right => input::Button::Right,
        Button::Back => input::Button::Back,
        Button::Forward => input::Button::Forward,
        Button::Other(_) => return None,
    })
}

impl MouseState {
    /// Whether the pointer belongs to the application right now (arrow cursor) rather than to
    /// local selection (I-beam).
    pub fn reporting(&self, mode: TermMode) -> bool {
        let action = MouseAction::Motion(None);
        mouse_disposition(action, keys::mods(self.modifiers), mode) == Disposition::Report
    }

    /// Sends `event` to the application when `mouse_disposition` says so; `false` leaves it to
    /// the local gestures.
    fn report(
        &mut self,
        event: &mouse::Event,
        cursor: mouse::Cursor,
        grid: &Grid,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        let mods = keys::mods(self.modifiers);
        match *event {
            // A local selection drag keeps the mouse until it ends.
            _ if self.drag.is_some() => false,
            mouse::Event::ButtonPressed(b) => {
                let (Some(button), Some(p)) = (report_button(b), cursor.position_over(grid.bounds))
                else {
                    return false;
                };
                let action = MouseAction::Press(button);
                if mouse_disposition(action, mods, grid.mode) != Disposition::Report {
                    return false;
                }
                self.report_held = Some(button);
                self.send(action, grid.cell(p), grid, shell);
                true
            }
            mouse::Event::ButtonReleased(b) => {
                let Some(button) = report_button(b).filter(|b| self.report_held == Some(*b)) else {
                    return false;
                };
                self.report_held = None;
                let at = cursor.position().map(|p| grid.cell(p));
                let at = at.or(self.report_cell).unwrap_or_default();
                self.send(MouseAction::Release(button), at, grid, shell);
                true
            }
            mouse::Event::CursorMoved { position } => {
                let held = self.report_held;
                let action = MouseAction::Motion(held);
                let ours = held.is_some()
                    || (cursor.is_over(grid.bounds)
                        && mouse_disposition(action, mods, grid.mode) == Disposition::Report);
                let at = grid.cell(position);
                if ours && self.report_cell != Some(at) {
                    self.send(action, at, grid, shell);
                }
                ours
            }
            mouse::Event::WheelScrolled { delta } => match cursor.position_over(grid.bounds) {
                Some(p) => self.report_wheel(delta, grid.cell(p), grid, shell),
                None => false,
            },
            _ => false,
        }
    }

    fn report_wheel(
        &mut self,
        delta: ScrollDelta,
        at: (usize, usize),
        grid: &Grid,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        let mods = keys::mods(self.modifiers);
        let probe = MouseAction::Wheel(Direction::Up);
        let disposition = mouse_disposition(probe, mods, grid.mode);
        if disposition == Disposition::Local {
            return false;
        }
        self.report_wheel += match delta {
            ScrollDelta::Lines { y, .. } => y,
            ScrollDelta::Pixels { y, .. } => y / (grid.metrics.cell_h * wheel_lines(grid)),
        };
        if !self.report_wheel.is_finite() {
            self.report_wheel = 0.0;
        }
        let whole = self.report_wheel.trunc();
        self.report_wheel -= whole;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "non-negative and clamped to a small count"
        )]
        let notches = whole.abs().min(100.0) as usize;
        let direction = if whole > 0.0 {
            Direction::Up
        } else {
            Direction::Down
        };
        let bytes = if disposition == Disposition::AlternateScroll {
            let lines = notches * grid.config.wheel_lines as usize;
            input::alternate_scroll(direction, lines, grid.mode)
        } else {
            let action = MouseAction::Wheel(direction);
            input::encode_mouse(action, at.0, at.1, mods, grid.mode)
                .unwrap_or_default()
                .repeat(notches)
        };
        if !bytes.is_empty() {
            shell.publish(Message::MouseInput(bytes));
        }
        true
    }

    fn send(
        &mut self,
        action: MouseAction,
        at: (usize, usize),
        grid: &Grid,
        shell: &mut Shell<'_, Message>,
    ) {
        self.report_cell = Some(at);
        let mods = keys::mods(self.modifiers);
        if let Some(bytes) = input::encode_mouse(action, at.0, at.1, mods, grid.mode) {
            shell.publish(Message::MouseInput(bytes));
        }
    }
}
