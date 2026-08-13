//! Editor-side handling for [`BlockEvent`] values emitted by child blocks.
//!
//! This is the central mutation engine for split, merge, indent, outdent,
//! delete, multiline paste, focus transfer, and dirty-state tracking. Runtime
//! tree mutations are delegated to [`DocumentTree`](super::tree::DocumentTree)
//! so visible-order metadata stays in sync with every edit.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow};
use gpui::*;

use super::Editor;
use crate::wysiwyg::components::{
    BlockEvent, BlockKind, BlockRecord, CollapsedCaretAffinity, IndentBlock, InlineTextTree,
    OutdentBlock, PastedImageSource, TableCellPosition, is_table_row_candidate,
    parse_root_table_region, parse_table_body_row,
};
use crate::wysiwyg::config::ImagePasteBehavior;

impl Editor {
    fn focused_block_for_tab_key(
        &self,
        window: &mut Window,
        cx: &App,
    ) -> Option<Entity<super::Block>> {
        let is_focused = |block: &Entity<super::Block>| {
            let block = block.read(cx);
            block.focus_handle.is_focused(window)
                || block.code_language_focus_handle.is_focused(window)
        };

        if let Some(block) = self
            .active_entity_id
            .and_then(|entity_id| self.focusable_entity_by_id(entity_id))
            .filter(is_focused)
        {
            return Some(block);
        }

        for binding in self.table_cells.values() {
            if is_focused(&binding.cell) {
                return Some(binding.cell.clone());
            }
        }

        self.document
            .visible_blocks()
            .iter()
            .find_map(|visible| is_focused(&visible.entity).then(|| visible.entity.clone()))
    }

    pub(crate) fn on_editor_key_down_capture(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key != "tab" {
            return;
        }

        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || modifiers.function {
            return;
        }

        let Some(target) = self.focused_block_for_tab_key(window, cx) else {
            return;
        };

        let handles_tab = {
            let block = target.read(cx);
            if block.code_language_focus_handle.is_focused(window) {
                cx.stop_propagation();
                return;
            }
            block.is_table_cell()
                || block.kind().is_list_item()
                || block.kind() == BlockKind::Paragraph
                || block.kind().is_code_block()
        };

        if !handles_tab {
            return;
        }

        if modifiers.shift {
            target.update(cx, |block, block_cx| {
                block.on_outdent_block(&OutdentBlock, window, block_cx);
            });
        } else {
            target.update(cx, |block, block_cx| {
                block.on_indent_block(&IndentBlock, window, block_cx);
            });
        }
        cx.stop_propagation();
    }

