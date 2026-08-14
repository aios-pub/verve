//! Rendered-mode context menus and native table insertion dialog.

use std::time::Duration;

use gpui::*;

use super::{Editor, TableAxisSelection, ViewMode};
use crate::wysiwyg::components::{
    DeleteTable, DismissTransientUi, InsertHorizontalRule, InsertImage, InsertMedia, InsertTable,
    TableAxisKind, TableCellPosition, TableColumnAlignment, TableData,
};
use crate::wysiwyg::i18n::I18nManager;
use crate::wysiwyg::theme::Theme;

/// Target block position for inserting a native table.
#[derive(Clone, Copy)]
pub(super) enum TableInsertTarget {
    /// Insert the table immediately after the referenced block.
    After(EntityId),
    /// Append the table to the end of the current root list.
    Append,
}

/// Rendered-mode context menu currently open in the editor.
pub(super) enum ContextMenuState {
    /// General block context menu with an insert submenu.
    Insert {
        position: Point<Pixels>,
        target: TableInsertTarget,
        insert_hovered: bool,
        submenu_hovered: bool,
        submenu_open: bool,
    },
    /// Table row or column context menu for an existing native table.
    TableAxis {
        position: Point<Pixels>,
        selection: TableAxisSelection,
    },
    /// Aggregated table context menu opened by right-clicking inside a table
    /// cell. Carries the owning table block and the cell's position so menu
    /// items can target the cell's row and column.
    TableCell {
        position: Point<Pixels>,
        table_block_id: EntityId,
        cell: TableCellPosition,
    },
}

/// State for the table insertion dialog opened from the context menu.
pub(super) struct TableInsertDialogState {
    pub target: TableInsertTarget,
    pub body_rows: usize,
    pub columns: usize,
}

/// State for the image-insert dialog (Cmd/Ctrl+Shift+I). The user fills in a
/// URL (or picks a local file, which populates the URL field with the path)
/// and an optional alt text; confirming inserts a standalone image block.
pub(super) struct ImageInsertDialogState {
    pub target: TableInsertTarget,
    pub url: Entity<gpui_component::input::InputState>,
    pub alt: Entity<gpui_component::input::InputState>,
}

impl Editor {
    pub(super) fn root_ancestor_entity_id(&self, entity_id: EntityId) -> EntityId {
        let mut current = entity_id;
        while let Some(location) = self.document.find_block_location(current) {
            let Some(parent) = location.parent else {
                break;
            };
            current = parent.entity_id();
        }
        current
    }

    fn open_insert_context_menu(
        &mut self,
        position: Point<Pixels>,
        target: TableInsertTarget,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }

