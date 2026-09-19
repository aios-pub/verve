//! Terminal rendering bridge: `alacritty_terminal` ↔ GPUI.

use std::sync::{Arc, Mutex as StdMutex};

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, SEMANTIC_ESCAPE_CHARS, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use gpui::prelude::FluentBuilder as _;
use gpui::{StatefulInteractiveElement as _, *};
use gpui_component::ActiveTheme;

/// Invisible entity used as a drag-preview for terminal text selection.
struct EmptyDrag;
impl Render for EmptyDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(0.)).h(px(0.))
    }
}

/// A null event listener for the alacritty terminal.
#[derive(Clone, Default)]
pub struct NullListener;

impl alacritty_terminal::event::EventListener for NullListener {
    fn send_event(&self, _event: alacritty_terminal::event::Event) {}
}

/// Simple dimensions struct implementing `alacritty_terminal::grid::Dimensions`.
struct TermDims {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermDims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

impl EntityInputHandler for TerminalView {
    /// GPUI calls this for text input that bypassed on_key_down (Tab, IME
    /// characters, punctuation with key_char). We forward ALL text directly
    /// to the SSH channel as raw bytes.
    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // If we are waiting for a keypress to reconnect, the first IME-typed
        // character restarts the session instead of being dropped on a dead
        // channel.
        if let Ok(mut g) = self.reconnect_trigger.lock() {
            if let Some(trigger) = g.take() {
                let _ = trigger.send(());
                return;
            }
        }
        // IME composition finished or direct text input.
        // Track last_marked_text to avoid double-sending characters that were
        // already sent incrementally via replace_and_mark_text_in_range.
        let prev = self.last_marked_text.clone();
        if !prev.is_empty() && text.starts_with(&prev) {
            let delta = &text[prev.len()..];
            if !delta.is_empty() {
                log::info!("INPUT replace_text delta: [{:?}]", delta);
                self.send_input(delta.as_bytes());
            }
        } else {
            log::info!("INPUT replace_text full: [{:?}]", text);
            // Composition commit: cancel any selection and jump to the newest
            // line, same as a plain keypress.
            self.send_user_input(text.as_bytes());
        }
        self.last_marked_text.clear();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // macOS IME sends ACCUMULATED composing text (e.g. "l", "ls", "lss").
        // We only send the delta (new chars since last call).
        let prev = self.last_marked_text.clone();
        if text.starts_with(&prev) {
            let delta = &text[prev.len()..];
            if !delta.is_empty() {
                log::info!("INPUT mark delta: [{:?}]", delta);
                self.send_input(delta.as_bytes());
            }
        } else if text.len() < prev.len() {
            // Text shortened (user backspaced during composition).
            let diff = prev.len().saturating_sub(text.len());
            for _ in 0..diff {
                self.send_input(b"\x7f");
            }
        } else {
            log::info!("INPUT mark full: [{:?}]", text);
            // First composing text of a fresh IME sequence: treat as user input
            // (clear selection / snap bottom) while later deltas stay raw sends.
            self.send_user_input(text.as_bytes());
        }
        self.last_marked_text = text.to_string();
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        if self.last_marked_text.is_empty() {
            None
        } else {
            Some(0..self.last_marked_text.len())
        }
    }

    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.last_marked_text.clear();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }
}

