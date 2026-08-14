//! Notepad panel — markdown notes with a WYSIWYG block editor, search, and nested sub-documents.

use gpui::actions;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::list::ListItem;
use gpui_component::popover::Popover;
use gpui_component::tree::{TreeItem, TreeState, tree};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex,
};

use crate::notes::{
    self, Note, NotesStore, children_of, count_descendants, create_child_note, delete_note_cascade,
    has_children,
};
use crate::state::{AppEvent, AppState};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::Arc;
use std::time::Instant;

/// Ignore watcher events arriving within this window after our own write,
/// since `notify` surfaces our own saves too.
const NOTES_SELF_SAVE_FILTER_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

/// Resolved media referenced by a note that is about to be deleted, offered for
/// optional cleanup in a second confirmation dialog.
struct PendingMediaDelete {
    note_id: String,
    local_paths: Vec<std::path::PathBuf>,
    s3_keys: Vec<String>,
}

impl PendingMediaDelete {
    fn total(&self) -> usize {
        self.local_paths.len() + self.s3_keys.len()
    }
}

pub struct NotesPanel {
    state: Entity<AppState>,
    store: NotesStore,
    editor: Entity<crate::wysiwyg::editor::Editor>,
    /// Keep-alive arena for previously-active editors. NotesPanel swaps in a
    /// fresh editor per note switch (see `reload_editor`); holding the outgoing
    /// editor here prevents it (and its focus handles) from being dropped
    /// mid-frame, which would corrupt the new editor's repainting.
    prev_editors: Vec<Entity<crate::wysiwyg::editor::Editor>>,
    /// Resizable state for the left note-list sidebar width.
    sidebar_resize: Entity<gpui_component::resizable::ResizableState>,
    search: Entity<InputState>,
    title_input: Entity<InputState>,
    editing_title_id: Option<String>,
    pending_delete: Option<String>,
    /// Second-stage delete confirmation: the user confirmed note deletion and
    /// the note references media; this holds the resolved media to optionally
    /// delete alongside the note.
    pending_media_delete: Option<PendingMediaDelete>,
    save_timer: Option<Task<()>>,
    /// Last seen editor `scroll_revision`. The editor bumps it on scroll-only
    /// notifies (scrollbar show/hide); we use it to avoid rebuilding the whole
    /// panel — including the heavy note sidebar — on every scroll tick.
    last_scroll_revision: u64,
    _subs: Vec<Subscription>,
    // Nested-notes tree state.
    tree: Entity<TreeState>,
    /// When true, reload editor content from active note on next render (which
    /// has &mut Window access — used by tree selection observer).
    editor_needs_reload: bool,
    /// Whether the per-note export popover (in the editor header) is open.
    export_popover_open: bool,
    /// Whether the left-side note tree is collapsed (hidden).
    list_collapsed: bool,
    /// Saved sidebar width loaded once at construction. Render uses it as the
    /// resizable's default size; the live size lives in `sidebar_resize`, so
    /// re-reading `layout.json` every frame (two disk reads) was wasted work.
    sidebar_default_width: gpui::Pixels,
    /// Per-row open "..." menu, by node id (e.g. "note:<uuid>").
    row_menu_open: Option<String>,
    /// FS watcher for the notes directory — picks up external edits so the
    /// sidebar reflects files changed/added/removed outside the app.
    watcher: Option<RecommendedWatcher>,
    watcher_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    /// Time of the last save initiated by this panel; used to ignore the
    /// watcher events triggered by our own writes.
    last_self_save: Option<Instant>,
    /// Full-text search index (tantivy). `None` when the on-disk index could
    /// not be opened/built — search then falls back to the in-memory matcher.
    notes_index: Option<Arc<crate::notes_index::NotesIndex>>,
    /// Lifecycle of the index, surfaced in the UI so the user can tell while a
    /// full rebuild is in progress (search falls back to memory meanwhile).
    index_status: crate::notes_index::IndexStatus,
    /// Sender to the single background index worker. `None` until the index is
    /// opened (or if it failed to open) — mutations are then simply dropped,
    /// which is fine because search falls back to the in-memory matcher.
    index_tx: Option<std::sync::mpsc::Sender<IndexCmd>>,
}

fn note_node(id: &str) -> String {
    format!("note:{id}")
}
fn parse_note_id(node: &str) -> Option<&str> {
    node.strip_prefix("note:")
}

/// A mutation to apply to the notes full-text index. All mutations are sent
/// through a single channel to one background worker (see
/// [`NotesPanel::spawn_index_worker`]) which applies them **in send order**.
/// This is what makes background indexing safe: a newer upsert can never be
/// overwritten by an older one, and a rebuild never interleaves with a
/// concurrent upsert (which would duplicate documents).
enum IndexCmd {
    Upsert(Note),
    Delete(String),
    /// Wipe and rebuild from the on-disk markdown in this dir. The worker
    /// refreshes `index_status` when it finishes (Ready/Failed).
    Rebuild(std::path::PathBuf),
}

// ---------------------------------------------------------------------------
// Full-text note search
// ---------------------------------------------------------------------------

/// A single full-text search hit, ready to render.
struct NoteSearchResult {
    note_id: String,
    /// Display title (never empty — falls back to the localized "untitled").
    title: String,
    /// Byte range of the match within `title` (char-boundary safe), if any.
    title_match: Option<(usize, usize)>,
    /// Content excerpt around the first content match.
    snippet: Option<String>,
    /// Byte range of the match within `snippet` (when `snippet` is present).
    snippet_match: Option<(usize, usize)>,
}

/// Case-insensitive substring search. Returns the first match as a byte range
/// **into `haystack`** (the original-case string), with both ends guaranteed to
/// be UTF-8 char boundaries.
///
/// Matching folds each original character to its first lowercase form (enough
/// for ASCII + CJK, which is what notes contain). Multi-codepoint case foldings
/// (e.g. 'İ') are approximated by their first lowercase char; this only affects
/// exotic inputs and never panics. `None` for an empty needle.
fn find_ci(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    let n: Vec<char> = needle
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    if n.is_empty() {
        return None;
    }
    // Lowercased view of the haystack, parallel to `indices` (one entry per
    // original char). `indices[i]` is the byte offset in `haystack` where the
    // i-th original char starts — always a char boundary.
    let h_lower: Vec<char> = haystack
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let indices: Vec<usize> = haystack.char_indices().map(|(b, _)| b).collect();
    let last = h_lower.len().saturating_sub(n.len());
    for i in 0..=last {
        let mut ok = true;
        for j in 0..n.len() {
            if h_lower.get(i + j) != n.get(j) {
                ok = false;
                break;
            }
        }
        if ok {
            // Start of the match = char boundary at original index `i`.
            let start = *indices.get(i)?;
            // End = char boundary just past the last matched char; fall back to
            // `haystack.len()` (also a char boundary) when off the end.
            let end = *indices.get(i + n.len()).unwrap_or(&haystack.len());
            return Some((start, end));
        }
    }
    None
}

/// Build a short excerpt of `content` centered on `[match_start, match_end)`.
/// `match_start` / `match_end` must be char-boundary byte offsets into `content`
/// (as returned by [`find_ci`]). Returns `(snippet, hl_start, hl_end)` where the
/// highlight range is relative to the returned snippet and is char-boundary safe.
fn build_content_snippet(
    content: &str,
    match_start: usize,
    match_end: usize,
) -> (String, usize, usize) {
    const CHARS_BEFORE: usize = 16;
    const CHARS_AFTER: usize = 40;

    // Snippet start: at most CHARS_BEFORE original chars before `match_start`.
    let mut starts: Vec<usize> = content
        .char_indices()
        .map(|(b, _)| b)
        .filter(|b| *b <= match_start)
        .collect();
    let snip_start = if starts.len() > CHARS_BEFORE {
        *starts.get(starts.len() - 1 - CHARS_BEFORE).unwrap_or(&0)
    } else {
        0
    };
    // Drop the temp vec so `content` borrows below are cleaner (optional).
    starts.clear();

    // Snippet end: CHARS_AFTER original chars after `match_end`.
    let mut end_byte = content.len();
    let mut count = 0usize;
    for (b, _) in content.char_indices() {
        if b < match_end {
            continue;
        }
        count += 1;
        if count > CHARS_AFTER {
            end_byte = b;
            break;
        }
    }

    let leading = snip_start > 0;
    let trailing = end_byte < content.len();
    let before = content.get(snip_start..match_start).unwrap_or("");
    let matched = content.get(match_start..match_end).unwrap_or("");
    let after = content.get(match_end..end_byte).unwrap_or("");

    let mut text = String::new();
    if leading {
        text.push('…');
    }
    text.push_str(before);
    let hl_start = text.len();
    text.push_str(matched);
    let hl_end = text.len();
    text.push_str(after);
    if trailing {
        text.push('…');
    }
    (text, hl_start, hl_end)
}

/// Build a highlight-ready [`NoteSearchResult`] for a single note — locates
/// the query in the title and/or content and builds a content snippet. Used by
/// both the in-memory matcher and the tantivy index path (which already ranked
/// the note but still needs the highlight spans).
fn result_for_note(note: &crate::notes::Note, q: &str, untitled: &str) -> NoteSearchResult {
    let title_match = find_ci(&note.title, q);
    let snippet = find_ci(&note.content, q).map(|(s, e)| build_content_snippet(&note.content, s, e));
    NoteSearchResult {
        note_id: note.id.clone(),
        title: if note.title.is_empty() {
            untitled.to_string()
        } else {
            note.title.clone()
        },
        title_match,
        snippet: snippet.as_ref().map(|(t, _, _)| t.clone()),
        snippet_match: snippet.map(|(_, hs, he)| (hs, he)),
    }
}

