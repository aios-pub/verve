//! Focused-state child-block runtime for editable HTML wrapper blocks
//! (`<div align="center">…inner markdown…</div>`).
//!
//! Mirrors `table_edit.rs`: while a wrapper HtmlBlock is focused, its inner
//! Markdown is parsed into N independent child `Entity<Block>`s (a heading, a
//! paragraph, a `<table>`, …) stored out-of-band on the block's
//! [`HtmlWrapperRuntime`] + the editor's `html_wrapper_children` side map. Each
//! child is a normal editable block; clicking focuses the child that was hit.
//! On every child edit the whole inner Markdown is re-serialized and re-wrapped
//! into the original HTML structure ([`HtmlWrapperShape::wrap`]) so the on-disk
//! file keeps the `<div …>…</div>` shape.

use super::*;

use crate::wysiwyg::components::block::{
    VisualLineHit, VisualLineKind, VisualLinePrefix, hard_line_ranges, wrapped_line_for_y,
};
use crate::wysiwyg::components::{InlineRenderCache, InlineStyle};

impl Editor {
    /// Per-frame sync of HTML-wrapper runtimes, driven by focus. Installs a
    /// child-block runtime on the focused wrapper HtmlBlock (if it is an
    /// editable wrapper) and tears down runtimes whose focus has moved outside
    /// the wrapper. Mirrors how `sync_inline_projection_for_focus` is render-
    /// driven at the block level, but lives on the editor because it needs the
    /// `html_wrapper_children` side map + the markdown importer.
    pub(super) fn sync_html_wrapper_runtimes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_mode != ViewMode::Rendered {
            self.teardown_all_html_wrapper_runtimes(cx);
            return;
        }

        // Resolve the focused block: either a document block or a wrapper child.
        // `frame_focused_target` is computed once at the top of `render`.
        let focused_id = self.frame_focused_target.or(self.pending_focus);

        // The wrapper HtmlBlock that currently holds focus (whether or not its
        // runtime is already installed).
        let focused_wrapper = focused_id.and_then(|id| {
            self.document.block_entity_by_id(id).filter(|block| {
                block.read(cx).kind() == BlockKind::HtmlBlock
                    && block
                        .read(cx)
                        .record
                        .html
                        .as_ref()
                        .is_some_and(|doc| extract_editable_inline_wrapper(doc).is_some())
            })
        });

        // Install on the focused wrapper (if newly focused and not yet built).
        if let Some(wrapper) = &focused_wrapper {
            self.install_html_wrapper_runtime(wrapper, cx);
        }

        // First click on a wrapper: the runtime was just installed (in the
        // RequestFocus handler that set `pending_wrapper_click_point`). Resolve
        // the click precisely against the wrapper's last-painted HTML visual —
        // pick the hit visual line, find the character offset the click landed
        // on, and map (visual line → wrapper child, child hard-line) so the
        // caret lands exactly where the user clicked. See `resolve_wrapper_click`.
        // On any failure (e.g. no recorded line hits, or a structural child we
        // can't map), fall through to the "first editable child" focus below.
        if let Some(click_point) = self.pending_wrapper_click_point.take() {
            if let Some(wrapper) = &focused_wrapper {
                if wrapper.read(cx).html_wrapper_runtime.is_some() {
                    self.resolve_wrapper_click(wrapper, click_point, window, cx);
                }
            }
        } else if let Some(wrapper) = &focused_wrapper {
            // No pending click: if the wrapper container itself holds focus and
            // no child has focus yet, focus the first editable child.
            let wrapper_is_focused = wrapper.read(cx).focus_handle.is_focused(window);
            if wrapper_is_focused && wrapper.read(cx).html_wrapper_runtime.is_some() {
                let any_child_focused = wrapper
                    .read(cx)
                    .html_wrapper_runtime
                    .as_ref()
                    .map(|rt| {
                        rt.children
                            .iter()
                            .any(|c| c.read(cx).focus_handle.is_focused(window))
                    })
                    .unwrap_or(false);
                if !any_child_focused && self.pending_focus.is_none() {
                    let first_editable = wrapper
                        .read(cx)
                        .html_wrapper_runtime
                        .as_ref()
                        .and_then(|rt| {
                            rt.children
                                .iter()
                                .position(|c| c.read(cx).kind() != BlockKind::HtmlBlock)
                        })
                        .unwrap_or(0);
                    self.focus_html_wrapper_child(wrapper, first_editable, cx);
                }
            }
        }

        // The wrapper that should remain active: the focused wrapper itself, or
        // the parent of the currently-focused child.
        let active_wrapper_id = focused_wrapper.as_ref().map(|w| w.entity_id()).or_else(|| {
            focused_id.and_then(|id| {
                self.html_wrapper_block_for_child(id)
                    .map(|wrapper| wrapper.entity_id())
            })
        });