        self.close_menu_bar(cx);
        self.context_menu_submenu_close_task = None;
        self.context_menu = Some(ContextMenuState::Insert {
            position,
            target,
            insert_hovered: false,
            submenu_hovered: false,
            submenu_open: false,
        });
        cx.notify();
    }

    pub(super) fn open_table_axis_context_menu(
        &mut self,
        position: Point<Pixels>,
        selection: TableAxisSelection,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }

        self.close_menu_bar(cx);
        self.context_menu_submenu_close_task = None;
        self.context_menu = Some(ContextMenuState::TableAxis {
            position,
            selection,
        });
        cx.notify();
    }

    /// Opens the aggregated table context menu anchored to a cell's row and
    /// column. Triggered by right-clicking inside a table cell.
    pub(super) fn open_table_cell_context_menu(
        &mut self,
        position: Point<Pixels>,
        table_block_id: EntityId,
        cell: TableCellPosition,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }

        self.close_menu_bar(cx);
        self.context_menu_submenu_close_task = None;
        self.context_menu = Some(ContextMenuState::TableCell {
            position,
            table_block_id,
            cell,
        });
        cx.notify();
    }

    pub(super) fn close_table_insert_dialog(&mut self, cx: &mut Context<Self>) {
        if self.table_insert_dialog.take().is_some() {
            cx.notify();
        }
    }

    /// Resolve where a keyboard-triggered insert (Cmd-T, Cmd-Shift--, ...) should
    /// land: after the currently focused block, or appended to the document root
    /// when nothing is focused.  Mirrors the target resolution the context-menu
    /// table-insert flow already uses.
    pub(super) fn insert_target_from_focus(&self, window: &Window, cx: &App) -> TableInsertTarget {
        match self.document.focused_block_entity_id(window, cx) {
            Some(entity_id) => TableInsertTarget::After(entity_id),
            None => TableInsertTarget::Append,
        }
    }

    /// Inserts a new structural block (e.g. separator, image) at `target` and
    /// lands the caret on the trailing paragraph that follows it.  Shared by the
    /// horizontal-rule and image insert actions.
    pub(super) fn insert_structural_block(
        &mut self,
        target: TableInsertTarget,
        record: crate::wysiwyg::components::BlockRecord,
        cx: &mut Context<Self>,
    ) {
        let new_block = Self::new_block(cx, record);
        let inserted_id = new_block.entity_id();
        match target {
            TableInsertTarget::After(entity_id) => {
                if let Some(location) = self.document.find_block_location(entity_id) {
                    self.document.insert_blocks_at(
                        location.parent,
                        location.index + 1,
                        vec![new_block.clone()],
                        cx,
                    );
                } else {
                    self.document.insert_blocks_at(
                        None,
                        self.document.root_count(),
                        vec![new_block.clone()],
                        cx,
                    );
                }
            }
            TableInsertTarget::Append => {
                self.document.insert_blocks_at(
                    None,
                    self.document.root_count(),
                    vec![new_block.clone()],
                    cx,
                );
            }
        }
        // Structural blocks (separator/image) leave no editable caret inside, so
        // ensure a trailing paragraph exists, then focus it.
        self.ensure_trailing_paragraph_after_structural(&new_block, cx);
        if let Some(location) = self.document.find_block_location(inserted_id) {
            let sibling_count = match location.parent.as_ref() {
                Some(parent) => parent.read(cx).children.len(),
                None => self.document.root_count(),
            };
            if location.index + 1 < sibling_count {
                // The trailing paragraph (if one was added) sits right after the
                // structural block we just inserted.
                let next_id = match location.parent.as_ref() {
                    Some(parent) => parent
                        .read(cx)
                        .children
                        .get(location.index + 1)
                        .map(|b| b.entity_id()),
                    None => self
                        .document
                        .root_blocks()
                        .get(location.index + 1)
                        .map(|b| b.entity_id()),
                };
                if let Some(id) = next_id {
                    self.focus_block(id);
                } else {
                    self.focus_block(inserted_id);
                }
            } else {
                self.focus_block(inserted_id);
            }
        }
        self.rebuild_image_runtimes(cx);
        self.mark_dirty(cx);
        self.request_active_block_scroll_into_view(cx);
        cx.notify();
    }

    pub(super) fn on_insert_horizontal_rule(
        &mut self,
        _: &InsertHorizontalRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }
        let target = self.insert_target_from_focus(window, cx);
        let record = crate::wysiwyg::components::BlockRecord::new(
            crate::wysiwyg::components::BlockKind::Separator,
            crate::wysiwyg::components::InlineTextTree::plain(String::new()),
        );
        self.insert_structural_block(target, record, cx);
    }

    pub(super) fn on_insert_table(
        &mut self,
        _: &InsertTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }
        // Open the existing table-insert dialog seeded from the focused block,
        // reusing the same confirm flow as the context-menu entry point.
        let target = self.insert_target_from_focus(window, cx);
        self.close_menu_bar(cx);
        self.context_menu_submenu_close_task = None;
        self.table_insert_dialog = Some(TableInsertDialogState {
            target,
            body_rows: 2,
            columns: 2,
        });
        cx.notify();
    }

    pub(super) fn on_insert_image(
        &mut self,
        _: &InsertImage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }
        let target = self.insert_target_from_focus(window, cx);
        self.open_image_insert_dialog(target, window, cx);
    }

    /// Open a native multi-select file picker for videos and import each one:
    /// stored under `<media_root>/videos/`, embedded as `<video>` markdown, and
    /// Open a native multi-select file picker for media (images and videos,
    /// with filter dropdown: Images / Videos / All) and import each picked
    /// file. Images are stored under `<media_root>/images/` and embedded as
    /// `![](...)`; videos under `<media_root>/videos/` as `<video>`. Each is
    /// uploaded to S3 when its kind's storage is set to S3.
    pub(super) fn on_insert_media(
        &mut self,
        _: &InsertMedia,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }
        let target = self.insert_target_from_focus(window, cx);
        let window_handle = window.window_handle();
        let images_label = rust_i18n::t!("settings.media.card_image").to_string();
        let videos_label = rust_i18n::t!("settings.media.card_video").to_string();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .add_filter(
                    &images_label,
                    &["png", "jpg", "jpeg", "webp", "gif", "bmp", "svg", "tif", "tiff"],
                )
                .add_filter(&videos_label, &["mp4", "mov", "webm", "mkv", "avi", "m4v"])
                .pick_files();
            let _ = tx.send(res.unwrap_or_default());
        });
        cx.spawn(async move |this, cx| {
            let picked: Vec<std::path::PathBuf> = loop {
                match rx.try_recv() {
                    Ok(v) => break v,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(Duration::from_millis(50))
                            .await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break Vec::new(),
                }
            };
            if picked.is_empty() {
                return;
            }
            let _ = cx.update_window(window_handle, move |_view: AnyView, _window, cx| {
                let _ = this.update(cx, |editor, cx| {
                    for path in picked {
                        editor.import_media_file(path, target, cx);
                    }
                });
            });
        })
        .detach();
    }

    /// Route a picked file to the image or video import flow by extension.
    fn import_media_file(
        &mut self,
        path: std::path::PathBuf,
        target: TableInsertTarget,
        cx: &mut Context<Self>,
    ) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
            .unwrap_or_default();
        if crate::media::path::is_video_extension(&ext) {
            self.import_video_file(path, target, cx);
        } else {
            self.import_image_file(path, target, cx);
        }
    }

    /// Store one image file locally, insert an image block, and (when image
    /// storage is S3) enqueue a PUT upload that rewrites the src on success.
    fn import_image_file(
        &mut self,
        path: std::path::PathBuf,
        target: TableInsertTarget,
        cx: &mut Context<Self>,
    ) {
        use crate::media::path::MediaKind;

        let cfg = crate::media::config::load_media_config();
        let root = match crate::media::config::resolved_media_root(&cfg) {
            Ok(r) => r,
            Err(err) => {
                self.show_image_paste_error(err, cx);
                return;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(err) => {
                self.show_image_paste_error(err.into(), cx);
                return;
            }
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_string)
            .or_else(|| crate::media::path::detect_image_extension(&bytes).map(str::to_string))
            .unwrap_or_else(|| "png".to_string());
        let stored = match crate::media::local::store_local(&root, MediaKind::Image, &bytes, &ext) {
            Ok(s) => s,
            Err(err) => {
                self.show_image_paste_error(err, cx);
                return;
            }
        };
        let src = crate::media::path::local_markdown_ref(MediaKind::Image, &stored.filename);
        // `./media/...` paths contain no markdown-special chars, so no escaping.
        let markdown = format!("![image]({src})");
        let record = crate::wysiwyg::components::BlockRecord::paragraph(markdown);
        self.insert_structural_block(target, record, cx);
        self.maybe_enqueue_image_s3_upload(stored.abs_path.clone(), stored.filename.clone(), cx);
    }

    /// Store one video file locally, insert a `<video>` block, and (when video
    /// storage is S3) enqueue a multipart upload that rewrites the src on
    /// success.
    fn import_video_file(
        &mut self,
        path: std::path::PathBuf,
        target: TableInsertTarget,
        cx: &mut Context<Self>,
    ) {
        use crate::media::config::StorageMethod;
        use crate::media::path::MediaKind;

        let cfg = crate::media::config::load_media_config();
        let root = match crate::media::config::resolved_media_root(&cfg) {
            Ok(r) => r,
            Err(err) => {
                self.show_image_paste_error(err, cx);
                return;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(err) => {
                self.show_image_paste_error(err.into(), cx);
                return;
            }
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_string)
            .or_else(|| crate::media::path::detect_video_extension(&bytes).map(str::to_string))
            .unwrap_or_else(|| "mp4".to_string());
        let stored =
            match crate::media::local::store_local(&root, MediaKind::Video, &bytes, &ext) {
                Ok(s) => s,
                Err(err) => {
                    self.show_image_paste_error(err, cx);
                    return;
                }
            };
        let local_src = crate::media::path::local_markdown_ref(MediaKind::Video, &stored.filename);
        let markdown = crate::media::path::video_markdown(&local_src);
        let record = crate::wysiwyg::components::BlockRecord::paragraph(markdown);
        self.insert_structural_block(target, record, cx);

        if cfg.video.storage == StorageMethod::S3 && cfg.s3.is_configured() {
            let prefix = cfg.video.s3_prefix.trim().trim_matches('/');
            let key = if prefix.is_empty() {
                stored.filename.clone()
            } else {
                format!("{prefix}/{}", stored.filename)
            };
            self.enqueue_video_s3_upload(
                local_src,
                stored.abs_path,
                key,
                cfg.video.multipart_part_mb,
                cfg.s3.clone(),
                cx,
            );
        }
    }

    /// Enqueue a video multipart upload; on success rewrite the embedded
    /// `./media/videos/...` src to the S3 URL, on failure keep the local copy
    /// and surface the error.
    pub(super) fn enqueue_video_s3_upload(
        &mut self,
        local_src: String,
        local_path: std::path::PathBuf,
        key: String,
        part_size_mb: u64,
        s3: crate::media::config::S3Config,
        cx: &mut Context<Self>,
    ) {
        use crate::media::upload::{enqueue_video_upload, VideoUploadEvent, VideoUploadRequest};

        let rx = enqueue_video_upload(VideoUploadRequest {
            local_path,
            key,
            s3,
            part_size_mb,
        });
        let old_src = local_src;
        cx.spawn(async move |this, cx| loop {
            match rx.try_recv() {
                Ok(VideoUploadEvent::Progress { done, total }) => {
                    // Shared-map update: blocks read the new value on the
                    // re-render triggered by notify — no full-document rebuild
                    // per part.
                    let _ = this.update(cx, |editor, cx| {
                        editor
                            .video_uploads
                            .write()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(old_src.clone(), (done, total));
                        cx.notify();
                    });
                }
                Ok(VideoUploadEvent::Done(res)) => {
                    let _ = this.update(cx, |editor, cx| {
                        editor
                            .video_uploads
                            .write()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&old_src);
                        // rewrite_media_src rebuilds runtimes itself on success;
                        // on failure the local copy stays and only the error is
                        // surfaced.
                        match res {
                            Ok(url) => editor.rewrite_media_src(&old_src, &url, cx),
                            Err(err) => editor.show_image_paste_error(err, cx),
                        }
                        cx.notify();
                    });
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        })
        .detach();
    }

    /// Open the image-insert dialog seeded with empty URL/alt fields.
    fn open_image_insert_dialog(
        &mut self,
        target: TableInsertTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu_bar(cx);
        self.context_menu_submenu_close_task = None;
        let url = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx).placeholder("图片 URL 或本地路径")
        });
        let alt = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx).placeholder("替代文本（可选）")
        });
        self.image_insert_dialog = Some(ImageInsertDialogState { target, url, alt });
        // Focus the URL field so the user can type or paste immediately.
        if let Some(dialog) = self.image_insert_dialog.as_ref() {
            dialog.url.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn on_cancel_image_insert_dialog(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_image_insert_dialog(cx);
    }

    fn close_image_insert_dialog(&mut self, cx: &mut Context<Self>) {
        if self.image_insert_dialog.take().is_some() {
            cx.notify();
        }
    }

    /// Native file picker for a local image. Runs on a background thread (rfd
    /// blocks) and writes the chosen absolute path into the dialog's URL field.
    pub(super) fn on_pick_image_file(
        &mut self,
        _event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url_field = match self.image_insert_dialog.as_ref() {
            Some(dialog) => dialog.url.clone(),
            None => return,
        };
        let window_handle = window.window_handle();
        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .add_filter(
                    "Images",
                    &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"],
                )
                .pick_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        cx.spawn(async move |_this, cx| {
            let picked = loop {
                match rx.try_recv() {
                    Ok(Some(path)) => break Some(path),
                    Ok(None) => break None,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break None,
                }
            };
            if let Some(path) = picked {
                let path_str = path.to_string_lossy().to_string();
                // set_value needs a &mut Window; reach back into the originating
                // window to update the InputState.
                let _ = cx.update_window(
                    window_handle,
                    move |_view: AnyView, window: &mut Window, cx: &mut App| {
                        let _ = url_field.update(cx, |state, cx| {
                            state.set_value(&path_str, window, cx);
                        });
                    },
                );
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn on_confirm_image_insert_dialog(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.image_insert_dialog.take() else {
            return;
        };
        let url = dialog.url.read(cx).value().to_string();
        let alt = dialog.alt.read(cx).value().to_string();
        let target = dialog.target;
        // A blank URL inserts nothing; just close.
        if url.trim().is_empty() {
            cx.notify();
            return;
        }
        // A standalone image is a paragraph whose title markdown is
        // `![alt](src)`. The standalone-image detection path routes it through
        // ImageRuntime on render; the resolver handles both http(s) URLs and
        // local paths.
        let markdown = format!("![{alt}]({url})");
        let record = crate::wysiwyg::components::BlockRecord::paragraph(markdown);
        self.insert_structural_block(target, record, cx);
    }

    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        let had_menu = self.context_menu.take().is_some();
        let had_submenu_close = self.context_menu_submenu_close_task.take().is_some();
        if had_menu || had_submenu_close {
            cx.notify();
        }
    }

    pub(super) fn dismiss_contextual_overlays(&mut self, cx: &mut Context<Self>) {
        let had_menu = self.context_menu.take().is_some();
        let had_dialog = self.table_insert_dialog.take().is_some();
        let had_image_dialog = self.image_insert_dialog.take().is_some();
        let had_submenu_close = self.context_menu_submenu_close_task.take().is_some();
        if had_menu || had_dialog || had_image_dialog || had_submenu_close {
            cx.notify();
        }
    }

    fn schedule_context_menu_submenu_close(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.context_menu, Some(ContextMenuState::Insert { .. })) {
            return;
        }

        let weak_editor = cx.entity().downgrade();
        self.context_menu_submenu_close_task = Some(cx.spawn(
            async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let _ = weak_editor.update(cx, |editor, cx| {
                    editor.context_menu_submenu_close_task = None;
                    let Some(ContextMenuState::Insert {
                        insert_hovered,
                        submenu_hovered,
                        submenu_open,
                        ..
                    }) = editor.context_menu.as_mut()
                    else {
                        return;
                    };
                    if !*insert_hovered && !*submenu_hovered && *submenu_open {
                        *submenu_open = false;
                        cx.notify();
                    }
                });
            },
        ));
    }

    fn set_context_menu_hover_state(
        &mut self,
        hovered: bool,
        submenu: bool,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        let mut should_clear_close = false;
        let mut should_schedule_close = false;

        if let Some(ContextMenuState::Insert {
            insert_hovered,
            submenu_hovered,
            submenu_open,
            ..
        }) = self.context_menu.as_mut()
        {
            if submenu {
                if *submenu_hovered != hovered {
                    *submenu_hovered = hovered;
                    changed = true;
                }
            } else if *insert_hovered != hovered {
                *insert_hovered = hovered;
                changed = true;
            }

            if hovered {
                should_clear_close = true;
                if !*submenu_open {
                    *submenu_open = true;
                    changed = true;
                }
            } else {
                let insert_still_hovered = *insert_hovered;
                let submenu_still_hovered = *submenu_hovered;
                if !insert_still_hovered && !submenu_still_hovered {
                    should_schedule_close = true;
                }
            }
        }

        if should_clear_close {
            self.context_menu_submenu_close_task = None;
        }
        if should_schedule_close {
            self.schedule_context_menu_submenu_close(cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub(super) fn on_editor_context_menu_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }
        cx.stop_propagation();
        self.open_insert_context_menu(event.position, TableInsertTarget::Append, cx);
    }

    pub(super) fn on_block_context_menu_mouse_down(
        &mut self,
        entity_id: EntityId,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            return;
        }
        cx.stop_propagation();
        // Right-clicking inside a table cell opens the aggregated table context
        // menu (insert row/column, delete table, ...) anchored to the cell.
        if let Some(binding) = self.table_cell_binding(entity_id) {
            self.open_table_cell_context_menu(
                event.position,
                binding.table_block.entity_id(),
                binding.position,
                cx,
            );
            return;
        }
        let allows_insert = self
            .focusable_entity_by_id(entity_id)
            .is_none_or(|block| block.read(cx).kind().allows_context_table_insert());
        if !allows_insert {
            return;
        }
        let target = TableInsertTarget::After(self.root_ancestor_entity_id(entity_id));
        self.open_insert_context_menu(event.position, target, cx);
    }

    pub(super) fn on_dismiss_context_menu_overlay(
        &mut self,
        _event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_contextual_overlays(cx);
    }

    pub(super) fn on_dismiss_transient_ui(
        &mut self,
        _: &DismissTransientUi,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_contextual_overlays(cx);
    }

    pub(super) fn on_context_menu_insert_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_context_menu_hover_state(*hovered, false, cx);
    }

    pub(super) fn on_context_menu_submenu_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_context_menu_hover_state(*hovered, true, cx);
    }

    pub(super) fn on_open_table_insert_dialog(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ContextMenuState::Insert { target, .. }) = self.context_menu.take() else {
            return;
        };
        self.context_menu_submenu_close_task = None;
        self.table_insert_dialog = Some(TableInsertDialogState {
            target,
            body_rows: 2,
            columns: 2,
        });
        cx.notify();
    }

    pub(super) fn on_table_rows_decrement(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.table_insert_dialog.as_mut() {
            dialog.body_rows = dialog.body_rows.saturating_sub(1).max(1);
            cx.notify();
        }
    }

    pub(super) fn on_table_rows_increment(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.table_insert_dialog.as_mut() {
            dialog.body_rows += 1;
            cx.notify();
        }
    }

    pub(super) fn on_table_columns_decrement(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.table_insert_dialog.as_mut() {
            dialog.columns = dialog.columns.saturating_sub(1).max(1);
            cx.notify();
        }
    }

    pub(super) fn on_table_columns_increment(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.table_insert_dialog.as_mut() {
            dialog.columns += 1;
            cx.notify();
        }
    }

    pub(super) fn on_cancel_table_insert_dialog(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_table_insert_dialog(cx);
    }

    pub(super) fn on_confirm_table_insert_dialog(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.table_insert_dialog.take() else {
            return;
        };

        let table = TableData::new_empty(dialog.body_rows, dialog.columns);
        let new_block = Self::new_table_block(cx, table);

        match dialog.target {
            TableInsertTarget::After(entity_id) => {
                if let Some(location) = self.document.find_block_location(entity_id) {
                    self.document.insert_blocks_at(
                        location.parent,
                        location.index + 1,
                        vec![new_block.clone()],
                        cx,
                    );
                } else {
                    self.document.insert_blocks_at(
                        None,
                        self.document.root_count(),
                        vec![new_block.clone()],
                        cx,
                    );
                }
            }
            TableInsertTarget::Append => {
                self.document.insert_blocks_at(
                    None,
                    self.document.root_count(),
                    vec![new_block.clone()],
                    cx,
                );
            }
        }

        // A table inserted as the last block in its container leaves no line
        // below it, so in rendered mode the caret cannot move past the table.
        // Add a trailing empty paragraph to land on when nothing follows it.
        self.ensure_trailing_paragraph_after_structural(&new_block, cx);

        self.rebuild_table_runtimes(cx);
        if let Some(first_cell) = new_block
            .read(cx)
            .table_runtime
            .as_ref()
            .and_then(|runtime| runtime.header.first())
        {
            self.focus_block(first_cell.entity_id());
        }
        self.mark_dirty(cx);
        self.request_active_block_scroll_into_view(cx);
        cx.notify();
    }

    fn active_axis_menu_selection(&self) -> Option<TableAxisSelection> {
        match self.context_menu.as_ref() {
            Some(ContextMenuState::TableAxis { selection, .. }) => Some(*selection),
            _ => None,
        }
    }

    /// Context for the cell context menu: the owning table block id and the
    /// cell's position (visual row index, column index).
    fn active_cell_menu_context(&self) -> Option<(EntityId, TableCellPosition)> {
        match self.context_menu.as_ref() {
            Some(ContextMenuState::TableCell {
                table_block_id,
                cell,
                ..
            }) => Some((*table_block_id, *cell)),
            _ => None,
        }
    }

    fn on_apply_column_alignment(
        &mut self,
        alignment: TableColumnAlignment,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Column {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.set_table_column_alignment(&table_block, selection.index, alignment, cx);
    }

    pub(super) fn on_align_table_column_left(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Left is the default, so emit the unmarked `---` form rather than an
        // explicit `:---`; an explicit colon is only kept when the source had
        // one. This keeps the menu's output unchanged from before.
        self.on_apply_column_alignment(TableColumnAlignment::Default, cx);
    }

    pub(super) fn on_align_table_column_center(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_apply_column_alignment(TableColumnAlignment::Center, cx);
    }

    pub(super) fn on_align_table_column_right(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_apply_column_alignment(TableColumnAlignment::Right, cx);
    }

    pub(super) fn on_move_table_row_up(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Row || selection.index == 0 {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.move_table_row(&table_block, selection.index, -1, cx);
    }

    pub(super) fn on_move_table_row_down(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Row {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        let can_move = table_block
            .read(cx)
            .record
            .table
            .as_ref()
            .map(|table| selection.index < table.rows.len())
            .unwrap_or(false);
        if !can_move {
            return;
        }
        self.close_context_menu(cx);
        self.move_table_row(&table_block, selection.index, 1, cx);
    }

    pub(super) fn on_move_table_column_left(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Column || selection.index == 0 {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.move_table_column(&table_block, selection.index, -1, cx);
    }

    pub(super) fn on_move_table_column_right(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Column {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        let can_move = table_block
            .read(cx)
            .record
            .table
            .as_ref()
            .map(|table| selection.index + 1 < table.column_count())
            .unwrap_or(false);
        if !can_move {
            return;
        }
        self.close_context_menu(cx);
        self.move_table_column(&table_block, selection.index, 1, cx);
    }

    pub(super) fn on_delete_table_row(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Row {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        let row_count = table_block
            .read(cx)
            .record
            .table
            .as_ref()
            .map(|table| table.rows.len());
        self.close_context_menu(cx);
        // Visual index 0 is the header: deleting it promotes the first body row,
        // unless there is no body row left, in which case it was the table's last
        // row and the whole table is removed.
        if selection.index == 0 {
            if row_count == Some(0) {
                self.remove_table_block(&table_block, cx);
            } else {
                self.delete_table_header_row(&table_block, cx);
            }
        } else {
            self.delete_table_row(&table_block, selection.index - 1, cx);
        }
    }

    pub(super) fn on_toggle_table_headers(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = !crate::wysiwyg::config::EditorSettings::show_table_headers(cx);
        crate::wysiwyg::config::EditorSettings::set_show_table_headers(cx, next);
        self.close_context_menu(cx);
        // The preference is read while rendering table cells; re-render the
        // editor (and with it every table) to reflect the new styling.
        cx.notify();
    }

    pub(super) fn on_delete_table_column(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Column {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        let column_count = table_block
            .read(cx)
            .record
            .table
            .as_ref()
            .map(|table| table.column_count());
        self.close_context_menu(cx);
        // Removing the only column empties the table, so drop the whole block.
        if column_count == Some(1) {
            self.remove_table_block(&table_block, cx);
        } else {
            self.delete_table_column(&table_block, selection.index, cx);
        }
    }

    pub(super) fn on_insert_table_column_left(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Column {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.insert_table_column(&table_block, selection.index, cx);
    }

    pub(super) fn on_insert_table_column_right(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Column {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.insert_table_column(&table_block, selection.index + 1, cx);
    }

    pub(super) fn on_insert_table_row_above(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Row {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        // Visual index 0 is the header; "above" inserts the new body row
        // directly after the header (body index 0). For other rows, the new
        // row goes before the current body row.
        let body_row_index = selection.index.saturating_sub(1);
        self.insert_table_row(&table_block, body_row_index, cx);
    }

    pub(super) fn on_insert_table_row_below(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        if selection.kind != TableAxisKind::Row {
            return;
        }
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        // "Below" the header (visual 0) lands after body row 0; below body row
        // `n` lands after body row `n-1`. Saturating_sub keeps the header case
        // correct without underflow.
        let body_row_index = selection.index.saturating_sub(1) + 1;
        self.insert_table_row(&table_block, body_row_index, cx);
    }

    pub(super) fn on_delete_table(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.active_axis_menu_selection() else {
            return;
        };
        let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.delete_table(&table_block, cx);
    }

    /// Keyboard-shortcut handler for deleting the table that owns the focused
    /// cell. No-op when the focus is not inside a table cell.
    pub(super) fn on_delete_table_shortcut(
        &mut self,
        _: &DeleteTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(focused_id) = self.focused_edit_target_entity_id(window, cx) else {
            return;
        };
        let Some(binding) = self.table_cell_binding(focused_id) else {
            return;
        };
        let table_block = binding.table_block;
        self.prepare_undo_capture(
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        );
        self.delete_table(&table_block, cx);
        self.finalize_pending_undo_capture(cx);
    }

    // ---- Cell context-menu handlers ----
    // These mirror the axis-menu handlers above but resolve the target table
    // and row/column indices from the active cell menu rather than an axis
    // selection. `cell.row` is a visual index (0 = header, 1.. = body rows).

    pub(super) fn on_cell_insert_row_above(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table_block_id, cell)) = self.active_cell_menu_context() else {
            return;
        };
        let Some(table_block) = self.table_block_by_id(table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        let body_row_index = cell.row.saturating_sub(1);
        self.insert_table_row(&table_block, body_row_index, cx);
    }

    pub(super) fn on_cell_insert_row_below(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table_block_id, cell)) = self.active_cell_menu_context() else {
            return;
        };
        let Some(table_block) = self.table_block_by_id(table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        let body_row_index = cell.row.saturating_sub(1) + 1;
        self.insert_table_row(&table_block, body_row_index, cx);
    }

    pub(super) fn on_cell_insert_column_left(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table_block_id, cell)) = self.active_cell_menu_context() else {
            return;
        };
        let Some(table_block) = self.table_block_by_id(table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.insert_table_column(&table_block, cell.column, cx);
    }

    pub(super) fn on_cell_insert_column_right(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table_block_id, cell)) = self.active_cell_menu_context() else {
            return;
        };
        let Some(table_block) = self.table_block_by_id(table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.insert_table_column(&table_block, cell.column + 1, cx);
    }

    pub(super) fn on_cell_delete_row(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table_block_id, cell)) = self.active_cell_menu_context() else {
            return;
        };
        let Some(table_block) = self.table_block_by_id(table_block_id, cx) else {
            return;
        };
        let row_count = table_block
            .read(cx)
            .record
            .table
            .as_ref()
            .map(|table| table.rows.len());
        self.close_context_menu(cx);
        if cell.row == 0 {
            if row_count == Some(0) {
                self.delete_table(&table_block, cx);
            } else {
                self.delete_table_header_row(&table_block, cx);
            }
        } else {
            self.delete_table_row(&table_block, cell.row - 1, cx);
        }
    }

    pub(super) fn on_cell_delete_column(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table_block_id, cell)) = self.active_cell_menu_context() else {
            return;
        };
        let Some(table_block) = self.table_block_by_id(table_block_id, cx) else {
            return;
        };
        let column_count = table_block
            .read(cx)
            .record
            .table
            .as_ref()
            .map(|table| table.column_count());
        self.close_context_menu(cx);
        if column_count == Some(1) {
            self.delete_table(&table_block, cx);
        } else {
            self.delete_table_column(&table_block, cell.column, cx);
        }
    }

    pub(super) fn on_cell_delete_table(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table_block_id, _cell)) = self.active_cell_menu_context() else {
            return;
        };
        let Some(table_block) = self.table_block_by_id(table_block_id, cx) else {
            return;
        };
        self.close_context_menu(cx);
        self.delete_table(&table_block, cx);
    }

    /// Builds the horizontal divider used between groups of table axis/cell
    /// menu items.
    fn separator_item(theme: &Theme) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        div()
            .mx(px(d.menu_separator_margin_x))
            .my(px(d.menu_separator_margin_y))
            .h(px(d.menu_separator_height))
            .bg(c.dialog_border)
            .into_any_element()
    }

    fn render_axis_menu_item(
        theme: &Theme,
        id: &'static str,
        label: String,
        enabled: bool,
        danger: bool,
        on_click: fn(&mut Editor, &ClickEvent, &mut Window, &mut Context<Editor>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        if enabled {
            div()
                .id(id)
                .h(px(d.menu_item_height))
                .px(px(d.menu_item_padding_x))
                .flex()
                .items_center()
                .rounded(px(d.menu_item_radius))
                .bg(c.dialog_surface)
                .text_size(px(d.menu_text_size))
                .font_weight(t.dialog_body_weight.to_font_weight())
                .text_color(if danger {
                    c.dialog_danger_button_bg
                } else {
                    c.dialog_secondary_button_text
                })
                .child(label)
                .hover(|this| this.bg(c.dialog_secondary_button_hover))
                .cursor_pointer()
                .on_click(cx.listener(on_click))
                .into_any_element()
        } else {
            div()
                .id(id)
                .h(px(d.menu_item_height))
                .px(px(d.menu_item_padding_x))
                .flex()
                .items_center()
                .rounded(px(d.menu_item_radius))
                .bg(c.dialog_surface)
                .text_size(px(d.menu_text_size))
                .font_weight(t.dialog_body_weight.to_font_weight())
                .text_color(if danger {
                    c.dialog_danger_button_bg
                } else {
                    c.dialog_muted
                })
                .child(label)
                .into_any_element()
        }
    }

    pub(super) fn render_context_menu_overlay(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.context_menu.as_ref()?;
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let s = cx.global::<I18nManager>().strings().clone();

        match menu {
            ContextMenuState::Insert {
                position,
                submenu_open,
                ..
            } => {
                let panel_x = position.x;
                let panel_y = position.y;
                let panel_width = px(d.context_menu_panel_width);

                let submenu = submenu_open.then(|| {
                    div()
                        .id("editor-context-menu-submenu")
                        .absolute()
                        .left(panel_x + panel_width + px(d.context_menu_submenu_gap))
                        .top(panel_y)
                        .w(px(d.context_menu_submenu_width))
                        .p(px(d.menu_panel_padding))
                        .flex()
                        .flex_col()
                        .gap(px(d.menu_panel_gap))
                        .occlude()
                        .bg(c.dialog_surface)
                        .border(px(d.dialog_border_width))
                        .border_color(c.dialog_border)
                        .rounded(px(d.menu_panel_radius))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                            cx.stop_propagation()
                        })
                        .on_hover(cx.listener(Self::on_context_menu_submenu_hover))
                        .child(
                            div()
                                .id("editor-context-menu-insert-table")
                                .h(px(d.menu_item_height))
                                .px(px(d.menu_item_padding_x))
                                .flex()
                                .items_center()
                                .rounded(px(d.menu_item_radius))
                                .bg(c.dialog_surface)
                                .hover(|this| this.bg(c.dialog_secondary_button_hover))
                                .active(|this| this.opacity(0.92))
                                .cursor_pointer()
                                .text_size(px(d.menu_text_size))
                                .font_weight(t.dialog_body_weight.to_font_weight())
                                .text_color(c.dialog_secondary_button_text)
                                .child(s.context_menu_table.clone())
                                .on_click(cx.listener(Self::on_open_table_insert_dialog)),
                        )
                });

                let overlay = div()
                    .id("editor-context-menu-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::on_dismiss_context_menu_overlay),
                    )
                    .child(
                        div()
                            .id("editor-context-menu-panel")
                            .absolute()
                            .left(panel_x)
                            .top(panel_y)
                            .w(panel_width)
                            .p(px(d.menu_panel_padding))
                            .flex()
                            .flex_col()
                            .gap(px(d.menu_panel_gap))
                            .bg(c.dialog_surface)
                            .border(px(d.dialog_border_width))
                            .border_color(c.dialog_border)
                            .rounded(px(d.menu_panel_radius))
                            .shadow_lg()
                            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                                cx.stop_propagation()
                            })
                            .child(
                                div()
                                    .id("editor-context-menu-insert")
                                    .h(px(d.menu_item_height))
                                    .px(px(d.menu_item_padding_x))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .rounded(px(d.menu_item_radius))
                                    .bg(if *submenu_open {
                                        c.dialog_secondary_button_hover
                                    } else {
                                        c.dialog_surface
                                    })
                                    .hover(|this| this.bg(c.dialog_secondary_button_hover))
                                    .text_size(px(d.menu_text_size))
                                    .font_weight(t.dialog_body_weight.to_font_weight())
                                    .text_color(c.dialog_secondary_button_text)
                                    .child(s.context_menu_insert.clone())
                                    .child("›")
                                    .on_hover(cx.listener(Self::on_context_menu_insert_hover)),
                            ),
                    );

                Some(if let Some(submenu) = submenu {
                    overlay.child(submenu).into_any_element()
                } else {
                    overlay.into_any_element()
                })
            }
            ContextMenuState::TableAxis {
                position,
                selection,
            } => {
                let Some(table_block) = self.table_block_by_id(selection.table_block_id, cx) else {
                    return None;
                };
                let table = table_block.read(cx).record.table.clone()?;
                let items = match selection.kind {
                    TableAxisKind::Column => vec![
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-align-column-left",
                            s.table_axis_align_column_left.clone(),
                            true,
                            false,
                            Self::on_align_table_column_left,
                            cx,
                        ),
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-align-column-center",
                            s.table_axis_align_column_center.clone(),
                            true,
                            false,
                            Self::on_align_table_column_center,
                            cx,
                        ),
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-align-column-right",
                            s.table_axis_align_column_right.clone(),
                            true,
                            false,
                            Self::on_align_table_column_right,
                            cx,
                        ),
                        div()
                            .mx(px(d.menu_separator_margin_x))
                            .my(px(d.menu_separator_margin_y))
                            .h(px(d.menu_separator_height))
                            .bg(c.dialog_border)
                            .into_any_element(),
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-insert-column-left",
                            s.table_axis_insert_column_left.clone(),
                            true,
                            false,
                            Self::on_insert_table_column_left,
                            cx,
                        ),
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-insert-column-right",
                            s.table_axis_insert_column_right.clone(),
                            true,
                            false,
                            Self::on_insert_table_column_right,
                            cx,
                        ),
                        div()
                            .mx(px(d.menu_separator_margin_x))
                            .my(px(d.menu_separator_margin_y))
                            .h(px(d.menu_separator_height))
                            .bg(c.dialog_border)
                            .into_any_element(),
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-move-column-left",
                            s.table_axis_move_column_left.clone(),
                            selection.index > 0,
                            false,
                            Self::on_move_table_column_left,
                            cx,
                        ),
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-move-column-right",
                            s.table_axis_move_column_right.clone(),
                            selection.index + 1 < table.column_count(),
                            false,
                            Self::on_move_table_column_right,
                            cx,
                        ),
                        div()
                            .mx(px(d.menu_separator_margin_x))
                            .my(px(d.menu_separator_margin_y))
                            .h(px(d.menu_separator_height))
                            .bg(c.dialog_border)
                            .into_any_element(),
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-delete-column",
                            s.table_axis_delete_column.clone(),
                            // Always enabled: deleting the last column removes the
                            // whole table.
                            true,
                            true,
                            Self::on_delete_table_column,
                            cx,
                        ),
                        div()
                            .mx(px(d.menu_separator_margin_x))
                            .my(px(d.menu_separator_margin_y))
                            .h(px(d.menu_separator_height))
                            .bg(c.dialog_border)
                            .into_any_element(),
                        Self::render_axis_menu_item(
                            theme,
                            "table-axis-delete-table-column",
                            s.table_delete.clone(),
                            true,
                            true,
                            Self::on_delete_table,
                            cx,
                        ),
                    ],
                    TableAxisKind::Row => {
                        let mut items: Vec<AnyElement> = Vec::new();
                        // The header row (visual index 0) shares the normal row
                        // menu, with its Header Row styling toggle added on top.
                        if selection.index == 0 {
                            let headers_shown =
                                crate::wysiwyg::config::EditorSettings::show_table_headers(cx);
                            items.push(
                                div()
                                    .id("table-header-toggle")
                                    .h(px(d.menu_item_height))
                                    .px(px(d.menu_item_padding_x))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(d.menu_item_padding_x))
                                    .rounded(px(d.menu_item_radius))
                                    .bg(c.dialog_surface)
                                    .text_size(px(d.menu_text_size))
                                    .font_weight(t.dialog_body_weight.to_font_weight())
                                    .text_color(c.dialog_secondary_button_text)
                                    .child(s.table_header_row.clone())
                                    .child(if headers_shown { "✓" } else { "" })
                                    .hover(|this| this.bg(c.dialog_secondary_button_hover))
                                    .cursor_pointer()
                                    .on_click(cx.listener(Self::on_toggle_table_headers))
                                    .into_any_element(),
                            );
                            items.push(
                                div()
                                    .mx(px(d.menu_separator_margin_x))
                                    .my(px(d.menu_separator_margin_y))
                                    .h(px(d.menu_separator_height))
                                    .bg(c.dialog_border)
                                    .into_any_element(),
                            );
                        }
                        items.push(Self::render_axis_menu_item(
                            theme,
                            "table-axis-insert-row-above",
                            s.table_axis_insert_row_above.clone(),
                            true,
                            false,
                            Self::on_insert_table_row_above,
                            cx,
                        ));
                        items.push(Self::render_axis_menu_item(
                            theme,
                            "table-axis-insert-row-below",
                            s.table_axis_insert_row_below.clone(),
                            true,
                            false,
                            Self::on_insert_table_row_below,
                            cx,
                        ));
                        items.push(
                            div()
                                .mx(px(d.menu_separator_margin_x))
                                .my(px(d.menu_separator_margin_y))
                                .h(px(d.menu_separator_height))
                                .bg(c.dialog_border)
                                .into_any_element(),
                        );
                        items.push(Self::render_axis_menu_item(
                            theme,
                            "table-axis-move-row-up",
                            s.table_axis_move_row_up.clone(),
                            selection.index > 0,
                            false,
                            Self::on_move_table_row_up,
                            cx,
                        ));
                        items.push(Self::render_axis_menu_item(
                            theme,
                            "table-axis-move-row-down",
                            s.table_axis_move_row_down.clone(),
                            selection.index < table.rows.len(),
                            false,
                            Self::on_move_table_row_down,
                            cx,
                        ));
                        items.push(
                            div()
                                .mx(px(d.menu_separator_margin_x))
                                .my(px(d.menu_separator_margin_y))
                                .h(px(d.menu_separator_height))
                                .bg(c.dialog_border)
                                .into_any_element(),
                        );
                        // Always enabled: deleting the header promotes the first
                        // body row, and deleting the last remaining row removes
                        // the whole table.
                        items.push(Self::render_axis_menu_item(
                            theme,
                            "table-axis-delete-row",
                            s.table_axis_delete_row.clone(),
                            true,
                            true,
                            Self::on_delete_table_row,
                            cx,
                        ));
                        items.push(
                            div()
                                .mx(px(d.menu_separator_margin_x))
                                .my(px(d.menu_separator_margin_y))
                                .h(px(d.menu_separator_height))
                                .bg(c.dialog_border)
                                .into_any_element(),
                        );
                        items.push(Self::render_axis_menu_item(
                            theme,
                            "table-axis-delete-table-row",
                            s.table_delete.clone(),
                            true,
                            true,
                            Self::on_delete_table,
                            cx,
                        ));
                        items
                    }
                };

                Some(
                    div()
                        .id("table-axis-context-menu-overlay")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(Self::on_dismiss_context_menu_overlay),
                        )
                        .child(
                            div()
                                .id("table-axis-context-menu-panel")
                                .absolute()
                                .left(position.x)
                                .top(position.y)
                                .w(px(d.context_menu_axis_panel_width))
                                .p(px(d.menu_panel_padding))
                                .flex()
                                .flex_col()
                                .gap(px(d.menu_panel_gap))
                                .bg(c.dialog_surface)
                                .border(px(d.dialog_border_width))
                                .border_color(c.dialog_border)
                                .rounded(px(d.menu_panel_radius))
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                                    cx.stop_propagation()
                                })
                                .children(items),
                        )
                        .into_any_element(),
                )
            }
            ContextMenuState::TableCell {
                position,
                table_block_id,
                ..
            } => {
                let Some(table_block) = self.table_block_by_id(*table_block_id, cx) else {
                    return None;
                };
                let table = table_block.read(cx).record.table.clone();
                let column_count = table.as_ref().map(|t| t.column_count()).unwrap_or(1);

                let mut items: Vec<AnyElement> = Vec::new();
                items.push(Self::render_axis_menu_item(
                    theme,
                    "table-cell-insert-row-above",
                    s.table_axis_insert_row_above.clone(),
                    true,
                    false,
                    Self::on_cell_insert_row_above,
                    cx,
                ));
                items.push(Self::render_axis_menu_item(
                    theme,
                    "table-cell-insert-row-below",
                    s.table_axis_insert_row_below.clone(),
                    true,
                    false,
                    Self::on_cell_insert_row_below,
                    cx,
                ));
                items.push(Self::render_axis_menu_item(
                    theme,
                    "table-cell-delete-row",
                    s.table_axis_delete_row.clone(),
                    true,
                    true,
                    Self::on_cell_delete_row,
                    cx,
                ));
                items.push(Self::separator_item(theme));
                items.push(Self::render_axis_menu_item(
                    theme,
                    "table-cell-insert-column-left",
                    s.table_axis_insert_column_left.clone(),
                    true,
                    false,
                    Self::on_cell_insert_column_left,
                    cx,
                ));
                items.push(Self::render_axis_menu_item(
                    theme,
                    "table-cell-insert-column-right",
                    s.table_axis_insert_column_right.clone(),
                    true,
                    false,
                    Self::on_cell_insert_column_right,
                    cx,
                ));
                items.push(Self::render_axis_menu_item(
                    theme,
                    "table-cell-delete-column",
                    s.table_axis_delete_column.clone(),
                    column_count > 1,
                    true,
                    Self::on_cell_delete_column,
                    cx,
                ));
                items.push(Self::separator_item(theme));
                items.push(Self::render_axis_menu_item(
                    theme,
                    "table-cell-delete-table",
                    s.table_delete.clone(),
                    true,
                    true,
                    Self::on_cell_delete_table,
                    cx,
                ));

                Some(
                    div()
                        .id("table-cell-context-menu-overlay")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(Self::on_dismiss_context_menu_overlay),
                        )
                        .child(
                            div()
                                .id("table-cell-context-menu-panel")
                                .absolute()
                                .left(position.x)
                                .top(position.y)
                                .w(px(d.context_menu_axis_panel_width))
                                .p(px(d.menu_panel_padding))
                                .flex()
                                .flex_col()
                                .gap(px(d.menu_panel_gap))
                                .bg(c.dialog_surface)
                                .border(px(d.dialog_border_width))
                                .border_color(c.dialog_border)
                                .rounded(px(d.menu_panel_radius))
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                                    cx.stop_propagation()
                                })
                                .children(items),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    pub(super) fn render_table_insert_dialog_overlay(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.table_insert_dialog.as_ref()?;
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let s = cx.global::<I18nManager>().strings().clone();

        let stepper =
            |id_prefix: &'static str,
             label: String,
             value: usize,
             on_dec: fn(&mut Editor, &ClickEvent, &mut Window, &mut Context<Editor>),
             on_inc: fn(&mut Editor, &ClickEvent, &mut Window, &mut Context<Editor>)| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(d.table_insert_stepper_gap))
                    .child(
                        div()
                            .text_size(px(t.dialog_body_size))
                            .font_weight(t.dialog_button_weight.to_font_weight())
                            .text_color(c.dialog_body)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(d.table_insert_stepper_gap))
                            .child(
                                div()
                                    .id((id_prefix, 0usize))
                                    .size(px(d.table_insert_stepper_button_size))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(d.table_insert_stepper_radius))
                                    .border(px(d.dialog_border_width))
                                    .border_color(c.dialog_border)
                                    .bg(c.dialog_secondary_button_bg)
                                    .hover(|this| this.bg(c.dialog_secondary_button_hover))
                                    .cursor_pointer()
                                    .text_color(c.dialog_secondary_button_text)
                                    .on_click(cx.listener(on_dec))
                                    .child("-"),
                            )
                            .child(
                                div()
                                    .min_w(px(d.table_insert_stepper_value_min_width))
                                    .h(px(d.table_insert_stepper_button_size))
                                    .px(px(d.table_insert_stepper_value_padding_x))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(d.table_insert_stepper_radius))
                                    .border(px(d.dialog_border_width))
                                    .border_color(c.dialog_border)
                                    .bg(c.dialog_surface)
                                    .text_size(px(t.dialog_body_size))
                                    .text_color(c.dialog_title)
                                    .child(value.to_string()),
                            )
                            .child(
                                div()
                                    .id((id_prefix, 1usize))
                                    .size(px(d.table_insert_stepper_button_size))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(d.table_insert_stepper_radius))
                                    .border(px(d.dialog_border_width))
                                    .border_color(c.dialog_border)
                                    .bg(c.dialog_secondary_button_bg)
                                    .hover(|this| this.bg(c.dialog_secondary_button_hover))
                                    .cursor_pointer()
                                    .text_color(c.dialog_secondary_button_text)
                                    .on_click(cx.listener(on_inc))
                                    .child("+"),
                            ),
                    )
            };

        Some(
            div()
                .id("table-insert-dialog-overlay")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(c.dialog_backdrop)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(Self::on_dismiss_context_menu_overlay),
                )
                .child(
                    div()
                        .w_full()
                        .px(px(d.editor_padding))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id("table-insert-dialog")
                                .w(px(d.dialog_width.min(d.table_insert_dialog_width)))
                                .max_w(relative(1.0))
                                .p(px(d.dialog_padding))
                                .flex()
                                .flex_col()
                                .gap(px(d.dialog_gap))
                                .bg(c.dialog_surface)
                                .border(px(d.dialog_border_width))
                                .border_color(c.dialog_border)
                                .rounded(px(d.dialog_radius))
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                                    cx.stop_propagation()
                                })
                                .child(
                                    div()
                                        .text_size(px(t.dialog_title_size))
                                        .font_weight(t.dialog_title_weight.to_font_weight())
                                        .text_color(c.dialog_title)
                                        .child(s.table_insert_title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(t.dialog_body_size))
                                        .font_weight(t.dialog_body_weight.to_font_weight())
                                        .text_color(c.dialog_body)
                                        .child(s.table_insert_description.clone()),
                                )
                                .child(stepper(
                                    "table-body-rows",
                                    s.table_insert_body_rows.clone(),
                                    dialog.body_rows,
                                    Self::on_table_rows_decrement,
                                    Self::on_table_rows_increment,
                                ))
                                .child(stepper(
                                    "table-columns",
                                    s.table_insert_columns.clone(),
                                    dialog.columns,
                                    Self::on_table_columns_decrement,
                                    Self::on_table_columns_increment,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(d.dialog_button_gap))
                                        .child(
                                            div()
                                                .id("cancel-table-insert-dialog")
                                                .h(px(d.dialog_button_height))
                                                .px(px(d.dialog_button_padding_x))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                                .border(px(d.dialog_border_width))
                                                .border_color(c.dialog_border)
                                                .bg(c.dialog_secondary_button_bg)
                                                .hover(|this| {
                                                    this.bg(c.dialog_secondary_button_hover)
                                                })
                                                .cursor_pointer()
                                                .text_size(px(t.dialog_button_size))
                                                .font_weight(
                                                    t.dialog_button_weight.to_font_weight(),
                                                )
                                                .text_color(c.dialog_secondary_button_text)
                                                .on_click(
                                                    cx.listener(
                                                        Self::on_cancel_table_insert_dialog,
                                                    ),
                                                )
                                                .child(s.table_insert_cancel.clone()),
                                        )
                                        .child(
                                            div()
                                                .id("confirm-table-insert-dialog")
                                                .h(px(d.dialog_button_height))
                                                .px(px(d.dialog_button_padding_x))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                                .bg(c.dialog_primary_button_bg)
                                                .hover(|this| {
                                                    this.bg(c.dialog_primary_button_hover)
                                                })
                                                .cursor_pointer()
                                                .text_size(px(t.dialog_button_size))
                                                .font_weight(
                                                    t.dialog_button_weight.to_font_weight(),
                                                )
                                                .text_color(c.dialog_primary_button_text)
                                                .on_click(
                                                    cx.listener(
                                                        Self::on_confirm_table_insert_dialog,
                                                    ),
                                                )
                                                .child(s.table_insert_confirm.clone()),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Centered modal for inserting an image (Cmd/Ctrl+Shift+I). Provides a URL
    /// field (with a "选择文件" button that opens the native file picker), an
    /// optional alt-text field, and Cancel / Insert buttons.
    pub(super) fn render_image_insert_dialog_overlay(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use gpui_component::button::{Button, ButtonVariants as _};
        use gpui_component::input::Input;
        use gpui_component::{Disableable as _, Sizable as _};

        let dialog = self.image_insert_dialog.as_ref()?;
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let url_input = dialog.url.clone();
        let alt_input = dialog.alt.clone();
        // Insert is enabled only when the URL field is non-empty.
        let url_value = url_input.read(cx).value().to_string();
        let can_confirm = !url_value.trim().is_empty();

        Some(
            div()
                .id("image-insert-dialog-overlay")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(c.dialog_backdrop)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(Self::on_dismiss_context_menu_overlay),
                )
                .child(
                    div()
                        .w_full()
                        .px(px(d.editor_padding))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id("image-insert-dialog")
                                .w(px(d.dialog_width.min(d.table_insert_dialog_width)))
                                .max_w(relative(1.0))
                                .p(px(d.dialog_padding))
                                .flex()
                                .flex_col()
                                .gap(px(d.dialog_gap))
                                .bg(c.dialog_surface)
                                .border(px(d.dialog_border_width))
                                .border_color(c.dialog_border)
                                .rounded(px(d.dialog_radius))
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                                    cx.stop_propagation()
                                })
                                .child(
                                    div()
                                        .text_size(px(t.dialog_title_size))
                                        .font_weight(t.dialog_title_weight.to_font_weight())
                                        .text_color(c.dialog_title)
                                        .child("插入图片"),
                                )
                                // URL row: input + "选择文件" button.
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(d.table_insert_stepper_gap))
                                        .child(
                                            div()
                                                .text_size(px(t.dialog_body_size))
                                                .font_weight(t.dialog_body_weight.to_font_weight())
                                                .text_color(c.dialog_body)
                                                .child("图片地址"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(d.dialog_button_gap))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .child(Input::new(&url_input).small()),
                                                )
                                                .child(
                                                    Button::new("image-pick-file")
                                                        .small()
                                                        .ghost()
                                                        .label("选择文件")
                                                        .on_click(
                                                            cx.listener(Self::on_pick_image_file),
                                                        ),
                                                ),
                                        ),
                                )
                                // Alt text row.
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(d.table_insert_stepper_gap))
                                        .child(
                                            div()
                                                .text_size(px(t.dialog_body_size))
                                                .font_weight(t.dialog_body_weight.to_font_weight())
                                                .text_color(c.dialog_body)
                                                .child("替代文本"),
                                        )
                                        .child(Input::new(&alt_input).small()),
                                )
                                // Cancel / Insert buttons.
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(d.dialog_button_gap))
                                        .child(
                                            div()
                                                .id("cancel-image-insert-dialog")
                                                .h(px(d.dialog_button_height))
                                                .px(px(d.dialog_button_padding_x))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                                .border(px(d.dialog_border_width))
                                                .border_color(c.dialog_border)
                                                .bg(c.dialog_secondary_button_bg)
                                                .hover(|this| {
                                                    this.bg(c.dialog_secondary_button_hover)
                                                })
                                                .cursor_pointer()
                                                .text_size(px(t.dialog_button_size))
                                                .font_weight(
                                                    t.dialog_button_weight.to_font_weight(),
                                                )
                                                .text_color(c.dialog_secondary_button_text)
                                                .on_click(
                                                    cx.listener(
                                                        Self::on_cancel_image_insert_dialog,
                                                    ),
                                                )
                                                .child("取消"),
                                        )
                                        .child(
                                            div()
                                                .id("confirm-image-insert-dialog")
                                                .h(px(d.dialog_button_height))
                                                .px(px(d.dialog_button_padding_x))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                                .border(px(d.dialog_border_width))
                                                .border_color(c.dialog_border)
                                                .bg(if can_confirm {
                                                    c.dialog_primary_button_bg
                                                } else {
                                                    c.dialog_border.opacity(0.5)
                                                })
                                                .hover(|this| {
                                                    this.bg(c.dialog_primary_button_hover)
                                                })
                                                .cursor_pointer()
                                                .text_size(px(t.dialog_button_size))
                                                .font_weight(
                                                    t.dialog_button_weight.to_font_weight(),
                                                )
                                                .text_color(c.dialog_primary_button_text)
                                                .on_click(
                                                    cx.listener(
                                                        Self::on_confirm_image_insert_dialog,
                                                    ),
                                                )
                                                .child("插入"),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(all(test, feature = "wysiwyg-gpui-tests"))]
