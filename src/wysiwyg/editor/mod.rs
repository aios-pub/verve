//! Top-level editor controller and window state.
//!
//! [`Editor`] owns window-level concerns such as view mode, save/close flow,
//! scroll state, and focus deferral. The runtime block tree itself lives in
//! [`DocumentTree`], which centralizes structural mutations and cached visible
//! order metadata.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::*;

use self::context_menu::{ContextMenuState, ImageInsertDialogState, TableInsertDialogState};
use self::find::FindState;
use self::toolbar::ToolbarState;
use self::tree::DocumentTree;
use crate::wysiwyg::components::{
    Block, BlockKind, BlockRecord, FootnoteDefinitionBinding, FootnoteReferenceLocation,
    FootnoteRegistry, FootnoteResolvedOccurrence, HtmlWrapperRuntime, HtmlWrapperShape,
    ImageReferenceDefinitions, InlineTextTree, LinkReferenceDefinitions,
    extract_editable_inline_wrapper, parse_image_reference_definitions,
    parse_link_reference_definitions,
};
use crate::wysiwyg::components::{
    TableAxisHighlight, TableAxisKind, TableAxisMarker, TableCellPosition, TableColumnAlignment,
    TableData, TableRuntime, UndoCaptureKind, serialize_table_cell_markdown,
};

mod close;
mod context_menu;
mod document;
mod events;
mod export;
mod file_drop;
mod find;
mod history;
mod html_wrapper_edit;
mod persistence;
mod render;
mod runtime_context;
mod selection;
mod source_mapping;
mod status_bar;
mod table_edit;
pub mod toolbar;
// velotype's editor integration tests (`tests.rs`) depend on gpui's
// `test-support` feature, which verve's zed-fork gpui does not expose. The
// file is copied but gated off until re-ported against verve's test harness
// (stage 5).
#[cfg(all(test, feature = "wysiwyg-gpui-tests"))]
mod tests;
mod tree;
mod update;
mod window_state;
mod workspace;

use self::status_bar::StatusBarState;
use self::workspace::WorkspaceState;

// Re-exported so hosts (e.g. the markdown side-view title bar) can count words
// without depending on the private `status_bar` module.
pub use self::status_bar::count_words;

/// Link navigation request deferred until a `Window` is available.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingOpenLink {
    pub(crate) prompt_target: String,
    pub(crate) open_target: String,
}