/// In-memory full-text search across notes, returning at most `limit` hits.
///
/// Title matches are listed first (in store order), then content-only matches,
/// so the most relevant results surface at the top. This is the fallback path
/// used while the tantivy index is still building or unavailable; the index
/// path (see [`NotesPanel::collect_search_results`]) takes over once Ready.
fn search_notes(notes: &[crate::notes::Note], query: &str, limit: usize) -> Vec<NoteSearchResult> {
    let q = query.trim();
    if q.is_empty() || limit == 0 {
        return Vec::new();
    }
    let untitled = rust_i18n::t!("notes.untitled").to_string();

    // Partition into title-matches vs content-only matches.
    let mut with_title: Vec<&crate::notes::Note> = Vec::new();
    let mut content_only: Vec<&crate::notes::Note> = Vec::new();
    for note in notes.iter() {
        let tm = find_ci(&note.title, q).is_some();
        let cm = find_ci(&note.content, q).is_some();
        if !tm && !cm {
            continue;
        }
        if tm {
            with_title.push(note);
        } else {
            content_only.push(note);
        }
    }

    let mut out: Vec<NoteSearchResult> = Vec::new();
    for note in with_title.iter().chain(content_only.iter()) {
        if out.len() >= limit {
            break;
        }
        out.push(result_for_note(note, q, &untitled));
    }
    out
}

/// Render a single text line, highlighting the byte range `range` (if valid)
/// with `hl_bg`/`hl_fg`. The rest of the text uses `base` color. All slicing
/// goes through `.get(..)` / `is_char_boundary`, so a malformed range simply
/// falls back to a plain (un-highlighted) line — never panics.
fn render_highlighted(
    text: &str,
    range: Option<(usize, usize)>,
    base: Hsla,
    hl_fg: Hsla,
    hl_bg: Hsla,
) -> Div {
    let mut row = h_flex().items_center().min_w_0();
    let valid = range.filter(|(s, e)| {
        s <= e && *s <= text.len() && *e <= text.len() && text.is_char_boundary(*s) && text.is_char_boundary(*e)
    });
    match valid {
        Some((s, e)) => {
            let before = text.get(..s).unwrap_or("");
            let mid = text.get(s..e).unwrap_or("");
            let after = text.get(e..).unwrap_or("");
            if !before.is_empty() {
                row = row.child(div().min_w_0().truncate().text_color(base).child(before.to_string()));
            }
            if !mid.is_empty() {
                row = row.child(
                    div()
                        .text_color(hl_fg)
                        .bg(hl_bg)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(mid.to_string()),
                );
            }
            if !after.is_empty() {
                row = row.child(div().min_w_0().truncate().text_color(base).child(after.to_string()));
            }
            row
        }
        None => row.child(
            div()
                .min_w_0()
                .truncate()
                .text_color(base)
                .child(text.to_string()),
        ),
    }
}

