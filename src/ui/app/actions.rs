//! Keybinding action handlers: open/save/close Markdown files and the
//! workspace. These are wired via `on_action` in the root Render.

use gpui::{img, *};
use super::SideView;
use super::{CloseFile, NewFile, OpenMarkdownFile, OpenMarkdownFileNewTab, SaveWorkspace, VerveApp};

impl VerveApp {
    pub(super) fn on_open_markdown_file(
        &mut self,
        _: &OpenMarkdownFile,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_view = SideView::MarkdownEditor;
        let _ = self.markdown.update(cx, |p, cx| p.open_file_dialog(cx));
        cx.notify();
    }


    pub(super) fn on_open_markdown_file_new_tab(
        &mut self,
        _: &OpenMarkdownFileNewTab,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_view = SideView::MarkdownEditor;
        let _ = self.markdown.update(cx, |p, cx| p.open_file_dialog(cx));
        cx.notify();
    }

    /// cmd-s — save. Routes to the active editor: Markdown → save the active
    /// document tab; Notes → flush the current note immediately (bypassing the
    /// debounce). In other views this is a no-op (the editor owns file saving;
    /// there is no "workspace" to persist here). The WYSIWYG editor's own
    /// SaveDocument action is intentionally unregistered in the embedded build

    pub(super) fn on_save_workspace(&mut self, _: &SaveWorkspace, w: &mut Window, cx: &mut Context<Self>) {
        match self.active_view {
            SideView::MarkdownEditor => {
                let _ = self.markdown.update(cx, |p, cx| p.save(w, cx));
            }
            SideView::Notes => {
                let _ = self.notes.update(cx, |p, cx| p.save_current(cx));
            }
            _ => {}
        }
        cx.notify();
    }

    /// cmd-n — new file. Routes to the active editor: Markdown → new document
    /// tab; Notes → new note. In other views it switches to Markdown and opens

    pub(super) fn on_new_file(&mut self, _: &NewFile, w: &mut Window, cx: &mut Context<Self>) {
        match self.active_view {
            SideView::Notes => {
                let _ = self.notes.update(cx, |p, cx| p.new_note(w, cx));
            }
            SideView::MarkdownEditor => {
                let _ = self.markdown.update(cx, |p, cx| p.new_document(cx));
            }
            _ => {
                self.active_view = SideView::MarkdownEditor;
                let _ = self.markdown.update(cx, |p, cx| p.new_document(cx));
            }
        }
        cx.notify();
    }

    /// cmd-w — close. Markdown editor: close the active tab, prompting to save

    pub(super) fn on_close_file(&mut self, _: &CloseFile, w: &mut Window, cx: &mut Context<Self>) {
        match self.active_view {
            SideView::MarkdownEditor => {
                let _ = self.markdown.update(cx, |p, cx| p.close_active_tab(w, cx));
            }
            SideView::Notes => {
                let _ = self.notes.update(cx, |p, cx| p.close_current_note(w, cx));
            }
            _ => {}
        }
        cx.notify();
    }
}