/// Top-level controller that owns editor-wide state and delegates tree
/// mutations to [`DocumentTree`].
///
/// The editor subscribes to every [`BlockEvent`](crate::wysiwyg::components::BlockEvent)
/// emitted by child blocks. Structural changes are handled centrally so focus,
/// scrolling, dirty tracking, and serialization stay synchronized.
pub struct Editor {
    document: DocumentTree,
    table_cells: HashMap<EntityId, TableCellBinding>,
    /// Side map of HTML-wrapper child blocks (only populated while a wrapper
    /// is focused). Key = child `EntityId`, value = binding to its wrapper
    /// parent + index. Mirrors `table_cells`.
    html_wrapper_children: HashMap<EntityId, HtmlWrapperChildBinding>,
    /// Which view the editor is currently presenting.
    pub(crate) view_mode: ViewMode,
    /// Deferred focus target applied during render when a [`Window`] is
    /// available.
    pending_focus: Option<EntityId>,
    /// When a wrapper HtmlBlock is first clicked, the click point is stashed
    /// here so that once the child-block runtime installs and the children
    /// render, the editor can focus the child nearest the click and place the
    /// caret at the exact click X offset instead of always the first child.
    /// Cleared once consumed.
    pending_wrapper_click_point: Option<Point<Pixels>>,
    active_entity_id: Option<EntityId>,
    pending_scroll_active_block_into_view: bool,
    pending_scroll_recheck_after_layout: bool,
    /// Bounded retry counter for the post-open body-measurement race (see
    /// `apply_pending_scroll_into_view`). Reset whenever a document is opened.
    scroll_layout_recheck_count: u32,
    pending_save: bool,
    pending_save_as: bool,
    pending_open_link: Option<PendingOpenLink>,
    pending_window_edited: bool,
    pending_window_title_refresh: bool,
    document_dirty: bool,
    file_path: Option<PathBuf>,
    /// Whether this editor owns its host window (standalone app mode). When
    /// true the editor mutates the window: close guard, `set_window_edited`,
    /// `set_window_title`, save-via-prompt, in-window menu bar. Embedded hosts
    /// (verve's notes panel, the markdown side-view) set this false. See
    /// [`Self::embedded`] / [`Self::side_view`].
    owns_window: bool,
    /// Whether to render the editor's own UI chrome: the status bar (word
    /// count, view-mode switch, cursor position) and the workspace drawer
    /// (outline / file tree). The standalone markdown side-view enables this;
    /// the notes panel (which has its own header) disables it.
    show_chrome: bool,
    scroll_handle: ScrollHandle,
    last_scroll_viewport_size: Option<Size<Pixels>>,
    /// Anchor recorded on a pointer click so the editor can hold the clicked
    /// block's on-screen position across the focus-driven layout change. HtmlBlock
    /// (and other source-raw blocks) shrink drastically when focused (rendered
    /// HTML with images → raw source text), which lowers the document's
    /// `max_offset`; GPUI then *clamps* the current scroll offset to the new
    /// smaller range, yanking the view up — the user reads this as "clicking
    /// the block scrolled me away." Since `focus_block_from_click` deliberately
    /// does not request a scroll-into-view, the only residual motion is this
    /// GPUI clamp; compensating it here keeps the click visually stable. The
    /// anchor is consumed once on the next render and cleared.
    click_scroll_anchor: Option<ClickScrollAnchor>,
    /// Last frame's visible block ids, to detect structural edits so the height
    /// cache is refreshed only when the row/block mapping is unchanged.
    prev_visible_block_ids: Vec<EntityId>,
    /// Per-row footprint (height plus trailing gap), keyed by the row's first
    /// block. Scroll-invariant, unlike raw painted positions, so windowing from
    /// their running sum stays correct as the document scrolls. Filled as rows
    /// paint; unknown rows use a minimum-height estimate.
    row_stride_cache: HashMap<EntityId, f32>,
    /// Per-row painted top in screen space, captured by a `canvas` wrapper on
    /// each row container during prepaint. This is authoritative for the row's
    /// actual rendered height (including rendered HtmlBlock images, which a
    /// block's own `last_bounds` does not reflect in its rendered form) and
    /// feeds `row_stride_cache` so windowing positions rows correctly.
    row_painted_tops: HashMap<EntityId, f32>,
    /// Row range mounted last frame; only those rows shared one scroll offset, so
    /// their adjacent-top differences are valid footprints for the cache.
    prev_render_window: Option<(usize, usize)>,
    close_guard_installed: bool,
    show_unsaved_changes_dialog: bool,
    /// When true, the window will close after the next successful save.
    pending_close_after_save: bool,
    /// Focus target to restore when the close dialog is dismissed.
    close_dialog_restore_focus: Option<EntityId>,
    pending_drop_replace_path: Option<PathBuf>,
    show_drop_replace_dialog: bool,
    pending_drop_replace_after_save: bool,
    drop_replace_restore_focus: Option<EntityId>,
    /// Optional informational dialog shown from the Help menu.
    info_dialog: Option<InfoDialogKind>,
    /// True while an online update check is running in the background.
    update_check_in_progress: bool,
    workspace: WorkspaceState,
    /// Resizable state for the workspace drawer (文件/大纲 sidebar) width.
    workspace_resize: Entity<gpui_component::resizable::ResizableState>,
    status_bar: StatusBarState,
    /// Top formatting toolbar (H1-3 / B I S U / Paragraph / Format dropdowns).
    toolbar: ToolbarState,
    context_menu: Option<ContextMenuState>,
    table_insert_dialog: Option<TableInsertDialogState>,
    image_insert_dialog: Option<ImageInsertDialogState>,
    /// Open in-editor find bar (Cmd/Ctrl+F). `None` when the bar is closed.
    find_state: Option<FindState>,
    context_menu_submenu_close_task: Option<Task<()>>,
    table_axis_preview: Option<TableAxisSelection>,
    table_axis_selection: Option<TableAxisSelection>,
    cross_block_selection: Option<CrossBlockSelection>,
    cross_block_drag: Option<CrossBlockDrag>,
    rendered_select_all_cycle: Option<RenderedSelectAllCycle>,
    /// Open top-level menu in the in-window fallback menu bar.
    menu_bar_open: Option<usize>,
    /// Open child submenu inside the in-window fallback menu panel.
    menu_submenu_open: Option<usize>,
    menu_bar_hovered: bool,
    menu_panel_hovered: bool,
    menu_submenu_panel_hovered: bool,
    /// Hover state for the invisible bridge spanning the gap between the menu
    /// panel and an open submenu. Tracked separately from
    /// `menu_submenu_panel_hovered` so the handoff between the two regions
    /// cannot clobber a single shared flag and tear the menu down.
    menu_submenu_bridge_hovered: bool,
    menu_close_task: Option<Task<()>>,
    scrollbar_hovered: bool,
    scrollbar_visible_until: Instant,
    scrollbar_fade_task: Option<Task<()>>,
    /// Forces a repaint shortly after a pending scroll-into-view that could
    /// not be satisfied yet (the target block has no measured bounds), so the
    /// scroll lands on the next frame instead of waiting for the cursor blink.
    scroll_recheck_task: Option<Task<()>>,
    scrollbar_drag: Option<ScrollbarDragSession>,
    undo_history: Vec<HistoryEntry>,
    redo_history: Vec<HistoryEntry>,
    pending_undo_capture: Option<PendingUndoCapture>,
    last_selection_snapshot: UndoSelectionSnapshot,
    last_stable_source_text: String,
    history_restore_in_progress: bool,
    image_reference_definitions: Arc<ImageReferenceDefinitions>,
    link_reference_definitions: Arc<LinkReferenceDefinitions>,
    footnote_registry: Arc<FootnoteRegistry>,
    /// In-flight video uploads keyed by the embedded local `src`
    /// (`./media/videos/...`), mapping to `(parts_done, parts_total)`. Pushed to
    /// blocks via runtime context so `<video>` placeholders render a progress bar.
    video_uploads: Arc<HashMap<String, (u32, u32)>>,
}