/// Return true if `haystack` contains the byte sequence `needle`. A simple
/// sliding-window search (needle is tiny, so this is plenty fast).
fn contains_sequence(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// A GPUI entity wrapping an alacritty terminal model.
pub struct TerminalView {
    pub term: Arc<std::sync::Mutex<Term<NullListener>>>,
    processor: Arc<std::sync::Mutex<Processor>>,
    pub cols: u16,
    pub rows: u16,
    pub font_size: f32,
    /// Sender for writing keystrokes to the SSH channel.
    pub input_tx: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>>,
    /// Sender for terminal resize notifications to SSH.
    pub resize_tx: Arc<
        std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<crate::ssh::client::ResizeCmd>>>,
    >,
    /// Keystroke interceptor subscription (catches Tab etc. before GPUI).
    _keystroke_sub: Option<gpui::Subscription>,
    /// Last IME composing text (for delta calculation).
    last_marked_text: String,
    /// Whether the left mouse button is currently held during a selection
    /// drag. The selection itself lives in the alacritty grid (`term.selection`)
    /// as ABSOLUTE grid coordinates, so it stays attached to its text while the
    /// viewport scrolls instead of floating over the viewport.
    selecting: bool,
    /// Last known mouse position (window-absolute px), used by the edge
    /// auto-scroll timer while dragging past the viewport borders.
    last_mouse: Option<(f32, f32)>,
    /// Whether the pointer moved at all during the current drag. A plain click
    /// (press + release without movement) clears its selection even when it
    /// lands on a cell's right half whose `Side::Right` would otherwise keep a
    /// one-cell selection alive.
    selection_moved: bool,
    /// Dwell counter for edge auto-scroll acceleration (reset once the pointer
    /// leaves the top/bottom hot zone).
    autoscroll_ticks: u32,
    /// Bumped on every mouse-down; stale auto-scroll tasks exit when their
    /// captured generation no longer matches, so only the latest drag scrolls.
    drag_generation: u64,
    /// Window-absolute px where the current left-button press started. A drag
    /// only begins selecting once the pointer leaves the 2 px threshold, so a
    /// tiny focus-click jitter leaves no selection (matches Zed).
    mouse_down_pos: Option<(f32, f32)>,
    /// Sub-line wheel accumulator (px). Touch pads emit small pixel deltas;
    /// whole lines are committed once the accumulator crosses a row boundary.
    scroll_px: f32,
    /// `(col, visible_row, button code)` of the last PTY mouse report, used to
    /// suppress duplicate reports while the pointer stays in one cell.
    last_report_cell: Option<(usize, usize, u8)>,
    /// Cached number of leading empty rows trimmed on last render — needed to
    /// map a screen-space Y coordinate back to the right visible row (since we
    /// render from `first_non_empty..`).
    last_trim_top: usize,
    /// Cached count of visible lines rendered last time (drives hit-testing).
    last_visible_rows: usize,
    /// Line height (px) used on last render — for mouse→row mapping.
    last_line_height: f32,
    /// Monospace character cell width (px) measured on last render — keeps
    /// mouse→column hit-testing aligned with the actual rendered glyphs.
    last_char_width: f32,
    /// Latest bounds (window-absolute rect) of the outer terminal div,
    /// captured during canvas paint. Mouse event coords are window-absolute
    /// so we subtract bounds.origin to get local coords.
    bounds: Arc<StdMutex<Bounds<Pixels>>>,
    /// Whether we've installed the global mouse handlers (done once).
    mouse_handlers_registered: bool,
    /// Tail of the previous feed chunk (up to 3 bytes). Used to detect the
    /// `ESC[2J` (clear-screen) sequence even when it straddles two SSH data
    /// chunks, so we can also purge scrollback history (alacritty only clears
    /// the viewport on `ESC[2J`, leaving history scrollable).
    last_feed_tail: std::sync::Mutex<Vec<u8>>,
    /// Guard counter so we retry layout a bounded number of times while
    /// waiting for the canvas paint hook to report the container size.
    bounds_retry: u8,
    /// When true, the terminal skips its per-render `window.focus(...)` call.
    /// Set by the SSH panel while an MFA/OTP prompt is awaiting input, so the
    /// OTP input field can keep focus instead of being stolen on every frame.
    pub suppress_auto_focus: bool,
    /// When `Some`, the session has dropped and auto-reconnect gave up; the
    /// terminal shows 已断开连接，按任意键重连. The next regular keystroke
    /// (not a Cmd shortcut) fires this oneshot exactly once and clears it,
    /// letting the SSH panel restart the connection. `None` = connected.
    reconnect_trigger: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl TerminalView {
    pub fn new(cols: u16, rows: u16, cx: &mut Context<Self>) -> Self {
        Self::new_inner(cols, rows, cx)
    }

    /// Create a terminal sized for log display (wider than shell, enough rows
    /// for scrollback). Long lines wrap normally (this is how terminals display
    /// text; "staircase" wrap was due to bad column count previously).
    pub fn new_log(cx: &mut Context<Self>) -> Self {
        Self::new_inner(240, 200, cx)
    }

    fn new_inner(cols: u16, rows: u16, cx: &mut Context<Self>) -> Self {
        let dims = TermDims {
            cols: cols as usize,
            rows: rows as usize,
        };
        let mut config = Config::default();
        // Match Zed: box-drawing `─` (tree output like `└─zms-demo.target`)
        // breaks semantic words, so double-click selects the name only.
        config.semantic_escape_chars = format!("{SEMANTIC_ESCAPE_CHARS}\u{2500}");
        let term = Term::new(config, &dims, NullListener);
        let term_arc = Arc::new(std::sync::Mutex::new(term));
        let input_tx: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let resize_tx: Arc<
            std::sync::Mutex<
                Option<tokio::sync::mpsc::UnboundedSender<crate::ssh::client::ResizeCmd>>,
            >,
        > = Arc::new(std::sync::Mutex::new(None));

        // "Press any key to reconnect" trigger; cloned into the Tab interceptor
        // below and stored on Self.
        let reconnect_trigger: Arc<
            std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        > = Arc::new(std::sync::Mutex::new(None));

        // Bounds (window-absolute rect) of the terminal div, written by the canvas
        // paint hook each frame and read by mouse handlers. Initialized to zero.
        let bounds: Arc<StdMutex<Bounds<Pixels>>> = Arc::new(StdMutex::new(Bounds::default()));

        let term_for_intercept = term_arc.clone();
        let tx_for_intercept = input_tx.clone();
        let reconnect_for_intercept = reconnect_trigger.clone();
        let sub = cx.intercept_keystrokes(move |event, _window, _cx| {
            let ks = &event.keystroke;
            if ks.key == "tab" {
                // Reconnect-on-keypress takes priority over sending a dead \t.
                if let Ok(mut g) = reconnect_for_intercept.lock() {
                    if let Some(trigger) = g.take() {
                        let _ = trigger.send(());
                        return;
                    }
                }
                let bytes = b"\t".to_vec();
                log::info!("INTERCEPT Tab → sending \\t");
                // Same user-input bookkeeping as the key handler: clear the
                // selection and snap back to the newest line.
                if let Ok(mut term) = term_for_intercept.lock() {
                    term.selection = None;
                    term.scroll_display(Scroll::Bottom);
                }
                if let Ok(tx) = tx_for_intercept.lock() {
                    if let Some(tx) = tx.as_ref() {
                        let _ = tx.send(bytes);
                    }
                }
            }
        });

        Self {
            term: term_arc,
            processor: Arc::new(std::sync::Mutex::new(Processor::new())),
            cols,
            rows,
            font_size: 14.0,
            input_tx,
            resize_tx,
            _keystroke_sub: Some(sub),
            last_marked_text: String::new(),
            selecting: false,
            last_mouse: None,
            selection_moved: false,
            autoscroll_ticks: 0,
            drag_generation: 0,
            mouse_down_pos: None,
            scroll_px: 0.0,
            last_report_cell: None,
            last_trim_top: 0,
            last_visible_rows: 0,
            last_line_height: 18.0,
            last_char_width: 14.0 * 0.58,
            bounds,
            mouse_handlers_registered: false,
            last_feed_tail: std::sync::Mutex::new(Vec::new()),
            bounds_retry: 0,
            suppress_auto_focus: false,
            reconnect_trigger,
        }
    }

    /// Map a visible row/col (row 0 = topmost rendered viewport line) to an
    /// ABSOLUTE grid point (negative line = scrollback history). This is the
    /// same viewport mapping `render` uses, so a point round-trips through
    /// scrolling exactly onto its cell.
    fn visible_to_point(&self, visible_row: usize, col: usize) -> Option<Point> {
        let term = self.term.lock().ok()?;
        let dims: &dyn Dimensions = &*term;
        let screen_lines = dims.screen_lines();
        let num_cols = dims.columns();
        let grid = term.grid();
        let viewport_top = grid.bottommost_line().0
            - screen_lines as i32 + 1
            - grid.display_offset() as i32;
        let row = visible_row.min(screen_lines.saturating_sub(1)) as i32;
        let col = Column(col.min(num_cols.saturating_sub(1)));
        Some(Point::new(Line(viewport_top + row), col))
    }

    /// Map a window-absolute mouse position to the `(col, visible row)` under
    /// it, clamped to the grid — coordinates outside the content area snap to
    /// its nearest edge (edge detection itself uses the unclamped values in the
    /// drag/up handlers).
    fn mouse_cell(&self, px: f32, py: f32) -> Option<(usize, usize)> {
        let bounds = self.bounds.lock().ok()?;
        if self.last_visible_rows == 0 || self.cols == 0 {
            return None;
        }
        let ox: f32 = bounds.origin.x.into();
        let oy: f32 = bounds.origin.y.into();
        let cw = self.last_char_width.max(0.5);
        let ch = self.last_line_height.max(1.0);
        let lx = (px - ox - 8.0).max(0.0);
        let ly = (py - oy - 4.0).max(0.0);
        let col = ((lx / cw).floor() as i64).clamp(0, self.cols as i64 - 1) as usize;
        let row =
            ((ly / ch).floor() as i64).clamp(0, self.last_visible_rows as i64 - 1) as usize;
        Some((col, row))
    }

    /// Begin a new selection at an absolute grid point. `ty` picks the
    /// mainstream behavior: `Simple` for a drag, `Semantic` for double-click
    /// (word; drag extends word by word), `Lines` for triple-click (whole line).
    fn start_selection(&mut self, ty: SelectionType, point: Point) {
        if let Ok(mut term) = self.term.lock() {
            let mut selection = Selection::new(ty, point, Side::Left);
            // Semantic/Lines selections expand immediately so the word/line is
            // highlighted before any drag.
            if matches!(ty, SelectionType::Semantic | SelectionType::Lines) {
                selection.update(point, Side::Right);
            }
            term.selection = Some(selection);
        }
    }

    /// Move the live end of the current selection to an absolute grid point.
    fn update_selection(&mut self, point: Point, side: Side) {
        if let Ok(mut term) = self.term.lock() {
            if let Some(selection) = term.selection.as_mut() {
                selection.update(point, side);
            }
        }
    }

    /// Finish a drag: optionally snap the live end to the release point, then
    /// drop a zero-length selection (a plain click without movement).
    fn end_selection(&mut self, point: Option<(Point, Side)>) {
        self.selecting = false;
        self.autoscroll_ticks = 0;
        let moved = self.selection_moved;
        self.selection_moved = false;
        if let Ok(mut term) = self.term.lock() {
            if let Some((point, side)) = point {
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
            }
            let clear = term.selection.as_ref().map_or(true, |s| {
                // A click that never moved clears a Simple selection even when
                // the release cell's right half makes it look one cell wide.
                // Semantic/Lines selections come from double/triple-click and
                // always remain.
                (!moved && s.ty == SelectionType::Simple) || s.to_range(&*term).is_none()
            });
            if clear {
                term.selection = None;
            }
        }
    }

    /// Scroll the viewport while the drag pointer rests beyond the top/bottom
    /// edge and extend the selection to the newly revealed edge line. Positive
    /// `lines` scrolls up into history (matching `Scroll::Delta`), negative
    /// returns toward the newest line. Called both per mouse-move event and by
    /// the dwell timer with an accelerating step count.
    fn selection_scroll(&mut self, lines: i32, col: usize) {
        if lines == 0 {
            return;
        }
        let Ok(mut term) = self.term.lock() else { return };
        if term.selection.is_none() {
            return;
        }
        term.scroll_display(Scroll::Delta(lines));
        let (viewport_top, screen_lines, num_cols) = {
            let dims: &dyn Dimensions = &*term;
            let grid = term.grid();
            let top = grid.bottommost_line().0
                - dims.screen_lines() as i32 + 1
                - grid.display_offset() as i32;
            (top, dims.screen_lines(), dims.columns())
        };
        let col = Column(col.min(num_cols.saturating_sub(1)));
        // Selecting UPWARD the head is the range start; Side::Left includes the
        // edge cell. Selecting DOWNWARD the head is the end; Side::Right does.
        let (line, side) = if lines > 0 {
            (viewport_top, Side::Left)
        } else {
            (viewport_top + screen_lines as i32 - 1, Side::Right)
        };
        if let Some(selection) = term.selection.as_mut() {
            selection.update(Point::new(Line(line), col), side);
        }
    }

    /// One edge auto-scroll dwell tick. Returns false when the drag is over and
    /// the polling task should exit.
    fn autoscroll_tick(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.selecting {
            return false;
        }
        let Some((px, py)) = self.last_mouse else { return true };
        if self.last_visible_rows == 0 {
            return true;
        }
        // Alt screen (vim/less/tmux, including Shift-forced local selection):
        // no edge auto-scroll — there is no primary-screen history to reveal.
        if self.term_mode().contains(TermMode::ALT_SCREEN) {
            self.autoscroll_ticks = 0;
            return true;
        }
        // Extract pointer-local coords in a block so the bounds lock guard is
        // released before the `&mut self` call below.
        let (lx, ly) = {
            let Ok(bounds) = self.bounds.lock() else { return true };
            let ox: f32 = bounds.origin.x.into();
            let oy: f32 = bounds.origin.y.into();
            (px - ox - 8.0, py - oy - 4.0)
        };
        let cw = self.last_char_width.max(0.5);
        let ch = self.last_line_height.max(1.0);
        let content_h = self.last_visible_rows as f32 * ch;
        // Hot zone: above the top padding or below the last rendered row.
        let dir = if ly < 0.0 {
            1
        } else if ly > content_h {
            -1
        } else {
            self.autoscroll_ticks = 0;
            return true;
        };
        self.autoscroll_ticks = self.autoscroll_ticks.saturating_add(1).min(100);
        // 1 line/tick (~20 lines/s at 50 ms), ramping up to 6 (~120 lines/s).
        let speed = (1 + self.autoscroll_ticks / 8).min(6);
        let col = ((lx / cw).floor() as i64).clamp(0, self.cols as i64 - 1) as usize;
        self.selection_scroll(dir * speed as i32, col);
        cx.notify();
        true
    }

    /// Snapshot the current terminal mode flags (TermMode is a Copy bitflag).
    /// On a poisoned lock, fall back to NONE (plain local behavior).
    fn term_mode(&self) -> TermMode {
        self.term.lock().map(|t| *t.mode()).unwrap_or(TermMode::NONE)
    }

    /// Whether the foreground app owns the mouse. Holding Shift is the
    /// conventional escape hatch: it forces local selection/scrolling.
    fn mouse_reporting(mode: TermMode, mods: Modifiers) -> bool {
        mode.intersects(TermMode::MOUSE_MODE) && !mods.shift
    }

    /// Send bytes typed/pasted by the USER (as opposed to internal writes):
    /// cancel the current selection and jump back to the newest line first,
    /// matching Zed's `write_input` behavior.
    fn send_user_input(&mut self, data: &[u8]) {
        if let Ok(mut term) = self.term.lock() {
            term.selection = None;
            term.scroll_display(Scroll::Bottom);
        }
        self.send_input(data);
    }

    /// Select everything from the first history line to the end of the
    /// bottommost line. Anchors are absolute grid coords, so the highlight
    /// stays attached to its content while scrolling.
    fn select_all(&mut self) {
        if let Ok(mut term) = self.term.lock() {
            let (start, end) = {
                let grid = term.grid();
                (
                    Point::new(grid.topmost_line(), Column(0)),
                    Point::new(grid.bottommost_line(), grid.last_column()),
                )
            };
            let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
            selection.update(end, Side::Right);
            term.selection = Some(selection);
        }
    }

    /// Cmd+K: clear scrollback and the viewport LOCALLY (no PTY bytes). The
    /// row under the cursor is preserved and moved to the top line, so the
    /// prompt context stays visible (approximates Zed's terminal.clear()).
    fn clear(&mut self) {
        let Ok(mut term) = self.term.lock() else { return };
        term.selection = None;

        // Snapshot the cursor row under a shared borrow, released before mut.
        let (cursor_point, num_cols, current_line) = {
            let dims: &dyn Dimensions = &*term;
            let num_cols = dims.columns();
            let grid = term.grid();
            let point = grid.cursor.point;
            let row: Vec<Cell> = grid[point.line][..Column(num_cols)].iter().cloned().collect();
            (point, num_cols, row)
        };

        // clear_history also zeroes display_offset; then blank every viewport row.
        term.grid_mut().clear_history();
        let screen_lines = {
            let dims: &dyn Dimensions = &*term;
            dims.screen_lines()
        };
        term.grid_mut().reset_region(Line(0)..Line(screen_lines as i32));

        // Move the preserved row (and cursor) to the top line.
        for (i, cell) in current_line.into_iter().enumerate() {
            term.grid_mut()[Line(0)][Column(i)] = cell;
        }
        term.grid_mut().cursor.point = Point::new(Line(0), cursor_point.column);
    }

    /// Left/middle/right button press. In mouse-reporting apps the event is
    /// encoded to the PTY and no local selection starts; otherwise the left
    /// button drives the normal selection gesture.
    fn handle_mouse_down(
        &mut self,
        button: MouseButton,
        click_count: usize,
        px: f32,
        py: f32,
        mods: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let mode = self.term_mode();

        if Self::mouse_reporting(mode, mods) {
            self.last_report_cell = None;
            if let Some((col, row)) = self.mouse_cell(px, py) {
                if let Some(code) = mouse_press_code(button) {
                    if let Some(bytes) = encode_mouse_report(
                        col,
                        row,
                        code,
                        true,
                        mods,
                        MouseEncoding::from_mode(mode),
                    ) {
                        self.send_input(&bytes);
                    }
                }
            }
            return;
        }

        // Local behavior: only the left button starts a selection gesture.
        if button != MouseButton::Left {
            return;
        }
        // click_count 0 is a synthetic release (ignored); four+ rapid clicks
        // clear the selection instead of creating one (matches Zed).
        if click_count == 0 {
            return;
        }
        if click_count >= 4 {
            if let Ok(mut term) = self.term.lock() {
                term.selection = None;
            }
            self.selecting = false;
            self.last_mouse = None;
            self.mouse_down_pos = None;
            cx.notify();
            return;
        }

        let Some((col, row)) = self.mouse_cell(px, py) else { return };

        // Double-click → semantic word selection (dragging extends it word by
        // word); triple-click → whole lines. Both resolve against the grid.
        let ty = if click_count >= 3 {
            SelectionType::Lines
        } else if click_count == 2 {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };
        let Some(point) = self.visible_to_point(row, col) else { return };
        self.start_selection(ty, point);

        // Start (or restart) the drag bookkeeping + edge auto-scroll poller.
        // A fresh generation retires any task lingering from a previous drag.
        self.selecting = true;
        self.mouse_down_pos = Some((px, py));
        self.last_mouse = Some((px, py));
        self.selection_moved = false;
        self.autoscroll_ticks = 0;
        self.drag_generation = self.drag_generation.wrapping_add(1);
        let generation = self.drag_generation;
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                smol::Timer::after(std::time::Duration::from_millis(50)).await;
                let alive = weak
                    .update(cx, |this, cx| {
                        if !this.selecting || this.drag_generation != generation {
                            return false;
                        }
                        this.autoscroll_tick(cx)
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();

        cx.notify();
    }

    /// Pointer motion. In mouse-reporting apps this sends motion/drag reports;
    /// otherwise it extends the live selection and edge-scrolls.
    fn handle_mouse_move(
        &mut self,
        px: f32,
        py: f32,
        pressed: Option<MouseButton>,
        mods: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let mode = self.term_mode();

        if Self::mouse_reporting(mode, mods) {
            let Some(code) = reportable_move_code(mode, pressed) else { return };
            let Some((col, row)) = self.mouse_cell(px, py) else { return };
            // Skip duplicates while the pointer jitters inside one cell.
            if self.last_report_cell == Some((col, row, code)) {
                return;
            }
            self.last_report_cell = Some((col, row, code));
            if let Some(bytes) =
                encode_mouse_report(col, row, code, true, mods, MouseEncoding::from_mode(mode))
            {
                self.send_input(&bytes);
            }
            return;
        }
        self.last_report_cell = None;

        // Local selection gesture: left button held after a local press.
        if !self.selecting || pressed != Some(MouseButton::Left) {
            return;
        }
        self.last_mouse = Some((px, py));

        // Ignore sub-threshold jitter from the press point (no head update,
        // no "moved" flag) — a click within 2 px behaves as a plain click.
        if let Some(down) = self.mouse_down_pos {
            if !exceeds_drag_threshold(down, (px, py)) {
                return;
            }
        }
        self.selection_moved = true;

        let Some((col, row)) = self.mouse_cell(px, py) else { return };
        let line_h = self.last_line_height.max(1.0);
        let cell_w = self.last_char_width.max(0.5);
        let content_h = self.last_visible_rows as f32 * line_h;

        // Local Y relative to the FIRST rendered row (may be negative above
        // the top edge or exceed the content height below it).
        let (lx, ly) = {
            let Ok(b) = self.bounds.lock() else { return };
            let ox: f32 = b.origin.x.into();
            let oy: f32 = b.origin.y.into();
            (px - ox - 8.0, py - oy - 4.0)
        };

        // Edge auto-scroll: speed follows overflow distance (^1.1), up to 3
        // lines per motion event; the dwell poller keeps scrolling while the
        // pointer rests. Disabled entirely on the alt screen (matches Zed).
        if !mode.contains(TermMode::ALT_SCREEN) {
            if let Some(lines) = drag_line_delta(ly, content_h, line_h) {
                self.selection_scroll(lines, col);
                cx.notify();
                return;
            }
        }

        // Inside the content area: the pixel half-cell decides whether the
        // cell itself is included (Right) or the boundary is before it (Left).
        if let Some(point) = self.visible_to_point(row, col) {
            self.update_selection(point, cell_horizontal_side(lx, cell_w));
        }
        cx.notify();
    }

    /// Button release: encode a PTY release report or finish the local gesture.
    fn handle_mouse_up(
        &mut self,
        button: MouseButton,
        px: f32,
        py: f32,
        mods: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let mode = self.term_mode();

        if Self::mouse_reporting(mode, mods) {
            if let Some((col, row)) = self.mouse_cell(px, py) {
                if let Some(code) = mouse_press_code(button) {
                    if let Some(bytes) = encode_mouse_report(
                        col,
                        row,
                        code,
                        false,
                        mods,
                        MouseEncoding::from_mode(mode),
                    ) {
                        self.send_input(&bytes);
                    }
                }
            }
            self.last_report_cell = None;
            return;
        }

        // Local: only finish a gesture actually started by a left press.
        if button != MouseButton::Left || !self.selecting {
            return;
        }
        // Finalize the head at the release cell, but only while the pointer
        // is inside the content area: an edge release keeps the head the
        // auto-scroll left on the edge line.
        let mut final_point = None;
        if let Some((col, row)) = self.mouse_cell(px, py) {
            if let Ok(b) = self.bounds.lock() {
                let oy: f32 = b.origin.y.into();
                let ox: f32 = b.origin.x.into();
                let ly = py - oy - 4.0;
                let content_h = self.last_visible_rows as f32 * self.last_line_height.max(1.0);
                if ly >= 0.0 && ly <= content_h {
                    if let Some(point) = self.visible_to_point(row, col) {
                        let lx = px - ox - 8.0;
                        final_point = Some((point, cell_horizontal_side(lx, self.last_char_width)));
                    }
                }
            }
        }
        self.end_selection(final_point);
        self.last_mouse = None;
        self.mouse_down_pos = None;
        cx.notify();
    }

    /// Wheel: extend an active selection, report to a mouse-aware app, translate
    /// to arrow keys on the alt screen, or scroll local history — in that order.
    fn handle_scroll_wheel(&mut self, ev: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
        let line_h = self.last_line_height.max(1.0);
        let content_h = self.last_visible_rows as f32 * line_h;

        // Normalize Pixels/Lines deltas to pixels, then commit whole lines via
        // the sub-line accumulator (smooth trackpad inertia / discrete notches).
        let delta_px_y: f32 = ev.delta.pixel_delta(px(line_h)).y.into();
        let Some(lines) =
            wheel_line_delta(&mut self.scroll_px, ev.touch_phase, delta_px_y, line_h, content_h)
        else {
            return;
        };

        let mode = self.term_mode();
        let shift = ev.modifiers.shift;

        // 1) Wheel while selecting extends the selection to the new edge.
        if self.selecting {
            let col = self
                .last_mouse
                .and_then(|(x, _)| {
                    let b = self.bounds.lock().ok()?;
                    let ox: f32 = b.origin.x.into();
                    let cw = self.last_char_width.max(0.5);
                    Some((((x - ox - 8.0) / cw).floor() as i64).clamp(0, self.cols as i64 - 1) as usize)
                })
                .unwrap_or(0);
            self.selection_scroll(lines, col);
            cx.notify();
            return;
        }

        // 2) App owns the mouse: repeated wheel reports (press-only).
        if mode.intersects(TermMode::MOUSE_MODE) && !shift {
            let p = ev.position;
            if let Some((col, row)) = self.mouse_cell(p.x.into(), p.y.into()) {
                for report in encode_wheel_reports(col, row, lines, ev.modifiers, mode) {
                    self.send_input(&report);
                }
            }
            cx.notify();
            return;
        }

        // 3) Alt-screen app with alternate scroll (the default): wheel acts as
        // Up/Down cursor-key presses, so less/vim/tmux scroll without a mouse.
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !shift {
            let bytes = alt_scroll_bytes(lines);
            self.send_input(&bytes);
            cx.notify();
            return;
        }

        // 4) Primary screen (or Shift escape hatch): local history scroll.
        if let Ok(mut term) = self.term.lock() {
            term.scroll_display(Scroll::Delta(lines));
        }
        cx.notify();
    }

    /// Feed raw bytes from SSH into the terminal parser.
    pub fn feed(&self, data: &[u8]) {
        // Log raw bytes to verify ANSI escape sequences are present.
        if data.contains(&0x1b) {
            log::info!(
                "FEED: {} bytes with ANSI escape, preview={:?}",
                data.len(),
                String::from_utf8_lossy(&data[..data.len().min(80)])
            );
        }

        // Advance first so escape sequences are fully applied: in particular
        // `ESC[2J` (shell `clear`) makes alacritty `clear_viewport`, which
        // scrolls the old viewport content INTO the scrollback history. We must
        // let that happen *before* we purge the history — purging first then
        // advancing would just re-push the cleared lines back into history.
        let had_clear_screen = self.scan_for_clear_screen(data);
        {
            if let (Ok(mut term), Ok(mut proc_)) = (self.term.lock(), self.processor.lock()) {
                for &byte in data {
                    proc_.advance(&mut *term, byte);
                }
            }
        }

        // After `ESC[2J` has been applied: on the PRIMARY screen, also drop the
        // scrollback history so `clear` truly clears (otherwise the old lines
        // just moved into history remain scrollable). On the alternate screen
        // (vim/less/top) we leave history untouched — those apps redraw their
        // own grid and must not wipe the primary screen's scrollback.
        if had_clear_screen {
            if let Ok(mut term) = self.term.lock() {
                let is_alt = term
                    .mode()
                    .contains(alacritty_terminal::term::TermMode::ALT_SCREEN);
                if !is_alt {
                    term.grid_mut().clear_history();
                    term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                    log::info!("FEED: ESC[2J on primary screen — scrollback history purged");
                }
            }
        }
    }

    /// Detect the `ESC[2J` (clear-screen) sequence in `data`, honouring a
    /// possible split across two feed chunks (the previous tail is carried
    /// forward). Returns true if the sequence is present in this chunk (joined
    /// with the previous tail).
    fn scan_for_clear_screen(&self, data: &[u8]) -> bool {
        const SEQ: &[u8] = b"\x1b[2J";
        // Build a continuous view: previous tail + this chunk.
        let mut buf: Vec<u8> = Vec::with_capacity(SEQ.len() - 1 + data.len());
        if let Ok(tail) = self.last_feed_tail.lock() {
            buf.extend_from_slice(&tail);
        }
        buf.extend_from_slice(data);

        let found = contains_sequence(&buf, SEQ);

        // Save the trailing bytes for next chunk's split detection.
        let start = buf.len().saturating_sub(SEQ.len() - 1);
        if let Ok(mut tail) = self.last_feed_tail.lock() {
            tail.clear();
            tail.extend_from_slice(&buf[start..]);
        }

        found
    }

    /// Resize the terminal grid.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        if let Ok(mut term) = self.term.lock() {
            let dims = TermDims {
                cols: cols as usize,
                rows: rows as usize,
            };
            term.resize(dims);
        }
    }

    /// Re-fit the terminal grid to the current container bounds (captured by
    /// the canvas paint hook on the previous frame). This makes the terminal
    /// fill its panel instead of staying fixed at the initial 180×50 — so text
    /// spans the full width and the remote program wraps at the right column.
    /// Called every render; it only re-sizes (and notifies the remote via a
    /// window-change) when the computed column/row count actually changes.
    ///
    /// Padding matches the content div: `.px_2()` (8px each side) and
    /// `.pt_1()` (4px top).
    fn resize_to_bounds(&mut self, window: &Window, cx: &mut Context<Self>) {
        let bounds = match self.bounds.lock() {
            Ok(b) => *b,
            Err(_) => return,
        };
        let bw: f32 = bounds.size.width.into();
        let bh: f32 = bounds.size.height.into();
        // bounds is zero before the first paint completes. Schedule another
        // render so we re-check once the paint hook has filled it in (capped to
        // avoid spinning forever if the layout never reports a size).
        if bw < 1.0 || bh < 1.0 {
            if self.bounds_retry < 8 {
                self.bounds_retry += 1;
                cx.notify();
            }
            return;
        }
        self.bounds_retry = 0;

        // Measure the ACTUAL rendered monospace cell width so the PTY column
        // count lines up precisely with the visual width — otherwise long
        // lines wrap at a column that doesn't match the right edge.
        //
        // `ch_advance('0')` returns a raw font metric that can differ by a
        // subpixel amount from what GPUI's text shaper actually paints. Over
        // 200+ columns that drift accumulates into a multi-char mismatch, so
        // we shape a long run of `0`s with the same Font/TextRun the renderer
        // uses and divide the resulting line width by the char count. This is
        // the exact average advance StyledText will paint.
        let mono_font = cx.theme().mono_font_family.clone();
        let font = gpui::Font {
            family: mono_font,
            weight: gpui::FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
            features: gpui::FontFeatures::default(),
            fallbacks: None,
        };
        let font_size = px(self.font_size);
        let char_w = measure_mono_cell_width(window, &font, font_size)
            .unwrap_or_else(|_| char_cell_metrics(self.font_size, self.last_line_height).0);
        // Cache the measured cell width so mouse hit-testing stays aligned with
        // the rendered glyphs.
        self.last_char_width = char_w;
        let line_h = self.last_line_height.max(1.0);

        // Horizontal padding px_2 (8px both sides); vertical pt_1 (4px top).
        // char_w is the exact shaped advance, so `floor(avail / char_w)`
        // columns are guaranteed to fit. A tiny epsilon guards against float
        // rounding making the last column clip.
        let avail_w = (bw - 16.0 - 0.01).max(char_w);
        let avail_h = (bh - 4.0).max(line_h);
        let cols = ((avail_w / char_w).floor() as u16).max(1);
        let rows = ((avail_h / line_h).floor() as u16).max(1);
        if cols != self.cols || rows != self.rows {
            log::info!(
                "RESIZE_TO_BOUNDS: {}x{} -> {}x{} (container {:.0}x{:.0}, char_w={:.2})",
                self.cols,
                self.rows,
                cols,
                rows,
                bw,
                bh,
                char_w
            );
            self.resize(cols, rows);
            self.send_window_change(cols, rows);
            cx.notify();
        }
    }

    /// Send keystroke bytes to the SSH channel.
    pub fn send_input(&self, data: &[u8]) {
        log::info!("SEND_INPUT: bytes={:?} len={}", data, data.len());
        if let Ok(tx) = self.input_tx.lock() {
            if let Some(tx) = tx.as_ref() {
                match tx.send(data.to_vec()) {
                    Ok(()) => log::info!("SEND_INPUT: sent OK"),
                    Err(e) => log::error!("SEND_INPUT: send failed: {e}"),
                }
            } else {
                log::warn!("SEND_INPUT: input_tx is None (connection not established yet)");
            }
        } else {
            log::error!("SEND_INPUT: input_tx lock poisoned");
        }
    }

    /// Set the SSH input sender (called when the connection is established).
    pub fn set_input_tx(&self, tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) {
        log::info!("SET_INPUT_TX: input sender set, connection ready");
        if let Ok(mut guard) = self.input_tx.lock() {
            *guard = Some(tx);
        }
    }

    /// Mark the session as connected (for UI status display).
    pub fn status_connected(&mut self) {
        // Connection is live; terminal is ready for input.
    }

    /// Arm the "已断开连接，按任意键重连" state. The next regular keystroke
    /// (not a Cmd shortcut) fires `trigger` exactly once and clears it; the
    /// SSH panel waits on the corresponding receiver to restart the session.
    /// Any previously-armed trigger is dropped (its waiter sees Canceled).
    pub fn arm_reconnect_trigger(
        &self,
        trigger: tokio::sync::oneshot::Sender<()>,
    ) {
        log::info!("ARM_RECONNECT_TRIGGER: waiting for keypress to reconnect");
        if let Ok(mut g) = self.reconnect_trigger.lock() {
            *g = Some(trigger);
        }
    }

    /// Disarm a pending reconnect trigger (a new connection came up, so an old
    /// "press any key" wait must not resurrect a dead session).
    pub fn disarm_reconnect_trigger(&self) {
        if let Ok(mut g) = self.reconnect_trigger.lock() {
            if g.take().is_some() {
                log::info!("DISARM_RECONNECT_TRIGGER: new connection established");
            }
        }
    }

    /// Set the resize sender (called when the connection is established).
    pub fn set_resize_tx(
        &self,
        tx: tokio::sync::mpsc::UnboundedSender<crate::ssh::client::ResizeCmd>,
    ) {
        log::info!("SET_RESIZE_TX: resize sender set");
        if let Ok(mut guard) = self.resize_tx.lock() {
            *guard = Some(tx);
        }
    }

    /// Send a terminal resize notification to the SSH channel via window-change.
    pub fn send_window_change(&self, cols: u16, rows: u16) {
        if let Ok(guard) = self.resize_tx.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(crate::ssh::client::ResizeCmd {
                    cols: cols as u32,
                    rows: rows as u32,
                });
                log::info!("WINDOW_CHANGE sent: {}x{}", cols, rows);
            }
        }
    }
}

