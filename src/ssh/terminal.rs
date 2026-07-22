//! Terminal rendering bridge: `alacritty_terminal` ↔ GPUI.

use std::sync::{Arc, Mutex as StdMutex};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use gpui::prelude::FluentBuilder as _;
use gpui::{StatefulInteractiveElement as _, *};
use gpui_component::{h_flex, ActiveTheme};

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
            self.send_input(text.as_bytes());
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
            self.send_input(text.as_bytes());
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
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }
}

/// A selected (row, col) coordinate in the terminal grid. 0-indexed, row 0 is
/// the topmost visible line after trimming leading empties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectionPoint {
    row: usize,
    col: usize,
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
    /// Active text selection (start/end in visible row indices after trimming).
    /// None when no selection. When dragging, anchor stays fixed while head moves.
    selection_anchor: Option<SelectionPoint>,
    selection_head: Option<SelectionPoint>,
    /// Whether the mouse is currently being dragged to extend a selection.
    selecting: bool,
    /// Cached number of leading empty rows trimmed on last render — needed to
    /// map a screen-space Y coordinate back to the right visible row (since we
    /// render from `first_non_empty..`).
    last_trim_top: usize,
    /// Cached count of visible lines rendered last time (drives hit-testing).
    last_visible_rows: usize,
    /// Line height (px) used on last render — for mouse→row mapping.
    last_line_height: f32,
    /// Latest bounds (window-absolute rect) of the outer terminal div,
    /// captured during canvas paint. Mouse event coords are window-absolute
    /// so we subtract bounds.origin to get local coords.
    bounds: Arc<StdMutex<Bounds<Pixels>>>,
    /// Whether we've installed the global mouse handlers (done once).
    mouse_handlers_registered: bool,
}

impl TerminalView {
    pub fn new(cols: u16, rows: u16, cx: &mut Context<Self>) -> Self {
        let dims = TermDims {
            cols: cols as usize,
            rows: rows as usize,
        };
        let config = Config::default();
        let term = Term::new(config, &dims, NullListener);
        let term_arc = Arc::new(std::sync::Mutex::new(term));
        let input_tx: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let resize_tx: Arc<
            std::sync::Mutex<
                Option<tokio::sync::mpsc::UnboundedSender<crate::ssh::client::ResizeCmd>>,
            >,
        > = Arc::new(std::sync::Mutex::new(None));

        // Bounds (window-absolute rect) of the terminal div, written by the canvas
        // paint hook each frame and read by mouse handlers. Initialized to zero.
        let bounds: Arc<StdMutex<Bounds<Pixels>>> = Arc::new(StdMutex::new(Bounds::default()));

        let term_for_intercept = term_arc.clone();
        let tx_for_intercept = input_tx.clone();
        let sub = cx.intercept_keystrokes(move |event, _window, _cx| {
            let ks = &event.keystroke;
            if ks.key == "tab" {
                let bytes = b"\t".to_vec();
                log::info!("INTERCEPT Tab → sending \\t");
                if let Ok(tx) = tx_for_intercept.lock() {
                    if let Some(tx) = tx.as_ref() {
                        let _ = tx.send(bytes);
                    }
                }
                let _ = &term_for_intercept;
            }
        });

        Self {
            term: term_arc,
            processor: Arc::new(std::sync::Mutex::new(Processor::new())),
            cols,
            rows,
            font_size: 13.0,
            input_tx,
            resize_tx,
            _keystroke_sub: Some(sub),
            last_marked_text: String::new(),
            selection_anchor: None,
            selection_head: None,
            selecting: false,
            last_trim_top: 0,
            last_visible_rows: 0,
            last_line_height: 13.0,
            bounds,
            mouse_handlers_registered: false,
        }
    }

