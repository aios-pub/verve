//! Terminal rendering bridge: `alacritty_terminal` ↔ GPUI.

use std::sync::{Arc, Mutex as StdMutex};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term};
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

/// Convert a terminal grid **column** index into a UTF-8 **byte** offset within
/// a rendered row string.
///
/// Terminal rows are built one `char` per grid column (`text.push(cell.c)` per
/// column), so a column index is a *char* index — NOT a byte index. CJK glyphs
/// are 3 bytes and `·`/accented chars are 2 bytes, so slicing `row[col..]`
/// directly lands inside a multi-byte char and panics
/// ("byte index N is not a char boundary"). This maps the char index to a
/// safe byte boundary, clamped to the string length.
fn col_to_byte(s: &str, col: usize) -> usize {
    s.char_indices()
        .nth(col)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(s.len())
}

/// Slice a rendered row by terminal column (char) indices instead of byte
/// indices, snapping both ends to valid char boundaries. The two indices need
/// not be ordered; this returns the text between min and max.
fn slice_by_cols(s: &str, start_col: usize, end_col: usize) -> &str {
    let (a, b) = if start_col <= end_col {
        (start_col, end_col)
    } else {
        (end_col, start_col)
    };
    let start_byte = col_to_byte(s, a);
    let end_byte = col_to_byte(s, b).max(start_byte);
    &s[start_byte..end_byte]
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
            font_size: 14.0,
            input_tx,
            resize_tx,
            _keystroke_sub: Some(sub),
            last_marked_text: String::new(),
            selection_anchor: None,
            selection_head: None,
            selecting: false,
            last_trim_top: 0,
            last_visible_rows: 0,
            last_line_height: 18.0,
            last_char_width: 14.0 * 0.58,
            bounds,
            mouse_handlers_registered: false,
            last_feed_tail: std::sync::Mutex::new(Vec::new()),
            bounds_retry: 0,
            suppress_auto_focus: false,
        }
    }

    /// Return the currently selected text (normalized from anchor/head), or
    /// None if no selection exists.
    ///
    /// Column→byte mapping uses each row's `prefix` map (NOT `col_to_byte`) so
    /// wide-char spacer columns — which contribute 0 bytes — are handled
    /// correctly; a raw char index would drift once a CJK/emoji glyph precedes
    /// the selection endpoint.
    fn selected_text(&self, rows: &[RowData]) -> Option<String> {
        let (a, h) = self.selection_anchor.zip(self.selection_head)?;
        selection_to_string(rows, a, h)
    }

    /// Read the raw characters of one **visible** row (0 = top of viewport)
    /// directly from the alacritty grid, as a `Vec<char>` indexed by grid
    /// column. HIDDEN cells and `\0` read as spaces (matching how rows are
    /// rendered). Wide-char spacers are emitted as a space placeholder so the
    /// vector stays aligned 1:1 with grid columns (i.e. `chars[col]` is the
    /// char at grid column `col`). Returns `None` if the grid can't be locked
    /// or the row is off-screen.
    ///
    /// Used by double-click word selection to find word boundaries without
    /// needing the rendered `RowData` (which only exists transiently during
    /// `render`).
    fn chars_of_visible_row(&self, visible_row: usize) -> Option<Vec<char>> {
        let term = self.term.lock().ok()?;
        let dims: &dyn alacritty_terminal::grid::Dimensions = &*term;
        let screen_lines = dims.screen_lines();
        if visible_row >= screen_lines {
            return None;
        }
        let grid = term.grid();
        let num_cols = dims.columns();
        let display_offset = grid.display_offset();
        let bottommost = grid.bottommost_line().0;
        // Match the viewport mapping used in render() (see terminal.rs ~1283).
        let grid_line = alacritty_terminal::index::Line(
            bottommost - screen_lines as i32 + 1 - display_offset as i32 + visible_row as i32,
        );

        let mut chars: Vec<char> = Vec::with_capacity(num_cols);
        for col_idx in 0..num_cols {
            let point = alacritty_terminal::index::Point::new(
                grid_line,
                alacritty_terminal::index::Column(col_idx),
            );
            let cell: &Cell = &grid[point];
            // Keep 1:1 column alignment: wide-char spacers contribute a space
            // placeholder so indices still map to grid columns.
            let ch = if cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                || cell.flags.contains(Flags::HIDDEN)
                || cell.c == '\0'
            {
                ' '
            } else {
                cell.c
            };
            chars.push(ch);
        }
        Some(chars)
    }

    /// Select the word around `(row, col)` and return `(start_col, end_col)` —
    /// the half-open `[start, end)` column range of the run of non-whitespace
    /// chars containing `col`. If the clicked cell is whitespace, selects only
    /// that single column (so a double-click on space clears any selection
    /// rather than grabbing a whole run of blanks).
    fn word_bounds(chars: &[char], col: usize) -> (usize, usize) {
        let n = chars.len();
        if n == 0 {
            return (0, 0);
        }
        let c = col.min(n - 1);
        // A "word" is a run of identifier-like chars: letters, digits, and
        // underscore (Unicode-aware, so CJK/alphanumeric glyphs count too).
        // Everything else — whitespace AND punctuation (/, =, :, ., -, etc.) —
        // breaks words. This matches standard editor/terminal behavior, so a
        // double-click on `/usr/bin` selects just `usr` or `bin`, and on
        // `foo=bar` selects just `foo` or `bar`.
        let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';
        if !is_word_char(chars[c]) {
            // Clicked a separator/punctuation cell: select only that one cell
            // (e.g. double-clicking `/` selects `/`, not the adjacent path
            // segment).
            return (c, c + 1);
        }
        // Expand left.
        let mut start = c;
        while start > 0 && is_word_char(chars[start - 1]) {
            start -= 1;
        }
        // Expand right (half-open end).
        let mut end = c + 1;
        while end < n && is_word_char(chars[end]) {
            end += 1;
        }
        (start, end)
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

/// Navigation key (arrows/Home/End): the plain sequence, or the xterm
/// modified form `\x1b[1;<mods><F>` when shift/alt/ctrl are held (this is
/// what shells/readline expect for e.g. Ctrl+Left = backward-word).
fn nav_seq(ks: &gpui::Keystroke, plain: &[u8], final_ch: char) -> Vec<u8> {
    let m = xterm_modifier_param(ks);
    if m > 1 {
        format!("\x1b[1;{m}{final_ch}").into_bytes()
    } else {
        plain.to_vec()
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
fn keystroke_to_bytes(ks: &gpui::Keystroke) -> Vec<u8> {
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
        "up" => return nav_seq(ks, b"\x1b[A", 'A'),
        "down" => return nav_seq(ks, b"\x1b[B", 'B'),
        "right" => return nav_seq(ks, b"\x1b[C", 'C'),
        "left" => return nav_seq(ks, b"\x1b[D", 'D'),
        "home" => return nav_seq(ks, b"\x1b[H", 'H'),
        "end" => return nav_seq(ks, b"\x1b[F", 'F'),
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
    /// True when the grid line's last cell carries `WRAPLINE` — the terminal
    /// auto-wrapped this row and the logical line continues on the next visual
    /// row. Copy joins such rows without an intervening newline.
    wrapped: bool,
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

    // A wrapped row's WRAPLINE flag sits on the last column's cell (set by
    // `wrap_input` on the cursor cell right before it moves to the next line),
    // so checking the final column is enough — no full-row scan needed.
    let wrapped = num_cols > 0
        && grid[alacritty_terminal::index::Point::new(
            grid_line,
            alacritty_terminal::index::Column(num_cols - 1),
        )]
        .flags
        .contains(Flags::WRAPLINE);

    RowData {
        text,
        runs,
        prefix,
        wrapped,
    }
}

/// Assemble the copy text for a selection spanning the rendered `rows`, from
/// `a`/`h` (order-agnostic; normalized to top-left → bottom-right).
///
/// Column→byte mapping uses each row's `prefix` map (NOT `col_to_byte`) so
/// wide-char spacer columns — which contribute 0 bytes — are handled
/// correctly; a raw char index would drift once a CJK/emoji glyph precedes
/// the selection endpoint.
///
/// Copy semantics mirror the terminal's own selection copy
/// (`alacritty_terminal::Term::bounds_to_string`): every grid row is padded
/// with spaces to the full column width for rendering, and that padding is
/// trimmed instead of being copied out as trailing spaces; rows the terminal
/// visually wrapped (`wrapped`) are joined into one logical line with no
/// newline, while real line breaks stay newlines.
fn selection_to_string(rows: &[RowData], a: SelectionPoint, h: SelectionPoint) -> Option<String> {
    let (start, end) = if (a.row, a.col) <= (h.row, h.col) {
        (a, h)
    } else {
        (h, a)
    };
    if start.row >= rows.len() {
        return None;
    }
    let row_bytes = |r: &RowData, col: usize| -> usize {
        r.prefix.get(col).copied().unwrap_or(r.text.len())
    };
    if start.row == end.row {
        let r = rows.get(start.row)?;
        let b0 = row_bytes(r, start.col);
        let b1 = row_bytes(r, end.col).max(b0);
        return r.text.get(b0..b1).map(str::to_string);
    }
    let mut out = String::new();
    // The start row tail: from `start.col` to the row's end, minus the
    // right-edge grid padding.
    let r0 = rows.get(start.row)?;
    out.push_str(r0.text.get(row_bytes(r0, start.col)..)?.trim_end());
    // Rows past a viewport resize may no longer exist; copy what's there.
    let last_row = end.row.min(rows.len().saturating_sub(1));
    let mut prev = r0;
    for row_idx in (start.row + 1)..=last_row {
        let r = rows.get(row_idx)?;
        // A row whose predecessor was visually wrapped is a continuation
        // of the same logical line — join it without a newline.
        if !prev.wrapped {
            out.push('\n');
        }
        if row_idx == end.row {
            out.push_str(r.text.get(..row_bytes(r, end.col))?);
        } else {
            out.push_str(r.text.trim_end());
        }
        prev = r;
    }
    Some(out)
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

        let (lines, cursor_cell): (Vec<RowData>, Option<(usize, usize)>) = {
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

            // Do NOT trim trailing whitespace: trailing spaces can carry a
            // colored background (e.g. a TUI status bar), and trimming would
            // cut that background short.
            (
                (0..screen_lines)
                    .map(|row_idx| {
                        let grid_line = viewport_top + row_idx as i32;
                        build_row(
                            grid,
                            alacritty_terminal::index::Line(grid_line),
                            num_cols,
                            &fonts,
                        )
                    })
                    .collect::<Vec<_>>(),
                cursor_cell,
            )
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
                if (a.row, a.col) <= (h.row, h.col) {
                    (a, h)
                } else {
                    (h, a)
                }
            });

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
                // intersects this row and, if so, the byte range
                // [from_byte, to_byte) to highlight. Column indices map to
                // bytes via `row.prefix` (which correctly skips wide-char
                // spacer columns and always lands on char boundaries).
                let final_runs = match sel {
                    Some((start, end)) if row_idx >= start.row && row_idx <= end.row => {
                        let from_col = if row_idx == start.row { start.col } else { 0 };
                        let to_col = if row_idx == end.row {
                            end.col
                        } else {
                            // Full row: last prefix entry == text byte length.
                            row.prefix.len().saturating_sub(1)
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
                    let bytes = keystroke_to_bytes(&event.keystroke);
                    if !bytes.is_empty() {
                        // If the user is scrolled back into history, any input
                        // key first jumps back to the newest line before sending
                        // (standard terminal behaviour — avoids typing into the
                        // old scrollback view).
                        let need_reset = if let Ok(term) = this.term.lock() {
                            term.grid().display_offset() != 0
                        } else {
                            false
                        };
                        if need_reset {
                            if let Ok(mut term) = this.term.lock() {
                                term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                            }
                        }
                        this.send_input(&bytes);
                        cx.notify();
                        cx.stop_propagation();
                        window.refresh();
                    }
                })
            })
            // Mouse wheel → scroll the local scrollback history. We translate
            // GPUI's scroll delta into alacritty line deltas and call
            // `scroll_display`, which only moves the viewport offset (history is
            // already retained by the grid). Direction: wheel up (deltaY > 0)
            // scrolls up into history; wheel down returns toward the newest line.
            .on_scroll_wheel(
                cx.listener(|this, ev: &gpui::ScrollWheelEvent, _window, cx| {
                    let lines = match &ev.delta {
                        gpui::ScrollDelta::Pixels(p) => {
                            (p.y.as_f32() / this.last_line_height.max(1.0)).round() as i32
                        }
                        gpui::ScrollDelta::Lines(l) => l.y.round() as i32,
                    };
                    if lines != 0 {
                        if let Ok(mut term) = this.term.lock() {
                            term.scroll_display(alacritty_terminal::grid::Scroll::Delta(lines));
                        }
                        cx.notify();
                    }
                }),
            )
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
            // Mouse handlers live on the outer div (not canvas) so GPUI dispatches
            // them reliably. ev.position is window-absolute; we subtract bounds.origin.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    window.focus(&cx.focus_handle(), cx);
                    let px: f32 = ev.position.x.into();
                    let py: f32 = ev.position.y.into();
                    let bounds = match this.bounds.lock() {
                        Ok(g) => *g,
                        Err(_) => return,
                    };
                    let ox: f32 = bounds.origin.x.into();
                    let oy: f32 = bounds.origin.y.into();
                    let cw = this.last_char_width.max(0.5);
                    let ch = this.last_line_height.max(1.0);
                    let lx = (px - ox - 8.0).max(0.0);
                    let ly = (py - oy - 4.0).max(0.0);
                    let col = (lx / cw).floor() as usize;
                    let row = (ly / ch).floor() as usize;
                    let row = row.min(this.last_visible_rows.saturating_sub(1));

                    // Double-click → select the word under the cursor.
                    // Triple-click → select the whole line. Both set anchor/head
                    // and leave `selecting` false so a subsequent drag starts a
                    // fresh selection (standard terminal behavior).
                    if ev.click_count >= 3 {
                        // Select the entire row up to the last non-blank cell.
                        let end_col = this
                            .chars_of_visible_row(row)
                            .map(|chars| {
                                // Last non-whitespace column + 1 (half-open);
                                // 0 if the whole row is blank.
                                chars
                                    .iter()
                                    .rposition(|c| !c.is_whitespace())
                                    .map(|i| i + 1)
                                    .unwrap_or(0)
                            })
                            .unwrap_or(0);
                        this.selection_anchor = Some(SelectionPoint { row, col: 0 });
                        this.selection_head = Some(SelectionPoint { row, col: end_col });
                        this.selecting = false;
                        cx.notify();
                        return;
                    }
                    if ev.click_count == 2 {
                        if let Some(chars) = this.chars_of_visible_row(row) {
                            let (start, end) = Self::word_bounds(&chars, col);
                            this.selection_anchor = Some(SelectionPoint { row, col: start });
                            this.selection_head = Some(SelectionPoint { row, col: end });
                            this.selecting = false;
                            cx.notify();
                            return;
                        }
                    }

                    let pt = SelectionPoint { row, col };
                    this.selection_anchor = Some(pt);
                    this.selection_head = Some(pt);
                    this.selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                if !this.selecting {
                    return;
                }
                if ev.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let px: f32 = ev.position.x.into();
                let py: f32 = ev.position.y.into();
                let bounds = match this.bounds.lock() {
                    Ok(g) => *g,
                    Err(_) => return,
                };
                let ox: f32 = bounds.origin.x.into();
                let oy: f32 = bounds.origin.y.into();
                let bw: f32 = bounds.size.width.into();
                let bh: f32 = bounds.size.height.into();
                let pxc = px.clamp(ox + 8.0, ox + bw - 1.0);
                let pyc = py.clamp(oy + 4.0, oy + bh - 1.0);
                let cw = this.last_char_width.max(0.5);
                let ch = this.last_line_height.max(1.0);
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
        apply_log_highlights, apply_selection, col_to_byte, color_to_hsla, contains_sequence,
        selection_to_string, slice_by_cols, RowData, SelectionPoint,
    };
    use alacritty_terminal::vte::ansi::{Color, NamedColor};
    use gpui::{Font, FontFeatures, FontStyle, FontWeight, Hsla, TextRun};

    /// Reproduces the exact panic from the crash report: a row containing the
    /// 2-byte `·` char where a selection column landed mid-character
    /// ("end byte index 70 is not a char boundary; it is inside '·'").
    /// Before the fix, slicing `text[69..70]` panicked because column 70 is a
    /// char index that is not a valid byte boundary.
    #[test]
    fn col_to_byte_handles_multibyte_chars() {
        // `·` is U+00B7 (2 bytes), CJK chars are 3 bytes.
        let row = "ab·cd阅"; // chars: a(0) b(1) ·(2) c(3) d(4) 阅(5)
        // bytes: a=0 b=1 ·=2..4 c=4 d=5 阅=6..9
        assert_eq!(col_to_byte(row, 0), 0); // 'a'
        assert_eq!(col_to_byte(row, 2), 2); // '·' start byte
        assert_eq!(col_to_byte(row, 3), 4); // 'c' (skips the 2nd byte of '·')
        assert_eq!(col_to_byte(row, 5), 6); // '阅' start byte
        // Out of range clamps to end (byte length 9), never panics.
        assert_eq!(col_to_byte(row, 99), 9);
    }

    #[test]
    fn slice_by_cols_does_not_panic_on_multibyte() {
        // The panic case: a column index that falls inside a multi-byte char.
        let row = " prd.pdf   Ⅳ. 阅 读 理 解 （ 2025·福建泉州期中）.pdf";
        // Slicing at arbitrary column boundaries must never panic.
        let _ = slice_by_cols(row, 0, row.chars().count());
        let _ = slice_by_cols(row, 30, 50);
        let _ = slice_by_cols(row, 40, 40); // empty selection
        assert!(slice_by_cols(row, 0, 1).chars().count() <= 1);
    }

    #[test]
    fn slice_by_cols_orders_endpoints() {
        let row = "阅读测试"; // 4 chars, 12 bytes
        assert_eq!(slice_by_cols(row, 1, 3), "读测");
        // Reversed endpoints produce the same span.
        assert_eq!(slice_by_cols(row, 3, 1), "读测");
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

    // word_bounds is a pure associated fn on TerminalView (no self, no GPUI
    // types) so we can call it via the type path without pulling in macros.
    #[test]
    fn word_bounds_selects_inner_token() {
        let chars: Vec<char> = "git status".chars().collect();
        // Click the 'i' of "git" (col 1) → selects "git" (cols 0..3).
        assert_eq!(super::TerminalView::word_bounds(&chars, 1), (0, 3));
        // Click the 't' of "status" (col 5) → selects "status" (cols 4..10).
        assert_eq!(super::TerminalView::word_bounds(&chars, 5), (4, 10));
    }

    #[test]
    fn word_bounds_at_boundaries() {
        let chars: Vec<char> = "git status".chars().collect();
        // First char of a word.
        assert_eq!(super::TerminalView::word_bounds(&chars, 0), (0, 3));
        // Last char of a word.
        assert_eq!(super::TerminalView::word_bounds(&chars, 9), (4, 10));
    }

    #[test]
    fn word_bounds_on_whitespace_selects_single_cell() {
        // Double-clicking a space should NOT grab the whole blank run.
        let chars: Vec<char> = "a    b".chars().collect();
        // col 2 is a space.
        assert_eq!(super::TerminalView::word_bounds(&chars, 2), (2, 3));
    }

    #[test]
    fn word_bounds_with_punctuation() {
        // Punctuation breaks words: a path like /usr/bin selects only one
        // path segment per double-click (e.g. "usr" or "bin"), not the whole.
        let chars: Vec<char> = "/usr/bin".chars().collect();
        // Click "u" (col 1) → selects "usr" (cols 1..4).
        assert_eq!(super::TerminalView::word_bounds(&chars, 1), (1, 4));
        // Click "b" (col 5) → selects "bin" (cols 5..8).
        assert_eq!(super::TerminalView::word_bounds(&chars, 5), (5, 8));
        // Click "/" (col 0, a separator) → selects only that cell.
        assert_eq!(super::TerminalView::word_bounds(&chars, 0), (0, 1));
    }

    #[test]
    fn word_bounds_key_value_and_flags() {
        // `KEY=value` — double-click each side selects just that side.
        let chars: Vec<char> = "PATH=/usr/bin".chars().collect();
        assert_eq!(super::TerminalView::word_bounds(&chars, 1), (0, 4)); // PATH
        assert_eq!(super::TerminalView::word_bounds(&chars, 8), (6, 9)); // usr
        // `--rm` style flag: double-click "rm" selects "rm", not "--rm".
        let chars: Vec<char> = "--rm".chars().collect();
        assert_eq!(super::TerminalView::word_bounds(&chars, 3), (2, 4)); // rm
    }

    #[test]
    fn word_bounds_underscore_joins() {
        // Underscore is a word char, so error_code selects as one token.
        let chars: Vec<char> = "error_code foo".chars().collect();
        assert_eq!(super::TerminalView::word_bounds(&chars, 3), (0, 10));
    }

    #[test]
    fn word_bounds_empty_and_oob() {
        assert_eq!(super::TerminalView::word_bounds(&[], 5), (0, 0));
        // Col past the end is clamped to the last cell.
        let chars: Vec<char> = "hi".chars().collect();
        assert_eq!(super::TerminalView::word_bounds(&chars, 99), (0, 2));
    }

    /// A test RowData with a per-char `prefix` map (one char per column).
    fn copy_row(text: &str, wrapped: bool) -> RowData {
        let mut prefix = vec![0usize];
        let mut byte = 0usize;
        for ch in text.chars() {
            byte += ch.len_utf8();
            prefix.push(byte);
        }
        RowData {
            text: text.to_string(),
            runs: Vec::new(),
            prefix,
            wrapped,
        }
    }

    fn sel_point(row: usize, col: usize) -> SelectionPoint {
        SelectionPoint { row, col }
    }

    /// Regression: a long command the terminal visually wrapped across rows
    /// must copy back as ONE logical line (no inserted newlines) with the
    /// row-end padding spaces stripped — not "part1   \npart2".
    #[test]
    fn copy_joins_wrapped_rows_without_newline_or_padding() {
        // "echo hello w" wrapped onto row 1 ("orld"), which ends the line;
        // trailing spaces are right-edge grid padding.
        let rows = [
            copy_row("echo hello w     ", true),
            copy_row("orld continuation ", false),
            copy_row("next line     ", false),
        ];
        let out = selection_to_string(&rows, sel_point(0, 0), sel_point(2, 9)).unwrap();
        assert_eq!(out, "echo hello world continuation\nnext line");
    }

    /// Regression: the START row's tail used to keep its full-width padding,
    /// so a copied multi-line selection gained stray trailing spaces.
    #[test]
    fn copy_trims_padding_on_start_row_tail() {
        let rows = [copy_row("first   ", false), copy_row("second", false)];
        let out = selection_to_string(&rows, sel_point(0, 0), sel_point(1, 6)).unwrap();
        assert_eq!(out, "first\nsecond");
    }

    /// A single-row selection is exactly the columns the user picked — inner
    /// spaces are kept, nothing is trimmed.
    #[test]
    fn copy_single_row_keeps_user_range_verbatim() {
        let rows = [copy_row("abc def", false)];
        let out = selection_to_string(&rows, sel_point(0, 2), sel_point(0, 5)).unwrap();
        assert_eq!(out, "c d");
    }

    /// Selection endpoints past the end of the row list (viewport shrank
    /// after a resize) copy what exists instead of panicking.
    #[test]
    fn copy_with_end_row_beyond_viewport_is_clamped() {
        let rows = [copy_row("first", false), copy_row("second", false)];
        let out = selection_to_string(&rows, sel_point(0, 0), sel_point(5, 3)).unwrap();
        assert_eq!(out, "first\nsecond");
        // Selection starting entirely below the viewport → nothing to copy.
        assert_eq!(selection_to_string(&rows, sel_point(9, 0), sel_point(9, 2)), None);
    }
}