        // Tear down any wrapper runtime whose focus has left.
        let stale: Vec<Entity<Block>> = self
            .document
            .visible_blocks()
            .iter()
            .filter(|visible| visible.entity.read(cx).html_wrapper_runtime.is_some())
            .map(|visible| visible.entity.clone())
            .filter(|block| Some(block.entity_id()) != active_wrapper_id)
            .collect();
        for wrapper in stale {
            self.teardown_html_wrapper_runtime(&wrapper, cx);
        }
    }

    /// Tears down ALL active wrapper runtimes (used on view-mode switch /
    /// document replacement).
    pub(super) fn teardown_all_html_wrapper_runtimes(&mut self, cx: &mut Context<Self>) {
        let wrappers: Vec<Entity<Block>> = self
            .document
            .visible_blocks()
            .iter()
            .filter(|visible| visible.entity.read(cx).html_wrapper_runtime.is_some())
            .map(|visible| visible.entity.clone())
            .collect();
        for wrapper in wrappers {
            self.teardown_html_wrapper_runtime(&wrapper, cx);
        }
    }

    /// Clears the wrapper-children side map without syncing (used when the
    /// whole document is being replaced — the wrapper blocks are gone, so
    /// there's nothing to sync back into). Mirrors `table_cells.clear()`.
    pub(super) fn clear_html_wrapper_children(&mut self) {
        self.html_wrapper_children.clear();
    }

    /// Builds the focused child-block runtime for `wrapper_block` from its
    /// parsed HTML. The wrapper must be an editable inline-content wrapper
    /// (`extract_editable_inline_wrapper` returns `Some`); otherwise this is a
    /// no-op. Returns true if a runtime was newly installed this call.
    pub(super) fn install_html_wrapper_runtime(
        &mut self,
        wrapper_block: &Entity<Block>,
        cx: &mut Context<Self>,
    ) -> bool {
        // Already installed? Keep it (the focus sync runs every render).
        if wrapper_block.read(cx).html_wrapper_runtime.is_some() {
            return false;
        }

        let Some(shape) = wrapper_block
            .read(cx)
            .record
            .html
            .as_ref()
            .and_then(|doc| extract_editable_inline_wrapper(doc))
        else {
            return false;
        };
        let Some(inner_markdown) = wrapper_block.read(cx).record.html.as_deref().and_then(|doc| {
            extract_editable_inline_wrapper(doc)
                .map(|_| HtmlWrapperShape::inner_source(&doc.nodes[0]))
        }) else {
            return false;
        };

        // Parse the inner Markdown into a block tree using the same importer
        // the document uses. This handles headings/paragraphs/lists/tables/
        // quotes and falls back to RawMarkdown for anything it can't model.
        let parsed = Self::build_root_blocks_from_markdown(cx, &inner_markdown);

        // Drop empty-paragraph children: the importer emits them for blank
        // lines between blocks, but they carry no content and would otherwise
        // win the "nearest child" focus (their offset is always 0), landing the
        // caret in the wrong place. Blank-line spacing is regenerated by the
        // sync serializer (`collect_root_markdown_lines`), so dropping them is
        // round-trip-safe.
        //
        // A markdown pipe-table child is downgraded to a `RawMarkdown` source
        // block. `table_runtime` is installed by `rebuild_table_runtimes`,
        // which only walks `document.visible_blocks()` — wrapper children live
        // out-of-band, so a Table child never gets its runtime and would fall
        // through to the rich-text render fallback (the Table render arm with
        // no runtime), showing `| a | b |` as styled prose. As a RawMarkdown
        // block it renders as an editable source code block instead. Round-trip
        // is safe: `collect_single_block_markdown_lines` serializes RawMarkdown
        // from `raw_fallback`, and `serialize_table_markdown_lines` emits the
        // same standard pipe-table source the importer consumed.
        let mut filtered: Vec<_> = parsed
            .into_iter()
            .filter(|child| {
                let block = child.read(cx);
                !(block.kind() == BlockKind::Paragraph
                    && block.record.title.visible_text().is_empty())
            })
            .collect();
        for child in &mut filtered {
            let is_table = child.read(cx).kind() == BlockKind::Table;
            if is_table {
                child.update(cx, |block, _cx| {
                    let source = block
                        .record
                        .table
                        .take()
                        .map(|table| {
                            crate::wysiwyg::components::serialize_table_markdown_lines(&table)
                                .join("\n")
                        })
                        .unwrap_or_default();
                    block.record.kind = BlockKind::RawMarkdown;
                    // set_title triggers sync_raw_fallback (RawMarkdown
                    // keeps raw_fallback), seeding the editable source text.
                    block.record.set_title(InlineTextTree::plain(source));
                    block.sync_edit_mode_from_kind();
                    block.sync_render_cache();
                });
            }
        }

        // Adopt each parsed block as a wrapper child: subscribe (done by
        // new_block, but these were built via build_root_blocks_from_markdown
        // which also uses new_block), mark it, and register the binding.
        let mut children = Vec::with_capacity(filtered.len());
        for (index, child) in filtered.into_iter().enumerate() {
            // Ensure the editor is subscribed to this child's events.
            // build_root_blocks_from_markdown -> new_block already subscribes,
            // but be defensive: re-subscribe is harmless and covers any path
            // that constructs blocks without subscribing.
            let _ = cx.subscribe(&child, Self::on_block_event).detach();
            child.update(cx, |block, _cx| {
                block.set_html_wrapper_child_mode(index);
            });
            // Propagate runtime context (link refs, image base dir) so inline
            // links / images in the wrapper's children resolve like top-level
            // blocks.
            self.sync_runtime_context_for_block(&child, self.image_base_dir().as_deref(), cx);
            self.html_wrapper_children.insert(
                child.entity_id(),
                HtmlWrapperChildBinding {
                    wrapper_block: wrapper_block.clone(),
                    child: child.clone(),
                    index,
                },
            );
            children.push(child);
        }

        wrapper_block.update(cx, move |block, _cx| {
            block.set_html_wrapper_runtime(HtmlWrapperRuntime { children, shape });
        });
        cx.notify();
        true
    }

    /// Tears down the focused child-block runtime for `wrapper_block`,
    /// syncing any in-flight child edits back into `record` first so no edit is
    /// lost. Renders the block back to its HTML visual.
    pub(super) fn teardown_html_wrapper_runtime(
        &mut self,
        wrapper_block: &Entity<Block>,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = wrapper_block.read(cx).html_wrapper_runtime.clone() else {
            return;
        };
        // Sync child markdown -> record.title/raw_fallback/html before dropping
        // the runtime so disk serialization stays correct.
        self.sync_html_wrapper_record_from_runtime(wrapper_block, cx);

        // Detach every child: drop it from the side map and clear its marker.
        for child in &runtime.children {
            self.html_wrapper_children.remove(&child.entity_id());
            child.update(cx, |block, _cx| {
                block.clear_html_wrapper_child_mode();
            });
        }
        wrapper_block.update(cx, |block, _cx| {
            block.clear_html_wrapper_runtime();
        });
        cx.notify();
    }

    /// Re-collects each child's Markdown, joins them into the inner Markdown
    /// source, re-wraps into the HTML structure, and writes the result back to
    /// `record.title` / `raw_fallback` / `html`. Called on every child edit
    /// (analogous to `sync_table_record_from_runtime`).
    pub(super) fn sync_html_wrapper_record_from_runtime(
        &mut self,
        wrapper_block: &Entity<Block>,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = wrapper_block.read(cx).html_wrapper_runtime.clone() else {
            return;
        };

        // Serialize the children exactly as the document serializes root blocks
        // (handles list-item nesting, empty-paragraph separation, etc.).
        let mut lines = Vec::new();
        DocumentTree::collect_root_markdown_lines(&runtime.children, cx, &mut lines);
        let inner_markdown = lines.join("\n");

        let wrapped_html = runtime.shape.wrap(&inner_markdown);
        wrapper_block.update(cx, move |block, _cx| {
            block.record.kind = BlockKind::HtmlBlock;
            block.record.set_title(InlineTextTree::plain(wrapped_html));
        });
    }

    /// Focus the wrapper child at `index`, clamped to the children range.
    /// Returns false if there is no child at that index.
    pub(super) fn focus_html_wrapper_child(
        &mut self,
        wrapper_block: &Entity<Block>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(child) = wrapper_block
            .read(cx)
            .html_wrapper_runtime
            .as_ref()
            .and_then(|runtime| runtime.children.get(index).cloned())
        else {
            return false;
        };
        self.focus_block(child.entity_id());
        cx.notify();
        true
    }

    /// Focus the next/previous wrapper child relative to `from_index`. When
    /// moving past the last child, returns false so the caller can exit the
    /// wrapper to the following document block (mirrors table vertical-move).
    pub(super) fn focus_html_wrapper_child_neighbor(
        &mut self,
        wrapper_block: &Entity<Block>,
        from_index: usize,
        delta: i32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(count) = wrapper_block
            .read(cx)
            .html_wrapper_runtime
            .as_ref()
            .map(|runtime| runtime.children.len())
        else {
            return false;
        };
        let next = if delta > 0 {
            from_index.checked_add(1)
        } else {
            from_index.checked_sub(1)
        };
        match next {
            Some(index) if index < count => self.focus_html_wrapper_child(wrapper_block, index, cx),
            _ => false,
        }
    }

    /// Move focus out of the wrapper to the document block immediately before
    /// or after it. `delta < 0` → previous, `delta > 0` → next.
    pub(super) fn focus_block_adjacent_to_html_wrapper(
        &mut self,
        wrapper_block: &Entity<Block>,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        let visible = self.document.flatten_visible_blocks();
        let Some(index) = visible
            .iter()
            .position(|visible| visible.entity.entity_id() == wrapper_block.entity_id())
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
        self.focus_block(target.entity_id());
        if delta < 0 {
            let offset = target.read(cx).entry_offset_for_vertical_focus(true, None);
            target.update(cx, move |target, cx| {
                target.move_to_with_preferred_x(offset, None, cx);
            });
        } else {
            target.update(cx, |target, cx| target.move_to(0, cx));
        }
        cx.notify();
    }

    /// Resolves the first click on an editable HTML wrapper (`<div …>`) to a
    /// precise caret on the child block the user clicked, then returns `true`.
    ///
    /// The wrapper's unfocused view is a rendered HTML visual (plain `div`
    /// rows) whose geometry differs from the focused child-block layout
    /// (different per-line padding, font sizes, alignment), so the click point
    /// cannot be mapped pixel-for-pixel onto the children. Instead we resolve
    /// in the VISUAL's own space and transfer the result through the character
    /// domain, which is layout-independent:
    ///
    /// 1. **y → visual line**: the visual records, per rendered line, the
    ///    line's window-space bounds + which inner-Markdown source line it is
    ///    (see `Block::visual_line_hits`). We pick the line nearest the click.
    /// 2. **x → character offset** within that line's display text: re-shape
    ///    the line's text (`wrapper_visual_line_offset`) and
    ///    `closest_index_for_position`.
    /// 3. **visual line → wrapper child**: walk each child's serialized
    ///    Markdown lines against the inner source (`map_wrapper_click_target`)
    ///    to find `(child, intra-child line)`.
    /// 4. Add the intra-line offset to the child's display-text hard-line
    ///    start and `move_to` on the focused child.
    ///
    /// Returns `false` (caller falls back to the first editable child) when
    /// any step is unavailable: no recorded hits, a structural child (table /
    /// math / nested quote / callout), or a serialization mismatch.
    fn resolve_wrapper_click(
        &mut self,
        wrapper: &Entity<Block>,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let hits = wrapper.read(cx).visual_line_hits.borrow().clone();
        if hits.is_empty() {
            return false;
        }

        let Some(inner_source) =
            wrapper.read(cx).record.html.as_deref().and_then(
                crate::wysiwyg::components::markdown::html::editable_wrapper_inner_markdown,
            )
        else {
            return false;
        };

        // 1. Pick the visual line nearest the click by vertical distance.
        let Some(hit_idx) = Self::nearest_visual_line(&hits, point.y) else {
            return false;
        };
        let hit = &hits[hit_idx];
        if hit.bounds.is_none() {
            return false;
        }

        // Sanity: the hit's recorded text must match the inner source line it
        // claims — guards against stale or mixed-container line records.
        let inner_lines: Vec<&str> = inner_source.split('\n').collect();
        let source_line_ok = inner_lines.get(hit.source_line).is_some_and(|line| {
            matches!(hit.kind, VisualLineKind::Separator)
                || line.trim_end().ends_with(hit.raw_text.trim_end())
        });
        if !source_line_ok {
            return false;
        }

        // 3. Map the clicked source line to a wrapper child + intra-child line.
        let Some(target) =
            self.map_wrapper_click_target(wrapper, &inner_source, hit.source_line, cx)
        else {
            return false;
        };

        let Some(child) = wrapper
            .read(cx)
            .html_wrapper_runtime
            .as_ref()
            .and_then(|rt| rt.children.get(target.child_index).cloned())
        else {
            return false;
        };

        // Structural children can't receive a precise caret from the visual
        // (their focus surface differs); focus them at offset 0 — still the
        // correct child, unlike the old "always child 0" behavior.
        let structural = {
            let b = child.read(cx);
            let kind = b.kind();
            matches!(
                kind,
                BlockKind::Table
                    | BlockKind::MathBlock
                    | BlockKind::MermaidBlock
                    | BlockKind::HtmlBlock
            ) || (kind == BlockKind::Quote && !b.children.is_empty())
                || matches!(kind, BlockKind::Callout(_))
        };

        let offset = if structural {
            0
        } else {
            let line_offset = self
                .wrapper_visual_line_offset(wrapper, hit, point, window, cx)
                .unwrap_or(0);
            Self::wrapper_child_display_offset(&child, target.ser_line_index, line_offset, cx)
        };

        self.focus_block(child.entity_id());
        child.update(cx, |block, cx| block.move_to(offset, cx));
        cx.notify();
        true
    }

    /// Maps an inner-Markdown source line (index into `inner_source.split('\n')`)
    /// to the wrapper child that owns it and the line's index within that
    /// child's serialized Markdown. Walks each root child's serialized lines
    /// (via `DocumentTree::collect_root_markdown_lines`) and matches them
    /// against the inner source in order, skipping blank lines on both sides.
    /// Returns `None` on any mismatch (the on-disk round-trip is stable, so a
    /// mismatch means the visual and runtime diverged → caller falls back).
    fn map_wrapper_click_target(
        &self,
        wrapper: &Entity<Block>,
        inner_source: &str,
        source_line: usize,
        cx: &App,
    ) -> Option<WrapperClickTarget> {
        let children = wrapper
            .read(cx)
            .html_wrapper_runtime
            .as_ref()?
            .children
            .clone();
        let inner_lines: Vec<&str> = inner_source.split('\n').collect();

        // Serialize each root child in isolation (no inter-block separators,
        // matching how the inner Markdown is reconstructed), then flatten each
        // serialized entry on '\n': a multi-line paragraph, for instance,
        // serializes its whole title as one entry containing embedded newlines,
        // so splitting is required to align with `inner_source.split('\n')`.
        let child_ser_lines: Vec<Vec<String>> = children
            .iter()
            .map(|child| {
                let mut raw = Vec::new();
                DocumentTree::collect_root_markdown_lines(
                    std::slice::from_ref(child),
                    cx,
                    &mut raw,
                );
                raw.into_iter()
                    .flat_map(|entry| entry.split('\n').map(str::to_string).collect::<Vec<_>>())
                    .collect()
            })
            .collect();

        match_wrapper_source_line(&inner_lines, &child_ser_lines, source_line).map(
            |(child_index, ser_line_index)| WrapperClickTarget {
                child_index,
                ser_line_index,
            },
        )
    }

    /// Resolves the character offset within a clicked visual line's display
    /// text, by re-shaping the line exactly as the visual renders it (font
    /// size/weight from the line kind, prefix shift for quote/list-marker) and
    /// `closest_index_for_position` on the wrapped layout. Returns `None` if
    /// the line has no recorded bounds or shaping fails.
    fn wrapper_visual_line_offset(
        &self,
        wrapper: &Entity<Block>,
        hit: &VisualLineHit,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        if matches!(hit.kind, VisualLineKind::Separator) {
            return Some(0);
        }
        let bounds = hit.bounds?;
        let theme = cx
            .global::<crate::wysiwyg::theme::ThemeManager>()
            .current_arc();

        let tree = wrapper
            .read(cx)
            .inline_tree_from_markdown_with_context(&hit.raw_text);
        let cache = tree.render_cache();
        let text_len = cache.visible_text().len();
        let display: SharedString = cache.visible_text().to_string().into();

        let base_font = window.text_style().font();
        let runs =
            Self::visual_line_text_runs(&cache, text_len, base_font.clone(), hit.font_weight);
        let font_size = hit.font_size;
        let prefix_w =
            Self::visual_line_prefix_width(&hit.prefix, font_size, &base_font, &theme, window);
        let wrap_width = (bounds.size.width - prefix_w).max(px(1.0));
        let lines = window
            .text_system()
            .shape_text(display, font_size, &runs, Some(wrap_width), None)
            .ok()?
            .into_vec();
        let line_height = px(theme.typography.text_line_height * f32::from(window.rem_size()));

        let relative_y = point.y - bounds.top();
        let (line_idx, y_in_line) = wrapped_line_for_y(&lines, line_height, relative_y)?;
        let origin_x = bounds.left() + prefix_w;
        let offset = match lines[line_idx].closest_index_for_position(
            gpui::Point::new(point.x - origin_x, y_in_line),
            line_height,
        ) {
            Ok(i) | Err(i) => i,
        };
        Some(offset)
    }

    /// Converts a serialized-line index within a wrapper child plus an
    /// intra-line character offset into a flat child display-text offset,
    /// clamping to the line's length. Code blocks skip their fence lines
    /// (ser line 0 = opening fence → content start; trailing fence → content
    /// end). Non-text structural kinds should be handled before calling.
    fn wrapper_child_display_offset(
        child: &Entity<Block>,
        ser_line_index: usize,
        line_offset: usize,
        cx: &App,
    ) -> usize {
        let block = child.read(cx);
        wrapper_display_offset(
            block.display_text(),
            block.kind().is_code_block(),
            ser_line_index,
            line_offset,
        )
    }

    /// Index of the visual-line hit nearest `y` (0 if inside the line's
    /// bounds; otherwise the vertical distance to the nearest edge). Skips
    /// lines without recorded bounds.
    fn nearest_visual_line(hits: &[VisualLineHit], y: Pixels) -> Option<usize> {
        let mut best: Option<(usize, Pixels)> = None;
        for (i, hit) in hits.iter().enumerate() {
            let Some(b) = hit.bounds else {
                continue;
            };
            let dist = if y < b.top() {
                b.top() - y
            } else if y > b.bottom() {
                y - b.bottom()
            } else {
                px(0.0)
            };
            match best {
                None => best = Some((i, dist)),
                Some((_, bd)) if dist < bd => best = Some((i, dist)),
                _ => {}
            }
        }
        best.map(|(i, _)| i)
    }

    /// Builds styled [`TextRun`]s for a visual line from its inline render
    /// cache, mirroring how the visual shapes text (bold/italic/font-style per
    /// span). Colors are irrelevant for x→index resolution so a placeholder is
    /// used; inline-code/script size variations are not expressible in
    /// `TextRun` (single line-wide font size) and are approximated.
    fn visual_line_text_runs(
        cache: &InlineRenderCache,
        text_len: usize,
        base_font: gpui::Font,
        base_weight: gpui::FontWeight,
    ) -> Vec<gpui::TextRun> {
        let mut boundaries: Vec<usize> = vec![0, text_len];
        for span in cache.spans() {
            boundaries.push(span.range.start);
            boundaries.push(span.range.end);
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let color = gpui::Hsla::default();
        let mut runs = Vec::new();
        for window in boundaries.windows(2) {
            let start = window[0];
            let end = window[1];
            if start >= end {
                continue;
            }
            let style: InlineStyle = cache.style_at(start);
            let mut font = base_font.clone();
            font.weight = if style.bold {
                gpui::FontWeight::BOLD
            } else {
                base_weight
            };
            if style.italic {
                font.style = gpui::FontStyle::Italic;
            }
            runs.push(gpui::TextRun {
                len: end - start,
                font,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
        if runs.is_empty() {
            runs.push(gpui::TextRun {
                len: text_len,
                font: base_font,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
        runs
    }

    /// The pixel x-offset of a visual line's text from the line's left edge,
    /// contributed by its leading decoration (quote bar, list marker).
    fn visual_line_prefix_width(
        prefix: &VisualLinePrefix,
        font_size: Pixels,
        base_font: &gpui::Font,
        theme: &crate::wysiwyg::theme::Theme,
        window: &mut Window,
    ) -> Pixels {
        match prefix {
            VisualLinePrefix::None => px(0.0),
            VisualLinePrefix::Quote => {
                px(theme.dimensions.quote_border_width + theme.dimensions.quote_padding_left)
            }
            VisualLinePrefix::ListMarker(marker) => {
                let run = gpui::TextRun {
                    len: marker.len(),
                    font: base_font.clone(),
                    color: gpui::Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window
                    .text_system()
                    .shape_line(marker.clone(), font_size, &[run], None);
                line.x_for_index(marker.len()) + px(6.0)
            }
        }
    }
}

/// Result of mapping a clicked inner-Markdown source line to a wrapper child.
#[derive(Clone, Copy, Debug)]
struct WrapperClickTarget {
    child_index: usize,
    /// Index of the line within the child's serialized Markdown (absolute,
    /// counting fence / blank lines as the serializer emits them).
    ser_line_index: usize,
}

/// Pure core of [`Editor::map_wrapper_click_target`]: walks each child's
/// serialized Markdown lines against the inner source (blank-skipping on both
/// sides) and returns `(child_index, ser_line_index)` for `source_line`, or
/// `None` on the first mismatch (the on-disk round-trip is stable, so a
/// mismatch means the visual and runtime diverged → caller falls back).
/// Decoupled from entities so the walk is unit-testable without the gpui test
/// harness.
fn match_wrapper_source_line(
    inner_lines: &[&str],
    child_ser_lines: &[Vec<String>],
    source_line: usize,
) -> Option<(usize, usize)> {
    if source_line >= inner_lines.len() {
        return None;
    }
    let mut cursor = 0usize;
    for (child_index, ser) in child_ser_lines.iter().enumerate() {
        for (ser_idx, ser_line) in ser.iter().enumerate() {
            if ser_line.trim().is_empty() {
                continue;
            }
            while cursor < inner_lines.len() && inner_lines[cursor].trim().is_empty() {
                cursor += 1;
            }
            if cursor >= inner_lines.len() || inner_lines[cursor].trim() != ser_line.trim() {
                return None;
            }
            if cursor == source_line {
                return Some((child_index, ser_idx));
            }
            cursor += 1;
        }
    }
    None
}

/// Pure core of [`Editor::wrapper_child_display_offset`]: converts a
/// serialized-line index + intra-line character offset into a flat child
/// display-text offset, clamping to the line's length. Code blocks skip their
/// fence lines (ser line 0 = opening fence → content start; trailing fence →
/// content end).
fn wrapper_display_offset(
    display: &str,
    is_code_block: bool,
    ser_line_index: usize,
    line_offset: usize,
) -> usize {
    if display.is_empty() {
        return 0;
    }
    let ranges = hard_line_ranges(display);
    if ranges.is_empty() {
        return 0;
    }
    let last = ranges.len() - 1;
    let within = |line_idx: usize| -> usize {
        let idx = line_idx.min(last);
        let range = &ranges[idx];
        range.start + line_offset.min(range.end.saturating_sub(range.start))
    };
    if is_code_block {
        // Serialized code lines = [opening fence, content lines…, closing fence].
        // ser 0 = opening fence → content start; ser 1..=N map to content hard
        // lines 0..N-1; ser > N = closing fence → content end.
        if ser_line_index == 0 {
            0
        } else if ser_line_index <= ranges.len() {
            within(ser_line_index - 1)
        } else {
            display.len()
        }
    } else {
        within(ser_line_index)
    }
}

#[cfg(test)]
mod pure_tests {
    use super::{match_wrapper_source_line, wrapper_display_offset};

    fn inner(src: &str) -> Vec<&str> {
        src.split('\n').collect()
    }

    #[test]
    fn maps_heading_paragraph_list_quote() {
        // inner_source lines (0-indexed): 2="# Title", 4="para",
        // 6/7="- a"/"- b", 9="> q".
        let inner_lines = inner("\n\n# Title\n\npara\n\n- a\n- b\n\n> q\n\n");
        let child_ser = vec![
            vec!["# Title".to_string()],
            vec!["para".to_string()],
            vec!["- a".to_string()],
            vec!["- b".to_string()],
            vec!["> q".to_string()],
        ];
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 2),
            Some((0, 0))
        );
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 4),
            Some((1, 0))
        );
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 6),
            Some((2, 0))
        );
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 7),
            Some((3, 0))
        );
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 9),
            Some((4, 0))
        );
    }

    #[test]
    fn maps_multiline_paragraph_two_lines() {
        let inner_lines = inner("\n\nl1\nl2\n\n");
        let child_ser = vec![vec!["l1".to_string(), "l2".to_string()]];
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 2),
            Some((0, 0))
        );
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 3),
            Some((0, 1))
        );
    }

    #[test]
    fn maps_code_block_fence_lines() {
        let inner_lines = inner("\n\n```rust\nfn main() {}\n```\n\n");
        let child_ser = vec![vec![
            "```rust".to_string(),
            "fn main() {}".to_string(),
            "```".to_string(),
        ]];
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 2),
            Some((0, 0))
        );
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 3),
            Some((0, 1))
        );
        assert_eq!(
            match_wrapper_source_line(&inner_lines, &child_ser, 4),
            Some((0, 2))
        );
    }

    #[test]
    fn mismatch_returns_none() {
        let inner_lines = inner("\n\n# Title\n\n");
        let child_ser = vec![vec!["# Different".to_string()]];
        assert_eq!(match_wrapper_source_line(&inner_lines, &child_ser, 2), None);
    }

    #[test]
    fn source_line_in_blank_region_returns_none() {
        // source_line 3 is a blank line — no child maps to it.
        let inner_lines = inner("\n\n# Title\n\n\nbody\n");
        let child_ser = vec![vec!["# Title".to_string()], vec!["body".to_string()]];
        assert_eq!(match_wrapper_source_line(&inner_lines, &child_ser, 3), None);
    }

    #[test]
    fn display_offset_paragraph_and_code() {
        let display = "para line one\npara line two";
        // hard lines: [0..13), [14..27) (total length 27).
        assert_eq!(wrapper_display_offset(display, false, 0, 3), 3);
        assert_eq!(wrapper_display_offset(display, false, 1, 0), 14);
        assert_eq!(wrapper_display_offset(display, false, 1, 5), 19);
        // clamp beyond the line's length → line end (27).
        assert_eq!(wrapper_display_offset(display, false, 1, 999), 27);

        let code = "fn main() {}";
        assert_eq!(wrapper_display_offset(code, true, 0, 3), 0); // opening fence
        assert_eq!(wrapper_display_offset(code, true, 1, 3), 3); // content line 0
        assert_eq!(wrapper_display_offset(code, true, 2, 3), code.len()); // closing fence
    }
}