impl NotesPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = crate::state::persistence::load_notes();

        let initial_content = store
            .active_note
            .as_ref()
            .and_then(|id| store.notes.iter().find(|n| &n.id == id))
            .map(|n| n.content.clone())
            .unwrap_or_default();

        // The WYSIWYG block editor (ported from velotype). Content is loaded as
        // Markdown and edited as rendered blocks; `markdown()` serializes it
        // back out on save.
        let editor = cx.new(|cx| {
            crate::wysiwyg::editor::Editor::from_markdown(cx, initial_content, None).embedded()
        });

        let search = cx.new(|cx| {
            let mut input =
                InputState::new(window, cx).placeholder(rust_i18n::t!("notes.search_ph"));
            input.set_value(String::new(), window, cx);
            input
        });

        let title_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(String::new(), window, cx);
            input
        });

        let tree = cx.new(|cx| TreeState::new(cx));

        // Observe the editor for changes so we can debounce-save the serialized
        // Markdown back into `Note.content`. The editor emits `cx.notify()` on
        // every edit (and on undo/redo/mode switch), which fires this observer.
        let editor_sub = cx.observe(&editor, |this, editor, cx| {
            // The editor fires cx.notify() on every scroll tick to show/hide its
            // custom scrollbar, bumping `scroll_revision`. That is a view-only
            // change — the sidebar, word count, and autosave don't depend on it
            // — so skip the (expensive) full-panel rebuild + save reschedule on
            // scroll. Content/selection notifies leave the revision unchanged
            // and refresh as before.
            let current = editor.read(cx).scroll_revision;
            if current != this.last_scroll_revision {
                this.last_scroll_revision = current;
                return;
            }
            this.schedule_save(cx);
            // Re-notify the panel so the host title-bar toolbar (observed via
            // the panel in VerveApp) refreshes on caret/selection changes.
            cx.notify();
        });
        let search_sub = cx.subscribe(&search, Self::on_search_event);
        let title_sub = cx.subscribe(&title_input, Self::on_title_event);
        let tree_sub = cx.observe(&tree, Self::on_tree_notify);

        // FS watcher for the notes directory (detects external edits). notify
        // init failing is rare; when it does, external sync is simply disabled.
        let (wtx, wrx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
        let watcher = match notify::recommended_watcher(wtx) {
            Ok(mut w) => {
                if let Ok(dir) = crate::state::persistence::data_dir() {
                    let notes_dir = dir.join("notes");
                    let _ = std::fs::create_dir_all(&notes_dir);
                    if let Err(err) = w.watch(&notes_dir, RecursiveMode::Recursive) {
                        eprintln!("notes_panel: failed to watch notes dir: {err}");
                    }
                }
                Some(w)
            }
            Err(err) => {
                eprintln!("notes_panel: fs watcher unavailable: {err}");
                None
            }
        };

        let mut panel = Self {
            state,
            store,
            editor,
            prev_editors: Vec::new(),
            sidebar_resize: cx.new(|_| gpui_component::resizable::ResizableState::default()),
            search,
            title_input,
            tree,
            editing_title_id: None,
            pending_delete: None,
            pending_media_delete: None,
            save_timer: None,
            last_scroll_revision: 0,
            _subs: vec![editor_sub, search_sub, title_sub, tree_sub],
            editor_needs_reload: false,
            export_popover_open: false,
            list_collapsed: crate::state::persistence::load_layout()
                .map(|l| l.notes_collapsed)
                .unwrap_or(false),
            sidebar_default_width: crate::state::persistence::load_layout()
                .and_then(|l| l.side_widths)
                .and_then(|w| w.first().copied())
                .filter(|&w| w > 0.0)
                .map(gpui::px)
                .unwrap_or_else(|| gpui::px(260.)),
            row_menu_open: None,
            watcher,
            watcher_rx: wrx,
            last_self_save: None,
            notes_index: None,
            index_status: crate::notes_index::IndexStatus::Building,
            index_tx: None,
        };
        panel.rebuild_tree(cx);
        panel.start_external_change_loop(cx);
        panel.start_index_build(cx);
        panel
    }

    // -- editor change subscription ------------------------------------------

    // -- other callbacks -----------------------------------------------------

    fn on_search_event(
        &mut self,
        _: Entity<InputState>,
        _event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_tree(cx);
        cx.notify();
    }

    // -- tree / hierarchy ---------------------------------------------------

    fn on_tree_notify(&mut self, _tree: Entity<TreeState>, cx: &mut Context<Self>) {
        let Some(item) = self.tree.read(cx).selected_item().cloned() else {
            return;
        };
        let node = item.id.to_string();
        if let Some(note_id) = parse_note_id(&node) {
            let note_id = note_id.to_string();
            if self.store.active_note.as_deref() != Some(note_id.as_str()) {
                // Update active_note and request editor reload on next render.
                if self.editing_title_id.is_some() {
                    self.commit_title_edit(cx);
                }
                self.save_current(cx);
                self.store.active_note = Some(note_id);
                // Mark that editor needs reloading with the new note's content.
                self.editor_needs_reload = true;
                cx.notify();
            }
        }
    }

    /// Rebuild the tree's flat `Vec<TreeItem>` from `self.store.notes` and the
    /// current search query.
    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_string().to_lowercase();
        let items = self.build_note_children(None, &query);
        self.tree.update(cx, |t, cx| t.set_items(items, cx));
        cx.notify();
    }

    /// Toggle a note's collapsed state (driven by chevron clicks) and persist.
    fn toggle_note_collapsed(&mut self, note_id: &str, cx: &mut Context<Self>) {
        if self.store.collapsed.iter().any(|c| c == note_id) {
            self.store.collapsed.retain(|c| c != note_id);
        } else {
            self.store.collapsed.push(note_id.to_string());
        }
        self.persist_meta();
        self.rebuild_tree(cx);
    }

    /// Recursively build TreeItems for children of `parent_id` (None = root).
    fn build_note_children(&self, parent_id: Option<&str>, query: &str) -> Vec<TreeItem> {
        let kids = children_of(&self.store, parent_id);
        let mut items = Vec::new();
        for note in kids {
            let id = note_node(&note.id);
            let has_kids = has_children(&self.store, &note.id);
            let descendants_match = if !query.is_empty() {
                self.descendants_match(&note.id, query)
            } else {
                false
            };
            let self_matches = query.is_empty()
                || note.title.to_lowercase().contains(query)
                || note.content.to_lowercase().contains(query);
            if !query.is_empty() && !self_matches && !descendants_match {
                continue;
            }
            let children = self.build_note_children(Some(&note.id), query);
            let is_collapsed = self.store.collapsed.contains(&note.id);
            let mut label = note.title.clone();
            if label.is_empty() {
                label = rust_i18n::t!("notes.untitled").to_string();
            }
            let item = TreeItem::new(id.clone(), label)
                .children(children)
                .expanded(!is_collapsed);
            items.push(item);
        }
        items
    }

    /// True if any descendant of `note_id` matches the search query.
    fn descendants_match(&self, note_id: &str, query: &str) -> bool {
        for note in &self.store.notes {
            if note.parent_id.as_deref() == Some(note_id) {
                let self_matches = note.title.to_lowercase().contains(query)
                    || note.content.to_lowercase().contains(query);
                if self_matches || self.descendants_match(&note.id, query) {
                    return true;
                }
            }
        }
        false
    }

    /// Create a new sub-note under the currently active note.
    fn new_child_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent_id) = self.store.active_note.clone() else {
            // No active note — create a root note instead.
            self.new_note(window, cx);
            return;
        };
        let new_id = create_child_note(&mut self.store, &parent_id, String::new());
        // New empty sub-note → body in "edit mode" (first block focused), like
        // `new_note`. Title stays "Untitled" (rename via double-click).
        self.reload_editor("", window, cx, true);
        self.persist_note_meta(&new_id);
        self.rebuild_tree(cx);
        // Select the new note in the tree.
        let target = TreeItem::new(note_node(&new_id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        self.store.active_note = Some(new_id.clone());
        self.editor_needs_reload = false;
        cx.notify();
    }

    fn on_title_event(
        &mut self,
        _: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => self.commit_title_edit(cx),
            InputEvent::Blur => self.commit_title_edit(cx),
            _ => {}
        }
    }

    fn commit_title_edit(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editing_title_id.take() else {
            return;
        };
        let new_title = self.title_input.read(cx).value().to_string();
        notes::update_note_title(&mut self.store, &id, new_title);
        // Write to disk for local safety, but do NOT emit Persisted — git sync
        // follows the configured interval, not every edit.
        self.persist_note_meta(&id);
        self.rebuild_tree(cx);
        cx.notify();
    }

    fn start_title_edit(
        &mut self,
        id: &str,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing_title_id = Some(id.to_string());
        self.title_input.update(cx, |s, cx| {
            s.set_value(current.to_string(), window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// Write only UI meta — use when no note content/frontmatter changed.
    fn persist_meta(&mut self) {
        let _ = crate::state::persistence::save_notes_meta(&self.store);
        self.last_self_save = Some(Instant::now());
    }

    /// Write the note `id` to its own `.md` file (a title change renames it)
    /// and refresh UI meta. No-op if the id is unknown.
    fn persist_note_meta(&mut self, id: &str) {
        if let Some(note) = self.store.notes.iter().find(|n| n.id == id).cloned() {
            let _ = crate::state::persistence::save_note_one(&note, &mut self.store.file_paths);
            // Keep the full-text index in sync (queued to the background
            // worker, non-blocking). This is the single chokepoint for
            // create/edit/title/pin/move/duplicate/import.
            self.index_upsert(note);
        }
        let _ = crate::state::persistence::save_notes_meta(&self.store);
        self.last_self_save = Some(Instant::now());
    }

    // -- external-change watcher --------------------------------------------

    /// Background loop draining `watcher_rx`; reloads the sidebar when the
    /// notes directory is touched from outside the app. Stops automatically
    /// when the panel entity is dropped (`this.update` fails).
    fn start_external_change_loop(&mut self, cx: &mut Context<Self>) {
        if self.watcher.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(80))
                    .await;
                let Ok(()) = this.update(cx, |panel, cx| {
                    panel.drain_watcher_events(cx);
                }) else {
                    return; // panel dropped
                };
            }
        })
        .detach();
    }

    // -- full-text search index ---------------------------------------------

    /// Bootstrap the full-text index in the background: open/create it, rebuild
    /// from the on-disk markdown, then start the single serial worker that owns
    /// the writer and drains all subsequent mutations in order. Flips the status
    /// to Ready (or Failed on error).
    fn start_index_build(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = crate::state::persistence::data_dir().ok() else {
            self.index_status = crate::notes_index::IndexStatus::Failed;
            return;
        };
        let index_dir = dir.join("notes.index");
        let notes_dir = dir.join("notes");
        self.index_status = crate::notes_index::IndexStatus::Building;
        cx.notify();
        cx.spawn(async move |this, cx| {
            // open_or_create + rebuild_from_disk are CPU/IO-bound → run on the
            // blocking pool, never the UI thread.
            let result = cx
                .background_executor()
                .spawn(async move {
                    let idx = crate::notes_index::NotesIndex::open_or_create(&index_dir)?;
                    idx.rebuild_from_disk(&notes_dir)?;
                    Ok::<_, anyhow::Error>(idx)
                })
                .await;
            let _ = this.update(cx, |panel, cx| {
                match result {
                    Ok(idx) => {
                        let arc = Arc::new(idx);
                        // Start the single serial worker before publishing the
                        // sender, so the very first upsert can't be missed.
                        let (tx, rx) = std::sync::mpsc::channel::<IndexCmd>();
                        panel.spawn_index_worker(arc.clone(), rx, cx);
                        panel.notes_index = Some(arc);
                        panel.index_tx = Some(tx);
                        panel.index_status = crate::notes_index::IndexStatus::Ready;
                    }
                    Err(e) => {
                        log::warn!("notes index build failed: {e:?}");
                        panel.notes_index = None;
                        panel.index_tx = None;
                        panel.index_status = crate::notes_index::IndexStatus::Failed;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Spawn the single background worker that owns the index writer. It polls
    /// `rx` and applies commands **strictly in send order**, so background
    /// indexing stays correct under rapid edits: no newer upsert is clobbered
    /// by an older one, and a rebuild never interleaves with an upsert.
    /// Stops automatically when the panel is dropped (`this.update` fails) or
    /// the sender is dropped (channel disconnects).
    fn spawn_index_worker(
        &mut self,
        idx: Arc<crate::notes_index::NotesIndex>,
        rx: std::sync::mpsc::Receiver<IndexCmd>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                // Drain everything queued since the last tick, in order. Each
                // command's tantivy work runs on the blocking pool (never on
                // this async thread) and is awaited before reading the next
                // command, so order is preserved while no runtime thread blocks.
                loop {
                    let cmd = match rx.try_recv() {
                        Ok(c) => c,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    };
                    let idx_for_cmd = idx.clone();
                    let rebuild_ok: Option<bool> = cx
                        .background_executor()
                        .spawn(async move {
                            match cmd {
                                IndexCmd::Upsert(note) => {
                                    if let Err(e) = idx_for_cmd.upsert(&note) {
                                        log::warn!("notes index upsert failed: {e:?}");
                                    }
                                    None
                                }
                                IndexCmd::Delete(id) => {
                                    if let Err(e) = idx_for_cmd.delete(&id) {
                                        log::warn!("notes index delete failed: {e:?}");
                                    }
                                    None
                                }
                                // A rebuild is heavier (reads all md files);
                                // running it on the same worker guarantees it
                                // never overlaps an upsert. Caller set Building.
                                IndexCmd::Rebuild(dir) => {
                                    Some(idx_for_cmd.rebuild_from_disk(&dir).is_ok())
                                }
                            }
                        })
                        .await;
                    if let Some(ok) = rebuild_ok {
                        let _ = this.update(cx, |panel, cx| {
                            panel.index_status = if ok {
                                crate::notes_index::IndexStatus::Ready
                            } else {
                                crate::notes_index::IndexStatus::Failed
                            };
                            cx.notify();
                        });
                    }
                }
            }
        })
        .detach();
    }

    /// Queue a full rebuild from the on-disk markdown (used after external file
    /// changes reload the store). Flips status to Building immediately; the
    /// worker resets it to Ready/Failed when the rebuild finishes. No-op if the
    /// worker isn't running.
    fn schedule_index_rebuild(&mut self, cx: &mut Context<Self>) {
        let Some(tx) = &self.index_tx else {
            return;
        };
        let Some(dir) = crate::state::persistence::data_dir().ok() else {
            return;
        };
        self.index_status = crate::notes_index::IndexStatus::Building;
        cx.notify();
        let _ = tx.send(IndexCmd::Rebuild(dir.join("notes")));
    }

    /// Queue a single-note upsert to the background worker (non-blocking).
    fn index_upsert(&self, note: Note) {
        if let Some(tx) = &self.index_tx {
            let _ = tx.send(IndexCmd::Upsert(note));
        }
    }

    /// Queue a single-note deletion to the background worker (non-blocking).
    fn index_delete(&self, note_id: String) {
        if let Some(tx) = &self.index_tx {
            let _ = tx.send(IndexCmd::Delete(note_id));
        }
    }

    /// Drain queued watcher events and, if any are relevant (and not our own
    /// write), reload notes from disk into the sidebar. The active note is
    /// preserved when it still exists; the editor is intentionally left alone
    /// so external changes don't disrupt in-progress edits.
    fn drain_watcher_events(&mut self, cx: &mut Context<Self>) {
        use notify::EventKind;
        let mut relevant = false;
        while let Ok(item) = self.watcher_rx.try_recv() {
            if !relevant {
                relevant = item
                    .map(|ev| {
                        matches!(
                            ev.kind,
                            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                                | EventKind::Any
                        )
                    })
                    .unwrap_or(false);
            }
        }
        if !relevant {
            return;
        }
        // Ignore the echo of our own recent save.
        if self
            .last_self_save
            .is_some_and(|t| t.elapsed() < NOTES_SELF_SAVE_FILTER_WINDOW)
        {
            return;
        }
        // Reload notes from disk, preserving the active selection when possible.
        let keep_active = self.store.active_note.clone();
        let reloaded = crate::state::persistence::load_notes();
        self.store.notes = reloaded.notes;
        // Refresh the id→path cache from the reloaded store (external edits may
        // have added/removed/renamed files).
        self.store.file_paths = reloaded.file_paths;
        let still_present = keep_active
            .as_ref()
            .is_some_and(|id| self.store.notes.iter().any(|n| &n.id == id));
        self.store.active_note = if still_present {
            keep_active
        } else {
            reloaded.active_note
        };
        // External edits changed the on-disk markdown (the index's source of
        // truth) — rebuild the full-text index from disk to match.
        self.schedule_index_rebuild(cx);
        self.rebuild_tree(cx);
        cx.notify();
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        self.save_timer = Some(cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let _ = weak.update(cx, |this, cx| {
                this.save_current(cx);
            });
        }));
    }

    pub fn save_current(&mut self, cx: &mut Context<Self>) {
        let editor_val = self.editor.read(cx).markdown(cx);
        if let Some(active_id) = self.store.active_note.clone() {
            notes::update_note_content(&mut self.store, &active_id, editor_val);
            // Write to local disk for crash safety, but do NOT emit Persisted.
            // Git sync runs on the configured interval (settings.git.sync_interval)
            // rather than firing on every edit.
            self.persist_note_meta(&active_id);
        }
    }

    pub fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let note = notes::create_note(&mut self.store, String::new());
        let new_id = note.id.clone();
        self.store.active_note = Some(new_id.clone());
        // New empty note → body in "edit mode" (like the markdown editor): a
        // fresh editor with the first block focused, so the user can type,
        // insert tables/images, etc. immediately. The title stays "Untitled"
        // (rename via double-click in the list).
        self.reload_editor("", window, cx, true);
        self.persist_note_meta(&new_id);
        self.rebuild_tree(cx);
        let target = TreeItem::new(note_node(&new_id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        cx.notify();
    }

    /// Select a note (called from list-row click handlers — has Window access).
    fn select_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_title_id.is_some() {
            self.commit_title_edit(cx);
        }
        self.save_current(cx);
        self.store.active_note = Some(id.to_string());
        self.editor_needs_reload = false;
        if let Some(note) = self.store.notes.iter().find(|n| n.id == id) {
            let content = note.content.clone();
            // Rebuild the editor fresh for this note (see `reload_editor`):
            // a reused editor + `set_content` leaves macOS IME unbound, so
            // typing is dead until the body is clicked.
            self.reload_editor(&content, window, cx, true);
        }
        self.rebuild_tree(cx);
        let target = TreeItem::new(note_node(id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        cx.notify();
    }

    fn delete_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Refuse to delete a note that still has children — sub-notes must be
        // removed bottom-up. The confirm dialog enforces this; this guard is a
        // defensive backstop for any other caller.
        if has_children(&self.store, id) {
            self.pending_delete = None;
            cx.notify();
            return;
        }
        if self.editing_title_id.is_some() {
            self.commit_title_edit(cx);
        }
        // Collect subtree ids before the cascade removes them from memory.
        let to_remove = notes::note_ids_in_subtree(&self.store, id);
        delete_note_cascade(&mut self.store, id);
        crate::state::persistence::delete_notes_files(&to_remove, &mut self.store.file_paths);
        // Drop the deleted notes (whole subtree) from the full-text index
        // (queued to the background worker, non-blocking).
        for rid in &to_remove {
            self.index_delete(rid.clone());
        }
        if let Some(note) = self.store.notes.first() {
            let content = note.content.clone();
            self.store.active_note = Some(note.id.clone());
            self.reload_editor(&content, window, cx, true);
        } else {
            self.reload_editor("", window, cx, true);
        }
        self.persist_meta();
        self.pending_delete = None;
        self.rebuild_tree(cx);
        // Deletions must reach the git remote immediately — otherwise the
        // startup `git checkout -f main` reverts the (uncommitted) local
        // deletion and the note reappears after restart. Persisted triggers
        // VerveApp's git sync (commit + push).
        let _ = self
            .state
            .update(cx, |_, cx| cx.emit(crate::state::AppEvent::Persisted));
        cx.notify();
    }

    /// Number of notes in the store (used for the title-bar badge).
    /// The embedded WYSIWYG editor entity (used by the title-bar toolbar).
    pub fn editor(&self) -> &Entity<crate::wysiwyg::editor::Editor> {
        &self.editor
    }

    /// cmd-w (Notes) — close the current note: save it, clear the active
    /// selection, and show an empty editor. The note itself is kept in the
    /// list (not deleted); Notes auto-save, so there is no unsaved prompt.
    pub fn close_current_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_current(cx);
        self.store.active_note = None;
        self.tree.update(cx, |t, cx| t.set_selected_item(None, cx));
        self.reload_editor("", window, cx, false);
        self.persist_meta();
        self.rebuild_tree(cx);
        cx.notify();
    }

    /// Toggle the editor between Rendered (WYSIWYG) and raw-Source view modes.
    /// Driven by the title-bar view-mode toggle (mirrors MarkdownPanel).
    pub fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |e, cx| e.toggle_view_mode_from_ui(cx));
    }

    /// Rebuild the embedded editor fresh for `content` — mirroring MarkdownPanel,
    /// which constructs a new [`Editor`] per document instead of reusing one.
    ///
    /// Reusing a single editor and swapping its content via `set_content` does
    /// not re-bind macOS IME on the subsequent programmatic focus: the block is
    /// GPUI-focused and registers its input handler, but keystrokes never reach
    /// it until the user clicks the editor body. A freshly constructed editor's
    /// first focus binds IME correctly, so the note is immediately typeable.
    /// Re-subscribes the autosave observer (the editor entity changed) and
    /// focuses the first block.
    fn reload_editor(
        &mut self,
        content: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
        focus_body: bool,
    ) {
        // Keep the outgoing editor alive (don't drop it). MarkdownPanel never
        // drops an editor (one per tab); notes replaces its single editor on
        // every switch, and dropping the just-focused editor appears to corrupt
        // GPUI so the new editor's block updates never repaint. Holding the old
        // entity (its focus handles stay valid) avoids that. This is a
        // keep-alive arena; entries are never re-embedded, only prevented from
        // being dropped mid-frame. Cap it so memory doesn't grow unboundedly
        // across many note switches.
        self.prev_editors.push(self.editor.clone());
        if self.prev_editors.len() > 3 {
            self.prev_editors.drain(0..self.prev_editors.len() - 3);
        }
        let content = content.to_string();
        let new_editor = cx
            .new(|cx| crate::wysiwyg::editor::Editor::from_markdown(cx, content, None).embedded());
        self.editor = new_editor;
        // The new editor starts at scroll_revision 0; sync our tracker so the
        // first content notify after the switch isn't mistaken for a scroll.
        self.last_scroll_revision = self.editor.read(cx).scroll_revision;
        // The autosave observer is bound to the (now-dropped) old editor entity;
        // re-subscribe to the new one. It's the first entry in `_subs`.
        let editor_sub = cx.observe(&self.editor, |this, editor, cx| {
            let current = editor.read(cx).scroll_revision;
            if current != this.last_scroll_revision {
                this.last_scroll_revision = current;
                return;
            }
            this.schedule_save(cx);
            cx.notify();
        });
        if !self._subs.is_empty() {
            self._subs[0] = editor_sub;
        }
        if !focus_body {
            // The caller focuses something else (e.g. the title input);
            // suppress the body auto-focus so it doesn't steal it back.
            self.editor.update(cx, |e, _cx| e.clear_pending_focus());
        }
    }

    pub fn note_count(&self) -> usize {
        self.store.notes.len()
    }

    /// Toggle whether the note list sidebar is collapsed (hidden).
    pub fn toggle_list_collapsed(&mut self, cx: &mut Context<Self>) {
        self.list_collapsed = !self.list_collapsed;
        // Persist the collapsed state.
        let mut layout = crate::state::persistence::load_layout().unwrap_or_default();
        layout.notes_collapsed = self.list_collapsed;
        let _ = crate::state::persistence::save_layout(&layout);
        cx.notify();
    }

    /// Whether the note list sidebar is currently collapsed.
    pub fn is_list_collapsed(&self) -> bool {
        self.list_collapsed
    }

    /// Whether the per-note export popover (in the title bar) is open.
    pub fn export_popover_open(&self) -> bool {
        self.export_popover_open
    }

    /// Toggle the export popover open/closed (used by the title-bar trigger).
    pub fn set_export_popover_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.export_popover_open != open {
            self.export_popover_open = open;
            cx.notify();
        }
    }

    /// Duplicate a note by id (creates a copy with "(Copy)" suffix, selects it).
    fn duplicate_note_by_id(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(new_id) = notes::duplicate_note(&mut self.store, id) {
            self.row_menu_open = None;
            self.persist_note_meta(&new_id);
            self.rebuild_tree(cx);
            self.select_note(&new_id, window, cx);
        }
    }

    /// Toggle pin for a note by id.
    fn toggle_pin_by_id(&mut self, id: &str, cx: &mut Context<Self>) {
        notes::toggle_pin(&mut self.store, id);
        self.row_menu_open = None;
        self.persist_note_meta(id);
        self.rebuild_tree(cx);
        cx.notify();
    }

    /// Move a note to root level by id.
    fn move_note_to_root_by_id(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        notes::move_note_to_root(&mut self.store, id);
        self.row_menu_open = None;
        self.persist_note_meta(id);
        self.rebuild_tree(cx);
        // Keep the note selected after move.
        if self.store.active_note.as_deref() == Some(id) {
            self.select_note(id, window, cx);
        } else {
            cx.notify();
        }
    }

    /// Create a new sub-note under an arbitrary parent (not necessarily active).
    fn new_child_note_for_id(
        &mut self,
        parent_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_id = create_child_note(&mut self.store, parent_id, String::new());
        // New empty sub-note → body in "edit mode" (first block focused).
        self.reload_editor("", window, cx, true);
        self.persist_note_meta(&new_id);
        self.rebuild_tree(cx);
        let target = TreeItem::new(note_node(&new_id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        self.store.active_note = Some(new_id.clone());
        self.editor_needs_reload = false;
        cx.notify();
    }

    /// Export a specific note (by id) in the given format.
    fn export_note_as(
        &mut self,
        note_id: &str,
        fmt: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If exporting the note currently shown in the editor, flush its
        // pending (debounced) edits first so the exported body is up to date.
        if self.store.active_note.as_deref() == Some(note_id) {
            self.save_current(cx);
        }
        let Some(note) = self.store.notes.iter().find(|n| n.id == note_id).cloned() else {
            return;
        };
        self.row_menu_open = None;
        self.export_popover_open = false;

        let suggested = sanitize_filename(&if note.title.is_empty() {
            rust_i18n::t!("notes.untitled").to_string()
        } else {
            note.title.clone()
        });
        let title = note.title.clone();
        let content = note.content.clone();
        let format = fmt.to_string();
        let format_for_dialog = format.clone();
        let format_for_save = format.clone();
        let window_handle: gpui::AnyWindowHandle = window.window_handle();

        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let (filename, filter_name, ext) = match format_for_dialog.as_str() {
                "pdf" => (format!("{suggested}.pdf"), "PDF", "pdf"),
                _ => (format!("{suggested}.md"), "Markdown", "md"),
            };
            let res = rfd::FileDialog::new()
                .set_file_name(&filename)
                .add_filter(filter_name, &[ext])
                .save_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let result = match format_for_save.as_str() {
                            "pdf" => crate::notes_pdf::markdown_to_pdf(&content, &title, &path),
                            _ => std::fs::write(&path, &content).map_err(anyhow::Error::from),
                        };
                        match result {
                            Ok(()) => log::info!("exported note to {:?}", path),
                            Err(e) => {
                                log::error!("export failed: {e:?}");
                                show_export_error(window_handle, &e.to_string(), cx);
                            }
                        }
                        break;
                    }
                    Ok(None) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        })
        .detach();
        cx.notify();
    }

    // -- import / export ---------------------------------------------------

    /// Open a native file picker and import the chosen `.md` file as a new note.
    pub fn import_markdown_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .add_filter("Markdown", &["md", "markdown", "txt"])
                .pick_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let _ = ent.update(cx, |panel, cx| {
                            panel.import_note_from_path(&path, cx);
                        });
                        break;
                    }
                    Ok(None) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        })
        .detach();
    }

    fn import_note_from_path(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("import markdown read failed: {e:?}");
                return;
            }
        };
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let note = notes::create_note(&mut self.store, title.clone());
        let new_id = note.id.clone();
        notes::update_note_content(&mut self.store, &new_id, content);
        self.store.active_note = Some(new_id.clone());
        self.persist_note_meta(&new_id);
        self.editor_needs_reload = true;
        self.rebuild_tree(cx);
        let target = TreeItem::new(note_node(&new_id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        log::info!("imported markdown note from {:?}", path);
        cx.notify();
    }

    /// Export the active note in a given format: `"md"` or `"pdf"`.
    pub fn export_active_note_as(
        &mut self,
        fmt: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active_id) = self.store.active_note.clone() else {
            log::warn!("export: no active note");
            return;
        };
        // Flush any pending (debounced) editor edits into `note.content` before
        // reading it, otherwise an export fired right after typing yields a
        // stale or empty body (autosave is debounced 500ms; see `schedule_save`).
        self.save_current(cx);
        let Some(note) = self.store.notes.iter().find(|n| n.id == active_id).cloned() else {
            return;
        };
        self.export_popover_open = false;

        let suggested = sanitize_filename(&if note.title.is_empty() {
            rust_i18n::t!("notes.untitled").to_string()
        } else {
            note.title.clone()
        });
        let title = note.title.clone();
        let content = note.content.clone();
        let format = fmt.to_string();
        let format_for_dialog = format.clone();
        let format_for_save = format.clone();
        let window_handle: gpui::AnyWindowHandle = window.window_handle();

        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let (filename, filter_name, ext) = match format_for_dialog.as_str() {
                "pdf" => (format!("{suggested}.pdf"), "PDF", "pdf"),
                _ => (format!("{suggested}.md"), "Markdown", "md"),
            };
            let res = rfd::FileDialog::new()
                .set_file_name(&filename)
                .add_filter(filter_name, &[ext])
                .save_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let result = match format_for_save.as_str() {
                            "pdf" => crate::notes_pdf::markdown_to_pdf(&content, &title, &path),
                            _ => std::fs::write(&path, &content).map_err(anyhow::Error::from),
                        };
                        match result {
                            Ok(()) => log::info!("exported note to {:?}", path),
                            Err(e) => {
                                log::error!("export failed: {e:?}");
                                show_export_error(window_handle, &e.to_string(), cx);
                            }
                        }
                        break;
                    }
                    Ok(None) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        })
        .detach();
        cx.notify();
    }

    // -- rendering -----------------------------------------------------------

    /// Resolve the search hits for `query`, preferring the tantivy index (when
    /// Ready) for jieba + n-gram recall and relevance ranking, and falling back
    /// to the in-memory matcher while the index is Building/Failed/unavailable
    /// so results stay complete at all times.
    fn collect_search_results(&self, query: &str, limit: usize) -> Vec<NoteSearchResult> {
        let q = query.trim();
        if q.is_empty() || limit == 0 {
            return Vec::new();
        }
        if self.index_status == crate::notes_index::IndexStatus::Ready {
            if let Some(idx) = &self.notes_index {
                if let Ok(hits) = idx.search(q, limit) {
                    let untitled = rust_i18n::t!("notes.untitled").to_string();
                    let mut out = Vec::with_capacity(hits.len());
                    for h in hits {
                        if let Some(note) = self.store.notes.iter().find(|n| n.id == h.note_id) {
                            out.push(result_for_note(note, q, &untitled));
                        }
                    }
                    return out;
                }
                // Index query errored — fall through to the in-memory matcher.
            }
        }
        search_notes(&self.store.notes, q, limit)
    }

    /// Full-text search results list (shown in the sidebar in place of the tree
    /// while a non-empty query is active). Each row highlights the matched span
    /// in the title and/or a content excerpt, and opens the note on click.
    /// Capped at 10 hits.
    fn render_search_results(&self, query: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let view = cx.entity().clone();

        let results = self.collect_search_results(query, 10);
        // High-contrast highlight: a solid accent block with its paired
        // accent_foreground text (the theme's designed contrast pair, as used
        // by buttons/selections). Solid + paired = vivid in both light & dark
        // themes — `warning_foreground` falls back to primary_foreground
        // (near black/white), which read as a flat monochrome before.
        let hl_bg = theme.accent;
        let hl_fg = theme.accent_foreground;

        if results.is_empty() {
            return v_flex()
                .w_full()
                .child(
                    div()
                        .px_3()
                        .py_4()
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .text_size(px(12.))
                        .child(rust_i18n::t!("notes.search_no_results").to_string()),
                )
                .into_any_element();
        }

        let mut col = v_flex().w_full().py_1().gap(px(1.));
        for (i, r) in results.iter().enumerate() {
            let nid = r.note_id.clone();
            let view_for_click = view.clone();
            let title = r.title.clone();
            let title_match = r.title_match;
            let snippet = r.snippet.clone();
            let snippet_match = r.snippet_match;
            let fg = theme.foreground;
            let muted = theme.muted_foreground;
            let hover_bg = theme.muted.opacity(0.6);

            let row = div()
                .id(("notes-search-row", i))
                .w_full()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |c| c.bg(hover_bg))
                .on_mouse_down(MouseButton::Left, move |_, window, cx: &mut App| {
                    let nid = nid.clone();
                    let _ = view_for_click.update(cx, |this, cx| {
                        this.select_note(&nid, window, cx);
                        // Clear the query so the tree comes back with the note
                        // selected — i.e. actually "locate" it in the list.
                        this.search
                            .update(cx, |s, cx| s.set_value("", window, cx));
                    });
                })
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(render_highlighted(&title, title_match, fg, hl_fg, hl_bg)),
                )
                .when(snippet.is_some(), |c| {
                    let s = snippet.as_deref().unwrap_or("");
                    c.child(
                        div().text_size(px(11.)).child(render_highlighted(
                            s,
                            snippet_match,
                            muted,
                            hl_fg,
                            hl_bg,
                        )),
                    )
                });
            col = col.child(row);
        }
        col.into_any_element()
    }

    fn render_note_list(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let view = cx.entity().clone();
        // Non-empty query → show full-text search results instead of the tree.
        let query = self.search.read(cx).value().to_string();

        v_flex()
            .h_full()
            .w_full()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.3))
            .child(
                h_flex()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("notes-new")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .flex_1()
                            .child(rust_i18n::t!("notes.new_note").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_note(window, cx);
                            })),
                    )
                    .child(
                        Button::new("notes-new-child")
                            .small()
                            .ghost()
                            .icon(IconName::ChevronRight)
                            .tooltip(rust_i18n::t!("notes.new_sub_note").to_string())
                            .disabled(self.store.active_note.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_child_note(window, cx);
                            })),
                    )
                    .child(Input::new(&self.search).w(px(100.)).small()),
            )
            // Index-build status bar — only visible while a full rebuild is in
            // progress, so the user can tell why search may briefly use the
            // in-memory fallback. Disappears on Ready/Failed.
            .when(
                self.index_status == crate::notes_index::IndexStatus::Building,
                |this| {
                    this.child(
                        h_flex()
                            .w_full()
                            .px_2()
                            .py_1()
                            .gap_1()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border)
                            .text_color(theme.muted_foreground)
                            .text_size(px(11.))
                            .child(gpui_component::spinner::Spinner::new())
                            .child(rust_i18n::t!("notes.index_building").to_string()),
                    )
                },
            )
            .child(
                div()
                    .id("notes-list-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .child(if !query.trim().is_empty() {
                        // Full-text search results replace the tree while a
                        // query is active (see `render_search_results`).
                        self.render_search_results(&query, cx)
                    } else {
                        tree(&self.tree, move |ix, entry, _selected, _window, cx| {
                            let item = entry.item();
                            let note_id = parse_note_id(item.id.as_ref()).unwrap_or("").to_string();
                            let note = view
                                .read(cx)
                                .store
                                .notes
                                .iter()
                                .find(|n| n.id == note_id);
                            let is_active = view.read(cx).store.active_note.as_deref()
                                == Some(note_id.as_str());
                            let is_folder = entry.is_folder();
                            let is_expanded = entry.is_expanded();
                            let depth = entry.depth();
                            let title = note
                                .map(|n| {
                                    if n.title.is_empty() {
                                        rust_i18n::t!("notes.untitled").to_string()
                                    } else {
                                        n.title.clone()
                                    }
                                })
                                .unwrap_or_default();
                            let updated = note
                                .map(|n| {
                                    rust_i18n::t!(
                                        "notes.last_updated",
                                        time = n.updated_at.as_str()
                                    )
                                    .to_string()
                                })
                                .unwrap_or_default();

                            let delete_id = note_id.clone();
                            let chevron_target = note_id.clone();
                            let view_for_chevron = view.clone();
                            let view_for_select = view.clone();
                            let view_for_delete = view.clone();
                            let view_for_add = view.clone();
                            let view_for_menu = view.clone();
                            let is_pinned = note.map(|n| n.pinned).unwrap_or(false);
                            let group = format!("notes-row-{ix}");
                            let add_nid = note_id.clone();
                            let menu_nid = note_id.clone();
                            let menu_nid2 = note_id.clone();
                            let menu_nid3 = note_id.clone();
                            let menu_nid4 = note_id.clone();
                            let menu_nid5 = note_id.clone();
                            let menu_nid_export = note_id.clone();
                            let row_menu_open = view.read(cx).row_menu_open.as_deref()
                                == Some(note_id.as_str());

                            ListItem::new(ix)
                                .w_full()
                                .selected(is_active)
                                .rounded(px(4.))
                                .px_2()
                                .pl(px(16.) * depth + px(4.))
                                .when(is_active, |li| {
                                    li.bg(theme.accent.opacity(0.3))
                                        .text_color(theme.foreground)
                                })
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_1()
                                        .items_center()
                                        .group(group.clone())
                                        // Pin indicator
                                        .when(is_pinned, |c| {
                                            c.child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(theme.accent)
                                                    .child(IconName::Star),
                                            )
                                        })
                                        // Chevron (only for notes that can have children)
                                        .child(
                                            div()
                                                .w(px(16.))
                                                .h(px(16.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_color(theme.muted_foreground)
                                                .when(is_folder, |c| {
                                                    c.child(if is_expanded {
                                                        IconName::ChevronDown
                                                    } else {
                                                        IconName::ChevronRight
                                                    })
                                                    .hover(|c| c.text_color(theme.foreground))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, _window, cx: &mut App| {
                                                            cx.stop_propagation();
                                                            let _ = view_for_chevron.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.toggle_note_collapsed(
                                                                        &chevron_target,
                                                                        cx,
                                                                    );
                                                                },
                                                            );
                                                        },
                                                    )
                                                }),
                                        )
                                        // Note icon
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(theme.muted_foreground)
                                                .child(if is_folder {
                                                    if is_expanded {
                                                        IconName::FolderOpen
                                                    } else {
                                                        IconName::Folder
                                                    }
                                                } else {
                                                    IconName::File
                                                }),
                                        )
                                        // Title + timestamp
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap(px(1.))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    {
                                                        let nid = note_id.clone();
                                                        let view = view_for_select;
                                                        move |_, window, cx: &mut App| {
                                                            let _ = view.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.select_note(
                                                                        &nid, window, cx,
                                                                    );
                                                                },
                                                            );
                                                        }
                                                    },
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .truncate()
                                                        .child(title),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.))
                                                        .text_color(theme.muted_foreground)
                                                        .truncate()
                                                        .child(updated),
                                                ),
                                        )
                                        // Hover-revealed "+" button (new sub-note)
                                        .child(
                                            div()
                                                .id(format!("notes-add-{}", ix))
                                                .px_1()
                                                .rounded_sm()
                                                .opacity(0.0)
                                                .text_color(theme.muted_foreground)
                                                .group_hover(group.clone(), |this| {
                                                    this.opacity(1.0).text_color(theme.foreground)
                                                })
                                                .hover(|this| this.bg(theme.border))
                                                .cursor_pointer()
                                                .child(IconName::Plus)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    move |_, window, cx: &mut App| {
                                                        cx.stop_propagation();
                                                        let pid = add_nid.clone();
                                                        let _ = view_for_add.update(
                                                            cx,
                                                            |this, cx| {
                                                                this.new_child_note_for_id(
                                                                    &pid, window, cx,
                                                                );
                                                            },
                                                        );
                                                    },
                                                ),
                                        )
                                        // Hover-revealed "..." menu button
                                        .child({
                                            let popover_id = format!("notes-row-pop-{}", note_id);
                                            let view_for_open = view.clone();
                                            let open_nid = menu_nid5.clone();
                                            Popover::new(popover_id)
                                                .anchor(gpui::Anchor::BottomRight)
                                                .open(row_menu_open)
                                                .on_open_change(move |open: &bool, _window, cx: &mut App| {
                                                    let _ = view_for_open.update(cx, |this, cx| {
                                                        if *open {
                                                            this.row_menu_open =
                                                                Some(open_nid.clone());
                                                        } else if this.row_menu_open.as_deref()
                                                            == Some(open_nid.as_str())
                                                        {
                                                            this.row_menu_open = None;
                                                        }
                                                        cx.notify();
                                                    });
                                                })
                                                .trigger(
                                                    Button::new(format!("notes-more-btn-{}", ix))
                                                        .ghost()
                                                        .xsmall()
                                                        .icon(IconName::EllipsisVertical),
                                                )
                                                .p(px(4.))
                                                .child(
                                                    v_flex()
                                                        .w(px(180.))
                                                        .gap(px(2.))
                                                        // Duplicate
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(IconName::Copy)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("notes.duplicate").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let nid = menu_nid.clone();
                                                                        let view = view_for_menu.clone();
                                                                        move |_, window, cx: &mut App| {
                                                                            let nid = nid.clone();
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.duplicate_note_by_id(&nid, window, cx);
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                        // Move to Root
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(IconName::ArrowUp)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("notes.move_to_root").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let nid = menu_nid2.clone();
                                                                        let view = view_for_menu.clone();
                                                                        move |_, window, cx: &mut App| {
                                                                            let nid = nid.clone();
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.move_note_to_root_by_id(&nid, window, cx);
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                        // Pin/Unpin
                                                        .child({
                                                            let nid_pin = menu_nid3.clone();
                                                            let view_pin = view_for_menu.clone();
                                                            let pin_label = if is_pinned {
                                                                rust_i18n::t!("notes.unpin").to_string()
                                                            } else {
                                                                rust_i18n::t!("notes.pin").to_string()
                                                            };
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(if is_pinned { IconName::StarOff } else { IconName::Star })
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(pin_label),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    move |_, _, cx: &mut App| {
                                                                        let nid = nid_pin.clone();
                                                                        let _ = view_pin.update(cx, |this, cx| {
                                                                            this.toggle_pin_by_id(&nid, cx);
                                                                        });
                                                                    },
                                                                )
                                                        })
                                                        // Divider
                                                        .child(
                                                            div()
                                                                .w_full()
                                                                .h(px(1.))
                                                                .my(px(2.))
                                                                .bg(theme.border),
                                                        )
                                                        // Export MD
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(IconName::ExternalLink)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("notes.row_export_md").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let nid = menu_nid_export.clone();
                                                                        let view = view_for_menu.clone();
                                                                        move |_, window, cx: &mut App| {
                                                                            let nid = nid.clone();
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.export_note_as(&nid, "md", window, cx);
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                        // Export PDF
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(IconName::ExternalLink)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("notes.row_export_pdf").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let nid = menu_nid4.clone();
                                                                        let view = view_for_menu.clone();
                                                                        move |_, window, cx: &mut App| {
                                                                            let nid = nid.clone();
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.export_note_as(&nid, "pdf", window, cx);
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                        // Divider
                                                        .child(
                                                            div()
                                                                .w_full()
                                                                .h(px(1.))
                                                                .my(px(2.))
                                                                .bg(theme.border),
                                                        )
                                                        // Delete
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.danger.opacity(0.2)))
                                                                .text_color(theme.danger)
                                                                .child(IconName::Delete)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("common.delete").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let did = delete_id.clone();
                                                                        let view = view_for_delete.clone();
                                                                        move |_, _, cx: &mut App| {
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.row_menu_open = None;
                                                                                this.pending_delete = Some(did.clone());
                                                                                cx.notify();
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                )
                                        })
                                )
                        })
                        .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// First-stage delete confirmation just succeeded. Scan the note for media
    /// owned by this app; if it references any, ask (second dialog) whether to
    /// delete the media too. Otherwise delete the note outright.
    fn request_delete(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = crate::media::config::load_media_config();
        let media = self
            .store
            .notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| crate::media::cleanup::collect_deletable_media(&n.content, &cfg))
            .unwrap_or_else(|| crate::media::cleanup::DeletableMedia {
                local_paths: Vec::new(),
                s3_keys: Vec::new(),
            });

        if media.is_empty() {
            self.delete_note(id, window, cx);
            return;
        }
        self.pending_media_delete = Some(PendingMediaDelete {
            note_id: id.to_string(),
            local_paths: media.local_paths,
            s3_keys: media.s3_keys,
        });
        self.pending_delete = None;
        cx.notify();
    }

    /// Carry out note deletion together with its referenced media (local sync,
    /// S3 async fire-and-forget). Failures are logged but never block the note
    /// deletion.
    fn confirm_delete_with_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let job = match self.pending_media_delete.take() {
            Some(j) => j,
            None => return,
        };
        let (_, local_failed) = crate::media::cleanup::delete_local_files(&job.local_paths);
        if local_failed > 0 {
            log::warn!("failed to delete {local_failed} local media file(s)");
        }
        if !job.s3_keys.is_empty() {
            let cfg = crate::media::config::load_media_config();
            if cfg.s3.is_configured() {
                let rx = crate::media::upload::enqueue_s3_object_deletes(
                    cfg.s3.clone(),
                    job.s3_keys.clone(),
                );
                cx.spawn(async move |_this, _cx| match rx.await {
                    Ok(Ok((deleted, failed))) => {
                        if failed > 0 {
                            log::warn!("S3 media delete: {deleted} ok, {failed} failed");
                        }
                    }
                    _ => {}
                })
                .detach();
            }
        }
        self.delete_note(&job.note_id, window, cx);
    }

    /// Second-stage dialog: delete the note together with its referenced media,
    /// or just the note, or cancel.
    fn render_media_delete_confirm(
        &self,
        job: &PendingMediaDelete,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let count = job.total();
        let body = rust_i18n::t!("notes.delete_media_msg", count = count).to_string();
        let note_id = job.note_id.clone();

        div()
            .absolute()
            .inset_0()
            .bg(black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(360.))
                    .p_4()
                    .gap_3()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(rust_i18n::t!("notes.delete_media_title").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child(body),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("md-cancel")
                                    .small()
                                    .ghost()
                                    .child(rust_i18n::t!("common.cancel").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pending_media_delete = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("md-note-only")
                                    .small()
                                    .ghost()
                                    .child(rust_i18n::t!("notes.delete_note_only").to_string())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let id = note_id.clone();
                                        this.pending_media_delete = None;
                                        this.delete_note(&id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("md-all")
                                    .small()
                                    .danger()
                                    .child(rust_i18n::t!("notes.delete_all").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_delete_with_media(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_delete_confirm(&self, note_id: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let note = self.store.notes.iter().find(|n| n.id == note_id);
        let note_title = note.map(|n| n.title.clone()).unwrap_or_default();
        let descendant_count = note
            .map(|n| count_descendants(&self.store, &n.id))
            .unwrap_or(0);

        let (title, body, buttons) = if descendant_count > 0 {
            // Blocked: the note still has sub-notes. Refuse to delete and tell
            // the user to remove sub-notes first (bottom-up, deepest first).
            let title = rust_i18n::t!("notes.cannot_delete").to_string();
            let body = rust_i18n::t!("notes.delete_blocked_msg", count = descendant_count)
                .to_string();
            let buttons = h_flex()
                .w_full()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("del-ok")
                        .small()
                        .ghost()
                        .child(rust_i18n::t!("common.ok").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pending_delete = None;
                            cx.notify();
                        })),
                )
                .into_any_element();
            (title, body, buttons)
        } else {
            let title = rust_i18n::t!("notes.confirm_delete").to_string();
            let body =
                rust_i18n::t!("notes.confirm_delete_msg", title = note_title.as_str()).to_string();
            let id = note_id.to_string();
            let buttons = h_flex()
                .w_full()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("del-cancel")
                        .small()
                        .ghost()
                        .child(rust_i18n::t!("common.cancel").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pending_delete = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("del-confirm")
                        .small()
                        .danger()
                        .child(rust_i18n::t!("common.delete").to_string())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_delete(&id, window, cx);
                        })),
                )
                .into_any_element();
            (title, body, buttons)
        };

        div()
            .absolute()
            .inset_0()
            .bg(black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(360.))
                    .p_4()
                    .gap_3()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child(body),
                    )
                    .child(buttons),
            )
            .into_any_element()
    }

    fn render_editor(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        let active_id = self.store.active_note.clone();
        let active_note = active_id
            .as_deref()
            .and_then(|id| self.store.notes.iter().find(|n| n.id == id));

        let editing_active = matches!(&self.editing_title_id, Some(eid) if Some(eid.as_str()) == active_id.as_deref());

        // Live word count of the active note's body. Computed from the editor's
        // serialized markdown (reflects unsaved typing) with `count_words`
        // (汉字 = 1; same metric as the editor status bar & markdown title bar).
        // The panel re-renders on every edit via the editor observer's
        // cx.notify(), so this stays live as the user types.
        let content_word_count: Option<usize> = if active_note.is_some() {
            // Throttled via the editor's word-count cache; the panel re-renders
            // on edits via the editor observer, so this stays live while typing.
            Some(self.editor.read(cx).word_count(cx).0)
        } else {
            None
        };

        // Single WYSIWYG content pane — the embedded block editor renders its
        // own scrollable block column (velotype's Editor::render). The legacy
        // source/live split and slash-command palette are gone; the editor has
        // native Markdown shortcuts (#, -, >, etc.), a right-click formatting
        // menu, and undo/redo.
        // Embed the editor exactly like MarkdownPanel:
        // `div().flex_1().min_h_0().child(editor)` — no stateful wrapper, no
        // extra w_full. (Previously a `div().id("notes-wysiwyg").w_full().h_full()`
        // wrapper sat between; it broke repainting of typed text.)
        let content = self.editor.clone();

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child({
                let mut header = h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .gap_2();
                if editing_active {
                    header = header.child(
                        Input::new(&self.title_input)
                            .w_full()
                            .small()
                            .appearance(false),
                    );
                } else {
                    let (title_display, current_title) = match &active_note {
                        Some(n) if !n.title.is_empty() => (n.title.clone(), n.title.clone()),
                        Some(_) => (rust_i18n::t!("notes.untitled").to_string(), String::new()),
                        None => (
                            rust_i18n::t!("notes.empty_editor").to_string(),
                            String::new(),
                        ),
                    };
                    let can_edit = active_note.is_some();
                    let nid = active_id.clone().unwrap_or_default();
                    let export_open = self.export_popover_open;
                    header = header
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .cursor_pointer()
                                .when(can_edit, |d| {
                                    d.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.start_title_edit(&nid, &current_title, window, cx);
                                        }),
                                    )
                                })
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .child(title_display),
                                ),
                        )
                        // Per-note export popover (MD/PDF).
                        .when(can_edit, |h| {
                            h.child(
                                Popover::new("notes-export-pop")
                                    .anchor(gpui::Anchor::BottomRight)
                                    .open(export_open)
                                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                                        this.export_popover_open = *open;
                                        cx.notify();
                                    }))
                                    .trigger(
                                        Button::new("notes-export-trig")
                                            .ghost()
                                            .xsmall()
                                            .icon({
                                                let mut icon = Icon::new(IconName::ExternalLink);
                                                icon = icon.path(crate::assets::EXPORT);
                                                icon
                                            })
                                            .tooltip(rust_i18n::t!("notes.export").to_string()),
                                    )
                                    .p(px(4.))
                                    .child(
                                        v_flex()
                                            .w(px(170.))
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(theme.muted))
                                                    .child(
                                                        div().text_size(px(12.)).child(
                                                            rust_i18n::t!("notes.export_md")
                                                                .to_string(),
                                                        ),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, window, cx| {
                                                            this.export_popover_open = false;
                                                            this.export_active_note_as(
                                                                "md", window, cx,
                                                            );
                                                        }),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(theme.muted))
                                                    .child(
                                                        div().text_size(px(12.)).child(
                                                            rust_i18n::t!("notes.export_pdf")
                                                                .to_string(),
                                                        ),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, window, cx| {
                                                            this.export_popover_open = false;
                                                            this.export_active_note_as(
                                                                "pdf", window, cx,
                                                            );
                                                        }),
                                                    ),
                                            ),
                                    ),
                            )
                        });
                }
                if let Some(c) = content_word_count {
                    header = header.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(rust_i18n::t!("notes.word_count", count = c).to_string()),
                    );
                }
                header
            })
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }
}