/// Runtime binding between a table block and one cell editor.
#[derive(Clone)]
struct TableCellBinding {
    table_block: Entity<Block>,
    cell: Entity<Block>,
    position: TableCellPosition,
}

/// Runtime binding between a focused HTML-wrapper block and one of its child
/// blocks (parsed from the wrapper's inner Markdown). Mirrors
/// [`TableCellBinding`]: the wrapper is a document-tree leaf, children live
/// out-of-band in this side map so focus routing + serialization can find them.
#[derive(Clone)]
pub(super) struct HtmlWrapperChildBinding {
    pub(super) wrapper_block: Entity<Block>,
    pub(super) child: Entity<Block>,
    pub(super) index: usize,
}

/// Selected row or column in a rendered native table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TableAxisSelection {
    table_block_id: EntityId,
    kind: TableAxisKind,
    index: usize,
}

/// Pixel geometry for the custom editor scrollbar.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarGeometry {
    track_height: f32,
    thumb_height: f32,
    thumb_top: f32,
    max_scroll_y: f32,
}

/// Windowing result: the run of rows to mount, plus the top/bottom spacer
/// heights standing in for the culled rows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RenderWindow {
    run_start: usize,
    run_end: usize,
    top_h: f32,
    bottom_h: f32,
}

/// Active drag session for the custom scrollbar thumb.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarDragSession {
    pointer_offset_y: f32,
    track_height: f32,
    thumb_height: f32,
    max_scroll_y: f32,
}