    /// Return the currently selected text (normalized from anchor/head), or
    /// None if no selection exists.
    fn selected_text(&self, lines: &[String]) -> Option<String> {
        let (a, h) = self.selection_anchor.zip(self.selection_head)?;
        let (start, end) = if (a.row, a.col) <= (h.row, h.col) { (a, h) } else { (h, a) };
        if start.row >= lines.len() {
            return None;
        }
        if start.row == end.row {
            let line = &lines[start.row];
            let end_col = end.col.min(line.len());
            let start_col = start.col.min(end_col);
            return Some(line[start_col..end_col].to_string());
        }
        let mut out = String::new();
        out.push_str(&lines[start.row][start.col.min(lines[start.row].len())..]);
        for row in (start.row + 1)..end.row.min(lines.len()) {
            out.push('\n');
            out.push_str(lines[row].trim_end());
        }
        if end.row < lines.len() {
            out.push('\n');
            out.push_str(&lines[end.row][..end.col.min(lines[end.row].len())]);
        }
        Some(out)
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
        if let (Ok(mut term), Ok(mut proc_)) = (self.term.lock(), self.processor.lock()) {
            for &byte in data {
                proc_.advance(&mut *term, byte);
            }
        }
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

/// Convert a GPUI keystroke to the byte sequence an SSH terminal expects
/// (ANSI escape sequences for special keys, raw bytes for printable chars).
fn keystroke_to_bytes(ks: &gpui::Keystroke) -> Vec<u8> {
    // Handle modifier combinations first.
    match ks.key.as_str() {
        "enter" | "return" => return b"\r".to_vec(),
        "backspace" => return b"\x7f".to_vec(),
        "tab" => return b"\t".to_vec(),
        "escape" => return b"\x1b".to_vec(),
        "space" => return b" ".to_vec(),
        "up" => return b"\x1b[A".to_vec(),
        "down" => return b"\x1b[B".to_vec(),
        "right" => return b"\x1b[C".to_vec(),
        "left" => return b"\x1b[D".to_vec(),
        "home" => return b"\x1b[H".to_vec(),
        "end" => return b"\x1b[F".to_vec(),
        "delete" => return b"\x1b[3~".to_vec(),
        "pageup" => return b"\x1b[5~".to_vec(),
        "pagedown" => return b"\x1b[6~".to_vec(),
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

/// Convert a VTE `Color` to a GPUI `Hsla`.
fn color_to_hsla(color: &Color, _default_fg: Hsla) -> Hsla {
    match color {
        Color::Named(nc) => named_color_to_hsla(*nc),
        Color::Spec(rgb) => hsla_from_rgb8(rgb.r, rgb.g, rgb.b),
        Color::Indexed(idx) => {
            // Map basic 16 colors + 216 color cube + 24 grayscale.
            if (*idx as usize) < 16 {
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
            } else {
                // 256-color fallback: approximate as grayscale.
                let v = *idx as f32 / 255.0;
                gpui::hsla(0.0, 0.0, v, 1.0)
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

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let bg = theme.background;
        let default_fg = theme.foreground;
        let font_size = self.font_size;
        let line_height = font_size;
        let focus_handle = cx.focus_handle();

        // Do NOT resize in render — it causes data loss and timing issues.
        // Terminal size stays at the initial 180x50 set at creation time.
        // Window-change was already sent when the connection was established.

        // Auto-focus.
        window.focus(&focus_handle, cx);

        // Build renderable lines from the alacritty grid.
        // Read ALL lines including scrollback history.
        let lines: Vec<String> = {
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

            // Only read the VISIBLE viewport (screen_lines rows from bottom).
            // This matches what a real terminal shows — not scrollback history.
            // After `clear`, the visible viewport is all blank with the cursor
            // at the top; the shell then prints the prompt there.
            let screen_lines = dims.screen_lines();
            let bottommost = grid.bottommost_line().0;
            let viewport_top = bottommost - screen_lines as i32 + 1;

            let mut all_text: Vec<String> = Vec::new();
            for row_idx in 0..screen_lines {
                let grid_line = viewport_top + row_idx as i32;
                let mut text = String::with_capacity(num_cols);
                for col_idx in 0..num_cols {
                    let point = alacritty_terminal::index::Point::new(
                        alacritty_terminal::index::Line(grid_line),
                        alacritty_terminal::index::Column(col_idx),
                    );
                    let cell: &Cell = &grid[point];
                    let ch = if cell.c == '\0' { ' ' } else { cell.c };
                    text.push(ch);
                }
                all_text.push(text);
            }

            // Trim leading empty rows (don't show 50 blank rows above content).
            // But always keep at least 1 row so clear shows a blank screen.
            let first_non_empty = all_text
                .iter()
                .position(|row| !row.trim().is_empty())
                .unwrap_or(0);

            let trimmed = all_text[first_non_empty..].to_vec();
            trimmed
        };

        // Update cached geometry so mouse events can map positions back to rows.
        self.last_trim_top = 0; // trim only affects what we render; mapping uses visible rows
        self.last_visible_rows = lines.len();
        self.last_line_height = line_height;

        // Normalize selection points (anchor ≤ head) for rendering.
        let sel = self
            .selection_anchor
            .zip(self.selection_head)
            .map(|(a, h)| {
                if (a.row, a.col) <= (h.row, h.col) { (a, h) } else { (h, a) }
            });

        // Selection highlight color (bright blue overlay for visibility).
        let sel_bg = gpui::hsla(0.60, 0.70, 0.50, 0.45);

        // Build the rendered row elements up-front so we can use them in both
        // the content area and move mouse handlers into the overlay.
        let lines_rendered: Vec<AnyElement> = lines
            .clone()
            .into_iter()
            .enumerate()
            .map(|(row_idx, text)| {
                let lh = line_height;
                let fs = font_size;
                let row_el = div()
                    .h(px(lh))
                    .line_height(px(lh))
                    .text_size(px(fs))
                    .whitespace_nowrap();

                let Some((start, end)) = sel else {
                    return row_el.text_color(default_fg).child(text).into_any_element();
                };

                if row_idx < start.row || row_idx > end.row {
                    return row_el.text_color(default_fg).child(text).into_any_element();
                }

                let len = text.len();
                let from = if row_idx == start.row { start.col } else { 0 };
                let to = if row_idx == end.row { end.col.min(len) } else { len };
                let from = from.min(len);
                let to = to.max(from);

                let before = text[..from].to_string();
                let selected = text[from..to].to_string();
                let after = text[to..].to_string();

                h_flex()
                    .h(px(lh))
                    .line_height(px(lh))
                    .text_size(px(fs))
                    .whitespace_nowrap()
                    .gap_0()
                    .text_color(default_fg)
                    .child(before)
                    .child(div().bg(sel_bg).text_color(default_fg).child(selected))
                    .child(after)
                    .into_any_element()
            })
            .collect();

        let entity = cx.entity();
        let fh = focus_handle.clone();
        let bounds_for_paint: Arc<StdMutex<Bounds<Pixels>>> = self.bounds.clone();

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
            .on_key_down({
                let lines_for_copy = lines.clone();
                cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let is_copy = (event.keystroke.modifiers.platform
                    && event.keystroke.key == "c")
                    || (event.keystroke.modifiers.control && event.keystroke.key == "insert");
                if is_copy {
                    if let Some(text) = this.selected_text(&lines_for_copy) {
                        if !text.is_empty() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                            cx.stop_propagation();
                            return;
                        }
                    }
                }
                if event.keystroke.modifiers.platform && event.keystroke.key == "v" {
                    if let Some(item) = cx.read_from_clipboard() {
                        let text = item.text().unwrap_or_default();
                        if !text.is_empty() {
                            this.send_input(text.as_bytes());
                            cx.notify();
                            cx.stop_propagation();
                            return;
                        }
                    }
                }
                let bytes = keystroke_to_bytes(&event.keystroke);
                if !bytes.is_empty() {
                    this.send_input(&bytes);
                    cx.notify();
                    cx.stop_propagation();
                    window.refresh();
                }
            })})
            // Absolutely-positioned canvas that covers the entire area so its
            // paint hook fires reliably. Only job: record bounds (for mouse coord
            // conversion) and register the keyboard input handler.
            .child(
                canvas(
                    |_bounds, _window, _cx| {},
                    move |bounds, _state, window, cx: &mut App| {
                        if let Ok(mut b) = bounds_for_paint.lock() {
                            *b = bounds.clone();
                        }
                        window.handle_input(&fh, ElementInputHandler::new(bounds, entity.clone()), cx);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
            )
            // Mouse handlers live on the outer div (not canvas) so GPUI dispatches
            // them reliably. ev.position is window-absolute; we subtract bounds.origin.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    window.focus(&cx.focus_handle(), cx);
                    let px: f32 = ev.position.x.into();
                    let py: f32 = ev.position.y.into();
                    let bounds = match this.bounds.lock() { Ok(g) => *g, Err(_) => return };
                    let ox: f32 = bounds.origin.x.into();
                    let oy: f32 = bounds.origin.y.into();
                    let (cw, ch) = char_cell_metrics(this.font_size, this.last_line_height);
                    let lx = (px - ox - 8.0).max(0.0);
                    let ly = (py - oy - 4.0).max(0.0);
                    let col = (lx / cw).floor() as usize;
                    let row = (ly / ch).floor() as usize;
                    let pt = SelectionPoint {
                        row: row.min(this.last_visible_rows.saturating_sub(1)),
                        col,
                    };
                    this.selection_anchor = Some(pt);
                    this.selection_head = Some(pt);
                    this.selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                if !this.selecting { return; }
                if ev.pressed_button != Some(MouseButton::Left) { return; }
                let px: f32 = ev.position.x.into();
                let py: f32 = ev.position.y.into();
                let bounds = match this.bounds.lock() { Ok(g) => *g, Err(_) => return };
                let ox: f32 = bounds.origin.x.into();
                let oy: f32 = bounds.origin.y.into();
                let bw: f32 = bounds.size.width.into();
                let bh: f32 = bounds.size.height.into();
                let pxc = px.clamp(ox + 8.0, ox + bw - 1.0);
                let pyc = py.clamp(oy + 4.0, oy + bh - 1.0);
                let (cw, ch) = char_cell_metrics(this.font_size, this.last_line_height);
                let lx = (pxc - ox - 8.0).max(0.0);
                let ly = (pyc - oy - 4.0).max(0.0);
                let col = (lx / cw).floor() as usize;
                let row = (ly / ch).floor() as usize;
                this.selection_head = Some(SelectionPoint {
                    row: row.min(this.last_visible_rows.saturating_sub(1)),
                    col,
                });
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                    this.selecting = false;
                    if let (Some(a), Some(h)) = (this.selection_anchor, this.selection_head) {
                        if a == h {
                            this.selection_anchor = None;
                            this.selection_head = None;
                        }
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }
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
