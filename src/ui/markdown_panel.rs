//! Standalone Markdown file editor side-view with multi-tab support.
//!
//! Hosts the WYSIWYG block editor (ported from velotype) as a self-contained
//! panel that edits `.md` files on disk. Unlike the notes panel (which stores
//! content in verve's notes DB and uses `.embedded()`), this panel uses
//! [`Editor::side_view`] so each tab's editor renders its own status bar (word
//! count, view-mode switch) and supports the workspace drawer (outline), while
//! the panel's title bar (rendered in `app.rs`) drives Open / Save / Save-As /
//! Export / Recent-files.
//!
//! Each open document lives in its own tab with an independent `Entity<Editor>`
//! (block tree, undo stack, file path), so switching tabs preserves per-doc
//! state. The panel is constructed once and kept alive on `VerveApp`, so
//! switching to another activity-rail view and back preserves everything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme, IconName, Sizable as _, h_flex, v_flex};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::state::AppState;
use crate::wysiwyg::editor::Editor;
use crate::wysiwyg::editor::ViewMode;
use crate::wysiwyg::export::ExportFormat;

/// Grace window after the panel writes a file itself: `notify` events arriving
/// within this interval are treated as our own write, not an external change.
const SELF_SAVE_FILTER_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);

/// One open document: its own editor + backing file path.
struct MarkdownTab {
    editor: Entity<Editor>,
    file_path: Option<PathBuf>,
    /// On-disk mtime the editor last synced against (read at open and after
    /// every reload / save). Comparing the live mtime against this lets us
    /// detect external modifications and de-duplicate duplicate `notify` events.
    disk_mtime: Option<SystemTime>,
    /// `Instant` recorded right after the panel saves this tab to disk, so the
    /// resulting `notify` event can be filtered out as our own write.
    last_self_save_time: Option<Instant>,
    /// Set when an external change is detected while the buffer is dirty. The
    /// panel renders a reload prompt over the editor until the user picks an
    /// option (overwrite / keep / cancel).
    pending_external_change: bool,
}

impl MarkdownTab {
    fn label(&self, cx: &App) -> String {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .map(String::from)
            .unwrap_or_else(|| "Untitled".to_string())
    }
}

pub struct MarkdownPanel {
    #[allow(dead_code)]
    state: Entity<AppState>,
    tabs: Vec<MarkdownTab>,
    active: usize,
    /// Cached recent-files list for the Open popover.
    recent: Vec<PathBuf>,
    /// Whether the Open popover is showing.
    open_popover: bool,
    /// Whether the Export popover is showing.
    export_popover: bool,
    /// A tab awaiting the unsaved-changes confirm dialog before closing
    /// (cmd-w on a dirty tab). Holds the tab index.
    pending_close_tab: Option<usize>,
    /// Cross-platform fs watcher that notifies the panel when any open file's
    /// parent directory changes. One watcher covers all tabs; the panel updates
    /// which directories it watches as tabs open and close.
    ///
    /// `Option` because `notify` init can fail (rare) and the panel must still
    /// be usable without live-change detection. When `None`, the feature is
    /// simply disabled and the editor behaves as before.
    watcher: Option<RecommendedWatcher>,
    /// Receiver fed by `watcher`. Drained by a background task spawned in
    /// [`Self::new`]; lives as long as the panel.
    watcher_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    /// Parent directories currently being watched. Tracked so we don't call
    /// `watch()` on a directory twice and can `unwatch()` only when no tab
    /// references it any more.
    watched_dirs: HashSet<PathBuf>,
    _subs: Vec<Subscription>,
}

impl MarkdownPanel {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // One fs watcher for the whole panel. notify init failing is extremely
        // rare; when it does, we keep the editor fully usable and simply lose
        // external-change detection (logs once, never panics).
        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
        let watcher = match notify::recommended_watcher(tx) {
            Ok(w) => Some(w),
            Err(err) => {
                eprintln!(
                    "markdown_panel: fs watcher unavailable, external-change detection disabled: {err}"
                );
                None
            }
        };