/// xterm modifier parameter for a keystroke: 1 + shift(1) + alt(2) + ctrl(4).
/// The platform key (Cmd/Win) is excluded — those combos are OS shortcuts
/// that previously sent (and keep sending) the plain sequence.
fn xterm_modifier_param(ks: &gpui::Keystroke) -> usize {
    1 + usize::from(ks.modifiers.shift)
        + usize::from(ks.modifiers.alt) * 2
        + usize::from(ks.modifiers.control) * 4
}

/// Navigation key (arrows/Home/End). The plain form honours DECCKM
/// (application cursor keys): full-screen apps like vim/less/htop send smkx
/// (`ESC[?1h`) and then expect `ESCO A`-style sequences — sending the plain
/// `ESC[A` instead makes older vims parse it as `[` + `A` (no cursor move;
/// Left even raises E349). The xterm modified form `ESC[1;<mods><F>` is not
/// affected by DECCKM, so it stays CSI.
fn nav_seq(ks: &gpui::Keystroke, app_cursor: bool, final_ch: char) -> Vec<u8> {
    let m = xterm_modifier_param(ks);
    if m > 1 {
        format!("\x1b[1;{m}{final_ch}").into_bytes()
    } else if app_cursor {
        format!("\x1bO{final_ch}").into_bytes()
    } else {
        format!("\x1b[{final_ch}").into_bytes()
    }
}