impl Render for NotesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Run any deferred slash command BEFORE rendering so the editor has the
        // If the tree selection changed without a Window context (observer),
        // reload the editor content from the active note here. The WYSIWYG
        // editor's `set_content` rebuilds the whole block tree.
        if self.editor_needs_reload {
            self.editor_needs_reload = false;
            if let Some(id) = self.store.active_note.clone() {
                if let Some(note) = self.store.notes.iter().find(|n| n.id == id) {
                    let content = note.content.clone();
                    // Rebuild the editor fresh (see `reload_editor`): a reused
                    // editor + `set_content` leaves macOS IME unbound.
                    self.reload_editor(&content, window, cx, true);
                }
            }
        }

        let theme = cx.theme().clone();

        // Build the editor element once; it goes INSIDE the `h_resizable`
        // (expanded) or beside the collapsed strip. Only one branch runs, so
        // the conditional move of `editor_el` is fine.
        let editor_el = self.render_editor(window, cx);
        let body = if !self.list_collapsed {
            // Resizable sidebar — drag handle on the right edge (h_resizable).
            // Both panels are children of h_resizable so the resize handle is
            // placed between them. Content is wrapped in plain divs so their
            // own flex classes don't interfere with the resizable layout.
            // The default width was loaded once at construction.
            let saved = self.sidebar_default_width;
            let state = self.sidebar_resize.clone();
            gpui_component::resizable::h_resizable("notes-sidebar")
                .with_state(&state)
                .on_resize(|state, _, cx| {
                    if let Some(w) = state.read(cx).sizes().first() {
                        let mut layout =
                            crate::state::persistence::load_layout().unwrap_or_default();
                        let mut arr = layout.side_widths.unwrap_or([260., 260., 260., 220.]);
                        arr[0] = w.as_f32();
                        layout.side_widths = Some(arr);
                        let _ = crate::state::persistence::save_layout(&layout);
                    }
                })
                .child(
                    gpui_component::resizable::resizable_panel()
                        .size(saved)
                        .size_range(gpui::px(180.)..gpui::px(520.))
                        .overflow_hidden()
                        .child(self.render_note_list(cx)),
                )
                .child(
                    gpui_component::resizable::resizable_panel()
                        .overflow_hidden()
                        .child(editor_el),
                )
                .into_any_element()
        } else {
            // Collapsed: a thin vertical strip with an expand button, then the
            // editor.
            h_flex()
                .w_full()
                .h_full()
                .child(
                    v_flex()
                        .w(px(28.))
                        .h_full()
                        .flex_shrink_0()
                        .border_r_1()
                        .border_color(theme.border)
                        .bg(theme.muted.opacity(0.3))
                        .items_center()
                        .pt_2()
                        .child(
                            Button::new("notes-expand-list")
                                .ghost()
                                .xsmall()
                                .icon(IconName::PanelLeftOpen)
                                .tooltip(rust_i18n::t!("notes.expand_list").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.list_collapsed = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(editor_el)
                .into_any_element()
        };

        let mut root = div()
            .w_full()
            .h_full()
            .relative()
            .bg(theme.background)
            .child(body);

        if let Some(id) = &self.pending_delete {
            root = root.child(self.render_delete_confirm(id, cx));
        }

        if let Some(job) = self.pending_media_delete.as_ref() {
            root = root.child(self.render_media_delete_confirm(job, cx));
        }

        root
    }
}

/// Replace path-unsafe characters in a filename with underscores.
fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    out.trim().trim_matches('.').to_string()
}