mod tests {
    use super::{ContextMenuState, Editor, TableInsertTarget};
    use gpui::{AppContext, Point, TestAppContext, px};

    #[gpui::test]
    async fn context_submenu_stays_open_while_crossing_hover_gap(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "alpha".to_string(), None));

        editor.update(cx, |editor, cx| {
            editor.open_insert_context_menu(
                Point {
                    x: px(24.0),
                    y: px(24.0),
                },
                TableInsertTarget::Append,
                cx,
            );

            editor.set_context_menu_hover_state(true, false, cx);
            let Some(ContextMenuState::Insert { submenu_open, .. }) = editor.context_menu.as_ref()
            else {
                panic!("expected insert context menu");
            };
            assert!(*submenu_open);
            assert!(editor.context_menu_submenu_close_task.is_none());

            editor.set_context_menu_hover_state(false, false, cx);
            let Some(ContextMenuState::Insert { submenu_open, .. }) = editor.context_menu.as_ref()
            else {
                panic!("expected insert context menu");
            };
            assert!(*submenu_open);
            assert!(editor.context_menu_submenu_close_task.is_some());

            editor.set_context_menu_hover_state(true, true, cx);
            let Some(ContextMenuState::Insert { submenu_open, .. }) = editor.context_menu.as_ref()
            else {
                panic!("expected insert context menu");
            };
            assert!(*submenu_open);
            assert!(editor.context_menu_submenu_close_task.is_none());
        });
    }
}