#[cfg(all(test, feature = "wysiwyg-gpui-tests"))]
mod tests {
    // NOTE: do NOT `use super::*` here — combined with `#[gpui::test]` it
    // triggers an infinite macro-expansion recursion on the `#[test]` the
    // proc-macro emits. Import the few items the tests touch explicitly.
    use super::WrapperClickTarget;
    use crate::wysiwyg::components::markdown::html::editable_wrapper_inner_markdown;
    use crate::wysiwyg::components::{Block, BlockKind};
    use crate::wysiwyg::editor::Editor;
    use gpui::{AppContext, Entity, TestAppContext};

    /// Installs the wrapper runtime on the document's single wrapper block and
    /// returns `(wrapper, inner_source)`.
    fn install_wrapper(
        editor: &Entity<Editor>,
        cx: &mut TestAppContext,
    ) -> (Entity<Block>, String) {
        editor.update(cx, |editor, cx| {
            let wrapper = editor.document.first_root().expect("wrapper root").clone();
            assert!(editor.install_html_wrapper_runtime(&wrapper, cx));
            let inner = wrapper
                .read(cx)
                .record
                .html
                .as_deref()
                .and_then(editable_wrapper_inner_markdown)
                .expect("inner markdown");
            (wrapper, inner)
        })
    }