/// `~`-terminated navigation key (Delete/PgUp/PgDn) with optional modifiers.
fn tilde_seq(ks: &gpui::Keystroke, plain: &[u8], code: u8) -> Vec<u8> {
    let m = xterm_modifier_param(ks);
    if m > 1 {
        format!("\x1b[{code};{m}~").into_bytes()
    } else {
        plain.to_vec()
    }
}

/// Convert a GPUI keystroke to the byte sequence an SSH terminal expects
/// (ANSI escape sequences for special keys, raw bytes for printable chars).
/// `app_cursor` is the terminal's DECCKM (application cursor keys) state.
fn keystroke_to_bytes(ks: &gpui::Keystroke, app_cursor: bool) -> Vec<u8> {
    // Handle modifier combinations first.
    match ks.key.as_str() {
        "enter" | "return" => return b"\r".to_vec(),
        "backspace" => {
            // Alt/Option+Backspace = delete word (ESC DEL) in shells.
            return if ks.modifiers.alt {
                b"\x1b\x7f".to_vec()
            } else {
                b"\x7f".to_vec()
            };
        }
        "tab" => return b"\t".to_vec(),
        "escape" => return b"\x1b".to_vec(),
        "space" => return b" ".to_vec(),
        "up" => return nav_seq(ks, app_cursor, 'A'),
        "down" => return nav_seq(ks, app_cursor, 'B'),
        "right" => return nav_seq(ks, app_cursor, 'C'),
        "left" => return nav_seq(ks, app_cursor, 'D'),
        "home" => return nav_seq(ks, app_cursor, 'H'),
        "end" => return nav_seq(ks, app_cursor, 'F'),
        "delete" => return tilde_seq(ks, b"\x1b[3~", 3),
        "pageup" => return tilde_seq(ks, b"\x1b[5~", 5),
        "pagedown" => return tilde_seq(ks, b"\x1b[6~", 6),
        _ => {}
    }

    // Ctrl+letter → control character.
    if ks.modifiers.control && !ks.modifiers.alt {
        if let Some(c) = ks.key.chars().next() {
            let code = c.to_ascii_lowercase() as u8;
            if (b'a'..=b'z').contains(&code) {
                return vec![code - b'a' + 1];
            }
        }
    }

    // Printable character.
    if let Some(key_char) = ks.key.as_str().chars().next() {
        if key_char.is_ascii() && !ks.modifiers.control && !ks.modifiers.platform {
            // Apply shift to letters.
            let c = if ks.modifiers.shift && key_char.is_ascii_alphabetic() {
                key_char.to_ascii_uppercase()
            } else if !ks.modifiers.shift && key_char.is_ascii_alphabetic() {
                key_char.to_ascii_lowercase()
            } else {
                key_char
            };
            return vec![c as u8];
        }
    }

    Vec::new()
}

/// Whether the pointer has moved more than 2 px from the press point — the
/// drag threshold used by Zed (`SELECTION_DRAG_THRESHOLD`), below which a
/// press/release is treated as a click instead of a drag-select.
fn exceeds_drag_threshold(down: (f32, f32), now: (f32, f32)) -> bool {
    let dx = now.0 - down.0;
    let dy = now.1 - down.1;
    (dx * dx + dy * dy).sqrt() > SELECTION_DRAG_THRESHOLD_PX
}

/// Lines to scroll for one mouse-motion event while the pointer is above the
/// top edge (positive) or below the bottom edge (negative). Speed grows with
/// overflow distance as `distance^1.1 / line_height`, clamped to ±3 lines per
/// event (ported from Zed `drag_line_delta`). Returns None inside the area.
fn drag_line_delta(ly: f32, content_h: f32, line_h: f32) -> Option<i32> {
    let line_h = line_h.max(1.0);
    if ly < 0.0 {
        Some((((-ly).powf(1.1) / line_h).ceil() as i32).clamp(1, 3))
    } else if ly > content_h {
        Some(((-(ly - content_h).powf(1.1) / line_h).floor() as i32).clamp(-3, -1))
    } else {
        None
    }
}

/// Fold one wheel pixel delta into the sub-line accumulator and return the
/// whole-line delta to commit. Started/Ended touch phases reset the
/// accumulator (trackpad gesture boundaries); on Moved, the change of the
/// truncated pixel/line quotient is emitted. Plain wheel mice always report
/// Moved, so a sign reversal against the residue resets as well, making the
/// first notch after a direction change respond immediately.
fn wheel_line_delta(
    acc: &mut f32,
    touch_phase: gpui::TouchPhase,
    dy_px: f32,
    line_h: f32,
    content_h: f32,
) -> Option<i32> {
    if matches!(touch_phase, gpui::TouchPhase::Started | gpui::TouchPhase::Ended) {
        *acc = 0.0;
        return None;
    }
    let line_h = line_h.max(1.0);
    if dy_px != 0.0 && *acc != 0.0 && dy_px.signum() != acc.signum() {
        *acc = 0.0;
    }
    let old = (*acc / line_h) as i32;
    *acc += dy_px;
    let new = (*acc / line_h) as i32;
    // Modulo the content height so residue wraps quickly at the scroll ends.
    *acc %= content_h.max(line_h);
    let lines = new - old;
    (lines != 0).then_some(lines)
}

/// Arrow-key bytes for alt-screen alternate scroll: `ESC O A` per line upward
/// (toward history), `ESC O B` downward (ported from Zed `alt_scroll`).
fn alt_scroll_bytes(lines: i32) -> Vec<u8> {
    let seq: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
    let mut out = Vec::with_capacity(lines.unsigned_abs() as usize * 3);
    for _ in 0..lines.unsigned_abs() {
        out.extend_from_slice(seq);
    }
    out
}

/// Horizontal half-cell of a local x coordinate: left half = Side::Left (the
/// boundary sits before the cell), right half = Side::Right (cell included).
fn cell_horizontal_side(lx: f32, cell_w: f32) -> Side {
    if (lx / cell_w.max(0.5)).fract() < 0.5 {
        Side::Left
    } else {
        Side::Right
    }
}

/// xterm mouse wire format selected by the terminal modes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MouseEncoding {
    /// SGR (1006): `ESC [ < b ; col ; row M/m`, coordinates are unlimited.
    Sgr,
    /// Legacy X10/normal (1000) and the UTF-8 coordinate extension (1005).
    Normal { utf8: bool },
}

impl MouseEncoding {
    fn from_mode(mode: TermMode) -> Self {
        if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Normal { utf8: true }
        } else {
            MouseEncoding::Normal { utf8: false }
        }
    }
}

/// xterm mouse button codes (modifier bits are added separately).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum MouseButtonCode {
    Left = 0,
    Middle = 1,
    Right = 2,
    LeftMove = 32,
    MiddleMove = 33,
    RightMove = 34,
    NoneMove = 35,
    ScrollUp = 64,
    ScrollDown = 65,
}

/// Modifier mask for mouse reports: Shift=4, Alt=8, Control=16 (Cmd excluded,
/// matching Zed).
fn mouse_mod_bits(mods: Modifiers) -> u8 {
    u8::from(mods.shift) * 4 + u8::from(mods.alt) * 8 + u8::from(mods.control) * 16
}

/// Button code for a press/release; unsupported buttons (Navigate) return None.
fn mouse_press_code(button: MouseButton) -> Option<u8> {
    Some(match button {
        MouseButton::Left => MouseButtonCode::Left as u8,
        MouseButton::Middle => MouseButtonCode::Middle as u8,
        MouseButton::Right => MouseButtonCode::Right as u8,
        MouseButton::Navigate(_) => return None,
    })
}

/// Button code for a motion event: held left/middle/right drag = 32/33/34,
/// plain hover = 35.
fn mouse_move_code(pressed: Option<MouseButton>) -> Option<u8> {
    Some(match pressed {
        Some(MouseButton::Left) => MouseButtonCode::LeftMove as u8,
        Some(MouseButton::Middle) => MouseButtonCode::MiddleMove as u8,
        Some(MouseButton::Right) => MouseButtonCode::RightMove as u8,
        Some(MouseButton::Navigate(_)) => return None,
        None => MouseButtonCode::NoneMove as u8,
    })
}

/// Whether a motion event should be reported under the active modes:
/// MOUSE_MOTION (1003) reports hover and drags; MOUSE_DRAG (1002) only drags;
/// MOUSE_REPORT_CLICK (1000) never reports motion.
fn reportable_move_code(mode: TermMode, pressed: Option<MouseButton>) -> Option<u8> {
    let code = mouse_move_code(pressed)?;
    if code == MouseButtonCode::NoneMove as u8 && !mode.contains(TermMode::MOUSE_MOTION) {
        return None;
    }
    if !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) {
        return None;
    }
    Some(code)
}

/// Encode one mouse report. `col`/`visible_row` are 0-based viewport
/// coordinates (clamped by `mouse_cell`); the protocols are 1-based.
fn encode_mouse_report(
    col: usize,
    visible_row: usize,
    button: u8,
    pressed: bool,
    mods: Modifiers,
    encoding: MouseEncoding,
) -> Option<Vec<u8>> {
    let mod_bits = mouse_mod_bits(mods);
    match encoding {
        MouseEncoding::Sgr => {
            let c = if pressed { 'M' } else { 'm' };
            Some(
                format!(
                    "\x1b[<{};{};{}{}",
                    button + mod_bits,
                    col + 1,
                    visible_row + 1,
                    c
                )
                .into_bytes(),
            )
        }
        MouseEncoding::Normal { utf8 } => {
            // Legacy releases always use button code 3 (plus modifier bits),
            // independent of which button was released.
            let code = if pressed { button + mod_bits } else { 3 + mod_bits };
            encode_normal_mouse(col, visible_row, code, utf8)
        }
    }
}

/// Legacy `ESC [ M Cb Cx Cy` encoding (Cb = 32 + button). Coordinates are 1
/// byte (<=223), extended to 2 UTF-8-like bytes (up to 2015) in utf8 mode.
fn encode_normal_mouse(col: usize, row: usize, button: u8, utf8: bool) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };
    if row >= max_point || col >= max_point {
        return None;
    }
    let mut msg = vec![0x1b, b'[', b'M', 32 + button];
    let encode_pos = |pos: usize| -> Vec<u8> {
        let pos = 32 + 1 + pos;
        vec![(0xC0 + pos / 64) as u8, (0x80 + (pos & 63)) as u8]
    };
    if utf8 && col >= 95 {
        msg.extend_from_slice(&encode_pos(col));
    } else {
        msg.push((32 + 1 + col) as u8);
    }
    if utf8 && row >= 95 {
        msg.extend_from_slice(&encode_pos(row));
    } else {
        msg.push((32 + 1 + row) as u8);
    }
    Some(msg)
}

/// Wheel reports are press-only (no matching release), repeated once per
/// scrolled line. Returns one byte vector per line.
fn encode_wheel_reports(
    col: usize,
    visible_row: usize,
    lines: i32,
    mods: Modifiers,
    mode: TermMode,
) -> Vec<Vec<u8>> {
    let button = if lines > 0 {
        MouseButtonCode::ScrollUp as u8
    } else {
        MouseButtonCode::ScrollDown as u8
    };
    match encode_mouse_report(
        col,
        visible_row,
        button,
        true,
        mods,
        MouseEncoding::from_mode(mode),
    ) {
        Some(bytes) => vec![bytes; lines.unsigned_abs() as usize],
        None => Vec::new(),
    }
}

/// Convert a VTE `Color` to a GPUI `Hsla`.
fn color_to_hsla(color: &Color) -> Hsla {
    match color {
        Color::Named(nc) => named_color_to_hsla(*nc),
        Color::Spec(rgb) => hsla_from_rgb8(rgb.r, rgb.g, rgb.b),
        Color::Indexed(idx) => {
            let i = *idx as usize;
            if i < 16 {
                let nc = match idx {
                    0 => NamedColor::Black,
                    1 => NamedColor::Red,
                    2 => NamedColor::Green,
                    3 => NamedColor::Yellow,
                    4 => NamedColor::Blue,
                    5 => NamedColor::Magenta,
                    6 => NamedColor::Cyan,
                    7 => NamedColor::White,
                    8 => NamedColor::BrightBlack,
                    9 => NamedColor::BrightRed,
                    10 => NamedColor::BrightGreen,
                    11 => NamedColor::BrightYellow,
                    12 => NamedColor::BrightBlue,
                    13 => NamedColor::BrightMagenta,
                    14 => NamedColor::BrightCyan,
                    15 => NamedColor::BrightWhite,
                    _ => NamedColor::White,
                };
                named_color_to_hsla(nc)
            } else if i <= 231 {
                // 6×6×6 color cube (indices 16..=231).
                let n = i - 16;
                let r = n / 36;
                let g = (n % 36) / 6;
                let b = n % 6;
                fn cube(v: usize) -> u8 {
                    if v == 0 { 0 } else { (55 + 40 * v) as u8 } // 0,95,135,175,215,255
                }
                hsla_from_rgb8(cube(r), cube(g), cube(b))
            } else {
                // 24-step grayscale ramp (indices 232..=255): 8..=238.
                let gray = (8 + (i - 232) * 10) as u8;
                hsla_from_rgb8(gray, gray, gray)
            }
        }
    }
}