    fn build_plain_paste_blocks_from_lines(
        cx: &mut Context<Self>,
        lines: &[String],
    ) -> Vec<Entity<super::Block>> {
        let mut blocks = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                Self::new_block(
                    cx,
                    BlockRecord::new(BlockKind::Paragraph, InlineTextTree::from_markdown(line)),
                )
            })
            .collect::<Vec<_>>();

        if blocks.is_empty() && !lines.is_empty() {
            blocks.push(Self::new_block(
                cx,
                BlockRecord::new(BlockKind::Paragraph, InlineTextTree::plain(String::new())),
            ));
        }

        blocks
    }

    pub(super) fn block_is_quote_structure_related(
        &self,
        block: &Entity<super::Block>,
        cx: &App,
    ) -> bool {
        if self.view_mode != super::ViewMode::Rendered {
            return false;
        }

        let block_ref = block.read(cx);
        block_ref.kind().is_quote_container()
            || block_ref.quote_depth > 0
            || block_ref.quote_group_anchor.is_some()
    }

    fn refresh_rendered_quote_metadata_if_needed(
        &mut self,
        block: &Entity<super::Block>,
        cx: &mut Context<Self>,
    ) {
        if !self.block_is_quote_structure_related(block, cx) {
            return;
        }

        self.document.rebuild_metadata_and_snapshot(cx);
    }

    fn rendered_quote_text_requires_reparse(block: &Entity<super::Block>, cx: &App) -> bool {
        let block_ref = block.read(cx);
        if block_ref.quote_depth == 0 && !block_ref.kind().is_quote_container() {
            return false;
        }

        let text = block_ref.display_text();
        if !text.contains('\n') {
            return false;
        }

        text.split('\n').skip(1).any(|line| {
            let trimmed_end = line.trim_end();
            if trimmed_end.is_empty() {
                return false;
            }

            let leading_spaces = trimmed_end.bytes().take_while(|b| *b == b' ').count();
            if leading_spaces >= 4 {
                return true;
            }

            BlockKind::detect_markdown_shortcut(&format!("{trimmed_end} "))
                .is_some_and(|(kind, _)| kind != BlockKind::Paragraph)
                || BlockKind::parse_code_fence_opening(trimmed_end).is_some()
                || BlockKind::parse_separator_line(trimmed_end)
                || BlockKind::parse_atx_heading_line(trimmed_end).is_some()
        })
    }

    fn block_event_clears_cross_block_selection(event: &BlockEvent) -> bool {
        matches!(
            event,
            BlockEvent::Changed
                | BlockEvent::RequestNewline { .. }
                | BlockEvent::RequestEnterCalloutBody
                | BlockEvent::RequestQuoteBreak
                | BlockEvent::RequestCalloutBreak
                | BlockEvent::RequestMergeIntoPrev { .. }
                | BlockEvent::RequestPasteMultiline { .. }
                | BlockEvent::RequestPasteImage { .. }
                | BlockEvent::RequestPasteVideo { .. }
                | BlockEvent::RequestIndent
                | BlockEvent::RequestOutdent
                | BlockEvent::RequestDowngradeNestedListItemToChildParagraph
                | BlockEvent::ToggleTaskChecked
                | BlockEvent::RequestAppendTableColumn
                | BlockEvent::RequestAppendTableRow
                | BlockEvent::RequestInsertTableRowBelow
                | BlockEvent::RequestInsertTableColumnRight
                | BlockEvent::RequestDelete
        )
    }

    pub(crate) fn focus_block(&mut self, entity_id: EntityId) {
        self.active_entity_id = Some(entity_id);
        self.pending_scroll_active_block_into_view = true;
        // While the find bar is open it must keep keyboard focus on its query
        // input: FindNext/FindPrevious (Cmd-G / F3 / Cmd-Shift-G / Shift-F3)
        // only fire under the `EditorFindBar` key context, active while the
        // input is focused. Honoring `pending_focus` here would call
        // `focus_handle.focus()` from `apply_pending_focus`, stealing focus
        // back to the matched block and deactivating the bar's context — the
        // user's next "next match" keystroke would land in the editor instead,
        // reading as "find closed itself." The match highlights and scroll still
        // work: the block's `selected_range` paints the selection rectangle,
        // and `ensure_focused_caret_visible` falls back to `active_entity_id`
        // to scroll to the match.
        if self.find_state.is_none() {
            self.pending_focus = Some(entity_id);
        }
    }

    /// Focus a block in response to a pointer click without requesting a
    /// scroll-into-view. Every `BlockEvent::RequestFocus` emitter is a
    /// `mouse_down` handler (text click, image click, footnote backref,
    /// task checkbox): the user clicked something already on screen, so
    /// scrolling would only ever move content away from where they just
    /// pointed. The plain `focus_block` path sets
    /// `pending_scroll_active_block_into_view`, which `ensure_focused_caret_visible`
    /// honors using the block's `last_bounds` — and for blocks whose layout
    /// changes on focus (HtmlBlock raw↔rendered, inline-projection expand)
    /// those bounds describe the *pre-focus* layout, so the computed
    /// caret position can fall outside the viewport and trigger a spurious
    /// scroll that yanks the just-clicked block out of view. Skipping the
    /// scroll request on click removes that whole failure mode: the click
    /// already placed the caret at a visible spot, so no recentering is wanted.
    pub(super) fn focus_block_from_click(&mut self, entity_id: EntityId, _cx: &mut Context<Self>) {
        self.active_entity_id = Some(entity_id);
        // Record the pre-click scroll offset. Focusing a source-raw block
        // (HtmlBlock, …) flips rendered HTML (tall, with images) to raw source
        // (short text), shrinking the document and lowering `max_offset`; GPUI
        // then clamps the current offset to the new smaller range on the next
        // prepaint, yanking the view up. Restoring this offset on the following
        // render (clamped to the new range) cancels that jump, so the clicked
        // block stays where the user pointed.
        self.click_scroll_anchor = Some(super::ClickScrollAnchor {
            pre_offset_y: f32::from(self.scroll_handle.offset().y),
            pending: true,
        });
        if self.find_state.is_none() {
            self.pending_focus = Some(entity_id);
        }
    }

    fn reset_block_cursor(block: &Entity<super::Block>, cursor: usize, cx: &mut Context<Self>) {
        block.update(cx, move |block, cx| {
            block.selected_range = cursor..cursor;
            block.selection_reversed = false;
            block.marked_range = None;
            block.vertical_motion_x = None;
            block.cursor_blink_epoch = Instant::now();
            cx.notify();
        });
    }

    pub(super) fn focus_block_range(
        &mut self,
        block: &Entity<super::Block>,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        block.update(cx, move |block, cx| {
            block.selected_range = range.clone();
            block.selection_reversed = false;
            block.marked_range = None;
            block.vertical_motion_x = None;
            block.cursor_blink_epoch = Instant::now();
            cx.notify();
        });
        self.focus_block(block.entity_id());
    }

    fn clipboard_image_extension(format: ImageFormat) -> &'static str {
        match format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Gif => "gif",
            ImageFormat::Svg => "svg",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Ico => "ico",
            ImageFormat::Pnm => "pnm",
        }
    }

    /// Persist a pasted/dropped image into the shared media store and return the
    /// portable markdown source (`./media/images/<sparkid>.<ext>`).
    ///
    /// Both clipboard image bytes and imported local-file paths are copied into
    /// `<media_root>/images/` with a SparkID name so notes stay portable and
    /// self-contained across machines.
    fn materialize_pasted_image(
        &self,
        source: &PastedImageSource,
    ) -> anyhow::Result<crate::media::local::StoredMedia> {
        use crate::media::{config, local, path::MediaKind};

        let cfg = config::load_media_config();
        let root = config::resolved_media_root(&cfg)?;

        match source {
            PastedImageSource::ClipboardImage(image) => {
                let ext = Self::clipboard_image_extension(image.format);
                local::store_local(&root, MediaKind::Image, &image.bytes, ext)
            }
            PastedImageSource::LocalPath(path) => {
                let bytes = std::fs::read(path)
                    .with_context(|| format!("read image {}", path.display()))?;
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_string)
                    .or_else(|| {
                        crate::media::path::detect_image_extension(&bytes).map(str::to_string)
                    })
                    .unwrap_or_else(|| "png".to_string());
                local::store_local(&root, MediaKind::Image, &bytes, &ext)
            }
        }
    }

    fn markdown_image_target(path: &str) -> String {
        path.chars()
            .flat_map(|ch| match ch {
                '\\' | '(' | ')' | '"' => ['\\', ch].into_iter().collect::<Vec<_>>(),
                _ => [ch].into_iter().collect::<Vec<_>>(),
            })
            .collect()
    }

    /// Persist a pasted image and produce both the markdown snippet to insert
    /// and the on-disk `StoredMedia` (needed to enqueue an S3 upload).
    fn pasted_image_markdown(
        &self,
        source: &PastedImageSource,
    ) -> anyhow::Result<(String, crate::media::local::StoredMedia)> {
        use crate::media::path::MediaKind;
        let stored = self.materialize_pasted_image(source)?;
        let src = crate::media::path::local_markdown_ref(MediaKind::Image, &stored.filename);
        let markdown = format!("![image]({})", Self::markdown_image_target(&src));
        Ok((markdown, stored))
    }

    /// Rewrite an already-inserted image `src` (the portable `./media/...` path)
    /// to `new_src` (an S3 URL) across all visible blocks. The sparkid-based
    /// local path contains no markdown-special chars, so it matches the
    /// serialized text verbatim.
    pub(super) fn rewrite_media_src(&mut self, old_src: &str, new_src: &str, cx: &mut Context<Self>) {
        let mut changed = false;
        for visible in self.document.visible_blocks() {
            let block = visible.entity.clone();
            let new_tree = block.read_with(cx, |b, _cx| {
                let md = b.record.title_markdown();
                if !md.contains(old_src) {
                    return None;
                }
                let new_md = md.replacen(old_src, new_src, 1);
                let tree = if b.uses_raw_text_editing() || b.kind().is_code_block() {
                    InlineTextTree::plain(new_md)
                } else {
                    InlineTextTree::from_markdown(&new_md)
                };
                Some(tree)
            });
            if let Some(tree) = new_tree {
                let _ = block.update(cx, |b, cx| {
                    b.record.set_title(tree);
                    b.sync_render_cache();
                    cx.notify();
                });
                changed = true;
            }
        }
        if changed {
            self.rebuild_image_runtimes(cx);
            self.mark_dirty(cx);
            cx.notify();
        }
    }

    pub(super) fn show_image_paste_error(&self, err: anyhow::Error, cx: &mut Context<Self>) {
        let strings = cx
            .global::<crate::wysiwyg::i18n::I18nManager>()
            .strings()
            .clone();
        if let Some(window) = cx.active_window() {
            let ok = strings.info_dialog_ok.clone();
            let title = strings.image_paste_failed_title.clone();
            let detail = err.to_string();
            let _ = window.update(cx, |_view, window, cx| {
                let buttons = [ok.as_str()];
                let _ = window.prompt(PromptLevel::Critical, &title, Some(&detail), &buttons, cx);
            });
        } else {
            eprintln!("{}: {err}", strings.image_paste_failed_title);
        }
    }

    fn inserted_image_tree_for_block(block: &super::Block, markdown: &str) -> InlineTextTree {
        if block.uses_raw_text_editing() || block.kind().is_code_block() {
            InlineTextTree::plain(markdown.to_string())
        } else {
            InlineTextTree::from_markdown(markdown)
        }
    }

    fn replace_current_block_selection_with_image_text(
        &mut self,
        block: &Entity<super::Block>,
        leading: &InlineTextTree,
        markdown: &str,
        trailing: &InlineTextTree,
        cx: &mut Context<Self>,
    ) {
        let (kind, title, cursor) = block.read_with(cx, |block, _cx| {
            let mut title = leading.clone();
            title.append_tree(Self::inserted_image_tree_for_block(block, markdown));
            let cursor = title.visible_len();
            title.append_tree(trailing.clone());
            (block.kind(), title, cursor)
        });
        Self::set_block_title_and_kind(block, kind, title, cursor, cx);
        if let Some(binding) = self.table_cell_binding(block.entity_id()) {
            self.sync_table_record_from_runtime(&binding.table_block, cx);
        }
        self.focus_block(block.entity_id());
        self.rebuild_image_runtimes(cx);
    }

    fn insert_image_block_after_paragraph(
        &mut self,
        block: &Entity<super::Block>,
        leading: &InlineTextTree,
        markdown: &str,
        trailing: &InlineTextTree,
        cx: &mut Context<Self>,
    ) {
        let Some(location) = self.document.find_block_location(block.entity_id()) else {
            return;
        };
        let leading_empty = leading.visible_len() == 0;
        let trailing_empty = trailing.visible_len() == 0;

        if leading_empty {
            Self::set_block_title_and_kind(
                block,
                BlockKind::Paragraph,
                InlineTextTree::plain(markdown.to_string()),
                markdown.len(),
                cx,
            );
            let image_block = block.clone();
            if !trailing_empty {
                let trailing_block =
                    Self::new_block(cx, BlockRecord::new(BlockKind::Paragraph, trailing.clone()));
                self.document.insert_blocks_at(
                    location.parent,
                    location.index + 1,
                    vec![trailing_block],
                    cx,
                );
            }
            self.focus_block(image_block.entity_id());
            self.rebuild_image_runtimes(cx);
            return;
        }

        Self::set_block_title_and_kind(
            block,
            BlockKind::Paragraph,
            leading.clone(),
            leading.visible_len(),
            cx,
        );
        let image_block = Self::new_block(cx, BlockRecord::paragraph(markdown.to_string()));
        let mut inserted = vec![image_block.clone()];
        if !trailing_empty {
            inserted.push(Self::new_block(
                cx,
                BlockRecord::new(BlockKind::Paragraph, trailing.clone()),
            ));
        }
        self.document
            .insert_blocks_at(location.parent, location.index + 1, inserted, cx);
        self.focus_block(image_block.entity_id());
        self.rebuild_image_runtimes(cx);
    }

    fn handle_paste_image_request(
        &mut self,
        block: Entity<super::Block>,
        leading: &InlineTextTree,
        source: &PastedImageSource,
        trailing: &InlineTextTree,
        cx: &mut Context<Self>,
    ) {
        let (markdown, stored) = match self.pasted_image_markdown(source) {
            Ok(value) => value,
            Err(err) => {
                self.show_image_paste_error(err, cx);
                return;
            }
        };

        // When image storage is set to S3, upload asynchronously. The local
        // copy renders immediately; on success its `./media/...` src is rewritten
        // to the S3 URL (and the local file removed); on failure the local copy
        // is kept and the user is notified. No media is ever lost.
        self.maybe_enqueue_image_s3_upload(stored.abs_path.clone(), stored.filename.clone(), cx);

        if self.replace_cross_block_selection_with_text(
            &markdown,
            None,
            false,
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        ) {
            return;
        }

        self.prepare_undo_capture(
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        );
        let can_insert_image_block = self.view_mode == super::ViewMode::Rendered
            && block.read(cx).kind() == BlockKind::Paragraph
            && self.table_cell_binding(block.entity_id()).is_none()
            && !block.read(cx).uses_raw_text_editing();

        if can_insert_image_block {
            self.insert_image_block_after_paragraph(&block, leading, &markdown, trailing, cx);
        } else {
            self.replace_current_block_selection_with_image_text(
                &block, leading, &markdown, trailing, cx,
            );
        }

        self.mark_dirty(cx);
        self.finalize_pending_undo_capture(cx);
        cx.notify();
    }

    /// Persist a pasted video file into the media store and produce both the
    /// `<video>` markdown to insert and the on-disk `StoredMedia`.
    fn pasted_video_markdown(
        &self,
        path: &std::path::Path,
    ) -> anyhow::Result<(String, crate::media::local::StoredMedia)> {
        use crate::media::path::MediaKind;
        let cfg = crate::media::config::load_media_config();
        let root = crate::media::config::resolved_media_root(&cfg)?;
        let bytes = std::fs::read(path)
            .with_context(|| format!("read video {}", path.display()))?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_string)
            .or_else(|| crate::media::path::detect_video_extension(&bytes).map(str::to_string))
            .unwrap_or_else(|| "mp4".to_string());
        let stored = crate::media::local::store_local(&root, MediaKind::Video, &bytes, &ext)?;
        let src = crate::media::path::local_markdown_ref(MediaKind::Video, &stored.filename);
        let markdown = crate::media::path::video_markdown(&src);
        Ok((markdown, stored))
    }

    /// Handle a pasted local video file path: import into the media store,
    /// insert a `<video>` block, and (when video storage is S3) enqueue a
    /// multipart upload.
    fn handle_paste_video_request(
        &mut self,
        block: Entity<super::Block>,
        leading: &InlineTextTree,
        path: &std::path::Path,
        trailing: &InlineTextTree,
        cx: &mut Context<Self>,
    ) {
        use crate::media::config::StorageMethod;
        use crate::media::path::MediaKind;

        let (markdown, stored) = match self.pasted_video_markdown(path) {
            Ok(value) => value,
            Err(err) => {
                self.show_image_paste_error(err, cx);
                return;
            }
        };

        let cfg = crate::media::config::load_media_config();
        if cfg.video.storage == StorageMethod::S3 && cfg.s3.is_configured() {
            let prefix = cfg.video.s3_prefix.trim().trim_matches('/');
            let key = if prefix.is_empty() {
                stored.filename.clone()
            } else {
                format!("{prefix}/{}", stored.filename)
            };
            let local_src = crate::media::path::local_markdown_ref(MediaKind::Video, &stored.filename);
            self.enqueue_video_s3_upload(
                local_src,
                stored.abs_path.clone(),
                key,
                cfg.video.multipart_part_mb,
                cfg.s3.clone(),
                cx,
            );
        }

        if self.replace_cross_block_selection_with_text(
            &markdown,
            None,
            false,
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        ) {
            return;
        }
        self.prepare_undo_capture(
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        );
        let can_insert_media_block = self.view_mode == super::ViewMode::Rendered
            && block.read(cx).kind() == BlockKind::Paragraph
            && self.table_cell_binding(block.entity_id()).is_none()
            && !block.read(cx).uses_raw_text_editing();
        if can_insert_media_block {
            self.insert_image_block_after_paragraph(&block, leading, &markdown, trailing, cx);
        } else {
            self.replace_current_block_selection_with_image_text(
                &block, leading, &markdown, trailing, cx,
            );
        }

        self.mark_dirty(cx);
        self.finalize_pending_undo_capture(cx);
        cx.notify();
    }
    /// just-written local image. On success the embedded `./media/...` path is
    /// rewritten to the S3 URL; on failure the local copy is preserved and the
    /// error is surfaced.
    pub(super) fn maybe_enqueue_image_s3_upload(
        &mut self,
        local_path: std::path::PathBuf,
        filename: String,
        cx: &mut Context<Self>,
    ) {
        use crate::media::config::StorageMethod;

        let cfg = crate::media::config::load_media_config();
        if cfg.image.storage != StorageMethod::S3 || !cfg.s3.is_configured() {
            return;
        }
        let prefix = cfg.image.s3_prefix.trim().trim_matches('/');
        let key = if prefix.is_empty() {
            filename.clone()
        } else {
            format!("{prefix}/{filename}")
        };
        let old_src = crate::media::path::local_markdown_ref(
            crate::media::path::MediaKind::Image,
            &filename,
        );
        let rx = crate::media::upload::enqueue_image_upload(
            crate::media::upload::ImageUploadRequest {
                local_path,
                key,
                s3: cfg.s3.clone(),
            },
        );
        cx.spawn(async move |this, cx| match rx.await {
            Ok(Ok(url)) => {
                let _ = this.update(cx, |editor, cx| editor.rewrite_media_src(&old_src, &url, cx));
            }
            Ok(Err(err)) => {
                let _ = this.update(cx, |editor, cx| editor.show_image_paste_error(err, cx));
            }
            Err(_) => {} // upload task dropped without sending
        })
        .detach();
    }

    fn jump_to_footnote_definition(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some(binding) = self.footnote_registry.binding(id) else {
            return false;
        };
        let Some(block) = self.focusable_entity_by_id(binding.definition_entity_id) else {
            return false;
        };
        self.focus_block_range(&block, 0..0, cx);
        true
    }

    fn jump_to_footnote_backref(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some(binding) = self.footnote_registry.binding(id) else {
            return false;
        };
        let Some(first_reference) = binding.first_reference.as_ref() else {
            return false;
        };
        let Some(block) = self.focusable_entity_by_id(first_reference.entity_id) else {
            return false;
        };
        let range = block
            .read(cx)
            .current_range_for_footnote_occurrence(first_reference.occurrence_index)
            .unwrap_or(0..0);
        self.focus_block_range(&block, range, cx);
        true
    }

    fn insert_list_group_separator_before(
        &mut self,
        entity_id: EntityId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(location) = self.document.find_block_location(entity_id) else {
            return false;
        };

        let separator = Self::new_block(cx, BlockRecord::paragraph(String::new()));
        self.document
            .insert_blocks_at(location.parent, location.index, vec![separator], cx);
        true
    }

    fn set_block_title_and_kind(
        block: &Entity<super::Block>,
        kind: BlockKind,
        title: InlineTextTree,
        cursor: usize,
        cx: &mut Context<Self>,
    ) {
        let (kind, title, cursor) = Self::apply_paragraph_shortcuts(kind, title, cursor);
        block.update(cx, move |block, cx| {
            block.record.kind = kind;
            block.record.set_title(title.clone());
            block.sync_edit_mode_from_kind();
            block.sync_render_cache();
            let clean_cursor = cursor.min(block.record.title.visible_len());
            block.selected_range = block.clean_to_current_range(clean_cursor..clean_cursor);
            block.selection_reversed = false;
            block.marked_range = None;
            block.vertical_motion_x = None;
            block.cursor_blink_epoch = Instant::now();
            cx.notify();
        });
    }

    /// A block that a setext underline below it can promote into a heading: a
    /// non-empty, single-line, plain paragraph with no children.
    fn is_setext_heading_target(block: &Entity<super::Block>, cx: &App) -> bool {
        let block = block.read(cx);
        if block.kind() != BlockKind::Paragraph || !block.children.is_empty() {
            return false;
        }
        let text = block.record.title.visible_text();
        !text.trim().is_empty() && !text.contains('\n')
    }

    /// Handles Enter pressed on a paragraph that is a pure setext underline.
    /// When a matching paragraph precedes it at the root, the two collapse into
    /// a heading; a lone dash run still falls back to a thematic break. Returns
    /// true when it consumed the newline.
    fn try_form_setext_heading_on_newline(
        &mut self,
        block: &Entity<super::Block>,
        cx: &mut Context<Self>,
    ) -> bool {
        let text = block.read(cx).display_text().to_string();
        let Some(level) = BlockKind::parse_setext_underline(&text) else {
            return false;
        };
        if block.read(cx).kind() != BlockKind::Paragraph {
            return false;
        }
        let Some(location) = self.document.find_block_location(block.entity_id()) else {
            return false;
        };

        // Only root paragraphs auto-form headings; nested contexts (quotes,
        // lists) keep their existing newline behavior.
        let target = if location.parent.is_none() {
            self.document
                .previous_sibling(block.entity_id(), cx)
                .filter(|prev| Self::is_setext_heading_target(prev, cx))
        } else {
            None
        };

        // A `=` underline with no heading target is ordinary text: defer to the
        // normal newline split. A dash run still has to become a separator.
        if target.is_none() && !BlockKind::parse_separator_line(&text) {
            return false;
        }

        // The newline's own capture was already finalized by the block's Changed
        // event (nothing had changed yet), so start a fresh one here that spans
        // the heading/separator conversion. prepare is a no-op if one is pending.
        self.prepare_undo_capture(
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        );

        if let Some(prev) = target {
            let heading_title = prev.read(cx).record.title.clone();
            let cursor = heading_title.visible_len();
            let removed_id = block.entity_id();
            let new_paragraph = Self::new_block(cx, BlockRecord::paragraph(String::new()));

            Self::set_block_title_and_kind(
                &prev,
                BlockKind::Heading { level },
                heading_title,
                cursor,
                cx,
            );
            self.document.with_structure_mutation(cx, |document, cx| {
                let _ = document.remove_block_by_id_raw(removed_id, cx);
            });
            if let Some(heading_location) = self.document.find_block_location(prev.entity_id()) {
                self.document.insert_blocks_at(
                    heading_location.parent,
                    heading_location.index + 1,
                    vec![new_paragraph.clone()],
                    cx,
                );
            }
            self.focus_block(new_paragraph.entity_id());
        } else {
            block.update(cx, |block, _cx| block.make_separator());
            let new_paragraph = Self::new_block(cx, BlockRecord::paragraph(String::new()));
            self.document.insert_blocks_at(
                location.parent,
                location.index + 1,
                vec![new_paragraph.clone()],
                cx,
            );
            self.focus_block(new_paragraph.entity_id());
        }

        self.rebuild_image_runtimes(cx);
        self.mark_dirty(cx);
        self.finalize_pending_undo_capture(cx);
        cx.notify();
        true
    }

    /// Handles Enter pressed on a paragraph that is a pipe-table row. A
    /// delimiter row under a header paragraph forms a native table; a body row
    /// directly under an existing table is absorbed into it. After either, the
    /// caret lands in a fresh paragraph below the table so consecutive rows can
    /// be typed. Returns true when it consumed the newline.
    fn try_form_or_extend_table_on_newline(
        &mut self,
        block: &Entity<super::Block>,
        cx: &mut Context<Self>,
    ) -> bool {
        let text = block.read(cx).display_text().to_string();
        if block.read(cx).kind() != BlockKind::Paragraph || !is_table_row_candidate(&text) {
            return false;
        }
        let Some(location) = self.document.find_block_location(block.entity_id()) else {
            return false;
        };
        if location.parent.is_some() {
            return false;
        }
        let Some(prev) = self.document.previous_sibling(block.entity_id(), cx) else {
            return false;
        };

        if prev.read(cx).kind() == BlockKind::Table {
            // A multi-column row typed directly under a table is meant as a row,
            // so absorb it and let the table normalize ragged cell counts the
            // same way pasted rows are padded or truncated to the header width.
            return self.extend_table_with_typed_row(&prev, block, &text, cx);
        }

        if prev.read(cx).kind() != BlockKind::Paragraph {
            return false;
        }
        let header_text = prev.read(cx).display_text().to_string();
        if !is_table_row_candidate(&header_text) {
            return false;
        }
        let Some(table) = parse_root_table_region(&[header_text, text]) else {
            return false;
        };

        self.prepare_undo_capture(
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        );
        // Remove the lower (delimiter) block first so the header index is stable.
        let header_index = location.index - 1;
        let removed_delimiter = block.entity_id();
        let removed_header = prev.entity_id();
        let table_block = Self::new_table_block(cx, table);
        let new_paragraph = Self::new_block(cx, BlockRecord::paragraph(String::new()));
        self.document.with_structure_mutation(cx, |document, cx| {
            let _ = document.remove_block_by_id_raw(removed_delimiter, cx);
            let _ = document.remove_block_by_id_raw(removed_header, cx);
        });
        self.document.insert_blocks_at(
            None,
            header_index,
            vec![table_block.clone(), new_paragraph.clone()],
            cx,
        );
        self.rebuild_table_runtimes(cx);
        self.focus_block(new_paragraph.entity_id());
        self.mark_dirty(cx);
        self.finalize_pending_undo_capture(cx);
        cx.notify();
        true
    }

    fn extend_table_with_typed_row(
        &mut self,
        table_block: &Entity<super::Block>,
        row_block: &Entity<super::Block>,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        // Capture any in-progress cell edits before mutating the record.
        self.sync_table_record_from_runtime(table_block, cx);
        let Some(mut table) = table_block.read(cx).record.table.clone() else {
            return false;
        };
        let Some(row) = parse_table_body_row(text, table.column_count()) else {
            return false;
        };

        self.prepare_undo_capture(
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        );
        table.rows.push(row);
        table_block.update(cx, |block, cx| {
            block.record.table = Some(table);
            cx.notify();
        });

        let removed_id = row_block.entity_id();
        self.document.with_structure_mutation(cx, |document, cx| {
            let _ = document.remove_block_by_id_raw(removed_id, cx);
        });
        let new_paragraph = Self::new_block(cx, BlockRecord::paragraph(String::new()));
        if let Some(table_location) = self.document.find_block_location(table_block.entity_id()) {
            self.document.insert_blocks_at(
                table_location.parent,
                table_location.index + 1,
                vec![new_paragraph.clone()],
                cx,
            );
        }
        self.rebuild_table_runtimes(cx);
        self.focus_block(new_paragraph.entity_id());
        self.mark_dirty(cx);
        self.finalize_pending_undo_capture(cx);
        cx.notify();
        true
    }

    /// Inserts an empty paragraph after `block` when it renders as a
    /// self-contained structure the caret cannot move past (table, code, math,
    /// separator, quote, callout, footnote definition, standalone image, ...)
    /// and nothing currently follows it in its container. This keeps a rendered
    /// document from ending on such a block, so a rendered-first user can keep
    /// typing past it rather than being stranded. No-op when something already
    /// follows the block or it is not a stranding structure.
    pub(super) fn ensure_trailing_paragraph_after_structural(
        &mut self,
        block: &Entity<super::Block>,
        cx: &mut Context<Self>,
    ) {
        let strands = {
            let block = block.read(cx);
            let kind = block.kind();
            kind.is_atomic_structural()
                || kind.is_quote_container()
                || kind.is_footnote_definition()
                || block.renders_as_standalone_image()
        };
        if !strands {
            return;
        }
        let Some(location) = self.document.find_block_location(block.entity_id()) else {
            return;
        };
        let sibling_count = match location.parent.as_ref() {
            Some(parent) => parent.read(cx).children.len(),
            None => self.document.root_count(),
        };
        if location.index + 1 < sibling_count {
            return;
        }
        let trailing = Self::new_block(cx, BlockRecord::paragraph(String::new()));
        self.document
            .insert_blocks_at(location.parent, location.index + 1, vec![trailing], cx);
    }

    fn apply_paragraph_shortcuts(
        kind: BlockKind,
        mut title: InlineTextTree,
        cursor: usize,
    ) -> (BlockKind, InlineTextTree, usize) {
        if kind == BlockKind::Paragraph {
            let visible_text = title.visible_text();
            if let Some((detected_kind, prefix_len)) =
                BlockKind::detect_markdown_shortcut(&visible_text)
            {
                title.remove_visible_prefix(prefix_len);
                return (detected_kind, title, cursor.saturating_sub(prefix_len));
            }
        }

        (kind, title, cursor)
    }

    pub(crate) fn bump_scrollbar_visibility(&mut self, cx: &mut Context<Self>) {
        let duration = Duration::from_millis(900);
        self.scrollbar_visible_until = Instant::now() + duration;

        let weak_editor = cx.entity().downgrade();
        self.scrollbar_fade_task = Some(cx.spawn(
            async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(duration + Duration::from_millis(50))
                    .await;
                let _ = weak_editor.update(cx, |this, cx| {
                    this.scrollbar_fade_task = None;
                    // Mark this as a view-only (scrollbar fade) notify so host
                    // observers can skip rebuilding on it.
                    this.scroll_revision = this.scroll_revision.wrapping_add(1);
                    cx.notify();
                });
            },
        ));

        // Bump before notifying so host observers can recognise this as a
        // view-only scroll notify and skip a full rebuild.
        self.scroll_revision = self.scroll_revision.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn on_editor_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scrollbar_hovered = *hovered;
        if *hovered {
            self.bump_scrollbar_visibility(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn on_menu_bar_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_menu_bar_hovered(*hovered, cx);
    }

    pub(crate) fn on_menu_panel_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_menu_panel_hovered(*hovered, cx);
    }

    pub(crate) fn on_menu_submenu_panel_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_menu_submenu_panel_hovered(*hovered, cx);
    }

    pub(crate) fn on_menu_submenu_bridge_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_menu_submenu_bridge_hovered(*hovered, cx);
    }

    pub(crate) fn on_editor_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_menu_bar_from_body(cx);
        self.clear_table_axis_preview(cx);
        self.clear_table_axis_selection(cx);
    }

    pub(crate) fn on_editor_scroll_wheel(
        &mut self,
        _event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bump_scrollbar_visibility(cx);
    }

    pub(crate) fn on_page_up(
        &mut self,
        _: &crate::wysiwyg::components::PageUp,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = self.scroll_handle.bounds().size.height;
        self.scroll_viewport_by(page, cx);
    }

    pub(crate) fn on_page_down(
        &mut self,
        _: &crate::wysiwyg::components::PageDown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = self.scroll_handle.bounds().size.height;
        self.scroll_viewport_by(-page, cx);
    }

    pub(crate) fn on_jump_to_top(
        &mut self,
        _: &crate::wysiwyg::components::JumpToTop,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_vertical_scroll_offset(px(0.0), cx);
    }

    pub(crate) fn on_jump_to_bottom(
        &mut self,
        _: &crate::wysiwyg::components::JumpToBottom,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let max_offset_y = self.scroll_handle.max_offset().y.max(px(0.0));
        self.set_vertical_scroll_offset(-max_offset_y, cx);
    }

    /// Scrolls the viewport vertically by `delta`. A positive `delta` moves
    /// toward the start of the document; a negative one moves toward the end.
    /// One page is the current viewport height, so the step tracks window size.
    fn scroll_viewport_by(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        let target = self.scroll_handle.offset().y + delta;
        self.set_vertical_scroll_offset(target, cx);
    }

    /// Applies an absolute vertical scroll offset, clamped to the scrollable
    /// range. Offsets run from 0 at the top to `-max_offset` at the bottom.
    fn set_vertical_scroll_offset(&mut self, target_y: Pixels, cx: &mut Context<Self>) {
        let max_offset_y = self.scroll_handle.max_offset().y.max(px(0.0));
        let mut offset = self.scroll_handle.offset();
        offset.y = target_y.min(px(0.0)).max(-max_offset_y);
        self.scroll_handle.set_offset(offset);
        // A direct viewport scroll should stick, so cancel any queued pass that
        // would otherwise re-center the active block on the next frame.
        self.pending_scroll_active_block_into_view = false;
        self.pending_scroll_recheck_after_layout = false;
        self.bump_scrollbar_visibility(cx);
        // `bump_scrollbar_visibility` already notified (and bumped the scroll
        // revision); bump again so this follow-up notify is also recognised as
        // view-only by host observers.
        self.scroll_revision = self.scroll_revision.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn start_scrollbar_drag(
        &mut self,
        pointer_offset_y: f32,
        track_height: f32,
        thumb_height: f32,
        max_scroll_y: f32,
        cx: &mut Context<Self>,
    ) {
        self.scrollbar_drag = Some(super::ScrollbarDragSession {
            pointer_offset_y: pointer_offset_y.clamp(0.0, thumb_height.max(0.0)),
            track_height,
            thumb_height,
            max_scroll_y,
        });
        self.pending_scroll_active_block_into_view = false;
        self.pending_scroll_recheck_after_layout = false;
        self.bump_scrollbar_visibility(cx);
        // `bump_scrollbar_visibility` already notified (and bumped the scroll
        // revision); bump again so this follow-up notify is also recognised as
        // view-only by host observers.
        self.scroll_revision = self.scroll_revision.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn update_scrollbar_drag(
        &mut self,
        pointer_y_in_track: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.scrollbar_drag else {
            return;
        };

        let travel = (drag.track_height - drag.thumb_height).max(0.0);
        let thumb_top = (pointer_y_in_track - drag.pointer_offset_y).clamp(0.0, travel);
        let scroll_y = Self::scroll_offset_for_thumb_top(
            thumb_top,
            drag.track_height,
            drag.thumb_height,
            drag.max_scroll_y,
        );

        let mut offset = self.scroll_handle.offset();
        offset.y = -px(scroll_y);
        self.scroll_handle.set_offset(offset);
        self.bump_scrollbar_visibility(cx);
        // Follow-up view-only scroll notify (drag) — bump so host observers
        // skip rebuilding on it.
        self.scroll_revision = self.scroll_revision.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn end_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        if self.scrollbar_drag.take().is_some() {
            self.bump_scrollbar_visibility(cx);
            self.scroll_revision = self.scroll_revision.wrapping_add(1);
            cx.notify();
        }
    }

    pub(super) fn focus_table_cell_position(
        &mut self,
        table_block: &Entity<super::Block>,
        position: TableCellPosition,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(cell) = table_block
            .read(cx)
            .table_runtime
            .as_ref()
            .and_then(|runtime| runtime.cell(position))
        else {
            return false;
        };
        self.focus_block(cell.entity_id());
        cx.notify();
        true
    }

    /// Focus a cell when keyboard navigation enters a table from an adjacent
    /// block. Entering from above lands on the first header cell; entering from
    /// below lands on the first cell of the last body row, falling back to the
    /// header when the table has no body rows.
    fn focus_table_entry_cell(
        &mut self,
        table_block: &Entity<super::Block>,
        from_top: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(runtime) = table_block.read(cx).table_runtime.clone() else {
            return false;
        };
        let cell = if from_top {
            runtime.header.first().cloned()
        } else {
            runtime
                .rows
                .last()
                .and_then(|row| row.first())
                .cloned()
                .or_else(|| runtime.header.first().cloned())
        };
        let Some(cell) = cell else {
            return false;
        };
        self.focus_block(cell.entity_id());
        cx.notify();
        true
    }

    /// Move focus from a table edge to the block immediately above (delta < 0)
    /// or below (delta > 0) it, mirroring how plain blocks transfer focus when
    /// the caret leaves their first or last line. When the neighbor is itself a
    /// table, drop into one of its cells so the caret stays editable instead of
    /// landing on the table container. `to_block_start` lands the caret at the
    /// neighbor's start (Block Up/Down semantics) rather than the nearest edge
    /// (Move Up/Down semantics).
    fn focus_block_adjacent_to_table(
        &mut self,
        table_block: &Entity<super::Block>,
        delta: i32,
        to_block_start: bool,
        cx: &mut Context<Self>,
    ) {
        let visible = self.document.flatten_visible_blocks();
        let Some(index) = visible
            .iter()
            .position(|visible| visible.entity.entity_id() == table_block.entity_id())
        else {
            return;
        };
        let target_index = if delta < 0 {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let Some(target) = target_index
            .and_then(|target_index| visible.get(target_index))
            .map(|visible| visible.entity.clone())
        else {
            return;
        };
        if target.read(cx).kind() == BlockKind::Table
            && self.focus_table_entry_cell(&target, delta > 0, cx)
        {
            return;
        }
        self.focus_block(target.entity_id());
        if to_block_start {
            target.update(cx, |target, cx| target.move_to(0, cx));
        } else {
            let prefer_last_line = delta < 0;
            let offset = target
                .read(cx)
                .entry_offset_for_vertical_focus(prefer_last_line, None);
            target.update(cx, move |target, cx| {
                target.move_to_with_preferred_x(offset, None, cx);
            });
        }
        cx.notify();
    }

    fn focus_table_cell_horizontal_neighbor(
        &mut self,
        table_block: &Entity<super::Block>,
        position: TableCellPosition,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = table_block.read(cx).table_runtime.clone() else {
            return;
        };
        let columns = runtime.header.len();
        let total_rows = 1 + runtime.rows.len();
        if columns == 0 || total_rows == 0 {
            return;
        }

        let linear = position.row * columns + position.column;
        let next = if delta < 0 {
            linear.checked_sub(delta.unsigned_abs() as usize)
        } else {
            linear.checked_add(delta as usize)
        };
        let Some(next) = next else {
            return;
        };
        if next >= total_rows * columns {
            return;
        }

        let next_position = TableCellPosition {
            row: next / columns,
            column: next % columns,
        };
        let _ = self.focus_table_cell_position(table_block, next_position, cx);
    }

    fn focus_table_cell_vertical_neighbor(
        &mut self,
        table_block: &Entity<super::Block>,
        position: TableCellPosition,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = table_block.read(cx).table_runtime.clone() else {
            return;
        };
        let max_row = runtime.rows.len();
        let next_row = if delta < 0 {
            position.row.checked_sub(delta.unsigned_abs() as usize)
        } else {
            position.row.checked_add(delta as usize)
        };
        // Moving past the first/last row leaves the table for the adjacent
        // block rather than stopping at the edge.
        let Some(next_row) = next_row.filter(|row| *row <= max_row) else {
            self.focus_block_adjacent_to_table(table_block, delta, false, cx);
            return;
        };

        let next_position = TableCellPosition {
            row: next_row,
            column: position.column.min(runtime.header.len().saturating_sub(1)),
        };
        let _ = self.focus_table_cell_position(table_block, next_position, cx);
    }

    fn on_table_cell_event(
        &mut self,
        binding: super::TableCellBinding,
        event: &BlockEvent,
        cx: &mut Context<Self>,
    ) {
        if Self::block_event_clears_cross_block_selection(event) {
            self.rendered_select_all_cycle = None;
            self.clear_cross_block_selection(cx);
        }

        match event {
            BlockEvent::Changed => {
                self.sync_table_record_from_runtime(&binding.table_block, cx);
                self.rebuild_image_runtimes(cx);
                self.mark_dirty(cx);
                self.request_active_block_scroll_into_view(cx);
                self.finalize_pending_undo_capture(cx);
            }
            BlockEvent::RequestOpenLink {
                prompt_target,
                open_target,
            } => {
                self.request_open_link_prompt(prompt_target.clone(), open_target.clone(), cx);
            }
            BlockEvent::RequestJumpToFootnoteDefinition { id, .. } => {
                let _ = self.jump_to_footnote_definition(id, cx);
            }
            BlockEvent::RequestJumpToFootnoteBackref { id } => {
                let _ = self.jump_to_footnote_backref(id, cx);
            }
            BlockEvent::RequestTableCellMoveHorizontal { delta } => {
                self.focus_table_cell_horizontal_neighbor(
                    &binding.table_block,
                    binding.position,
                    *delta,
                    cx,
                );
            }
            BlockEvent::RequestTableCellMoveVertical { delta } => {
                self.focus_table_cell_vertical_neighbor(
                    &binding.table_block,
                    binding.position,
                    *delta,
                    cx,
                );
            }
            BlockEvent::RequestNewline { .. } => {
                let Some(location) = self
                    .document
                    .find_block_location(binding.table_block.entity_id())
                else {
                    return;
                };
                self.clear_table_axis_preview(cx);
                self.clear_table_axis_selection(cx);
                self.sync_table_record_from_runtime(&binding.table_block, cx);
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );
                let new_block = Self::new_block(cx, BlockRecord::paragraph(String::new()));
                self.document.insert_blocks_at(
                    location.parent,
                    location.index + 1,
                    vec![new_block.clone()],
                    cx,
                );
                self.rebuild_image_runtimes(cx);
                self.focus_block(new_block.entity_id());
                self.mark_dirty(cx);
                self.request_active_block_scroll_into_view(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestFocus => {
                self.close_menu_bar(cx);
                self.clear_table_axis_preview(cx);
                self.clear_table_axis_selection(cx);
                self.focus_block_from_click(binding.cell.entity_id(), cx);
                cx.notify();
            }
            BlockEvent::RequestFocusPrev { .. } => {
                self.focus_table_cell_vertical_neighbor(
                    &binding.table_block,
                    binding.position,
                    -1,
                    cx,
                );
            }
            BlockEvent::RequestFocusNext { .. } => {
                self.focus_table_cell_vertical_neighbor(
                    &binding.table_block,
                    binding.position,
                    1,
                    cx,
                );
            }
            // Block Up/Down treat the table as a single block: leave it
            // entirely for the block above/below rather than stepping by cell.
            BlockEvent::RequestBlockUp => {
                self.focus_block_adjacent_to_table(&binding.table_block, -1, true, cx);
            }
            BlockEvent::RequestBlockDown => {
                self.focus_block_adjacent_to_table(&binding.table_block, 1, true, cx);
            }
            _ => {}
        }
    }

    /// Handler for events emitted by a child block of a focused HTML wrapper.
    /// The wrapper's inner content is a real multi-block document, so children
    /// behave like ordinary blocks for most gestures (Enter splits, Backspace
    /// merges, arrows move between children). The wrapper-specific concerns are:
    /// (1) every `Changed` re-syncs the child markdown back into the wrapper's
    /// HTML record; (2) navigation off the first/last child exits the wrapper.
    fn on_html_wrapper_child_event(
        &mut self,
        binding: super::HtmlWrapperChildBinding,
        event: &BlockEvent,
        cx: &mut Context<Self>,
    ) {
        if let BlockEvent::PrepareUndo { kind } = event {
            self.prepare_undo_capture_from_stable_snapshot(*kind);
            return;
        }

        match event {
            BlockEvent::Changed => {
                self.sync_html_wrapper_record_from_runtime(&binding.wrapper_block, cx);
                self.rebuild_image_runtimes(cx);
                self.mark_dirty(cx);
                self.request_active_block_scroll_into_view(cx);
                self.finalize_pending_undo_capture(cx);
            }
            BlockEvent::RequestFocus => {
                self.focus_block_from_click(binding.child.entity_id(), cx);
                cx.notify();
            }
            // FocusPrev off the first child → leave the wrapper upward.
            BlockEvent::RequestFocusPrev { .. } => {
                if binding.index == 0 {
                    self.focus_block_adjacent_to_html_wrapper(&binding.wrapper_block, -1, cx);
                } else if !self.focus_html_wrapper_child_neighbor(
                    &binding.wrapper_block,
                    binding.index,
                    -1,
                    cx,
                ) {
                    self.focus_block_adjacent_to_html_wrapper(&binding.wrapper_block, -1, cx);
                }
            }
            // FocusNext off the last child → leave the wrapper downward.
            BlockEvent::RequestFocusNext { .. } => {
                let last =
                    self.is_last_html_wrapper_child(&binding.wrapper_block, binding.index, cx);
                if last {
                    self.focus_block_adjacent_to_html_wrapper(&binding.wrapper_block, 1, cx);
                } else if !self.focus_html_wrapper_child_neighbor(
                    &binding.wrapper_block,
                    binding.index,
                    1,
                    cx,
                ) {
                    self.focus_block_adjacent_to_html_wrapper(&binding.wrapper_block, 1, cx);
                }
            }
            BlockEvent::RequestBlockUp => {
                if binding.index == 0 {
                    self.focus_block_adjacent_to_html_wrapper(&binding.wrapper_block, -1, cx);
                } else {
                    self.focus_html_wrapper_child_neighbor(
                        &binding.wrapper_block,
                        binding.index,
                        -1,
                        cx,
                    );
                }
            }
            BlockEvent::RequestBlockDown => {
                let last =
                    self.is_last_html_wrapper_child(&binding.wrapper_block, binding.index, cx);
                if last {
                    self.focus_block_adjacent_to_html_wrapper(&binding.wrapper_block, 1, cx);
                } else {
                    self.focus_html_wrapper_child_neighbor(
                        &binding.wrapper_block,
                        binding.index,
                        1,
                        cx,
                    );
                }
            }
            BlockEvent::RequestOpenLink {
                prompt_target,
                open_target,
            } => {
                self.request_open_link_prompt(prompt_target.clone(), open_target.clone(), cx);
            }
            BlockEvent::RequestJumpToFootnoteDefinition { id, .. } => {
                let _ = self.jump_to_footnote_definition(id, cx);
            }
            BlockEvent::RequestJumpToFootnoteBackref { id } => {
                let _ = self.jump_to_footnote_backref(id, cx);
            }
            // Structural events (Newline split / MergeIntoPrev / Paste / etc.)
            // are handled by the normal document-block path below: the child is
            // mutated in place, and the resulting `Changed` re-syncs the wrapper.
            // Falling through requires re-dispatch as a document block, which the
            // child is NOT (it lives out-of-band), so we re-emit the structural
            // handlers by calling the editor's block helpers directly below.
            other => {
                self.on_html_wrapper_child_structural_event(binding.clone(), other, cx);
            }
        }
    }

    fn is_last_html_wrapper_child(
        &self,
        wrapper_block: &Entity<super::Block>,
        index: usize,
        cx: &App,
    ) -> bool {
        wrapper_block
            .read(cx)
            .html_wrapper_runtime
            .as_ref()
            .map(|runtime| index + 1 >= runtime.children.len())
            .unwrap_or(true)
    }

    fn on_html_wrapper_child_structural_event(
        &mut self,
        binding: super::HtmlWrapperChildBinding,
        event: &BlockEvent,
        cx: &mut Context<Self>,
    ) {
        // Structural child events (newline split, merge, paste, indent, …) need
        // document-tree-style handling, but children live out-of-band in the
        // wrapper runtime, not in DocumentTree. The simplest correct behavior
        // for v1: let the child handle it locally (it already mutated itself
        // before emitting), then re-sync the wrapper. Most of these events are
        // preceeded by a PrepareUndo (handled above) and followed by Changed.
        match event {
            BlockEvent::RequestNewline { .. }
            | BlockEvent::RequestMergeIntoPrev { .. }
            | BlockEvent::RequestPasteMultiline { .. }
            | BlockEvent::RequestIndent
            | BlockEvent::RequestOutdent
            | BlockEvent::RequestQuoteBreak
            | BlockEvent::RequestCalloutBreak => {
                // Re-sync and mark dirty; the child's own state already reflects
                // the edit. Full structural mutation of the wrapper's child list
                // (split into two children, merge two children) is a follow-up;
                // for now the child edits in place.
                self.sync_html_wrapper_record_from_runtime(&binding.wrapper_block, cx);
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn nearest_quote_ancestor(
        &self,
        entity_id: EntityId,
        cx: &App,
    ) -> Option<Entity<super::Block>> {
        let mut current = self.focusable_entity_by_id(entity_id)?;
        loop {
            if current.read(cx).kind().is_quote_container() {
                return Some(current);
            }
            let location = self.document.find_block_location(current.entity_id())?;
            current = location.parent?;
        }
    }

    fn topmost_quote_ancestor(
        &self,
        entity_id: EntityId,
        cx: &App,
    ) -> Option<Entity<super::Block>> {
        let mut current = self.nearest_quote_ancestor(entity_id, cx)?;
        loop {
            let Some(location) = self.document.find_block_location(current.entity_id()) else {
                break;
            };
            let Some(parent) = location.parent.clone() else {
                break;
            };
            if !parent.read(cx).kind().is_quote_container() {
                break;
            }
            current = parent;
        }
        Some(current)
    }

    fn quote_break_insertion_target(
        &self,
        entity_id: EntityId,
        cx: &App,
    ) -> Option<(Option<Entity<super::Block>>, usize)> {
        let quote_block = self.nearest_quote_ancestor(entity_id, cx)?;
        let location = self.document.find_block_location(quote_block.entity_id())?;
        Some((location.parent.clone(), location.index + 1))
    }

    fn callout_break_insertion_target(
        &self,
        entity_id: EntityId,
        cx: &App,
    ) -> Option<(Option<Entity<super::Block>>, usize)> {
        let callout_root = self.topmost_quote_ancestor(entity_id, cx)?;
        let location = self
            .document
            .find_block_location(callout_root.entity_id())?;
        Some((location.parent.clone(), location.index + 1))
    }

    fn ensure_callout_body_entry(
        &mut self,
        callout: &Entity<super::Block>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<super::Block>> {
        if !matches!(callout.read(cx).kind(), BlockKind::Callout(_)) {
            return None;
        }

        if let Some(first_child) = callout.read(cx).children.first().cloned() {
            return Some(first_child);
        }

        let body = Self::new_block(cx, BlockRecord::paragraph(String::new()));
        self.document
            .insert_blocks_at(Some(callout.clone()), 0, vec![body.clone()], cx);
        Some(body)
    }

    fn materialize_empty_callout_shortcut(
        &mut self,
        block: &Entity<super::Block>,
        cx: &mut Context<Self>,
    ) -> Option<EntityId> {
        if self.view_mode != super::ViewMode::Rendered {
            return None;
        }

        let (kind, title_markdown, has_children) = block.read_with(cx, |block, _cx| {
            (
                block.kind(),
                block.record.title.serialize_markdown(),
                !block.children.is_empty(),
            )
        });
        if kind != BlockKind::Quote || has_children {
            return None;
        }

        let Some((variant, title)) =
            crate::wysiwyg::components::CalloutVariant::parse_header_line(&title_markdown)
        else {
            return None;
        };

        block.update(cx, |block, cx| {
            block.record.kind = BlockKind::Callout(variant);
            block
                .record
                .set_title(InlineTextTree::from_markdown(&title));
            block.sync_edit_mode_from_kind();
            block.sync_render_cache();
            block.cursor_blink_epoch = Instant::now();
            cx.notify();
        });
        let body = self.ensure_callout_body_entry(block, cx)?;
        Some(body.entity_id())
    }

    fn downgrade_empty_callout_body_to_quote(
        &mut self,
        block: &Entity<super::Block>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(location) = self.document.find_block_location(block.entity_id()) else {
            return false;
        };
        let Some(parent) = location.parent.clone() else {
            return false;
        };

        let (header_markdown, only_child, block_is_empty_leaf) = {
            let parent_ref = parent.read(cx);
            let Some(variant) = parent_ref.kind().callout_variant() else {
                return false;
            };
            let block_ref = block.read(cx);
            (
                variant.header_markdown(&parent_ref.record.title.serialize_markdown()),
                parent_ref.children.len() == 1,
                block_ref.kind() == BlockKind::Paragraph
                    && block_ref.display_text().is_empty()
                    && block_ref.children.is_empty(),
            )
        };
        if !only_child || !block_is_empty_leaf {
            return false;
        }

        self.prepare_undo_capture(
            crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
            cx,
        );
        self.document.with_structure_mutation(cx, |document, cx| {
            let _ = document.remove_block_by_id_raw(block.entity_id(), cx);
            parent.update(cx, |parent, cx| {
                parent.record.kind = BlockKind::Quote;
                parent
                    .record
                    .set_title(InlineTextTree::from_markdown(&header_markdown));
                parent.sync_edit_mode_from_kind();
                parent.sync_render_cache();
                parent.assign_collapsed_selection_offset(0, CollapsedCaretAffinity::Default, None);
                parent.marked_range = None;
                parent.cursor_blink_epoch = Instant::now();
                cx.notify();
            });
        });
        self.focus_block(parent.entity_id());
        self.rebuild_image_runtimes(cx);
        self.mark_dirty(cx);
        self.finalize_pending_undo_capture(cx);
        cx.notify();
        true
    }

    /// Handles all block-originated editor events against the current cached
    /// visible-order snapshot.
    pub(crate) fn on_block_event(
        &mut self,
        block: Entity<super::Block>,
        event: &BlockEvent,
        cx: &mut Context<Self>,
    ) {
        if let BlockEvent::PrepareUndo { kind } = event {
            self.prepare_undo_capture_from_stable_snapshot(*kind);
            return;
        }

        if let BlockEvent::RequestReplaceCrossBlockSelection {
            text,
            selected_range_relative,
            mark_inserted_text,
            undo_kind,
        } = event
            && self.replace_cross_block_selection_with_text(
                text,
                selected_range_relative.clone(),
                *mark_inserted_text,
                *undo_kind,
                cx,
            )
        {
            return;
        }

        if matches!(event, BlockEvent::RequestRenderedSelectAll) {
            self.on_rendered_select_all_press(block, cx);
            return;
        }

        if let BlockEvent::RequestPasteImage {
            leading,
            source,
            trailing,
        } = event
        {
            self.handle_paste_image_request(block, leading, source, trailing, cx);
            return;
        }

        if let BlockEvent::RequestPasteVideo {
            leading,
            path,
            trailing,
        } = event
        {
            self.handle_paste_video_request(block, leading, &path, trailing, cx);
            return;
        }

        if let Some(binding) = self.table_cell_binding(block.entity_id()) {
            self.on_table_cell_event(binding, event, cx);
            return;
        }

        if let Some(binding) = self.html_wrapper_child_binding(block.entity_id()) {
            self.on_html_wrapper_child_event(binding, event, cx);
            return;
        }

        if Self::block_event_clears_cross_block_selection(event) {
            self.rendered_select_all_cycle = None;
            self.clear_cross_block_selection(cx);
        }

        let visible_before = self.document.flatten_visible_blocks();
        let current_visible_index = visible_before
            .iter()
            .position(|visible| visible.entity.entity_id() == block.entity_id())
            .unwrap_or(0);

        match event {
            BlockEvent::Changed => {
                let should_restart_numbered_list = block.update(cx, |block, _cx| {
                    block.take_numbered_list_restart_requested()
                });
                if should_restart_numbered_list {
                    self.insert_list_group_separator_before(block.entity_id(), cx);
                }

                let callout_focus_target = self.materialize_empty_callout_shortcut(&block, cx);

                let should_normalize_quote =
                    block.update(cx, |block, _cx| {
                        let requested = block.take_quote_reparse_requested();
                        requested && block.marked_range.is_none()
                    }) || Self::rendered_quote_text_requires_reparse(&block, cx);

                self.refresh_rendered_quote_metadata_if_needed(&block, cx);
                if should_normalize_quote {
                    self.normalize_rendered_quote_structure(cx);
                } else {
                    self.rebuild_image_runtimes(cx);
                }
                if let Some(focus_id) = callout_focus_target {
                    self.focus_block(focus_id);
                }
                self.mark_dirty(cx);
                self.request_active_block_scroll_into_view(cx);
                self.finalize_pending_undo_capture(cx);
            }
            BlockEvent::RequestNewline {
                trailing,
                source_already_mutated,
            } => {
                // Typing a setext underline (`=====`/`-----`) under a paragraph
                // and pressing Enter turns that paragraph into a heading, the
                // same way the importer treats the two adjacent lines.
                if self.try_form_setext_heading_on_newline(&block, cx) {
                    return;
                }
                // Typing a delimiter row under a header forms a native table,
                // and typing further pipe rows below the table absorbs them.
                if self.try_form_or_extend_table_on_newline(&block, cx) {
                    return;
                }
                let Some(location) = self.document.find_block_location(block.entity_id()) else {
                    return;
                };
                if !source_already_mutated {
                    self.prepare_undo_capture(
                        crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                        cx,
                    );
                }
                let current_kind = block.read(cx).kind();
                let new_block = Self::new_block(
                    cx,
                    BlockRecord::new(current_kind.newline_sibling_kind(), trailing.clone()),
                );
                if self.view_mode == super::ViewMode::Source {
                    new_block.update(cx, |block, _cx| block.set_source_document_mode());
                }
                self.document.insert_blocks_at(
                    location.parent,
                    location.index + 1,
                    vec![new_block.clone()],
                    cx,
                );
                self.rebuild_image_runtimes(cx);
                self.focus_block(new_block.entity_id());
                if current_kind.is_quote_container() {
                    self.normalize_rendered_quote_structure(cx);
                }
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestEnterCalloutBody => {
                let needs_body = block.read(cx).children.is_empty();
                if needs_body {
                    self.prepare_undo_capture(
                        crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                        cx,
                    );
                }
                let created = self.ensure_callout_body_entry(&block, cx);
                if let Some(body) = created {
                    self.focus_block(body.entity_id());
                    self.rebuild_image_runtimes(cx);
                    if needs_body {
                        self.mark_dirty(cx);
                        self.finalize_pending_undo_capture(cx);
                    }
                    cx.notify();
                }
            }
            BlockEvent::RequestQuoteBreak => {
                let quote_ancestor = match self.nearest_quote_ancestor(block.entity_id(), cx) {
                    Some(q) => q,
                    None => return,
                };

                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );

                // If the triggering block is empty (the user pressed Enter on an
                // empty quote line to leave), remove that empty quote line so the
                // exit doesn't leave a dangling blank quote behind.
                let trigger_is_empty = block.read(cx).display_text().trim().is_empty();
                if trigger_is_empty {
                    self.document.with_structure_mutation(cx, |document, cx| {
                        document.remove_block_by_id_raw(block.entity_id(), cx)
                    });
                }

                // Insert a plain paragraph after the quote ancestor. Re-resolve
                // its location after the optional removal above (the index may
                // have shifted if the empty trigger was the ancestor itself).
                let Some(ancestor_location) = self
                    .document
                    .find_block_location(quote_ancestor.entity_id())
                else {
                    return;
                };
                let new_paragraph = Self::new_block(cx, BlockRecord::paragraph(String::new()));
                self.document.insert_blocks_at(
                    ancestor_location.parent,
                    ancestor_location.index + 1,
                    vec![new_paragraph.clone()],
                    cx,
                );
                self.normalize_rendered_quote_structure(cx);
                // normalize_rendered_quote_structure rebuilds the whole document
                // from markdown, so the inserted paragraph entity is gone. Find
                // the paragraph that now follows the quote (the last visible
                // block when exiting a trailing quote, else the next sibling)
                // and land the caret at its start.
                let target = self
                    .document
                    .flatten_visible_blocks()
                    .into_iter()
                    .rev()
                    .map(|v| v.entity)
                    .find(|b| {
                        let block = b.read(cx);
                        let kind = block.kind();
                        !kind.is_quote_container() && block.quote_depth == 0
                    });
                if let Some(target) = target {
                    Self::reset_block_cursor(&target, 0, cx);
                    self.focus_block(target.entity_id());
                }
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestCalloutBreak => {
                let Some((parent, insert_index)) =
                    self.callout_break_insertion_target(block.entity_id(), cx)
                else {
                    return;
                };

                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );
                let plain = Self::new_block(cx, BlockRecord::paragraph(String::new()));
                let blocks = if parent.is_none() {
                    vec![plain.clone()]
                } else {
                    vec![
                        Self::new_block(cx, BlockRecord::paragraph(String::new())),
                        plain.clone(),
                    ]
                };
                self.document
                    .insert_blocks_at(parent, insert_index, blocks, cx);
                self.focus_block(plain.entity_id());
                self.rebuild_image_runtimes(cx);
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestMergeIntoPrev { content } => {
                if current_visible_index == 0 {
                    return;
                }
                let prev = visible_before[current_visible_index - 1].entity.clone();
                let quote_related = self.block_is_quote_structure_related(&block, cx)
                    || self.block_is_quote_structure_related(&prev, cx);
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );

                let cursor_pos = prev.read(cx).display_text().len();
                let adopted_children = super::tree::DocumentTree::take_children(&block, cx);
                let removed_entity_id = block.entity_id();

                self.document.with_structure_mutation(cx, |document, cx| {
                    prev.update(cx, {
                        let content = content.clone();
                        let adopted_children = adopted_children.clone();
                        move |prev, cx| {
                            let mut next_title = prev.record.title.clone();
                            next_title.append_tree(content.clone());
                            prev.record.set_title(next_title);
                            prev.sync_render_cache();
                            prev.children.extend(adopted_children.clone());
                            prev.selected_range = cursor_pos..cursor_pos;
                            prev.selection_reversed = false;
                            prev.marked_range = None;
                            prev.vertical_motion_x = None;
                            prev.cursor_blink_epoch = Instant::now();
                            cx.notify();
                        }
                    });
                    let _ = document.remove_block_by_id_raw(removed_entity_id, cx);
                });

                self.focus_block(prev.entity_id());
                if quote_related {
                    self.normalize_rendered_quote_structure(cx);
                } else {
                    self.rebuild_image_runtimes(cx);
                }
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestPasteMultiline {
                leading,
                lines,
                trailing,
                split_physical_lines,
            } => {
                if lines.is_empty() {
                    return;
                }
                let quote_related = self.block_is_quote_structure_related(&block, cx);
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );

                let current_kind = block.read(cx).kind();
                // Structural Markdown (tables, fences, containers) must be parsed
                // as whole blocks. The plain-text path folds the first pasted line
                // into the current paragraph, which would strip a table's header
                // row, so structural pastes hand every line to the importer and
                // leave the pre-cursor text in place.
                let structural = !*split_physical_lines;
                let leading_empty = leading.visible_len() == 0;
                let (mut first_title, tail_lines) = if structural {
                    (leading.clone(), lines.clone())
                } else {
                    let mut first_title = leading.clone();
                    first_title.append_tree(InlineTextTree::from_markdown(&lines[0]));
                    (first_title, lines[1..].to_vec())
                };
                if tail_lines.is_empty() {
                    first_title.append_tree(trailing.clone());
                    let cursor = first_title.visible_len();
                    Self::set_block_title_and_kind(&block, current_kind, first_title, cursor, cx);
                    self.focus_block(block.entity_id());
                    if quote_related {
                        self.normalize_rendered_quote_structure(cx);
                    } else {
                        self.rebuild_image_runtimes(cx);
                    }
                    self.mark_dirty(cx);
                    self.finalize_pending_undo_capture(cx);
                    cx.notify();
                    return;
                }

                let cursor = first_title.visible_len();
                Self::set_block_title_and_kind(&block, current_kind, first_title, cursor, cx);

                let Some(location) = self.document.find_block_location(block.entity_id()) else {
                    return;
                };

                // Physical-line paste is for plain rendered text snippets. If
                // the classifier saw structural Markdown, delegate the tail to
                // the normal importer so tables, fences, and containers stay
                // intact instead of becoming paragraphs.
                let mut inserted_roots = if *split_physical_lines {
                    Self::build_plain_paste_blocks_from_lines(cx, &tail_lines)
                } else {
                    Self::build_blocks_from_lines(cx, &tail_lines)
                };
                if structural && trailing.visible_len() > 0 {
                    inserted_roots.push(Self::new_block(cx, BlockRecord::paragraph(String::new())));
                }
                self.document.insert_blocks_at(
                    location.parent,
                    location.index + 1,
                    inserted_roots.clone(),
                    cx,
                );
                self.rebuild_table_runtimes(cx);

                // A structural block pasted at the very end of the document leaves
                // no line below it; remember that so a trailing paragraph can be
                // added once the paste (and any quote normalization) settles.
                let inserted_at_doc_end = inserted_roots.last().is_some_and(|last| {
                    self.document
                        .find_block_location(last.entity_id())
                        .is_some_and(|location| {
                            location.parent.is_none()
                                && location.index + 1 >= self.document.root_count()
                        })
                });

                if let Some(last_root) = inserted_roots.last() {
                    let focus_block = if last_root.read(cx).kind() == BlockKind::Table {
                        last_root
                            .read(cx)
                            .table_runtime
                            .as_ref()
                            .and_then(|runtime| {
                                runtime
                                    .rows
                                    .last()
                                    .and_then(|row| row.last())
                                    .cloned()
                                    .or_else(|| runtime.header.last().cloned())
                            })
                    } else {
                        self.document.last_visible_descendant(last_root.entity_id())
                    };
                    let Some(focus_block) = focus_block else {
                        return;
                    };
                    focus_block.update(cx, {
                        let trailing = trailing.clone();
                        move |focus_block, cx| {
                            let mut next_title = focus_block.record.title.clone();
                            next_title.append_tree(trailing.clone());
                            focus_block.record.set_title(next_title);
                            focus_block.sync_render_cache();
                            focus_block.cursor_blink_epoch = Instant::now();
                            cx.notify();
                        }
                    });
                    let cursor = focus_block.read(cx).display_text().len();
                    Self::reset_block_cursor(&focus_block, cursor, cx);
                    self.rebuild_image_runtimes(cx);
                    if let Some(binding) = self.table_cell_binding(focus_block.entity_id()) {
                        self.sync_table_record_from_runtime(&binding.table_block, cx);
                    }
                    self.focus_block(focus_block.entity_id());
                }

                // When structural content is pasted onto an empty line there is
                // no pre-cursor text to keep, so drop the now-empty paragraph
                // rather than leaving a blank line above the pasted blocks.
                if structural && leading_empty {
                    self.document.with_structure_mutation(cx, |document, cx| {
                        document.remove_block_by_id_raw(block.entity_id(), cx);
                    });
                }

                if quote_related {
                    self.normalize_rendered_quote_structure(cx);
                }

                // Quote normalization rebuilds roots from Markdown, so resolve the
                // landing block from the live tree rather than the pasted handles.
                if inserted_at_doc_end {
                    if let Some(last_root) = self.document.root_blocks().last().cloned() {
                        self.ensure_trailing_paragraph_after_structural(&last_root, cx);
                    }
                }
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestPasteImage { .. }
            | BlockEvent::RequestPasteVideo { .. }
            | BlockEvent::RequestReplaceCrossBlockSelection { .. } => {}
            BlockEvent::RequestIndent => {
                if current_visible_index == 0 {
                    return;
                }

                let Some(location) = self.document.find_block_location(block.entity_id()) else {
                    return;
                };
                let current_kind = block.read(cx).kind();
                let target_parent = visible_before[current_visible_index - 1].entity.clone();
                if !current_kind.can_nest_under(&target_parent.read(cx).kind()) {
                    return;
                }
                if location
                    .parent
                    .as_ref()
                    .is_some_and(|parent| parent.entity_id() == target_parent.entity_id())
                {
                    return;
                }
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );

                let moved = self.document.with_structure_mutation(cx, |document, cx| {
                    let moved = document.remove_block_by_id_raw(block.entity_id(), cx)?.0;
                    let child_index = target_parent.read(cx).children.len();
                    document.insert_blocks_at_raw(
                        Some(target_parent.clone()),
                        child_index,
                        vec![moved.clone()],
                        cx,
                    );
                    Some(moved)
                });

                let Some(moved) = moved else {
                    return;
                };

                self.focus_block(moved.entity_id());
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestOutdent => {
                let Some(location) = self.document.find_block_location(block.entity_id()) else {
                    return;
                };
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );

                if let Some(parent) = location.parent.clone() {
                    let Some(parent_location) =
                        self.document.find_block_location(parent.entity_id())
                    else {
                        return;
                    };

                    let moved = self.document.with_structure_mutation(cx, |document, cx| {
                        let moved = document.remove_block_by_id_raw(block.entity_id(), cx)?.0;
                        document.insert_blocks_at_raw(
                            parent_location.parent,
                            parent_location.index + 1,
                            vec![moved.clone()],
                            cx,
                        );
                        Some(moved)
                    });

                    let Some(moved) = moved else {
                        return;
                    };
                    self.focus_block(moved.entity_id());
                } else {
                    block.update(cx, |block, cx| block.convert_to_paragraph(cx));
                    self.focus_block(block.entity_id());
                }

                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestDowngradeNestedListItemToChildParagraph => {
                let Some(location) = self.document.find_block_location(block.entity_id()) else {
                    return;
                };
                let Some(parent) = location.parent.clone() else {
                    return;
                };
                if !block.read(cx).kind().is_list_item() || !parent.read(cx).kind().is_list_item() {
                    return;
                }

                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );

                let downgraded = self.document.with_structure_mutation(cx, |document, cx| {
                    let (moved, removed_location) =
                        document.remove_block_by_id_raw(block.entity_id(), cx)?;
                    moved.update(cx, |block, cx| {
                        block.record.kind = BlockKind::Paragraph;
                        block.record.raw_fallback = None;
                        block.sync_edit_mode_from_kind();
                        block.sync_render_cache();
                        block.cursor_blink_epoch = Instant::now();
                        cx.notify();
                    });
                    document.insert_blocks_at_raw(
                        Some(parent.clone()),
                        removed_location.index,
                        vec![moved.clone()],
                        cx,
                    );
                    Some(moved)
                });

                let Some(downgraded) = downgraded else {
                    return;
                };

                self.focus_block(downgraded.entity_id());
                self.rebuild_image_runtimes(cx);
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::ToggleTaskChecked => {
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );
                block.update(cx, |block, cx| {
                    let checked = match block.kind() {
                        BlockKind::TaskListItem { checked } => checked,
                        _ => return,
                    };
                    block.record.kind = BlockKind::TaskListItem { checked: !checked };
                    block.sync_edit_mode_from_kind();
                    block.sync_render_cache();
                    block.cursor_blink_epoch = Instant::now();
                    cx.notify();
                });
                self.mark_dirty(cx);
                self.request_active_block_scroll_into_view(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestOpenLink {
                prompt_target,
                open_target,
            } => {
                self.request_open_link_prompt(prompt_target.clone(), open_target.clone(), cx);
            }
            BlockEvent::RequestJumpToFootnoteDefinition { id, .. } => {
                let _ = self.jump_to_footnote_definition(id, cx);
                cx.notify();
            }
            BlockEvent::RequestJumpToFootnoteBackref { id } => {
                let _ = self.jump_to_footnote_backref(id, cx);
                cx.notify();
            }
            BlockEvent::RequestAppendTableColumn => {
                if block.read(cx).kind() == BlockKind::Table {
                    self.prepare_undo_capture(
                        crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                        cx,
                    );
                    self.append_table_column(&block, cx);
                    self.finalize_pending_undo_capture(cx);
                }
            }
            BlockEvent::RequestAppendTableRow => {
                if block.read(cx).kind() == BlockKind::Table {
                    self.prepare_undo_capture(
                        crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                        cx,
                    );
                    self.append_table_row(&block, cx);
                    self.finalize_pending_undo_capture(cx);
                }
            }
            BlockEvent::RequestInsertTableRowBelow => {
                // Emitted by a focused table cell. Resolve the owning table and
                // the cell's visual row to compute the new row's body index.
                let Some(binding) = self.table_cell_binding(block.entity_id()) else {
                    return;
                };
                let body_row_index = binding.position.row.saturating_sub(1) + 1;
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );
                self.insert_table_row(&binding.table_block, body_row_index, cx);
                self.finalize_pending_undo_capture(cx);
            }
            BlockEvent::RequestInsertTableColumnRight => {
                let Some(binding) = self.table_cell_binding(block.entity_id()) else {
                    return;
                };
                let column = binding.position.column + 1;
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );
                self.insert_table_column(&binding.table_block, column, cx);
                self.finalize_pending_undo_capture(cx);
            }
            BlockEvent::RequestTableAxisPreview {
                kind,
                index,
                hovered,
            } => {
                if block.read(cx).kind() == BlockKind::Table {
                    self.preview_table_axis(block.entity_id(), *kind, *index, *hovered, cx);
                }
            }
            BlockEvent::RequestSelectTableAxis { kind, index } => {
                if block.read(cx).kind() == BlockKind::Table {
                    self.select_table_axis(block.entity_id(), *kind, *index, cx);
                }
            }
            BlockEvent::RequestOpenTableAxisMenu {
                kind,
                index,
                position,
            } => {
                if block.read(cx).kind() == BlockKind::Table {
                    self.open_table_axis_menu(block.entity_id(), *kind, *index, *position, cx);
                }
            }
            BlockEvent::RequestTableCellMoveHorizontal { .. }
            | BlockEvent::RequestTableCellMoveVertical { .. } => {}
            BlockEvent::RequestFocusPrev { preferred_x } => {
                if current_visible_index == 0 {
                    return;
                }

                let target = visible_before[current_visible_index - 1].entity.clone();
                // Entering a table from below lands in a body cell instead of
                // the non-editable table container.
                if target.read(cx).kind() == BlockKind::Table
                    && self.focus_table_entry_cell(&target, false, cx)
                {
                    return;
                }
                let target_x = preferred_x.map(px);
                let offset = target
                    .read(cx)
                    .entry_offset_for_vertical_focus(true, target_x);
                self.focus_block(target.entity_id());
                target.update(cx, move |target, cx| {
                    target.move_to_with_preferred_x(offset, target_x, cx);
                });
                cx.notify();
            }
            BlockEvent::RequestFocusNext { preferred_x } => {
                if current_visible_index + 1 >= visible_before.len() {
                    // A trailing multi-line block (code, math, ...) has nowhere
                    // below to move to, so give it a paragraph to land on and
                    // focus that, matching how a trailing table behaves.
                    if block.read(cx).kind().is_multiline_text_block() {
                        self.ensure_trailing_paragraph_after_structural(&block, cx);
                        let visible = self.document.flatten_visible_blocks();
                        if let Some(landing) = visible
                            .iter()
                            .position(|v| v.entity.entity_id() == block.entity_id())
                            .and_then(|index| visible.get(index + 1))
                            .map(|v| v.entity.clone())
                        {
                            self.focus_block(landing.entity_id());
                            landing.update(cx, |landing, cx| landing.move_to(0, cx));
                            cx.notify();
                        }
                    }
                    return;
                }

                let target = visible_before[current_visible_index + 1].entity.clone();
                // Entering a table from above lands in a header cell instead of
                // the non-editable table container.
                if target.read(cx).kind() == BlockKind::Table
                    && self.focus_table_entry_cell(&target, true, cx)
                {
                    return;
                }
                let target_x = preferred_x.map(px);
                let offset = target
                    .read(cx)
                    .entry_offset_for_vertical_focus(false, target_x);
                self.focus_block(target.entity_id());
                target.update(cx, move |target, cx| {
                    target.move_to_with_preferred_x(offset, target_x, cx);
                });
                cx.notify();
            }
            BlockEvent::RequestBlockUp => {
                if current_visible_index == 0 {
                    return;
                }

                let target = visible_before[current_visible_index - 1].entity.clone();
                if target.read(cx).kind() == BlockKind::Table
                    && self.focus_table_entry_cell(&target, false, cx)
                {
                    return;
                }
                self.focus_block(target.entity_id());
                target.update(cx, |target, cx| target.move_to(0, cx));
                cx.notify();
            }
            BlockEvent::RequestBlockDown => {
                if current_visible_index + 1 >= visible_before.len() {
                    return;
                }

                let target = visible_before[current_visible_index + 1].entity.clone();
                if target.read(cx).kind() == BlockKind::Table
                    && self.focus_table_entry_cell(&target, true, cx)
                {
                    return;
                }
                self.focus_block(target.entity_id());
                target.update(cx, |target, cx| target.move_to(0, cx));
                cx.notify();
            }
            BlockEvent::RequestDelete => {
                if self.downgrade_empty_callout_body_to_quote(&block, cx) {
                    return;
                }
                let quote_related = self.block_is_quote_structure_related(&block, cx);
                let is_last_visible_leaf =
                    visible_before.len() == 1 && block.read(cx).children.is_empty();
                if is_last_visible_leaf {
                    if block.read(cx).kind() == BlockKind::Paragraph {
                        Self::reset_block_cursor(&block, 0, cx);
                    } else {
                        block.update(cx, |block, cx| block.convert_to_paragraph(cx));
                    }
                    self.focus_block(block.entity_id());
                    cx.notify();
                    return;
                }
                self.prepare_undo_capture(
                    crate::wysiwyg::components::UndoCaptureKind::NonCoalescible,
                    cx,
                );

                let visible_before_ids = visible_before
                    .iter()
                    .map(|visible| visible.entity.entity_id())
                    .collect::<Vec<_>>();
                let focus_candidate = if current_visible_index > 0 {
                    Some(visible_before_ids[current_visible_index - 1])
                } else {
                    visible_before_ids.get(current_visible_index + 1).copied()
                };

                let adopted_children = super::tree::DocumentTree::take_children(&block, cx);
                let removed = self.document.with_structure_mutation(cx, |document, cx| {
                    let (_, location) = document.remove_block_by_id_raw(block.entity_id(), cx)?;
                    if !adopted_children.is_empty() {
                        document.insert_blocks_at_raw(
                            location.parent.clone(),
                            location.index,
                            adopted_children.clone(),
                            cx,
                        );
                    }
                    Some(location)
                });

                if removed.is_none() {
                    return;
                }

                if let Some(focus_id) = focus_candidate {
                    self.focus_block(focus_id);
                } else if let Some(first_root) = self.document.first_root() {
                    self.focus_block(first_root.entity_id());
                }

                if quote_related {
                    self.normalize_rendered_quote_structure(cx);
                }
                self.mark_dirty(cx);
                self.finalize_pending_undo_capture(cx);
                cx.notify();
            }
            BlockEvent::RequestFocus => {
                self.close_menu_bar(cx);
                self.clear_table_axis_preview(cx);
                self.clear_table_axis_selection(cx);
                // First click on a wrapper: install the runtime now and record
                // the click point so the editor's render sync
                // (`sync_html_wrapper_runtimes`) can resolve it to a precise
                // caret on the clicked child via the wrapper's HTML-visual hit
                // records. The runtime must be installed this frame so the
                // children exist when sync resolves the click.
                let is_editable_wrapper = block.read(cx).is_editable_html_wrapper();
                let click_point = block.read(cx).last_click_point;
                self.focus_block_from_click(block.entity_id(), cx);
                if is_editable_wrapper {
                    let installed = self.install_html_wrapper_runtime(&block, cx);
                    if installed {
                        self.pending_wrapper_click_point = click_point;
                        cx.notify();
                    }
                }
                for visible in self.document.flatten_visible_blocks() {
                    visible.entity.update(cx, |_, cx| cx.notify());
                }
                cx.notify();
            }
            BlockEvent::RequestRenderedSelectAll => {}
            BlockEvent::PrepareUndo { .. } => {}
        }
    }
}

// Block-event handler tests live in a sibling file so this module stays
// focused on production logic. Gated on the wysiwyg-gpui-tests feature.
#[cfg(all(test, feature = "wysiwyg-gpui-tests"))]
mod tests;
