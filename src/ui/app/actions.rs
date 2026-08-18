//! Keybinding action handlers: open/save/close Markdown files and the
//! workspace, plus rail-view switching (⌘/Ctrl + 1..=5). These are wired
//! via `on_action` in the root Render.

use super::SideView;
use super::{
    CloseFile, NewFile, OpenHelpCenter, OpenMarkdownFile, OpenMarkdownFileNewTab, SaveWorkspace,
    SelectRailSlot, VerveApp,
};
use gpui::{img, *};
use gpui_component::ActiveTheme as _;

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
    pub(super) fn on_save_workspace(
        &mut self,
        _: &SaveWorkspace,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    /// ⌘/Ctrl + 1..=5 — switch to one of the fixed shortcut views (Api, Ssh,
    /// JsonFormat, Notes, MarkdownEditor). The bindings carry a "!BlockEditor"
    /// context, so inside the Markdown editor ⌘1..⌘6/⌘0 keep their editor
    /// meaning; pressing Esc there refocuses the app root (see the root
    /// render's key handler) to make these shortcuts active again. Hidden
    /// rails are skipped.
    pub(super) fn on_select_rail_slot(
        &mut self,
        a: &SelectRailSlot,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(&view) = SideView::SHORTCUT_VIEWS.get(a.0) else {
            return;
        };
        if self.hidden_rails.contains(view.name()) {
            return;
        }
        self.activate_view(view, w, cx);
    }

    /// cmd-/ / F1 — open the help-center window (or bring the existing one
    /// to the front). See [`Self::open_help_center`].
    pub(super) fn on_open_help_center(
        &mut self,
        _: &OpenHelpCenter,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_help_center(cx);
    }

    /// Open the help-center window, or activate the already-open one. A
    /// stale handle (the window was closed since) makes `update` fail, in
    /// which case a fresh window opens.
    pub(super) fn open_help_center(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.help_window.take() {
            if handle
                .update(cx, |_root, window, _cx| window.activate_window())
                .is_ok()
            {
                self.help_window = Some(handle);
                return;
            }
        }
        let bounds = gpui::Bounds::centered(None, gpui::size(px(880.), px(640.)), cx);
        match cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                window_min_size: Some(gpui::size(px(660.), px(480.))),
                titlebar: None,
                is_minimizable: true,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| crate::ui::help_center::HelpCenter::new(window, cx));
                cx.new(|cx| {
                    gpui_component::Root::new(view, window, cx).bg(cx.theme().background)
                })
            },
        ) {
            Ok(handle) => self.help_window = Some(handle),
            Err(err) => log::error!("failed to open help-center window: {err:#}"),
        }
    }

    /// cmd-w — close. Closes the active tab of whichever view has focus:
    /// Markdown → save-prompt then close tab; Notes → close current note;
    /// Api → close the active request tab; Ssh → close the active terminal
    /// session; K8s → close the active pod session.
    pub(super) fn on_close_file(&mut self, _: &CloseFile, w: &mut Window, cx: &mut Context<Self>) {
        match self.active_view {
            SideView::MarkdownEditor => {
                let _ = self.markdown.update(cx, |p, cx| p.close_active_tab(w, cx));
            }
            SideView::Notes => {
                let _ = self.notes.update(cx, |p, cx| p.close_current_note(w, cx));
            }
            SideView::Api => {
                let _ = self.state.update(cx, |s, cx| {
                    if let Some(id) = s.active_tab_id.clone() {
                        s.close_tab(&id, cx);
                    }
                });
            }
            SideView::Ssh => {
                let _ = self.ssh.update(cx, |p, cx| {
                    // Read the index into a local first to avoid a borrow of
                    // `p` across the `&mut p` close call.
                    let idx = p.active_tab;
                    p.close_session(idx, cx);
                });
            }
            SideView::K8s => {
                let _ = self.k8s.update(cx, |p, cx| p.close_active_session(cx));
            }
            _ => {}
        }
        cx.notify();
    }
}