fn hsla_from_rgb8(r: u8, g: u8, b: u8) -> Hsla {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 0.001 {
        return gpui::hsla(0.0, 0.0, l, 1.0);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + (if g < b { 6.0 } else { 0.0 })
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    gpui::hsla(h, s, l, 1.0)
}

fn named_color_to_hsla(c: NamedColor) -> Hsla {
    // Color palette tuned to match Termius/VS Code dark terminal theme.
    match c {
        NamedColor::Black => gpui::hsla(0.0, 0.0, 0.2, 1.0),
        NamedColor::Red => gpui::hsla(0.0, 0.75, 0.55, 1.0),
        NamedColor::Green => gpui::hsla(0.33, 0.65, 0.5, 1.0),
        NamedColor::Yellow => gpui::hsla(0.13, 0.75, 0.5, 1.0),
        NamedColor::Blue => gpui::hsla(0.6, 0.7, 0.55, 1.0),
        NamedColor::Magenta => gpui::hsla(0.83, 0.65, 0.55, 1.0),
        NamedColor::Cyan => gpui::hsla(0.52, 0.65, 0.5, 1.0),
        NamedColor::White => gpui::hsla(0.0, 0.0, 0.8, 1.0),
        NamedColor::BrightBlack => gpui::hsla(0.0, 0.0, 0.45, 1.0),
        NamedColor::BrightRed => gpui::hsla(0.0, 0.8, 0.62, 1.0),
        NamedColor::BrightGreen => gpui::hsla(0.33, 0.7, 0.55, 1.0),
        NamedColor::BrightYellow => gpui::hsla(0.13, 0.8, 0.58, 1.0),
        NamedColor::BrightBlue => gpui::hsla(0.6, 0.75, 0.62, 1.0),
        NamedColor::BrightMagenta => gpui::hsla(0.83, 0.7, 0.62, 1.0),
        NamedColor::BrightCyan => gpui::hsla(0.52, 0.7, 0.58, 1.0),
        NamedColor::BrightWhite => gpui::hsla(0.0, 0.0, 0.92, 1.0),
        // Foreground/Background = default text/bg colors.
        NamedColor::Foreground | NamedColor::DimForeground => gpui::hsla(0.0, 0.0, 0.85, 1.0),
        NamedColor::BrightForeground => gpui::hsla(0.0, 0.0, 0.95, 1.0),
        NamedColor::Background => gpui::hsla(0.0, 0.0, 0.12, 1.0),
        NamedColor::Cursor => gpui::hsla(0.0, 0.0, 0.85, 1.0),
        // Dim variants map to slightly muted versions.
        NamedColor::DimBlack => gpui::hsla(0.0, 0.0, 0.15, 1.0),
        NamedColor::DimRed => gpui::hsla(0.0, 0.5, 0.4, 1.0),
        NamedColor::DimGreen => gpui::hsla(0.33, 0.4, 0.35, 1.0),
        NamedColor::DimYellow => gpui::hsla(0.13, 0.5, 0.4, 1.0),
        NamedColor::DimBlue => gpui::hsla(0.6, 0.5, 0.4, 1.0),
        NamedColor::DimMagenta => gpui::hsla(0.83, 0.4, 0.4, 1.0),
        NamedColor::DimCyan => gpui::hsla(0.52, 0.4, 0.4, 1.0),
        NamedColor::DimWhite => gpui::hsla(0.0, 0.0, 0.6, 1.0),
    }
}

/// Fixed dark terminal surface (always dark, independent of the app theme).
/// Matches the `Background`/`Foreground` arms in `named_color_to_hsla`.
const TERMINAL_BG: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.12,
    a: 1.0,
}; // ~#1f1f1f
const TERMINAL_FG: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.85,
    a: 1.0,
};
/// Extra leading (px) added to font_size to get line height. 13 + 4 = 17px.
const LINE_LEADING: f32 = 4.0;

/// Pointer must leave this radius (px) from the press point before a drag
/// selection starts (matches Zed's SELECTION_DRAG_THRESHOLD = 2.0).
const SELECTION_DRAG_THRESHOLD_PX: f32 = 2.0;

/// Resolved, coalesce-able visual attributes for a run of terminal cells.
#[derive(Clone, PartialEq)]
struct RunStyle {
    fg: Hsla,
    /// `None` means the terminal surface shows through (transparent bg).
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikeout: bool,
}

/// One fully-rendered terminal row: its text plus styled runs and a
/// column→byte prefix map for selection/copy.
#[derive(Clone)]
struct RowData {
    text: String,
    runs: Vec<TextRun>,
    /// `prefix[c]` is the byte offset in `text` *after* columns 0..c have been
    /// emitted. Length is `num_cols + 1`. Wide-char spacer columns contribute 0
    /// bytes, so `prefix[c+1] == prefix[c]` for a spacer. Every value is on a
    /// char boundary, so it is safe to slice `text` with these offsets.
    prefix: Vec<usize>,
}

/// Build a solid underline style colored with the run's foreground.
fn underline_style(fg: Hsla) -> UnderlineStyle {
    UnderlineStyle {
        thickness: px(1.0),
        color: Some(fg),
        wavy: false,
    }
}

/// Build a solid strikethrough style colored with the run's foreground.
fn strikethrough_style(fg: Hsla) -> StrikethroughStyle {
    StrikethroughStyle {
        thickness: px(1.0),
        color: Some(fg),
    }
}

/// Resolve one alacritty `Cell` into a `RunStyle` (fg/bg colors + font flags).
fn resolve_style(cell: &Cell) -> RunStyle {
    let flags = cell.flags;

    // INVERSE: swap fg and bg BEFORE resolving.
    let (fg_color, bg_color) = if flags.contains(Flags::INVERSE) {
        (&cell.bg, &cell.fg)
    } else {
        (&cell.fg, &cell.bg)
    };

    let mut fg = color_to_hsla(fg_color);

    // DIM: reduce lightness (preserves hue/saturation); combines with bold/color.
    if flags.contains(Flags::DIM) {
        fg.l = (fg.l * 0.7).clamp(0.0, 1.0);
    }

    // Background: Named(Background) is the terminal surface => transparent.
    // Anything else (including Named(Foreground), which can appear after an
    // INVERSE swap) becomes an explicit background.
    let bg = match bg_color {
        Color::Named(NamedColor::Background) => None,
        other => Some(color_to_hsla(other)),
    };

    RunStyle {
        fg,
        bg,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(
            Flags::UNDERLINE
                | Flags::DOUBLE_UNDERLINE
                | Flags::UNDERCURL
                | Flags::DOTTED_UNDERLINE
                | Flags::DASHED_UNDERLINE,
        ),
        strikeout: flags.contains(Flags::STRIKEOUT),
    }
}

/// Index into the 4-entry font cache: [regular, bold, italic, bold-italic].
#[inline]
fn font_index(bold: bool, italic: bool) -> usize {
    (bold as usize) | ((italic as usize) << 1)
}

/// Build a `RowData` from one grid line of `num_cols` cells.
///
/// `fonts` is the 4-entry cache `[regular, bold, italic, bold-italic]`.
fn build_row(
    grid: &alacritty_terminal::grid::Grid<Cell>,
    grid_line: alacritty_terminal::index::Line,
    num_cols: usize,
    fonts: &[Font; 4],
) -> RowData {
    let mut text = String::with_capacity(num_cols);
    // prefix[0] = 0 before any column is emitted.
    let mut prefix = Vec::with_capacity(num_cols + 1);
    prefix.push(0);

    // Coalesced style segments as (start_byte, RunStyle).
    let mut segs: Vec<(usize, RunStyle)> = Vec::new();
    let mut cur_style: Option<RunStyle> = None;
    let mut cur_start: usize = 0;

    for col_idx in 0..num_cols {
        let point = alacritty_terminal::index::Point::new(
            grid_line,
            alacritty_terminal::index::Column(col_idx),
        );
        let cell: &Cell = &grid[point];

        // WIDE_CHAR_SPACER: the trailing half of a CJK/emoji glyph. Skip it
        // entirely — no char, no run break, 0 bytes. The glyph itself was
        // emitted by the preceding (WIDE_CHAR) cell. Detect by FLAG, not by
        // `c == '\0'`: live parsing writes `c = ' '` for spacers.
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            prefix.push(text.len());
            continue;
        }

        let style = resolve_style(cell);
        if cur_style.as_ref() != Some(&style) {
            if let Some(prev) = cur_style.take() {
                segs.push((cur_start, prev));
            }
            cur_start = text.len(); // byte offset BEFORE pushing this glyph
            cur_style = Some(style);
        }

        // The character to render. HIDDEN => blank but keeps style (e.g.
        // password masking); '\0' => defensive blank (uninitialized storage).
        let ch = if cell.flags.contains(Flags::HIDDEN) {
            ' '
        } else if cell.c == '\0' {
            ' '
        } else {
            cell.c
        };
        text.push(ch);

        // Zero-width combining marks / emoji ZWJ joiners attach to this base
        // cell and share its run/style.
        if let Some(zw) = cell.zerowidth() {
            for z in zw {
                text.push(*z);
            }
        }

        prefix.push(text.len());
    }
    if let Some(prev) = cur_style.take() {
        segs.push((cur_start, prev));
    }

    // Convert segments -> TextRuns. Each run's byte length is the distance to
    // the next segment start (or to the end of the text), which always lands
    // on a char boundary because segments start/end only at cell boundaries.
    let mut runs = Vec::with_capacity(segs.len());
    for i in 0..segs.len() {
        let (start, style) = &segs[i];
        let end = segs
            .get(i + 1)
            .map(|(s, _)| *s)
            .unwrap_or_else(|| text.len());
        let font = &fonts[font_index(style.bold, style.italic)];
        runs.push(TextRun {
            len: end - start,
            font: font.clone(),
            color: style.fg,
            background_color: style.bg,
            underline: style.underline.then(|| underline_style(style.fg)),
            strikethrough: style.strikeout.then(|| strikethrough_style(style.fg)),
        });
    }

    debug_assert_eq!(
        runs.iter().map(|r| r.len).sum::<usize>(),
        text.len(),
        "terminal row TextRun lengths must sum to the text byte length"
    );

    RowData { text, runs, prefix }
}

/// Given base runs covering `text_len` fully, return a new run list in which
/// every byte within `[sel_start, sel_end)` has `background_color = sel_bg`
/// (a translucent wash). Text color, font, underline and strikethrough are
/// preserved — the selection is an overlay, exactly like a real terminal.
///
/// `sel_start`/`sel_end` are byte offsets that must fall on char boundaries
/// (they come from `RowData::prefix`). Output runs still sum to `text_len`.
fn apply_selection(
    runs: &[TextRun],
    text_len: usize,
    sel_start: usize,
    sel_end: usize,
    sel_bg: Hsla,
) -> Vec<TextRun> {
    if sel_start >= sel_end || sel_start >= text_len {
        return runs.to_vec();
    }
    let sel_end = sel_end.min(text_len);

    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut pos = 0usize;
    for run in runs {
        let r_start = pos;
        let r_end = pos + run.len;
        pos = r_end;

        // No overlap with the selection.
        if r_end <= sel_start || r_start >= sel_end {
            out.push(run.clone());
            continue;
        }

        // Portion before the selection (if any).
        if r_start < sel_start {
            let mut r = run.clone();
            r.len = sel_start - r_start;
            out.push(r);
        }

        // Selected portion: override the background, keep everything else.
        let mid_start = r_start.max(sel_start);
        let mid_end = r_end.min(sel_end);
        if mid_end > mid_start {
            let mut r = run.clone();
            r.len = mid_end - mid_start;
            r.background_color = Some(sel_bg);
            out.push(r);
        }

        // Portion after the selection (if any).
        if r_end > sel_end {
            let mut r = run.clone();
            r.len = r_end - sel_end;
            out.push(r);
        }
    }
    out
}

// --- Log-syntax highlighting overlay ---------------------------------------
//
// Remote log output (journalctl, Spring Boot, logback, python logging, etc.)
// usually prints timestamps and level keywords as PLAIN TEXT with no ANSI
// colors — the shell does not know they are semantically special. This overlay
// post-processes each rendered row's TextRuns to color those tokens, making
// logs far easier to scan. It is layered ON TOP of any ANSI SGR colors the
// remote did send (so already-colored regions still get the semantic recolor
// for these well-known tokens), and it never changes fonts/backgrounds.

/// Timestamp: `2026-08-06 06:21:18`, `2026-08-06T06:21:18.123`,
/// `2026-08-06 06:21:18,456+08:00`, `06:21:18.123`.
const HL_TIMESTAMP: Hsla = Hsla {
    h: 0.52,
    s: 0.70,
    l: 0.72,
    a: 1.0,
}; // vivid cyan
/// ERROR / FATAL / PANIC / EXCEPTION.
const HL_ERROR: Hsla = Hsla {
    h: 0.0,
    s: 0.75,
    l: 0.62,
    a: 1.0,
}; // bright red
/// WARN / WARNING.
const HL_WARN: Hsla = Hsla {
    h: 0.11,
    s: 0.80,
    l: 0.62,
    a: 1.0,
}; // amber
/// INFO.
const HL_INFO: Hsla = Hsla {
    h: 0.33,
    s: 0.55,
    l: 0.62,
    a: 1.0,
}; // green
/// DEBUG / TRACE.
const HL_DEBUG: Hsla = Hsla {
    h: 0.60,
    s: 0.55,
    l: 0.68,
    a: 1.0,
}; // soft blue
/// JSON booleans/null/numbers (when not already ANSI-colored).
const HL_VALUE: Hsla = Hsla {
    h: 0.78,
    s: 0.50,
    l: 0.72,
    a: 1.0,
}; // lavender