        let mut panel = Self {
            state,
            tabs: Vec::new(),
            active: 0,
            recent: recent_files(),
            open_popover: false,
            export_popover: false,
            pending_close_tab: None,
            watcher,
            watcher_rx: rx,
            watched_dirs: HashSet::new(),
            _subs: Vec::new(),
        };
        // Start with one blank untitled tab.
        panel.new_document(cx);
        // Drain the watcher's event queue in the background; convert each
        // relevant event into either a silent reload (clean buffer) or a
        // pending prompt (dirty buffer). Exits automatically when the panel
        // is dropped (`this.update` fails). Mirrors the pattern in
        // `VerveApp::start_pending_file_watcher` (app.rs).
        panel.start_external_change_loop(cx);
        panel
    }

    // -- active-tab accessor ------------------------------------------------

    /// The editor for the currently active tab (panics if there are no tabs —
    /// the panel always keeps at least one).
    pub fn editor(&self) -> &Entity<Editor> {
        &self.tabs[self.active].editor
    }

    fn active_tab(&self) -> &MarkdownTab {
        &self.tabs[self.active]
    }

    fn observe_active(&mut self, cx: &mut Context<Self>) {
        // Re-render (title bar dirty marker / filename) whenever the active
        // editor notifies. We re-subscribe on every tab switch.
        let editor = self.editor().clone();
        let sub = cx.observe(&editor, |_, _editor, cx| {
            cx.notify();
        });
        self._subs = vec![sub];
    }

    // -- fs watcher plumbing ------------------------------------------------

    /// Ensure the watcher is observing `dir` (a file's parent directory).
    /// Idempotent — tracks watched dirs so repeated tabs in the same dir don't
    /// double-watch.
    fn watch_dir(&mut self, dir: &Path) {
        if !self.watched_dirs.insert(dir.to_path_buf()) {
            return; // already watching
        }
        if let Some(w) = self.watcher.as_mut() {
            if let Err(err) = w.watch(dir, RecursiveMode::NonRecursive) {
                eprintln!("markdown_panel: failed to watch '{}': {err}", dir.display());
                // Roll back the bookkeeping so a later retry can re-attempt.
                self.watched_dirs.remove(dir);
            }
        }
    }

    /// `unwatch` every parent directory no longer referenced by any open tab.
    /// Called after a tab closes or a save-as moves a file to a new directory.
    fn prune_watched_dirs(&mut self) {
        if self.watcher.is_none() {
            self.watched_dirs.clear();
            return;
        }
        let live: HashSet<PathBuf> = self
            .tabs
            .iter()
            .filter_map(|t| t.file_path.as_ref())
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        let stale: Vec<PathBuf> = self
            .watched_dirs
            .iter()
            .filter(|d| !live.contains(*d))
            .cloned()
            .collect();
        for dir in &stale {
            // unwatch errors are harmless (path already gone / not watched).
            if let Some(w) = self.watcher.as_mut() {
                let _ = w.unwatch(dir);
            }
            self.watched_dirs.remove(dir);
        }
    }

    /// Read the current on-disk mtime for `path`, if the file still exists.
    fn read_mtime(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }

    /// Background loop draining `watcher_rx`. Each relevant event becomes
    /// either a silent reload (clean buffer) or a pending prompt (dirty
    /// buffer). Stops automatically when the panel entity is dropped.
    fn start_external_change_loop(&mut self, cx: &mut Context<Self>) {
        // No point spinning a poll loop if there's no watcher.
        if self.watcher.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                // Poll the channel, then sleep briefly so we're not busy-looping
                // but still feel near-realtime (~50ms).
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
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

    /// Drain all currently-queued watcher events, processing the last relevant
    /// state for each affected tab. Called on a timer from the background loop.
    fn drain_watcher_events(&mut self, cx: &mut Context<Self>) {
        use notify::EventKind;
        let mut events = Vec::new();
        while let Ok(item) = self.watcher_rx.try_recv() {
            events.push(item);
        }
        if events.is_empty() {
            return;
        }

        let now = Instant::now();
        // Collect the set of file paths referenced by any event we care about
        // (modify/create/move-into; ignore access/remove which need separate UX).
        let mut changed_paths: Vec<PathBuf> = Vec::new();
        for item in events {
            let Ok(ev) = item else {
                continue;
            };
            match ev.kind {
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Any => {}
                _ => continue,
            }
            for p in ev.paths {
                changed_paths.push(p);
            }
        }
        if changed_paths.is_empty() {
            return;
        }

        // Process each tab against the changed-path set. Collect indices first
        // because we mutate self inside the loop.
        let candidates: Vec<(usize, PathBuf)> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let fp = t.file_path.as_ref()?;
                if changed_paths.iter().any(|c| c == fp) {
                    Some((i, fp.clone()))
                } else {
                    None
                }
            })
            .collect();

        let mut any_prompt = false;
        let mut any_silent = false;
        for (idx, path) in candidates {
            // Filter our own recent save (see SELF_SAVE_FILTER_WINDOW).
            let recent_self_save = self.tabs[idx]
                .last_self_save_time
                .is_some_and(|t| now.duration_since(t) < SELF_SAVE_FILTER_WINDOW);
            // Always refresh the recorded mtime from disk so the next genuine
            // external change is detectable.
            let live_mtime = Self::read_mtime(&path);
            if recent_self_save {
                self.tabs[idx].disk_mtime = live_mtime;
                continue;
            }

            // De-duplicate: skip if mtime unchanged since last sync.
            let changed = match (live_mtime, self.tabs[idx].disk_mtime) {
                (Some(live), Some(known)) => live != known,
                (Some(_), None) => true,
                (None, _) => {
                    // File vanished — out of scope for now (future: deletion UX).
                    eprintln!(
                        "markdown_panel: watched file no longer exists: {}",
                        path.display()
                    );
                    continue;
                }
            };
            if !changed {
                continue;
            }

            let dirty = self.tabs[idx].editor.read(cx).document_dirty();
            if dirty {
                self.tabs[idx].pending_external_change = true;
                any_prompt = true;
            } else {
                // Clean buffer: reload silently.
                self.reload_tab_from_disk(idx, cx);
                any_silent = true;
            }
        }

        if any_prompt || any_silent {
            cx.notify();
        }
    }

    /// Reload tab `idx` from its on-disk file (replaces content, resets dirty
    /// + undo history via `Editor::open_file` → `replace_document_from_markdown`).
    fn reload_tab_from_disk(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(path) = self.tabs[idx].file_path.clone() else {
            self.tabs[idx].pending_external_change = false;
            return;
        };
        if let Err(err) = self.tabs[idx]
            .editor
            .update(cx, |e, cx| e.open_file(&path, cx))
        {
            eprintln!(
                "markdown_panel: failed to reload '{}': {err}",
                path.display()
            );
        }
        self.tabs[idx].disk_mtime = Self::read_mtime(&path);
        self.tabs[idx].pending_external_change = false;
    }

    // -- external-change dialog actions -------------------------------------

    /// Dialog "用磁盘覆盖": discard local edits, reload from disk.
    fn confirm_external_reload(&mut self, cx: &mut Context<Self>) {
        if self.tabs[self.active].pending_external_change {
            self.reload_tab_from_disk(self.active, cx);
        }
        cx.notify();
    }

    /// Dialog "保留我的修改" / "取消": keep the buffer as-is, dismiss the prompt.
    fn dismiss_external_change_dialog(&mut self, cx: &mut Context<Self>) {
        if self.tabs.get_mut(self.active).is_some() {
            self.tabs[self.active].pending_external_change = false;
        }
        cx.notify();
    }

    // -- file actions (driven from the title bar; operate on active tab) ----

    /// "New" — open a fresh blank untitled tab and focus it.
    pub fn new_document(&mut self, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, String::new(), None).side_view());
        self.tabs.push(MarkdownTab {
            editor,
            file_path: None,
            disk_mtime: None,
            last_self_save_time: None,
            pending_external_change: false,
        });
        self.active = self.tabs.len() - 1;
        self.observe_active(cx);
        cx.notify();
    }

    /// "Open" — show a native file picker, then load the chosen file.
    pub fn open_file_dialog(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .add_filter("Markdown", &["md", "markdown", "txt"])
                .pick_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let _ = weak.update(cx, |panel, cx| panel.open_path(&path, cx));
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

    /// Open a specific path (from the picker or the recent-files list). If the
    /// file is already open in a tab, just focus that tab.
    pub fn open_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        // Focus existing tab if this file is already open.
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.file_path.as_deref() == Some(path))
        {
            self.set_active_tab(idx, cx);
            return;
        }

        let mtime = Self::read_mtime(path);
        // If the active tab is a fresh blank untitled doc, replace it in place
        // rather than opening yet another tab.
        let active_is_blank = self.active_tab().file_path.is_none()
            && self.active_tab().editor.read(cx).markdown(cx).is_empty();
        if active_is_blank {
            let result = self.tabs[self.active]
                .editor
                .update(cx, |e, cx| e.open_file(path, cx));
            if let Err(err) = result {
                eprintln!("failed to open markdown file '{}': {err}", path.display());
            }
            self.tabs[self.active].file_path = Some(path.to_path_buf());
            self.tabs[self.active].disk_mtime = mtime;
            self.tabs[self.active].pending_external_change = false;
        } else {
            let editor = cx.new(|cx| {
                let mut e = Editor::from_markdown(cx, String::new(), None).side_view();
                // Load content into the new editor.
                if let Ok(content) = std::fs::read_to_string(path) {
                    e.set_content(&content, cx);
                }
                e
            });
            // Re-attach the path after construction (from_markdown's path arg
            // is for window-title in standalone mode; we set it via the tab).
            self.tabs.push(MarkdownTab {
                editor,
                file_path: Some(path.to_path_buf()),
                disk_mtime: mtime,
                last_self_save_time: None,
                pending_external_change: false,
            });
            self.active = self.tabs.len() - 1;
            self.observe_active(cx);
        }
        // Watch the file's parent directory so external edits are detected.
        if let Some(parent) = path.parent() {
            self.watch_dir(parent);
        }
        self.refresh_recent();
        cx.notify();
    }

    /// "Save" — save the active tab to its path, or prompt if untitled.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor()
            .update(cx, |e, cx| e.save_document(window, cx));
        self.sync_active_save(cx);
        self.refresh_recent();
        cx.notify();
    }

    /// "Save As" — always prompt for a new path.
    pub fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prev_path = self.tabs[self.active].file_path.clone();
        self.editor()
            .update(cx, |e, cx| e.save_document_as(window, cx));
        self.sync_active_save(cx);
        // Save-As may have written to a new directory: watch the new parent and
        // stop watching the old one if no other tab keeps it alive. Clone out
        // of `self` first so we don't hold an immutable borrow across
        // `watch_dir`/`prune_watched_dirs`.
        let new_parent = self.tabs[self.active]
            .file_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf);
        if let Some(parent) = new_parent {
            self.watch_dir(&parent);
        }
        let _ = prev_path; // prune_watched_dirs below handles unwatch.
        self.prune_watched_dirs();
        self.refresh_recent();
        cx.notify();
    }

    /// "Export" → HTML.
    pub fn export_html(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor().update(cx, |e, cx| {
            e.export_document_via_prompt(ExportFormat::Html, window, cx)
        });
        self.export_popover = false;
        cx.notify();
    }

    /// "Export" → PDF.
    pub fn export_pdf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor().update(cx, |e, cx| {
            e.export_document_via_prompt(ExportFormat::Pdf, window, cx)
        });
        self.export_popover = false;
        cx.notify();
    }

    /// Toggle the outline / file-tree drawer on the active editor.
    pub fn toggle_outline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor()
            .update(cx, |e, cx| e.toggle_workspace_drawer(window, cx));
        cx.notify();
    }

    /// Close the tab at `index`. If it has unsaved edits, this currently
    /// discards them (a save prompt can be added later).
    pub fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        self.prune_watched_dirs();
        if self.tabs.is_empty() {
            // Always keep at least one (blank) tab.
            self.new_document(cx);
            return;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if self.active > index {
            self.active -= 1;
        }
        self.observe_active(cx);
        cx.notify();
    }

    /// cmd-w — close the active tab. If it has unsaved changes, show a
    /// confirm dialog (save / discard / cancel); otherwise close immediately.
    pub fn close_active_tab(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.active;
        if idx < self.tabs.len() && self.tabs[idx].editor.read(cx).document_dirty() {
            self.pending_close_tab = Some(idx);
            cx.notify();
        } else if idx < self.tabs.len() {
            self.close_tab(idx, cx);
        }
    }

    /// Dialog "Save": save the pending tab, then close it.
    fn confirm_close_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(idx) = self.pending_close_tab.take() {
            // `pending_close_tab` was the active tab when cmd-w fired and the
            // modal blocks tab switching, so `save()` (which targets the active
            // editor) hits the right tab.
            self.save(window, cx);
            if idx < self.tabs.len() {
                self.close_tab(idx, cx);
            }
        }
    }

    /// Dialog "Don't Save": close the pending tab without saving.
    fn confirm_close_discard(&mut self, cx: &mut Context<Self>) {
        if let Some(idx) = self.pending_close_tab.take()
            && idx < self.tabs.len()
        {
            self.close_tab(idx, cx);
        }
    }

    /// Dialog "Cancel": dismiss, keep the tab open.
    fn cancel_pending_close(&mut self, cx: &mut Context<Self>) {
        self.pending_close_tab = None;
        cx.notify();
    }

    /// The unsaved-changes modal overlay shown when `pending_close_tab` is set.
    fn render_close_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.background.opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .p_4()
                    .gap_3()
                    .w(px(360.))
                    .child(
                        div()
                            .text_color(theme.foreground)
                            .child("此文档有未保存的更改，是否保存？"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("md-close-save")
                                    .primary()
                                    .small()
                                    .label("保存")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_close_save(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("md-close-discard")
                                    .ghost()
                                    .small()
                                    .label("不保存")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_close_discard(cx);
                                    })),
                            )
                            .child(
                                Button::new("md-close-cancel")
                                    .ghost()
                                    .small()
                                    .label("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_pending_close(cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The "file changed on disk" modal overlay shown when the active tab's
    /// `pending_external_change` flag is set (an external edit arrived while the
    /// buffer was dirty). Layout mirrors `render_close_dialog`.
    fn render_external_change_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let filename = self
            .active_tab()
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let message = if filename.is_empty() {
            "此文件已被外部程序修改，是否重新加载磁盘版本？".to_string()
        } else {
            format!(
                "“{filename}” 已被外部程序修改，是否重新加载磁盘版本？\n未保存的本地更改将被丢弃。"
            )
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.background.opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .p_4()
                    .gap_3()
                    .w(px(420.))
                    .child(
                        div()
                            .text_color(theme.foreground)
                            .text_size(px(13.))
                            .child(message),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("md-extchg-reload")
                                    .primary()
                                    .small()
                                    .label("用磁盘覆盖")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_external_reload(cx);
                                    })),
                            )
                            .child(
                                Button::new("md-extchg-keep")
                                    .ghost()
                                    .small()
                                    .label("保留我的修改")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dismiss_external_change_dialog(cx);
                                    })),
                            )
                            .child(
                                Button::new("md-extchg-cancel")
                                    .ghost()
                                    .small()
                                    .label("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dismiss_external_change_dialog(cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Switch the active tab.
    pub fn set_active_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() && self.active != index {
            self.active = index;
            self.observe_active(cx);
            cx.notify();
        }
    }

    // -- title-bar state (read by app.rs) -----------------------------------

    pub fn dirty(&self, cx: &App) -> bool {
        self.active_tab().editor.read(cx).document_dirty()
    }

    pub fn title(&self, cx: &App) -> String {
        self.active_tab()
            .editor
            .read(cx)
            .display_title()
            .to_string()
    }

    pub fn file_path(&self, cx: &App) -> Option<String> {
        self.active_tab()
            .editor
            .read(cx)
            .file_path()
            .map(|p| p.display().to_string())
    }

    pub fn outline_open(&self, cx: &App) -> bool {
        self.active_tab().editor.read(cx).workspace_open()
    }

    /// The active tab's view mode (Rendered WYSIWYG or raw Source). Read by the
    /// title bar to render the mode-switch button.
    pub fn view_mode(&self, cx: &App) -> ViewMode {
        self.active_tab().editor.read(cx).view_mode()
    }

    /// `(total, selection)` word counts for the active tab's document. The
    /// selection count is `Some(n)` only when text is selected. Read by the
    /// title bar to render the word-count label.
    pub fn word_count(&self, cx: &App) -> (usize, Option<usize>) {
        // Delegate to the editor's throttled cache so scrolling (which rebuilds
        // this panel every tick) doesn't re-serialize the whole document 60×/sec.
        self.active_tab().editor.read(cx).word_count(cx)
    }

    /// Toggle the active tab between Rendered and Source view (UI entry point).
    pub fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        self.editor().clone().update(cx, |e, cx| {
            e.toggle_view_mode_from_ui(cx);
        });
        cx.notify();
    }

    pub fn tab_labels(&self, cx: &App) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, t)| (t.label(cx), t.editor.read(cx).document_dirty()))
            .collect()
    }

    pub fn active_tab_index(&self) -> usize {
        self.active
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn recent_files(&self) -> &[PathBuf] {
        &self.recent
    }

    pub fn open_popover_open(&self) -> bool {
        self.open_popover
    }

    pub fn set_open_popover(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open_popover != open {
            self.open_popover = open;
            if open {
                self.refresh_recent();
            }
            cx.notify();
        }
    }

    pub fn export_popover_open(&self) -> bool {
        self.export_popover
    }

    pub fn set_export_popover(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.export_popover != open {
            self.export_popover = open;
            cx.notify();
        }
    }

    /// After a save / save-as, mirror the editor's resolved path back onto the
    /// tab, refresh the on-disk mtime snapshot, and stamp the self-save time so
    /// the resulting `notify` event is treated as our own write.
    fn sync_active_save(&mut self, cx: &App) {
        if let Some(path) = self.active_tab().editor.read(cx).file_path() {
            let p = path.to_path_buf();
            self.tabs[self.active].disk_mtime = Self::read_mtime(&p);
            self.tabs[self.active].last_self_save_time = Some(Instant::now());
            self.tabs[self.active].pending_external_change = false;
            self.tabs[self.active].file_path = Some(p);
        }
    }

    fn refresh_recent(&mut self) {
        self.recent = recent_files();
    }

    // -- tab bar rendering --------------------------------------------------

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.active;
        let labels = self.tab_labels(cx);
        let has_tabs = !labels.is_empty();

        h_flex()
            .w_full()
            .h(px(34.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .when(has_tabs, |bar| {
                bar.children(labels.iter().enumerate().map(|(i, (label, dirty))| {
                    let is_active = i == active;
                    let bg = if is_active {
                        theme.background
                    } else {
                        gpui::transparent_black()
                    };
                    h_flex()
                        .id(gpui::ElementId::Name(format!("md-tab-{i}").into()))
                        .h_full()
                        .px(px(10.))
                        .gap(px(6.))
                        .items_center()
                        .border_r_1()
                        .border_color(theme.border)
                        .bg(bg)
                        .cursor_pointer()
                        .hover(|s| {
                            s.bg(if is_active {
                                theme.background
                            } else {
                                theme.muted
                            })
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.set_active_tab(i, cx);
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(if is_active {
                                    theme.foreground
                                } else {
                                    theme.muted_foreground
                                })
                                .max_w(px(160.))
                                .truncate()
                                .child(label.clone()),
                        )
                        .when(*dirty, |d| {
                            d.child(div().text_color(theme.primary).child("●"))
                        })
                        .child(
                            Button::new(("md-tab-close", i))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_tab(i, cx);
                                })),
                        )
                }))
            })
    }
}

impl Render for MarkdownPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.editor().clone();
        // Build modal overlays first (if any) so they can be attached as
        // absolute overlays without re-borrowing self.
        let close_dialog = self
            .pending_close_tab
            .is_some()
            .then(|| self.render_close_dialog(cx));
        let extchg_dialog = self
            .active_tab()
            .pending_external_change
            .then(|| self.render_external_change_dialog(cx));
        div()
            .id("markdown-panel-root")
            .relative()
            .w_full()
            .h_full()
            .child(
                v_flex()
                    .w_full()
                    .h_full()
                    .child(self.render_tab_bar(cx))
                    .child(div().flex_1().min_h_0().child(editor)),
            )
            .when_some(close_dialog, |d, dlg| d.child(dlg))
            .when_some(extchg_dialog, |d, dlg| d.child(dlg))
    }
}

/// Reads the recent-markdown-files history from `~/.verve/editor_packs/.history`.
fn recent_files() -> Vec<PathBuf> {
    crate::wysiwyg::config::read_recent_files().unwrap_or_default()
}