/// Records the scroll offset at click time so `apply_click_scroll_anchor` can
/// cancel the view jump GPUI's offset clamp introduces when a click focuses a
/// source-raw block (HtmlBlock, …) and shrinks the document (rendered HTML →
/// raw text), lowering `max_offset`. GPUI clamps the current offset to the new
/// smaller range on the next prepaint, yanking the view; restoring the
/// pre-click offset (clamped to the new range) keeps the click visually
/// stable.
///
/// `pending` gates a one-frame deferral: the anchor is recorded during the
/// click event, but the focus flip and GPUI's clamp both happen in the
/// *following* prepaint, so the compensation must run one render later. The
/// first render after the click sees `pending == true`, clears it, and defers;
/// the second render performs the restore.
#[derive(Clone, Copy, Debug)]
pub(super) struct ClickScrollAnchor {
    pre_offset_y: f32,
    pending: bool,
}

/// Source-mode selection snapshot stored with undo history.
#[derive(Clone, Debug, PartialEq, Eq)]
struct UndoSelectionSnapshot {
    range: std::ops::Range<usize>,
    reversed: bool,
}

/// One undo history entry containing source text and selection state.
#[derive(Clone, Debug)]
struct HistoryEntry {
    source_text: String,
    selection: UndoSelectionSnapshot,
    timestamp: Instant,
    kind: UndoCaptureKind,
}

/// Deferred undo capture used to coalesce adjacent typing edits.
#[derive(Clone, Debug)]
struct PendingUndoCapture {
    snapshot: HistoryEntry,
}

/// Cross-block selection endpoint in visible block order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CrossBlockSelectionEndpoint {
    pub(super) entity_id: EntityId,
    pub(super) offset: usize,
}

/// Editor-level selection spanning two visible block endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CrossBlockSelection {
    pub(super) anchor: CrossBlockSelectionEndpoint,
    pub(super) focus: CrossBlockSelectionEndpoint,
}

/// Drag state while creating or extending a cross-block selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CrossBlockDrag {
    pub(super) anchor: CrossBlockSelectionEndpoint,
}

/// Short-lived Ctrl/Cmd+A press counter for rendered-mode selection upgrade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RenderedSelectAllCycle {
    entity_id: EntityId,
    count: u8,
    last_pressed_at: Instant,
}

/// Mapping from one visible block's text range to canonical Markdown offsets.
#[derive(Clone)]
pub(super) struct SourceTargetMapping {
    entity: Entity<Block>,
    full_source_range: std::ops::Range<usize>,
    content_to_source: Vec<usize>,
    source_to_content: Vec<usize>,
}

/// The two editing views the editor can present.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Rich rendered view where each block is styled by its semantic kind.
    Rendered,
    /// Plain source view where the full Markdown document is edited as a
    /// single raw buffer.
    Source,
}

/// The informational dialogs that can be shown from the Help menu.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InfoDialogKind {
    /// Dialog describing update-check availability.
    CheckForUpdates,
    /// Dialog with app name and version information.
    About,
}

impl Editor {
    const HISTORY_LIMIT: usize = 200;
    const HISTORY_COALESCE_WINDOW: Duration = Duration::from_millis(1_000);
    const RENDERED_SELECT_ALL_CYCLE_WINDOW: Duration = Duration::from_millis(750);