fn log_highlighter() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        // Patterns are ordered most-specific first; regex's leftmost-longest
        // alternation plus named capture groups lets us map the winning group
        // back to its color. All patterns are ASCII and land on char boundaries.
        regex::Regex::new(
            r"(?x)
            (?P<timestamp>
                \d{4}-\d{2}-\d{2}[T\x20]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?
              | \d{2}:\d{2}:\d{2}(?:[.,]\d+)?
            )
          | (?P<error>\b(?:ERROR|FATAL|PANIC|EXCEPTION|Error|Fatal|Exception)\b)
          | (?P<warn>\b(?:WARN(?:ING)?|Warning)\b)
          | (?P<info>\b(?:INFO|Info)\b)
          | (?P<debug>\b(?:DEBUG|TRACE|Debug|Trace)\b)
          | (?P<value>\b(?:true|false|null|TRUE|FALSE|NULL)\b)
            ",
        )
        .expect("valid log highlight regex")
    })
}

/// Color to use for a winning named capture group.
fn highlight_color_for(name: &str) -> Option<Hsla> {
    match name {
        "timestamp" => Some(HL_TIMESTAMP),
        "error" => Some(HL_ERROR),
        "warn" => Some(HL_WARN),
        "info" => Some(HL_INFO),
        "debug" => Some(HL_DEBUG),
        "value" | "number" => Some(HL_VALUE),
        _ => None,
    }
}

