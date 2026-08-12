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
use std::time::Instant;

/// Ignore watcher events arriving within this window after our own write,
/// since `notify` surfaces our own saves too.
const NOTES_SELF_SAVE_FILTER_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

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
    save_timer: Option<Task<()>>,
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
    /// Per-row open "..." menu, by node id (e.g. "note:<uuid>").
    row_menu_open: Option<String>,
    /// FS watcher for the notes directory — picks up external edits so the
    /// sidebar reflects files changed/added/removed outside the app.
    watcher: Option<RecommendedWatcher>,
    watcher_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    /// Time of the last save initiated by this panel; used to ignore the
    /// watcher events triggered by our own writes.
    last_self_save: Option<Instant>,
}

fn note_node(id: &str) -> String {
    format!("note:{id}")
}
fn parse_note_id(node: &str) -> Option<&str> {
    node.strip_prefix("note:")
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
            let mut input = InputState::new(window, cx);
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
        let editor_sub = cx.observe(&editor, |this, _editor, cx| {
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
            save_timer: None,
            _subs: vec![editor_sub, search_sub, title_sub, tree_sub],
            editor_needs_reload: false,
            export_popover_open: false,
            list_collapsed: crate::state::persistence::load_layout()
                .map(|l| l.notes_collapsed)
                .unwrap_or(false),
            row_menu_open: None,
            watcher,
            watcher_rx: wrx,
            last_self_save: None,
        };
        panel.rebuild_tree(cx);
        panel.start_external_change_loop(cx);
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
        if self.editing_title_id.is_some() {
            self.commit_title_edit(cx);
        }
        // Collect subtree ids before the cascade removes them from memory.
        let to_remove = notes::note_ids_in_subtree(&self.store, id);
        delete_note_cascade(&mut self.store, id);
        crate::state::persistence::delete_notes_files(&to_remove, &mut self.store.file_paths);
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
        // The autosave observer is bound to the (now-dropped) old editor entity;
        // re-subscribe to the new one. It's the first entry in `_subs`.
        let editor_sub = cx.observe(&self.editor, |this, _editor, cx| {
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
        let window_handle: gpui::AnyWindowHandle = window.window_handle().into();

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
        let window_handle: gpui::AnyWindowHandle = window.window_handle().into();

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

    fn render_note_list(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let view = cx.entity().clone();

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
            .child(
                div()
                    .id("notes-list-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .child(
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
                        .into_any_element(),
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

        let msg = if descendant_count > 0 {
            rust_i18n::t!(
                "notes.confirm_delete_cascade",
                title = note_title.as_str(),
                count = descendant_count
            )
            .to_string()
        } else {
            rust_i18n::t!("notes.confirm_delete_msg", title = note_title.as_str()).to_string()
        };
        let id = note_id.to_string();

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
                            .child(rust_i18n::t!("notes.confirm_delete").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child(msg),
                    )
                    .child(
                        h_flex()
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
                                        this.delete_note(&id, window, cx);
                                    })),
                            ),
                    ),
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
            let saved = crate::state::persistence::load_layout()
                .and_then(|l| l.side_widths)
                .and_then(|w| w.first().copied())
                .filter(|&w| w > 0.0)
                .map(gpui::px)
                .unwrap_or_else(|| gpui::px(260.));
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