/// Surface an export failure to the user via a critical prompt dialog.
///
/// Runs on the async context (the save completes on a background thread) and
/// dispatches the blocking `window.prompt` back onto the window thread via
/// `update_window`, mirroring the WYSIWYG editor's `show_export_error`.
fn show_export_error(window_handle: gpui::AnyWindowHandle, detail: &str, cx: &mut gpui::AsyncApp) {
    let detail = detail.to_string();
    let _ = cx.update_window(
        window_handle,
        move |_view: gpui::AnyView, window: &mut gpui::Window, cx: &mut gpui::App| {
            let title = rust_i18n::t!("notes.export_failed").to_string();
            let ok = rust_i18n::t!("common.ok").to_string();
            let buttons = [ok.as_str()];
            let _ = window.prompt(
                gpui::PromptLevel::Critical,
                &title,
                Some(&detail),
                &buttons,
                cx,
            );
        },
    );
}

#[cfg(test)]
mod tests {
    // NOTE: do NOT `use super::*` here — the top of this file does `use gpui::*`,
    // whose re-exported `test` macro would shadow the built-in `#[test]`.
    use super::{build_content_snippet, find_ci, search_notes};
    use crate::notes::Note;

    fn note(id: &str, title: &str, content: &str) -> Note {
        Note {
            id: id.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            tags: Vec::new(),
            parent_id: None,
            pinned: false,
        }
    }