/// Apply log-syntax coloring on top of base runs. Returns a new run list with
/// matches split out and recolored; everything else (font, background,
/// underline, ANSI colors outside matches) is preserved unchanged.
///
/// Matches are found with a byte-oriented (ASCII) regex, so all offsets are
/// already byte offsets. We mark every byte covered by<[PLHD82_never_used_51bce0c785ca2f68081bfa7d91973934]> by a match with its
/// color, then walk the runs and split at marked boundaries.
fn apply_log_highlights(text: &str, runs: &[TextRun]) -> Vec<TextRun> {
    // Fast path: nothing matches → reuse the base runs untouched.
    if !log_highlighter().is_match(text) {
        return runs.to_vec();
    }

    // per-byte highlight color. 0 = no highlight; indices 1.. map to the
    // palette. Using a u8 palette id keeps the mark array small (text is
    // ASCII where we mark, so byte == char here).
    const PALETTE: [Hsla; 6] = [HL_TIMESTAMP, HL_ERROR, HL_WARN, HL_INFO, HL_DEBUG, HL_VALUE];
    fn palette_id(c: Hsla) -> u8 {
        PALETTE
            .iter()
            .position(|p| p.h == c.h && p.s == c.s && p.l == c.l)
            .map(|i| (i + 1) as u8)
            .unwrap_or(0)
    }

    let mut mark = vec![0u8; text.len()];
    for caps in log_highlighter().captures_iter(text) {
        let full = caps.get(0).expect("overall match");
        // Find which named group matched.
        let mut color = None;
        for name in ["timestamp", "error", "warn", "info", "debug", "value"] {
            if let Some(m) = caps.name(name) {
                if m.start() == full.start() && m.end() == full.end() {
                    color = highlight_color_for(name);
                    break;
                }
            }
        }
        let Some(color) = color else { continue };
        let id = palette_id(color);
        let end = full.end().min(text.len());
        for b in &mut mark[full.start()..end] {
            *b = id;
        }
    }

    // Walk runs and split wherever the mark changes. Each output segment keeps
    // the original run's font/background/underline/strikethrough; only `color`
    // is overridden when the segment is highlighted.
    let mut out: Vec<TextRun> = Vec::with_capacity(runs.len() + 8);
    let mut pos = 0usize;
    for run in runs {
        let run_start = pos;
        let run_end = pos + run.len;
        let mut seg_start = run_start;
        let mut seg_id = mark.get(seg_start).copied().unwrap_or(0);
        for off in (run_start + 1)..run_end {
            let id = mark[off];
            if id != seg_id {
                let mut r = run.clone();
                r.len = off - seg_start;
                if seg_id != 0 {
                    r.color = PALETTE[(seg_id - 1) as usize];
                }
                out.push(r);
                seg_start = off;
                seg_id = id;
            }
        }
        // Final segment of this run.
        let mut r = run.clone();
        r.len = run_end - seg_start;
        if seg_id != 0 {
            r.color = PALETTE[(seg_id - 1) as usize];
        }
        out.push(r);
        pos = run_end;
    }

    // Merge adjacent segments that ended up identical (same font, color, bg,
    // decorations) — common when no highlight fell inside a run.
    out.dedup_by(|a, b| {
        a.len == 0
            || (a.font == b.font
                && a.color == b.color
                && a.background_color == b.background_color
                && a.underline == b.underline
                && a.strikethrough == b.strikethrough
                && {
                    b.len += a.len;
                    true
                })
    });
    out.retain(|r| r.len > 0);
    out
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = TERMINAL_BG;
        let default_fg = TERMINAL_FG;
        let font_size = self.font_size;
        let line_height = font_size + LINE_LEADING;

        // Resolve the monospace family up-front (this is the only thing we need
        // from the theme). Do it before the `&mut cx` calls below so no
        // immutable borrow of `cx` is held across them.
        let mono_font = cx.theme().mono_font_family.clone();
        let focus_handle = cx.focus_handle();

        // Fit the grid to the container's actual pixel size so text spans the
        // full width (instead of the initial fixed 180 cols). Uses the bounds
        // captured on the previous frame; self-corrects on the next render.
        // Only acts when the column/row count actually changes, so it settles
        // and won't thrash once sized.
        self.resize_to_bounds(window, cx);

        // Auto-focus (unless a sibling UI — e.g. the MFA/OTP input — needs it).
        if !self.suppress_auto_focus {
            window.focus(&focus_handle, cx);
        }

        // Build renderable lines from the alacritty grid, carrying per-cell
        // ANSI color/style as coalesced TextRuns. Honors scrollback history.
        // One Font per weight/style variant; shared (cheaply cloned) across
        // every run instead of constructing a Font per cell.
        let fonts = [
            Font {
                family: mono_font.clone(),
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
                features: FontFeatures::default(),
                fallbacks: None,
            },
            Font {
                family: mono_font.clone(),
                weight: FontWeight::BOLD,
                style: FontStyle::Normal,
                features: FontFeatures::default(),
                fallbacks: None,
            },
            Font {
                family: mono_font.clone(),
                weight: FontWeight::NORMAL,
                style: FontStyle::Italic,
                features: FontFeatures::default(),
                fallbacks: None,
            },
            Font {
                family: mono_font.clone(),
                weight: FontWeight::BOLD,
                style: FontStyle::Italic,
                features: FontFeatures::default(),
                fallbacks: None,
            },
        ];

        let (lines, cursor_cell, viewport_top, sel_pts): (
            Vec<RowData>,
            Option<(usize, usize)>,
            i32,
            Option<(Point, Point)>,
        ) = {
            let Ok(term) = self.term.lock() else {
                return div()
                    .id("terminal-view")
                    .w_full()
                    .bg(bg)
                    .child("终端加载中…")
                    .into_any_element();
            };
            let dims: &dyn Dimensions = &*term;
            let num_cols = dims.columns();
            let grid = term.grid();

            // Read the VISIBLE viewport (screen_lines rows). Normally this is
            // the bottommost screen; but when the user scrolls back into
            // history (mouse wheel / Shift+PgUp), alacritty's `display_offset`
            // shifts the viewport up into the scrollback history (negative line
            // coords). We honor it here so scrolled content actually renders.
            let screen_lines = dims.screen_lines();
            let display_offset = grid.display_offset();
            let bottommost = grid.bottommost_line().0;
            // display_offset=0 → newest screen; >0 → shift viewport up into
            // history (negative line coordinates, which alacritty supports).
            let viewport_top = bottommost - screen_lines as i32 + 1 - display_offset as i32;

            // Cursor cell (visible row index, column) for the caret overlay.
            // Hidden while the program disables it (ESC[?25l, e.g. vim) or
            // when the cursor sits outside the visible viewport.
            let cursor_cell = if term
                .mode()
                .contains(alacritty_terminal::term::TermMode::SHOW_CURSOR)
            {
                let row = grid.cursor.point.line.0 - viewport_top;
                let col = grid.cursor.point.column.0;
                if row >= 0 && (row as usize) < screen_lines && col < num_cols {
                    Some((row as usize, col))
                } else {
                    None
                }
            } else {
                None
            };

            // Resolve the live selection to ABSOLUTE grid points (negative line
            // = scrollback history). `to_range` already normalized it to
            // top-left → bottom-right and clamped it to the grid. We intersect
            // these with each visible row below, so the highlight stays pinned
            // to its text while the viewport scrolls instead of floating.
            let sel_pts = term
                .selection
                .as_ref()
                .and_then(|s| s.to_range(&*term))
                .map(|range| (range.start, range.end));

            // Do NOT trim trailing whitespace: trailing spaces can carry a
            // colored background (e.g. a TUI status bar), and trimming would
            // cut that background short.
            (
                (0..screen_lines)
                    .map(|row_idx| {
                        let grid_line = viewport_top + row_idx as i32;
                        build_row(grid, Line(grid_line), num_cols, &fonts)
                    })
                    .collect::<Vec<_>>(),
                cursor_cell,
                viewport_top,
                sel_pts,
            )
        };

        // Update cached geometry so mouse events can map positions back to rows.
        self.last_trim_top = 0; // trim only affects what we render; mapping uses visible rows
        self.last_visible_rows = lines.len();
        self.last_line_height = line_height;

        // Selection highlight color (bright blue overlay for visibility).
        let sel_bg = gpui::hsla(0.60, 0.70, 0.50, 0.45);
        // `mono_font` was built above (used by the per-run Font cache); set on
        // each row div too so the monospace family cascades to StyledText.

        // Build the rendered row elements up-front so we can use them in both
        // the content area and move mouse handlers into the overlay.
        let lines_rendered: Vec<AnyElement> = lines
            .iter()
            .enumerate()
            .map(|(row_idx, row)| {
                let lh = line_height;
                let fs = font_size;
                let row_el = div()
                    .h(px(lh))
                    .w_full()
                    .line_height(px(lh))
                    .text_size(px(fs))
                    .font_family(mono_font.clone())
                    .text_color(default_fg)
                    .whitespace_nowrap();

                // Layer 1: semantic log highlighting (timestamps, levels, JSON
                // numbers/bools) on top of any ANSI SGR colors the remote sent.
                let base_runs = apply_log_highlights(&row.text, &row.runs);

                // Layer 2: selection overlay. Determine whether the selection
                // intersects this VISIBLE row by translating it to the absolute
                // grid line it renders; if so, [from_byte, to_byte) is the byte
                // range to highlight. Column indices map to bytes via
                // `row.prefix` (which skips wide-char spacer columns and always
                // lands on char boundaries). The selection end column is
                // inclusive, hence `+ 1`.
                let abs_line = viewport_top + row_idx as i32;
                let final_runs = match sel_pts {
                    Some((start, end))
                        if abs_line >= start.line.0 && abs_line <= end.line.0 =>
                    {
                        let num_row_cols = row.prefix.len().saturating_sub(1);
                        let from_col = if abs_line == start.line.0 {
                            start.column.0
                        } else {
                            0
                        };
                        let to_col = if abs_line == end.line.0 {
                            (end.column.0 + 1).min(num_row_cols)
                        } else {
                            // Full row: last prefix entry == text byte length.
                            num_row_cols
                        };
                        let from_byte = row.prefix.get(from_col).copied().unwrap_or(row.text.len());
                        let to_byte = row
                            .prefix
                            .get(to_col)
                            .copied()
                            .unwrap_or(row.text.len())
                            .max(from_byte);
                        apply_selection(&base_runs, row.text.len(), from_byte, to_byte, sel_bg)
                    }
                    _ => base_runs,
                };

                if row.text.is_empty() || final_runs.is_empty() {
                    row_el.into_any_element()
                } else {
                    row_el
                        .child(StyledText::new(row.text.clone()).with_runs(final_runs))
                        .into_any_element()
                }
            })
            .collect();

        let entity = cx.entity();
        let fh = focus_handle.clone();
        let bounds_for_paint: Arc<StdMutex<Bounds<Pixels>>> = self.bounds.clone();

        // Caret overlay: a translucent block at the cursor cell, painted after
        // (above) the text rows. Uses the same px_2/pt_1 padding as the content
        // div and the measured cell width from resize_to_bounds; skipped until
        // a real cell width exists (first frame).
        let cursor_el = {
            let char_w = self.last_char_width;
            if char_w > 0.0 {
                cursor_cell.map(|(row, col)| {
                    div()
                        .absolute()
                        .left(px(8.0 + col as f32 * char_w))
                        .top(px(4.0 + row as f32 * line_height))
                        .w(px(char_w.ceil()))
                        .h(px(line_height))
                        .bg(default_fg.opacity(0.45))
                        .rounded(px(1.))
                })
            } else {
                None
            }
        };

        div()
            .id("terminal-view")
            .track_focus(&focus_handle)
            .tab_stop(false)
            .size_full()
            .bg(bg)
            .text_color(default_fg)
            .relative()
            // Content rows (inside padding).
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .size_full()
                    .items_start()
                    .children(lines_rendered),
            )
            .when_some(cursor_el, |el, caret| el.child(caret))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    let is_copy = (event.keystroke.modifiers.platform
                        && event.keystroke.key == "c")
                        || (event.keystroke.modifiers.control && event.keystroke.key == "insert");
                    if is_copy {
                        // Copy straight from the grid: works for selections that
                        // span scrollback (edge auto-scroll / wheel-drag) and
                        // handles visual wrap, tabs and wide chars correctly.
                        if let Ok(term) = this.term.lock() {
                            if let Some(text) = term.selection_to_string() {
                                if !text.is_empty() {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                                    cx.stop_propagation();
                                    return;
                                }
                            }
                        }
                    }
                    // Cmd+A: select all scrollback + viewport content.
                    if event.keystroke.modifiers.platform
                        && event.keystroke.key == "a"
                    {
                        this.select_all();
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    // Cmd+K: clear the screen locally (no PTY bytes).
                    if event.keystroke.modifiers.platform
                        && event.keystroke.key == "k"
                    {
                        this.clear();
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    if event.keystroke.modifiers.platform && event.keystroke.key == "v" {
                        if let Some(item) = cx.read_from_clipboard() {
                            let text = item.text().unwrap_or_default();
                            if !text.is_empty() {
                                this.send_user_input(text.as_bytes());
                                cx.notify();
                                cx.stop_propagation();
                                return;
                            }
                        }
                    }
                    // Shift+PgUp/PgDn → scroll the local scrollback history
                    // (matches iTerm2 / VS Code). Plain PgUp/PgDn (no Shift)
                    // keep falling through to be sent to the remote program.
                    if event.keystroke.modifiers.shift {
                        let scroll = match event.keystroke.key.as_str() {
                            "pageup" => Some(alacritty_terminal::grid::Scroll::PageUp),
                            "pagedown" => Some(alacritty_terminal::grid::Scroll::PageDown),
                            _ => None,
                        };
                        if let Some(scroll) = scroll {
                            if let Ok(mut term) = this.term.lock() {
                                term.scroll_display(scroll);
                            }
                            cx.notify();
                            cx.stop_propagation();
                            return;
                        }
                    }
                    // Arrows/Home/End must honour the remote's DECCKM
                    // (application cursor keys) request — see nav_seq.
                    let app_cursor = if let Ok(term) = this.term.lock() {
                        term.mode()
                            .contains(alacritty_terminal::term::TermMode::APP_CURSOR)
                    } else {
                        false
                    };
                    // Disconnected after auto-reconnect gave up: the first real
                    // keystroke restarts the session instead of being dropped on
                    // the dead channel. Local Cmd shortcuts above already ran.
                    if let Ok(mut g) = this.reconnect_trigger.lock() {
                        if let Some(trigger) = g.take() {
                            let _ = trigger.send(());
                            cx.notify();
                            cx.stop_propagation();
                            window.refresh();
                            return;
                        }
                    }
                    let bytes = keystroke_to_bytes(&event.keystroke, app_cursor);
                    if !bytes.is_empty() {
                        // User keystrokes clear any selection and snap the
                        // viewport back to the newest line (see send_user_input).
                        this.send_user_input(&bytes);
                        cx.notify();
                        cx.stop_propagation();
                        window.refresh();
                    }
                })
            )
            // Mouse wheel. Selection-drag extends the selection; mouse-aware
            // TUI apps get wheel reports; alt-screen apps with alternate scroll
            // (less/vim/tmux) get cursor-key presses; otherwise the local
            // scrollback history scrolls. Sub-pixel deltas accumulate, so
            // trackpad inertia is smooth. See `handle_scroll_wheel`.
            .on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _window, cx| {
                this.handle_scroll_wheel(ev, cx);
            }))
            // Absolutely-positioned canvas that covers the entire area so its
            // paint hook fires reliably. Only job: record bounds (for mouse coord
            // conversion) and register the keyboard input handler.
            .child(
                canvas(
                    |_bounds, _window, _cx| {},
                    move |bounds, _state, window, cx: &mut App| {
                        if let Ok(mut b) = bounds_for_paint.lock() {
                            *b = bounds;
                        }
                        window.handle_input(
                            &fh,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0(),
            )
            // Mouse handlers live on the outer div (not canvas) so GPUI
            // dispatches them reliably. ev.position is window-absolute; the
            // handlers subtract bounds.origin. All three buttons are
            // registered so TUI apps receive middle/right mouse reports.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    window.focus(&cx.focus_handle(), cx);
                    let p = ev.position;
                    this.handle_mouse_down(
                        ev.button,
                        ev.click_count,
                        p.x.into(),
                        p.y.into(),
                        ev.modifiers,
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    window.focus(&cx.focus_handle(), cx);
                    let p = ev.position;
                    this.handle_mouse_down(
                        ev.button,
                        ev.click_count,
                        p.x.into(),
                        p.y.into(),
                        ev.modifiers,
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    window.focus(&cx.focus_handle(), cx);
                    let p = ev.position;
                    this.handle_mouse_down(
                        ev.button,
                        ev.click_count,
                        p.x.into(),
                        p.y.into(),
                        ev.modifiers,
                        cx,
                    );
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                let p = ev.position;
                this.handle_mouse_move(
                    p.x.into(),
                    p.y.into(),
                    ev.pressed_button,
                    ev.modifiers,
                    cx,
                );
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _window, cx| {
                    let p = ev.position;
                    this.handle_mouse_up(ev.button, p.x.into(), p.y.into(), ev.modifiers, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, ev: &MouseUpEvent, _window, cx| {
                    let p = ev.position;
                    this.handle_mouse_up(ev.button, p.x.into(), p.y.into(), ev.modifiers, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseUpEvent, _window, cx| {
                    let p = ev.position;
                    this.handle_mouse_up(ev.button, p.x.into(), p.y.into(), ev.modifiers, cx);
                }),
            )
            .into_any_element()
    }
}

/// Measure the true per-cell advance of a monospace font by shaping a long run
/// of `0`s and dividing the painted line width by the char count. This matches
/// what `StyledText` actually renders (unlike `ch_advance`, whose raw font
/// metric can drift by a subpixel amount that accumulates over many columns),
/// so the PTY column count lines up with the visual wrap width.
fn measure_mono_cell_width(window: &Window, font: &gpui::Font, font_size: Pixels) -> Result<f32> {
    const SAMPLE_CHARS: usize = 64;
    let sample: String = "0".repeat(SAMPLE_CHARS);
    let run = TextRun {
        len: sample.len(),
        font: font.clone(),
        color: TERMINAL_FG,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let lines = window
        .text_system()
        .shape_text(sample.into(), font_size, &[run], None, None)?;
    let w: f32 = lines
        .first()
        .map(|l| l.width().into())
        .ok_or_else(|| anyhow::anyhow!("shaped empty line"))?;
    Ok((w / SAMPLE_CHARS as f32).max(1.0))
}

/// Given font size and line height (both px), returns the monospace character
/// cell width in pixels. Hardcoded aspect ratio calibrated for Menlo/JetBrains
/// Mono at common UI sizes. If selection drifts, tune this value.
fn char_cell_metrics(font_size: f32, line_height: f32) -> (f32, f32) {
    // Monospace fonts typically render at ~0.55-0.60 × font_size per cell.
    // Line height is the vertical advance per rendered row.
    let w = font_size * 0.58;
    (w, line_height)
}

#[cfg(test)]
mod tests {
    // Import only the pure helpers under test — NOT `super::*`, which would
    // pull in GPUI's macro-heavy prelude and blow the `#[test]` macro's
    // recursion limit.
    use super::{
        MouseEncoding,
        alt_scroll_bytes, apply_log_highlights, apply_selection, cell_horizontal_side,
        color_to_hsla, contains_sequence, drag_line_delta, encode_mouse_report,
        encode_wheel_reports, exceeds_drag_threshold, keystroke_to_bytes, reportable_move_code,
        wheel_line_delta,
    };
    use alacritty_terminal::term::TermMode;
    use alacritty_terminal::vte::ansi::{Color, NamedColor};
    use gpui::{
        Font, FontFeatures, FontStyle, FontWeight, Hsla, Modifiers, MouseButton, TextRun,
        TouchPhase,
    };

    fn ks(key: &str) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers: Default::default(),
            key: key.to_string(),
            key_char: None,
        }
    }

    fn ks_ctrl(key: &str) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control: true,
                ..Default::default()
            },
            key: key.to_string(),
            key_char: None,
        }
    }

    /// Normal (cursor) mode sends the CSI arrow forms.
    #[test]
    fn arrows_csi_without_decckm() {
        assert_eq!(keystroke_to_bytes(&ks("up"), false), b"\x1b[A");
        assert_eq!(keystroke_to_bytes(&ks("down"), false), b"\x1b[B");
        assert_eq!(keystroke_to_bytes(&ks("right"), false), b"\x1b[C");
        assert_eq!(keystroke_to_bytes(&ks("left"), false), b"\x1b[D");
        assert_eq!(keystroke_to_bytes(&ks("home"), false), b"\x1b[H");
        assert_eq!(keystroke_to_bytes(&ks("end"), false), b"\x1b[F");
    }

    /// After smkx (DECCKM application cursor keys — what vim enters at
    /// startup) arrows/Home/End must switch to the SS3 `ESCO x` forms.
    /// Sending CSI here is what broke cursor movement on older remote vims
    /// (`ESC[D` parsed as `[` + `D` → E349, no movement).
    #[test]
    fn arrows_ss3_with_decckm() {
        assert_eq!(keystroke_to_bytes(&ks("up"), true), b"\x1bOA");
        assert_eq!(keystroke_to_bytes(&ks("down"), true), b"\x1bOB");
        assert_eq!(keystroke_to_bytes(&ks("right"), true), b"\x1bOC");
        assert_eq!(keystroke_to_bytes(&ks("left"), true), b"\x1bOD");
        assert_eq!(keystroke_to_bytes(&ks("home"), true), b"\x1bOH");
        assert_eq!(keystroke_to_bytes(&ks("end"), true), b"\x1bOF");
    }

    /// Modifier combos keep the xterm `ESC[1;<mods><F>` form in both modes
    /// (DECCKM does not affect the modified encoding), and tilde keys
    /// (Delete/PgUp/PgDn) never change either.
    #[test]
    fn modified_and_tilde_keys_unchanged_by_decckm() {
        for app in [false, true] {
            assert_eq!(
                keystroke_to_bytes(&ks_ctrl("right"), app),
                b"\x1b[1;5C"
            );
            assert_eq!(keystroke_to_bytes(&ks("delete"), app), b"\x1b[3~");
            assert_eq!(keystroke_to_bytes(&ks("pageup"), app), b"\x1b[5~");
            assert_eq!(keystroke_to_bytes(&ks("pagedown"), app), b"\x1b[6~");
        }
    }

    #[test]
    fn contains_sequence_detects_clear_screen() {
        // ESC[2J is the "clear screen" sequence the shell `clear` command emits.
        let esc_2j = b"\x1b[2J";
        // Plain match.
        assert!(contains_sequence(b"hello\x1b[2Jworld", esc_2j));
        // `clear` typically emits cursor-home first then clear.
        assert!(contains_sequence(b"\x1b[H\x1b[2J", esc_2j));
        // Negative: ESC[3J (clear scrollback) must NOT match the ESC[2J detector.
        assert!(!contains_sequence(b"\x1b[3J", esc_2j));
        // Negative: ESC[1J / ESC[0J must not match.
        assert!(!contains_sequence(b"\x1b[1J", esc_2j));
        assert!(!contains_sequence(b"\x1b[0J", esc_2j));
        // Edge: empty / too-short haystack.
        assert!(!contains_sequence(b"", esc_2j));
        assert!(!contains_sequence(b"\x1b", esc_2j));
    }

    /// Simulate the cross-chunk split detection used by `feed`: the tail bytes
    /// of one chunk are carried into the next so an `ESC[2J` split across two
    /// feed calls is still detected. We replicate just the join logic here
    /// (the GPUI entity can't be constructed in a pure lib test).
    #[test]
    fn clear_screen_detected_across_chunk_split() {
        const SEQ: &[u8] = b"\x1b[2J";
        // Split "abc\x1b[2J" as "abc\x1b[" + "2J".
        let chunk1 = b"abc\x1b[";
        let chunk2 = b"2J";
        // Tail of chunk1 (last SEQ.len()-1 bytes) + chunk2 forms the joined view.
        let tail_len = SEQ.len() - 1;
        let tail = &chunk1[chunk1.len().saturating_sub(tail_len)..];
        let mut joined = Vec::new();
        joined.extend_from_slice(tail);
        joined.extend_from_slice(chunk2);
        assert!(contains_sequence(&joined, SEQ));
    }

    /// 256-color cube: spot-check standard xterm indices against the expected
    /// RGB channels (0,95,135,175,215,255) and the 24-step grayscale ramp.
    #[test]
    fn indexed_256_color_cube_and_grayscale() {
        fn expect_rgb(idx: u8, r: u8, g: u8, b: u8) {
            let got = color_to_hsla(&Color::Indexed(idx));
            let want = super::hsla_from_rgb8(r, g, b);
            assert!(
                (got.h - want.h).abs() < 0.02
                    && (got.s - want.s).abs() < 0.02
                    && (got.l - want.l).abs() < 0.02,
                "index {idx}: got {:?}, want ~{:?}",
                got,
                want
            );
        }
        // i=16  -> n=0   -> (0,0,0)
        expect_rgb(16, 0, 0, 0);
        // i=196 -> n=180 -> r=5,g=0,b=0 -> (255,0,0) pure red
        expect_rgb(196, 255, 0, 0);
        // i=51  -> n=35  -> r=0,g=5,b=5 -> (0,255,255) cyan
        expect_rgb(51, 0, 255, 255);
        // i=21  -> n=5   -> r=0,g=0,b=5 -> (0,0,255) blue
        expect_rgb(21, 0, 0, 255);
        // Grayscale ramp: index 232 -> gray 8, 255 -> gray 238.
        expect_rgb(232, 8, 8, 8);
        expect_rgb(255, 238, 238, 238);
        // The first 16 indices map through the named palette.
        assert_eq!(
            color_to_hsla(&Color::Indexed(1)),
            color_to_hsla(&Color::Named(NamedColor::Red))
        );
    }

    /// Build a bare-bones run of `len` bytes for the selection tests.
    fn test_run(len: usize, bg: Option<Hsla>) -> TextRun {
        TextRun {
            len,
            font: Font {
                family: "mono".into(),
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
                features: FontFeatures::default(),
                fallbacks: None,
            },
            color: super::TERMINAL_FG,
            background_color: bg,
            underline: None,
            strikethrough: None,
        }
    }

    #[test]
    fn apply_selection_preserves_total_length_and_colors() {
        // Two runs covering 10 bytes total ("0123456789").
        let runs = vec![test_run(4, None), test_run(6, None)];
        let sel_bg = gpui::hsla(0.6, 0.7, 0.5, 0.45);

        // Selection spans both runs (bytes 2..8).
        let out = apply_selection(&runs, 10, 2, 8, sel_bg);
        let total: usize = out.iter().map(|r| r.len).sum();
        assert_eq!(total, 10, "output runs must still cover the whole text");
        // The selected sub-runs must carry the selection background.
        assert!(out.iter().any(|r| r.background_color == Some(sel_bg)));
        // And the run text COLOR must be unchanged (not replaced).
        assert!(out.iter().all(|r| r.color == super::TERMINAL_FG));
    }

    #[test]
    fn apply_selection_wholly_inside_one_run_splits_into_three() {
        let runs = vec![test_run(10, None)];
        let sel_bg = gpui::hsla(0.6, 0.7, 0.5, 0.45);
        let out = apply_selection(&runs, 10, 3, 7, sel_bg);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].len, 3);
        assert_eq!(out[0].background_color, None);
        assert_eq!(out[1].len, 4);
        assert_eq!(out[1].background_color, Some(sel_bg));
        assert_eq!(out[2].len, 3);
        assert_eq!(out[2].background_color, None);
    }

    #[test]
    fn apply_selection_at_boundaries_and_empty_range() {
        let runs = vec![test_run(5, None)];
        let sel_bg = gpui::hsla(0.6, 0.7, 0.5, 0.45);
        // Selection covering the entire first run.
        let out = apply_selection(&runs, 5, 0, 5, sel_bg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len, 5);
        assert_eq!(out[0].background_color, Some(sel_bg));
        // Empty / out-of-range selection returns the runs untouched.
        let out = apply_selection(&runs, 5, 3, 3, sel_bg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].background_color, None);
        let out = apply_selection(&runs, 5, 99, 100, sel_bg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].background_color, None);
    }

    #[test]
    fn log_highlights_recolors_levels_and_timestamps() {
        let base = || test_run(0, None);
        // Build a base run that spans the whole line; the highlighter splits it.
        let line = "2026-08-06 06:21:18 INFO  starting up";
        let runs = vec![TextRun {
            len: line.len(),
            ..base()
        }];
        let out = apply_log_highlights(line, &runs);

        // The output runs must still cover every byte exactly.
        assert_eq!(out.iter().map(|r| r.len).sum::<usize>(), line.len());

        let color_at = |byte_idx: usize| -> Hsla {
            let mut pos = 0;
            for r in &out {
                if byte_idx < pos + r.len {
                    return r.color;
                }
                pos += r.len;
            }
            panic!("byte {byte_idx} past end");
        };

        // Timestamp (bytes 0..19) → HL_TIMESTAMP.
        assert_eq!(color_at(0), super::HL_TIMESTAMP);
        assert_eq!(color_at(10), super::HL_TIMESTAMP);
        // INFO token starts at byte 20 → HL_INFO.
        assert_eq!(color_at(20), super::HL_INFO);
        // The word "starting" reverts to the base fg.
        let starting = line.find("starting").unwrap();
        assert_eq!(color_at(starting), super::TERMINAL_FG);
    }

    #[test]
    fn log_highlights_error_and_warn_colors() {
        for (word, want) in [
            ("ERROR", super::HL_ERROR),
            ("FATAL", super::HL_ERROR),
            ("WARN", super::HL_WARN),
            ("WARNING", super::HL_WARN),
            ("DEBUG", super::HL_DEBUG),
            ("TRACE", super::HL_DEBUG),
        ] {
            let line = format!("2026-01-01 00:00:00 {word} something");
            let runs = vec![TextRun {
                len: line.len(),
                ..test_run(0, None)
            }];
            let out = apply_log_highlights(&line, &runs);
            assert_eq!(out.iter().map(|r| r.len).sum::<usize>(), line.len());
            let start = line.find(word).unwrap();
            let mut pos = 0;
            let mut got = super::TERMINAL_FG;
            for r in &out {
                if start < pos + r.len {
                    got = r.color;
                    break;
                }
                pos += r.len;
            }
            assert_eq!(got, want, "wrong color for {word}");
        }
    }

    #[test]
    fn log_highlights_passthrough_when_no_match() {
        // A line with nothing to highlight returns the base runs untouched.
        let line = "plain uncolored output";
        let runs = vec![TextRun {
            len: line.len(),
            ..test_run(0, None)
        }];
        let out = apply_log_highlights(line, &runs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len, line.len());
        assert_eq!(out[0].color, super::TERMINAL_FG);
    }

    #[test]
    fn drag_threshold_two_px() {
        assert!(!exceeds_drag_threshold((0.0, 0.0), (1.0, 1.0)));
        // Exactly 2 px is still below the strict threshold.
        assert!(!exceeds_drag_threshold((0.0, 0.0), (2.0, 0.0)));
        assert!(exceeds_drag_threshold((0.0, 0.0), (2.1, 0.0)));
        assert!(exceeds_drag_threshold((10.0, 10.0), (10.0, 5.0)));
    }

    #[test]
    fn edge_drag_delta_curve_and_clamp() {
        const LH: f32 = 18.0;
        // Inside the content area: no edge scroll.
        assert_eq!(drag_line_delta(0.0, 100.0, LH), None);
        assert_eq!(drag_line_delta(50.0, 100.0, LH), None);
        // Just past the top: one line; far past the top accelerates but caps
        // at 3 per motion event.
        assert_eq!(drag_line_delta(-1.0, 100.0, LH), Some(1));
        assert_eq!(drag_line_delta(-18.0, 100.0, LH), Some(2));
        assert_eq!(drag_line_delta(-100.0, 100.0, LH), Some(3));
        // Below the bottom edge: symmetric, negative (toward the newest line).
        assert_eq!(drag_line_delta(101.0, 100.0, LH), Some(-1));
        assert_eq!(drag_line_delta(200.0, 100.0, LH), Some(-3));
    }

    #[test]
    fn wheel_accumulator_phases_and_sub_lines() {
        let (lh, h) = (18.0_f32, 360.0_f32);
        let mut acc = 9.0;
        // Touch phases reset the accumulator and never commit a scroll.
        assert_eq!(wheel_line_delta(&mut acc, TouchPhase::Started, 10.0, lh, h), None);
        assert_eq!(acc, 0.0);
        // Two small trackpad deltas accumulate into one line.
        assert_eq!(wheel_line_delta(&mut acc, TouchPhase::Moved, 12.0, lh, h), None);
        assert_eq!(wheel_line_delta(&mut acc, TouchPhase::Moved, 12.0, lh, h), Some(1));
        acc = 5.0;
        assert_eq!(wheel_line_delta(&mut acc, TouchPhase::Ended, 99.0, lh, h), None);
        assert_eq!(acc, 0.0);
        // A reversed notch after accumulated motion reacts immediately.
        let mut acc = 0.0;
        assert_eq!(wheel_line_delta(&mut acc, TouchPhase::Moved, 30.0, lh, h), Some(1));
        assert_eq!(wheel_line_delta(&mut acc, TouchPhase::Moved, -30.0, lh, h), Some(-1));
        // Steady stream of exactly one line per event, 20 times (modulo wrap).
        let mut acc = 0.0;
        let mut total = 0;
        for _ in 0..20 {
            if let Some(n) = wheel_line_delta(&mut acc, TouchPhase::Moved, 18.0, lh, h) {
                total += n;
            }
        }
        assert_eq!(total, 20);
    }

    #[test]
    fn alt_scroll_sequences() {
        assert_eq!(alt_scroll_bytes(3), b"\x1bOA\x1bOA\x1bOA");
        assert_eq!(alt_scroll_bytes(-2), b"\x1bOB\x1bOB");
    }

    #[test]
    fn cell_side_left_and_right_half() {
        const CW: f32 = 8.0;
        assert_eq!(cell_horizontal_side(0.25 * CW, CW), super::Side::Left);
        // Whole-cell offsets must not affect the fractional half-cell check.
        assert_eq!(cell_horizontal_side(10.0 * CW + 0.75 * CW, CW), super::Side::Right);
    }

    #[test]
    fn sgr_mouse_reports() {
        let no_mods = Modifiers::default();
        // Left press/release at the origin cell (protocol coords 1-based).
        assert_eq!(
            encode_mouse_report(0, 0, 0, true, no_mods, MouseEncoding::Sgr).unwrap(),
            b"\x1b[<0;1;1M"
        );
        assert_eq!(
            encode_mouse_report(0, 0, 0, false, no_mods, MouseEncoding::Sgr).unwrap(),
            b"\x1b[<0;1;1m"
        );
        // Modifier bits: control=16, shift+alt=12.
        let ctrl = Modifiers { control: true, ..Default::default() };
        assert_eq!(
            encode_mouse_report(0, 0, 0, true, ctrl, MouseEncoding::Sgr).unwrap(),
            b"\x1b[<16;1;1M"
        );
        let shift_alt = Modifiers { shift: true, alt: true, ..Default::default() };
        assert_eq!(
            encode_mouse_report(0, 0, 0, true, shift_alt, MouseEncoding::Sgr).unwrap(),
            b"\x1b[<12;1;1M"
        );
        // col 4, row 2 → 5,3.
        assert_eq!(
            encode_mouse_report(4, 2, 0, true, no_mods, MouseEncoding::Sgr).unwrap(),
            b"\x1b[<0;5;3M"
        );
        // Wheel: press-only reports repeated per scrolled line.
        let sgr_mode = TermMode::SGR_MOUSE;
        let ups = encode_wheel_reports(0, 0, 3, no_mods, sgr_mode);
        assert_eq!(ups.len(), 3);
        assert!(ups.iter().all(|b| b == b"\x1b[<64;1;1M"));
        let downs = encode_wheel_reports(0, 0, -2, no_mods, sgr_mode);
        assert_eq!(downs.len(), 2);
        assert!(downs.iter().all(|b| b == b"\x1b[<65;1;1M"));
    }

    #[test]
    fn normal_mouse_reports() {
        let no_mods = Modifiers::default();
        let enc = MouseEncoding::Normal { utf8: false };
        // ESC [ M, 32+0, 32+1+col, 32+1+row.
        assert_eq!(
            encode_mouse_report(0, 0, 0, true, no_mods, enc).unwrap(),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
        // Release uses button code 3 regardless of the released button.
        let up = encode_mouse_report(0, 0, 0, false, no_mods, enc).unwrap();
        assert_eq!(up[3], 32 + 3);
        // One-byte coordinate ceiling is 223 (222 → final byte 255).
        assert!(encode_mouse_report(223, 0, 0, true, no_mods, enc).is_none());
        let edge = encode_mouse_report(222, 0, 0, true, no_mods, enc).unwrap();
        assert_eq!(edge[4], 255);

        // UTF-8 extended coordinates: col 95 encodes as two bytes, and the
        // range extends to 2015.
        let u8enc = MouseEncoding::Normal { utf8: true };
        let pos = 32 + 1 + 95; // 128 → 0xC2, 0x80
        let msg = encode_mouse_report(95, 0, 0, true, no_mods, u8enc).unwrap();
        assert_eq!(&msg[4..6], &[(0xC0 + pos / 64) as u8, (0x80 + (pos & 63)) as u8]);
        assert!(encode_mouse_report(0, 2014, 0, true, no_mods, u8enc).is_some());
        assert!(encode_mouse_report(0, 2015, 0, true, no_mods, u8enc).is_none());
    }

    #[test]
    fn move_reports_follow_mode_bits() {
        // Click-only (1000): no motion reports at all.
        assert_eq!(
            reportable_move_code(TermMode::MOUSE_REPORT_CLICK, Some(MouseButton::Left)),
            None
        );
        assert_eq!(reportable_move_code(TermMode::MOUSE_REPORT_CLICK, None), None);
        // Button-event/drag (1002): drags report, plain hover does not.
        assert_eq!(
            reportable_move_code(TermMode::MOUSE_DRAG, Some(MouseButton::Left)),
            Some(32)
        );
        assert_eq!(reportable_move_code(TermMode::MOUSE_DRAG, None), None);
        // Any-motion (1003): hover = 35, right-drag = 34.
        assert_eq!(reportable_move_code(TermMode::MOUSE_MOTION, None), Some(35));
        assert_eq!(
            reportable_move_code(TermMode::MOUSE_MOTION, Some(MouseButton::Right)),
            Some(34)
        );
        // Nav buttons have no mouse encoding.
        assert_eq!(
            reportable_move_code(
                TermMode::MOUSE_MOTION,
                Some(MouseButton::Navigate(gpui::NavigationDirection::Back))
            ),
            None
        );
    }

}
