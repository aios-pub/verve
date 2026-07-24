//! Standalone Markdown file editor side-view.
//!
//! Hosts the WYSIWYG block editor (ported from velotype) as a self-contained
//! panel that edits `.md` files on disk. Unlike the notes panel (which stores
//! content in verve's notes DB and uses `.embedded()`), this panel uses
//! [`Editor::side_view`] so the editor renders its own status bar (word count,
//! view-mode switch) and supports the workspace drawer (outline), while the
//! panel's title bar (rendered in `app.rs`) drives Open / Save / Save-As /
//! Export / Recent-files.
//!
//! The panel is constructed once and kept alive on `VerveApp`, so switching to
//! another activity-rail view and back preserves the in-progress edit.

use std::path::PathBuf;

use gpui::*;
use gpui_component::{ActiveTheme, Sizable as _};

use crate::state::AppState;
use crate::wysiwyg::editor::Editor;
use crate::wysiwyg::export::ExportFormat;

pub struct MarkdownPanel {
    pub editor: Entity<Editor>,
    #[allow(dead_code)]
    state: Entity<AppState>,
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
        let editor = cx.new(|cx| Editor::from_markdown(cx, String::new(), None).side_view());
        // Re-render the title bar (dirty marker / filename) whenever the
        // editor notifies (every edit / save / open).
        let sub = cx.observe(&editor, |_, _editor, cx| {
            cx.notify();
        });
        Self {
            editor,
            state,
            recent: recent_files(),
            open_popover: false,
            export_popover: false,
            _subs: vec![sub],
        }
    }

    // -- file actions (driven from the title bar) ---------------------------

    /// "New" — discard the current doc and start a blank untitled one.
    pub fn new_document(&mut self, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.reset_to_blank(cx));
        self.refresh_recent();
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

    /// Open a specific path (from the picker or the recent-files list).
    pub fn open_path(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let result = self.editor.update(cx, |e, cx| e.open_file(path, cx));
        if let Err(err) = result {
            eprintln!("failed to open markdown file '{}': {err}", path.display());
        }
        self.refresh_recent();
        cx.notify();
    }

    /// "Save" — save to the existing path, or prompt for a path if untitled.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.save_document(window, cx));
        self.refresh_recent();
        cx.notify();
    }

    /// "Save As" — always prompt for a new path.
    pub fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |e, cx| e.save_document_as(window, cx));
        self.refresh_recent();
        cx.notify();
    }

    /// "Export" → HTML.
    pub fn export_html(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| {
            e.export_document_via_prompt(ExportFormat::Html, window, cx)
        });
        self.export_popover = false;
        cx.notify();
    }

    /// "Export" → PDF.
    pub fn export_pdf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| {
            e.export_document_via_prompt(ExportFormat::Pdf, window, cx)
        });
        self.export_popover = false;
        cx.notify();
    }

    /// Toggle the outline / file-tree drawer.
    pub fn toggle_outline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |e, cx| e.toggle_workspace_drawer(window, cx));
        cx.notify();
    }

    /// Whether the editor's outline drawer is currently open.
    pub fn outline_open(&self, cx: &App) -> bool {
        self.editor.read(cx).workspace_open()
    }

    // -- title-bar state (read by app.rs) -----------------------------------

    pub fn dirty(&self, cx: &App) -> bool {
        self.editor.read(cx).document_dirty()
    }

    pub fn title(&self, cx: &App) -> String {
        self.editor.read(cx).display_title().to_string()
    }

    pub fn file_path(&self, cx: &App) -> Option<String> {
        self.editor
            .read(cx)
            .file_path()
            .map(|p| p.display().to_string())
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

    fn refresh_recent(&mut self) {
        self.recent = recent_files();
    }
}

impl Render for MarkdownPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // The editor renders its own scrollable block column + status bar +
        // (optionally open) workspace drawer. The title bar lives in app.rs.
        div().w_full().h_full().child(self.editor.clone())
    }
}

/// Reads the recent-markdown-files history from `~/.verve/editor_packs/.history`.
fn recent_files() -> Vec<PathBuf> {
    crate::wysiwyg::config::read_recent_files().unwrap_or_default()
}