    pub fn from_markdown(
        cx: &mut Context<Self>,
        markdown: String,
        file_path: Option<PathBuf>,
    ) -> Self {
        let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
        let mut roots = Self::build_root_blocks_from_markdown(cx, &normalized);
        if roots.is_empty() {
            roots.push(Self::new_block(cx, BlockRecord::paragraph(String::new())));
        }

        let mut document = DocumentTree::new(roots);
        document.rebuild_metadata_and_snapshot(cx);
        let pending_focus = document.first_root().map(|block| block.entity_id());

        let mut editor = Self {
            document,
            table_cells: HashMap::new(),
            html_wrapper_children: HashMap::new(),
            view_mode: ViewMode::Rendered,
            pending_focus,
            pending_wrapper_click_point: None,
            active_entity_id: pending_focus,
            pending_scroll_active_block_into_view: true,
            pending_scroll_recheck_after_layout: true,
            scroll_layout_recheck_count: 0,
            pending_save: false,
            pending_save_as: false,
            pending_open_link: None,
            pending_window_edited: false,
            pending_window_title_refresh: false,
            document_dirty: false,
            file_path,
            owns_window: false,
            show_chrome: false,
            scroll_handle: ScrollHandle::new(),
            last_scroll_viewport_size: None,
            click_scroll_anchor: None,
            prev_visible_block_ids: Vec::new(),
            row_stride_cache: HashMap::new(),
            row_painted_tops: HashMap::new(),
            prev_render_window: None,
            close_guard_installed: false,
            show_unsaved_changes_dialog: false,
            pending_close_after_save: false,
            close_dialog_restore_focus: None,
            pending_drop_replace_path: None,
            show_drop_replace_dialog: false,
            pending_drop_replace_after_save: false,
            drop_replace_restore_focus: None,
            info_dialog: None,
            update_check_in_progress: false,
            workspace: WorkspaceState::default(),
            workspace_resize: cx.new(|_| gpui_component::resizable::ResizableState::default()),
            status_bar: StatusBarState::default(),
            toolbar: ToolbarState::new(),
            context_menu: None,
            table_insert_dialog: None,
            image_insert_dialog: None,
            find_state: None,
            context_menu_submenu_close_task: None,
            table_axis_preview: None,
            table_axis_selection: None,
            cross_block_selection: None,
            cross_block_drag: None,
            rendered_select_all_cycle: None,
            menu_bar_open: None,
            menu_submenu_open: None,
            menu_bar_hovered: false,
            menu_panel_hovered: false,
            menu_submenu_panel_hovered: false,
            menu_submenu_bridge_hovered: false,
            menu_close_task: None,
            scrollbar_hovered: false,
            scrollbar_visible_until: Instant::now(),
            scrollbar_fade_task: None,
            scroll_recheck_task: None,
            scrollbar_drag: None,
            undo_history: Vec::new(),
            redo_history: Vec::new(),
            pending_undo_capture: None,
            last_selection_snapshot: Self::empty_selection_snapshot(),
            last_stable_source_text: normalized,
            history_restore_in_progress: false,
            image_reference_definitions: Arc::default(),
            link_reference_definitions: Arc::default(),
            footnote_registry: Arc::default(),
            video_uploads: Arc::new(HashMap::new()),
        };
        editor.rebuild_table_runtimes(cx);
        editor.rebuild_image_runtimes(cx);
        editor.pending_focus = editor.first_focusable_entity_id(cx);
        editor.active_entity_id = editor.pending_focus;
        editor.refresh_stable_document_snapshot(cx);
        editor
    }

    /// Embeds this editor inside another view that owns its own chrome and
    /// persistence (verve's notes panel). The editor neither mutates the host
    /// window nor renders its own status bar / workspace drawer — the host
    /// provides the surrounding UI.
    pub fn embedded(mut self) -> Self {
        self.owns_window = false;
        self.show_chrome = false;
        self
    }

    /// Embeds this editor as a self-contained side-view (verve's standalone
    /// markdown editor). The editor does NOT mutate the host window (no close
    /// guard, no window-title/edited sync) but DOES render its own status bar
    /// (word count, view-mode switch) and supports the workspace drawer
    /// (document outline / file tree). The host drives save/open/export via
    /// the public Editor API.
    pub fn side_view(mut self) -> Self {
        self.owns_window = false;
        self.show_chrome = true;
        self
    }

    // -----------------------------------------------------------------
    // Embedded-integration bridge API (verve stage 3)
    // -----------------------------------------------------------------
    //
    // verve embeds the editor inside its notes panel and persists content as a
    // plain Markdown string in `Note.content`. These two methods are the only
    // surface the panel needs: construct via `from_markdown`, read back via
    // `markdown`, and swap documents via `set_content`.

    /// Serializes the current document back to canonical Markdown.
    ///
    /// This is the read path verve's notes panel calls (debounced) to persist
    /// edits into `Note.content`. Honors the active view mode (Rendered emits
    /// canonical Markdown; Source emits the raw visible text).
    pub fn markdown(&self, cx: &App) -> String {
        self.serialized_document_text(cx)
    }