    fn map(
        editor: &Entity<Editor>,
        cx: &mut TestAppContext,
        wrapper: &Entity<Block>,
        inner: &str,
        source_line: usize,
    ) -> WrapperClickTarget {
        editor.update(cx, |editor, cx| {
            editor
                .map_wrapper_click_target(wrapper, inner, source_line, cx)
                .expect("mapping resolves")
        })
    }

    #[gpui::test]
    async fn maps_heading_paragraph_list_quote(cx: &mut TestAppContext) {
        let markdown = [
            "<div align=\"center\">",
            "",
            "# Title",
            "",
            "some **bold** text",
            "",
            "- item one",
            "- item two",
            "",
            "> a quote",
            "",
            "</div>",
        ]
        .join("\n");
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));
        let (wrapper, inner) = install_wrapper(&editor, cx);

        // inner source line indices: 2 = "# Title", 4 = paragraph,
        // 6/7 = list items, 9 = quote.
        let t = map(&editor, cx, &wrapper, &inner, 2);
        assert_eq!(t.child_index, 0);
        assert_eq!(t.ser_line_index, 0);

        let t = map(&editor, cx, &wrapper, &inner, 4);
        assert_eq!(t.child_index, 1);

        let t = map(&editor, cx, &wrapper, &inner, 6);
        assert_eq!(t.child_index, 2);
        let t = map(&editor, cx, &wrapper, &inner, 7);
        assert_eq!(t.child_index, 3);

        let t = map(&editor, cx, &wrapper, &inner, 9);
        assert_eq!(t.child_index, 4);
        assert_eq!(t.ser_line_index, 0);

        // Child kinds match the clicked lines.
        editor.update(cx, |editor, cx| {
            let children = &wrapper
                .read(cx)
                .html_wrapper_runtime
                .as_ref()
                .unwrap()
                .children;
            assert_eq!(children[0].read(cx).kind(), BlockKind::Heading { level: 1 });
            assert_eq!(children[1].read(cx).kind(), BlockKind::Paragraph);
            assert!(children[2].read(cx).kind().is_list_item());
            assert!(children[3].read(cx).kind().is_list_item());
            assert_eq!(children[4].read(cx).kind(), BlockKind::Quote);
        });
    }

    #[gpui::test]
    async fn maps_multiline_paragraph_intra_line(cx: &mut TestAppContext) {
        let markdown = [
            "<div align=\"center\">",
            "",
            "para line one",
            "para line two",
            "",
            "</div>",
        ]
        .join("\n");
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));
        let (wrapper, inner) = install_wrapper(&editor, cx);

        // Both source lines belong to the same paragraph child (ser 0 and 1).
        let t = map(&editor, cx, &wrapper, &inner, 2);
        assert_eq!(t.child_index, 0);
        assert_eq!(t.ser_line_index, 0);
        let t = map(&editor, cx, &wrapper, &inner, 3);
        assert_eq!(t.child_index, 0);
        assert_eq!(t.ser_line_index, 1);

        // Display offset: ser line 1 + line offset → 2nd hard line of display.
        editor.update(cx, |editor, cx| {
            let child = &wrapper
                .read(cx)
                .html_wrapper_runtime
                .as_ref()
                .unwrap()
                .children[0];
            let off = Editor::wrapper_child_display_offset(child, 1, 0, cx);
            assert_eq!(
                child.read(cx).display_text(),
                "para line one\npara line two"
            );
            // Hard line 1 starts at "para line one\n".len() + 1.
            assert_eq!(off, "para line one\n".len());
        });
    }

    #[gpui::test]
    async fn maps_code_block_fence_lines(cx: &mut TestAppContext) {
        let markdown = [
            "<div align=\"center\">",
            "",
            "```rust",
            "fn main() {}",
            "```",
            "",
            "</div>",
        ]
        .join("\n");
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));
        let (wrapper, inner) = install_wrapper(&editor, cx);

        // ser 0 = opening fence, ser 1 = content, ser 2 = closing fence.
        assert_eq!(map(&editor, cx, &wrapper, &inner, 2).ser_line_index, 0);
        assert_eq!(map(&editor, cx, &wrapper, &inner, 3).ser_line_index, 1);
        assert_eq!(map(&editor, cx, &wrapper, &inner, 4).ser_line_index, 2);

        editor.update(cx, |editor, cx| {
            let child = &wrapper
                .read(cx)
                .html_wrapper_runtime
                .as_ref()
                .unwrap()
                .children[0];
            assert!(child.read(cx).kind().is_code_block());
            // Opening fence → caret before content (0); closing fence → end.
            assert_eq!(Editor::wrapper_child_display_offset(child, 0, 3, cx), 0);
            assert_eq!(
                Editor::wrapper_child_display_offset(child, 2, 3, cx),
                "fn main() {}".len()
            );
        });
    }

    /// A markdown pipe-table inside an editable wrapper becomes a `RawMarkdown`
    /// source-block child (rendered as an editable code-style block), NOT a
    /// `Table`. This is because `table_runtime` is never installed for wrapper
    /// children — a `Table` child would fall through to the rich-text render
    /// fallback. See `install_html_wrapper_runtime`.
    #[gpui::test]
    async fn wrapper_table_child_becomes_source_block(cx: &mut TestAppContext) {
        let markdown = [
            "<div align=\"center\">",
            "",
            "| A | B |",
            "|---|---|",
            "| 1 | 2 |",
            "",
            "</div>",
        ]
        .join("\n");
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));
        editor.update(cx, |editor, cx| {
            let wrapper = editor.document.first_root().expect("wrapper root").clone();
            assert!(editor.install_html_wrapper_runtime(&wrapper, cx));
            let children = &wrapper
                .read(cx)
                .html_wrapper_runtime
                .as_ref()
                .unwrap()
                .children;
            assert_eq!(children.len(), 1);
            let child = &children[0];
            // Downgraded to RawMarkdown, not Table.
            assert_eq!(child.read(cx).kind(), BlockKind::RawMarkdown);
            // Editable source text carries the full pipe-table source
            // (serialized to canonical form: "| h |" rows + "| --- |" divider).
            let display = child.read(cx).display_text().to_string();
            assert!(display.contains("| A | B |"));
            assert!(display.contains("| 1 | 2 |"));
            assert!(display.contains("---"));
            // No native table runtime (which would require rebuild_table_runtimes
            // coverage wrapper children don't have).
            assert!(child.read(cx).table_runtime.is_none());
            // raw_fallback is seeded so serialization round-trips.
            assert_eq!(
                child.read(cx).record.raw_fallback.as_deref(),
                Some(display.as_str())
            );
        });
    }

    /// Editing the wrapper syncs the children back into the wrapper's HTML
    /// record; a downgraded table child keeps the full pipe-table source in the
    /// re-wrapped HTML, so the on-disk file is unchanged.
    #[gpui::test]
    async fn wrapper_table_child_round_trips(cx: &mut TestAppContext) {
        let markdown = [
            "<div align=\"center\">",
            "",
            "| A | B |",
            "|---|---|",
            "| 1 | 2 |",
            "",
            "</div>",
        ]
        .join("\n");
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));
        editor.update(cx, |editor, cx| {
            let wrapper = editor.document.first_root().expect("wrapper root").clone();
            assert!(editor.install_html_wrapper_runtime(&wrapper, cx));
            // Simulate a child edit triggering the wrapper re-sync.
            editor.sync_html_wrapper_record_from_runtime(&wrapper, cx);
            let html = wrapper.read(cx).record.title.visible_text().to_string();
            assert!(html.starts_with("<div align=\"center\">"));
            assert!(html.ends_with("</div>"));
            // The pipe-table source survives the round-trip (canonical form).
            assert!(html.contains("| A | B |"));
            assert!(html.contains("| 1 | 2 |"));
            assert!(html.contains("---"));
        });
    }
}
