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

use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme, IconName, Sizable as _, h_flex, v_flex};

use crate::state::AppState;
use crate::wysiwyg::editor::count_words;
use crate::wysiwyg::editor::Editor;
use crate::wysiwyg::editor::ViewMode;
use crate::wysiwyg::export::ExportFormat;

/// One open document: its own editor + backing file path.
struct MarkdownTab {
    editor: Entity<Editor>,
    file_path: Option<PathBuf>,
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
    _subs: Vec<Subscription>,
}

impl MarkdownPanel {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            state,
            tabs: Vec::new(),
            active: 0,
            recent: recent_files(),
            open_popover: false,
            export_popover: false,
            _subs: Vec::new(),
        };
        // Start with one blank untitled tab.
        panel.new_document(cx);
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

    // -- file actions (driven from the title bar; operate on active tab) ----

    /// "New" — open a fresh blank untitled tab and focus it.
    pub fn new_document(&mut self, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, String::new(), None).side_view());
        self.tabs.push(MarkdownTab {
            editor,
            file_path: None,
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
            });
            self.active = self.tabs.len() - 1;
            self.observe_active(cx);
        }
        self.refresh_recent();
        cx.notify();
    }

    /// "Save" — save the active tab to its path, or prompt if untitled.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor()
            .update(cx, |e, cx| e.save_document(window, cx));
        // save_document sets the editor's internal file_path on save-as; mirror
        // it onto the tab record so the tab label / dedup stays correct.
        self.sync_active_path(cx);
        self.refresh_recent();
        cx.notify();
    }

    /// "Save As" — always prompt for a new path.
    pub fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor()
            .update(cx, |e, cx| e.save_document_as(window, cx));
        self.sync_active_path(cx);
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
        let editor = self.active_tab().editor.read(cx);
        let total = count_words(&editor.serialized_document_text(cx));
        let selection = editor
            .selected_markdown_text(cx)
            .map(|t| count_words(&t));
        (total, selection)
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

    fn sync_active_path(&mut self, cx: &App) {
        if let Some(path) = self.active_tab().editor.read(cx).file_path() {
            self.tabs[self.active].file_path = Some(path.to_path_buf());
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
        v_flex()
            .w_full()
            .h_full()
            .child(self.render_tab_bar(cx))
            .child(div().flex_1().min_h_0().child(editor))
    }
}

/// Reads the recent-markdown-files history from `~/.verve/editor_packs/.history`.
fn recent_files() -> Vec<PathBuf> {
    crate::wysiwyg::config::read_recent_files().unwrap_or_default()
}