    /// Replaces the entire document with `markdown`, resetting the block tree,
    /// undo/redo history, and selection.
    ///
    /// verve calls this when the user switches to a different note so the same
    /// embedded `Entity<Editor>` shows the new content.
    pub fn set_content(&mut self, markdown: &str, cx: &mut Context<Self>) {
        let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
        let mut roots = Self::build_root_blocks_from_markdown(cx, &normalized);
        if roots.is_empty() {
            roots.push(Self::new_block(cx, BlockRecord::paragraph(String::new())));
        }

        self.document = DocumentTree::new(roots);
        self.document.rebuild_metadata_and_snapshot(cx);
        self.view_mode = ViewMode::Rendered;
        self.cross_block_selection = None;
        self.cross_block_drag = None;
        self.rendered_select_all_cycle = None;
        self.context_menu = None;
        self.table_insert_dialog = None;
        // The note switch rebuilds every block with fresh entity ids, so the
        // previous note's scroll offset and windowing caches are now stale.
        // Reset them (mirroring `from_markdown`) so the first focusable block
        // starts at the top of the viewport and is painted on the next frame.
        // Otherwise `apply_pending_focus` may target a block outside the
        // windowed range whose element never mounts, the single-shot focus
        // never lands, and the editor is left without keyboard focus — so
        // typing is dead until the body is clicked.
        self.scroll_handle = ScrollHandle::new();
        self.last_scroll_viewport_size = None;
        self.click_scroll_anchor = None;
        self.prev_visible_block_ids.clear();
        self.row_stride_cache.clear();
        self.row_painted_tops.clear();
        self.prev_render_window = None;
        self.undo_history.clear();
        self.redo_history.clear();
        self.pending_undo_capture = None;
        self.last_stable_source_text = normalized;
        self.history_restore_in_progress = false;

        self.rebuild_table_runtimes(cx);
        self.clear_html_wrapper_children();
        self.rebuild_image_runtimes(cx);
        self.pending_focus = self.first_focusable_entity_id(cx);
        self.active_entity_id = self.pending_focus;
        self.pending_scroll_active_block_into_view = true;
        self.pending_scroll_recheck_after_layout = true;
        self.scroll_layout_recheck_count = 0;
        self.refresh_stable_document_snapshot(cx);

        // Focus the first block so typing lands in the right place. The actual
        // focus is applied during the next render via `apply_pending_focus`.
        cx.notify();
    }

    /// Returns the active view mode (Rendered WYSIWYG or raw Source).
    pub fn view_mode(&self) -> ViewMode {
        self.view_mode
    }

    /// Drop any pending focus request. Hosts use this when they focus
    /// something else (e.g. a title input) right after reloading content, so
    /// `apply_pending_focus` does not steal focus back to the editor body on
    /// the next render.
    pub fn clear_pending_focus(&mut self) {
        self.pending_focus = None;
    }

    /// Whether the document has unsaved edits. Hosts (e.g. the markdown
    /// side-view title bar) read this to show a dirty marker.
    pub fn document_dirty(&self) -> bool {
        self.document_dirty
    }

    /// The backing file path, if any. `None` for an untitled document. Hosts
    /// read this to decide Save vs Save-As and to display the filename.
    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    /// The document title for display: the file stem, or "Untitled" when there
    /// is no path. Hosts use this for their title bar.
    pub fn display_title(&self) -> &str {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
    }

    /// Resets to a blank untitled document (file_path cleared, dirty cleared,
    /// undo history cleared). Used by the markdown side-view's "New" button.
    pub fn reset_to_blank(&mut self, cx: &mut Context<Self>) {
        self.replace_document_from_markdown(String::new(), None, cx);
    }

    /// Opens a Markdown file: reads it from disk and replaces the document,
    /// recording it in the recent-files history. Used by the markdown
    /// side-view's "Open" button and recent-files list.
    pub fn open_file(&mut self, path: &Path, cx: &mut Context<Self>) -> anyhow::Result<()> {
        self.replace_document_from_path(path, cx)
    }

    /// Whether the workspace drawer (outline / file tree) is currently open.
    pub fn workspace_open(&self) -> bool {
        self.workspace.is_open
    }
}