    #[test]
    fn find_ci_ascii_case_insensitive() {
        // Byte offsets match the original-case haystack.
        assert_eq!(find_ci("Hello World", "world"), Some((6, 11)));
        assert_eq!(find_ci("Hello World", "WORLD"), Some((6, 11)));
        assert_eq!(find_ci("Hello World", "xy"), None);
        assert_eq!(find_ci("abc", ""), None);
    }

    #[test]
    fn find_ci_cjk_returns_char_boundary_offsets() {
        // "笔记应用" — each CJK char is 3 bytes. "应用" starts at byte 6.
        let hay = "笔记应用";
        let Some((s, e)) = find_ci(hay, "应用") else {
            panic!("expected a match");
        };
        assert_eq!(s, 6);
        assert_eq!(e, 12);
        assert!(hay.is_char_boundary(s));
        assert!(hay.is_char_boundary(e));
    }

    #[test]
    fn find_ci_mixed_case_and_cjk() {
        let hay = "Rust 笔记";
        let r = find_ci(hay, "RUST");
        assert_eq!(r, Some((0, 4)));
        let r = find_ci(hay, "笔记");
        assert!(hay.is_char_boundary(r.unwrap().0));
    }

    #[test]
    fn build_snippet_centers_on_match_and_uses_char_boundaries() {
        // The match ("测试") sits well past 16 chars in, so the excerpt should
        // carry a leading ellipsis.
        let content = "这是一段很长的笔记内容前缀填充用来定位摘要的截取逻辑，然后测试是否正确。后面还有很多字符。";
        let (s, e) = find_ci(content, "测试").unwrap();
        let (snippet, hs, he) = build_content_snippet(content, s, e);
        // Highlight region must be valid char boundaries in the snippet.
        assert!(snippet.is_char_boundary(hs));
        assert!(snippet.is_char_boundary(he));
        // The highlighted slice must equal the query.
        assert_eq!(&snippet[hs..he], "测试");
        // Leading ellipsis because content starts before the window.
        assert!(snippet.starts_with('…'));
    }

    #[test]
    fn search_notes_ranks_title_first_and_caps_limit() {
        let notes = vec![
            note("1", "Rust 指南", "学习编程语言"), // content-only match for "编程"
            note("2", "编程入门", "基础内容"),      // title match for "编程"
            note("3", "其它", "无关内容"),          // no match
        ];
        let out = search_notes(&notes, "编程", 10);
        assert_eq!(out.len(), 2);
        // Title match first.
        assert_eq!(out[0].note_id, "2");
        assert!(out[0].title_match.is_some());
        // Content-only match second, with a snippet.
        assert_eq!(out[1].note_id, "1");
        assert!(out[1].title_match.is_none());
        assert!(out[1].snippet.is_some());
    }

    #[test]
    fn search_notes_respects_limit() {
        let mut notes = Vec::new();
        for i in 0..15 {
            notes.push(note(&format!("n{i}"), &format!("标题{i}"), "正文匹配词"));
        }
        let out = search_notes(&notes, "匹配词", 10);
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn search_notes_empty_query_returns_nothing() {
        let notes = vec![note("1", "title", "content")];
        assert!(search_notes(&notes, "", 10).is_empty());
        assert!(search_notes(&notes, "   ", 10).is_empty());
    }
}
