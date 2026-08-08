//! Runtime context synchronization for blocks, references, images, and focus.

use super::*;

impl Editor {
    pub(super) fn current_edit_target_entity_id_from_state(&self, cx: &App) -> Option<EntityId> {
        self.active_entity_id
            .filter(|entity_id| self.focusable_entity_by_id(*entity_id).is_some())
            .or_else(|| {
                self.pending_focus
                    .filter(|entity_id| self.focusable_entity_by_id(*entity_id).is_some())
            })
            .or_else(|| self.first_focusable_entity_id(cx))
    }

    pub(super) fn current_edit_target_from_state(&self, cx: &App) -> Option<Entity<Block>> {
        self.current_edit_target_entity_id_from_state(cx)
            .and_then(|entity_id| self.focusable_entity_by_id(entity_id))
    }

    fn end_block_pointer_selection_sessions_inner(
        &mut self,
        cx: &mut Context<Self>,
        notify: bool,
    ) -> bool {
        let mut changed = false;

        if let Some(target) = self.current_edit_target_from_state(cx) {
            target.update(cx, |block, _cx| {
                changed |= block.end_pointer_selection_session();
            });
        }

        for visible in self.document.visible_blocks().to_vec() {
            visible.entity.update(cx, |block, _cx| {
                changed |= block.end_pointer_selection_session();
            });
        }

        // Collect only the cell Entity handles, not the whole TableCellBinding
        // (3-field struct of Entity<Block> + TableCellPosition). The collect()
        // exists to drop the &self borrow before the .update() loop; cloning
        // an Entity is an Arc bump, so we pay that once per cell either way —
        // skipping the surrounding struct clone makes the per-frame work
        // proportional to "cell count" not "binding count + position copy".
        let cells: Vec<Entity<Block>> = self
            .table_cells
            .values()
            .map(|binding| binding.cell.clone())
            .collect();
        for cell in cells {
            cell.update(cx, |block, _cx| {
                changed |= block.end_pointer_selection_session();
            });
        }

        if changed && notify {
            cx.notify();
        }
        changed
    }

    pub(super) fn end_block_pointer_selection_sessions(&mut self, cx: &mut Context<Self>) -> bool {
        self.end_block_pointer_selection_sessions_inner(cx, true)
    }

    /// Creates a new block entity and subscribes this editor to its
    /// [`BlockEvent`](crate::wysiwyg::components::BlockEvent) stream.
    pub(super) fn new_block(cx: &mut Context<Self>, record: BlockRecord) -> Entity<Block> {
        let block = cx.new(|cx| Block::with_record(cx, record));
        cx.subscribe(&block, Self::on_block_event).detach();
        block
    }

    pub(super) fn new_table_cell_block(
        cx: &mut Context<Self>,
        title: InlineTextTree,
        position: TableCellPosition,
        alignment: TableColumnAlignment,
    ) -> Entity<Block> {
        let block = Self::new_block(cx, BlockRecord::new(BlockKind::Paragraph, title));
        block.update(cx, |block, _cx| {
            block.set_table_cell_mode(position, alignment);
        });
        block
    }

    /// Wraps an already-built child block (parsed from the wrapper's inner
    /// Markdown) into wrapper-child mode at `index`. The block's own kind
    /// (Paragraph/Heading/…) is preserved; only the wrapper-child marker +
    /// rich-text edit mode are applied.
    pub(super) fn mark_html_wrapper_child(
        cx: &mut Context<Self>,
        block: &Entity<Block>,
        index: usize,
    ) {
        block.update(cx, |block, _cx| {
            block.set_html_wrapper_child_mode(index);
        });
    }

    pub(super) fn image_base_dir(&self) -> Option<PathBuf> {
        self.file_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| std::env::current_dir().ok())
    }

    pub(super) fn sync_runtime_context_for_block(
        &self,
        block: &Entity<Block>,
        base_dir: Option<&Path>,
        cx: &mut Context<Self>,
    ) {
        let next_base_dir = base_dir.map(Path::to_path_buf);
        let image_reference_definitions = self.image_reference_definitions.clone();
        let link_reference_definitions = self.link_reference_definitions.clone();
        let footnote_registry = self.footnote_registry.clone();
        block.update(cx, move |block, cx| {
            block.set_runtime_context(
                next_base_dir.clone(),
                image_reference_definitions.clone(),
                link_reference_definitions.clone(),
                footnote_registry.clone(),
            );
            cx.notify();
        });
    }

    pub(super) fn rebuild_footnote_registry(&mut self, cx: &App) {
        let mut definitions = HashMap::new();
        let visible = self.document.visible_blocks().to_vec();
        for visible_block in &visible {
            let block = visible_block.entity.read(cx);
            if block.kind() != BlockKind::FootnoteDefinition {
                continue;
            }

            let allow_definition = self
                .document
                .find_block_location(visible_block.entity.entity_id())
                .is_some_and(|location| {
                    location.parent.is_none()
                        || location
                            .parent
                            .as_ref()
                            .is_some_and(|parent| parent.read(cx).kind().is_quote_container())
                });
            if !allow_definition {
                continue;
            }

            definitions
                .entry(block.record.title.visible_text().to_string())
                .or_insert(visible_block.entity.entity_id());
        }

        let mut bindings = HashMap::<String, FootnoteDefinitionBinding>::new();
        for (id, entity_id) in definitions {
            bindings.insert(
                id,
                FootnoteDefinitionBinding {
                    ordinal: None,
                    definition_entity_id: entity_id,
                    first_reference: None,
                },
            );
        }

        let mut next_ordinal = 1usize;
        let mut occurrence_index = 0usize;
        let mut block_occurrences = HashMap::<uuid::Uuid, Vec<FootnoteResolvedOccurrence>>::new();
        for visible_block in visible {
            let block = visible_block.entity.read(cx);
            let block_id = block.record.id;
            for fragment in &block.record.title.fragments {
                let Some(footnote) = fragment.footnote.as_ref() else {
                    continue;
                };
                let ordinal = if let Some(binding) = bindings.get_mut(&footnote.id) {
                    if binding.ordinal.is_none() {
                        binding.ordinal = Some(next_ordinal);
                        next_ordinal += 1;
                    }
                    if binding.first_reference.is_none() {
                        binding.first_reference = Some(FootnoteReferenceLocation {
                            entity_id: visible_block.entity.entity_id(),
                            occurrence_index,
                        });
                    }
                    binding.ordinal
                } else {
                    None
                };
                block_occurrences
                    .entry(block_id)
                    .or_default()
                    .push(FootnoteResolvedOccurrence {
                        id: footnote.id.clone(),
                        ordinal,
                        occurrence_index,
                    });
                if ordinal.is_none() {
                    occurrence_index += 1;
                    continue;
                }
                occurrence_index += 1;
            }
        }

        self.footnote_registry = Arc::new(FootnoteRegistry {
            bindings,
            block_occurrences,
        });
    }

    pub(super) fn rebuild_image_runtimes(&mut self, cx: &mut Context<Self>) {
        let base_dir = self.image_base_dir();
        let markdown = self.document.markdown_text(cx);
        self.image_reference_definitions = Arc::new(parse_image_reference_definitions(&markdown));
        self.link_reference_definitions = Arc::new(parse_link_reference_definitions(&markdown));
        self.rebuild_footnote_registry(cx);
        let visible = self.document.visible_blocks().to_vec();
        for visible_block in visible {
            self.sync_runtime_context_for_block(&visible_block.entity, base_dir.as_deref(), cx);
            if visible_block.entity.read(cx).kind() != BlockKind::Table {
                continue;
            }
            let Some(runtime) = visible_block.entity.read(cx).table_runtime.clone() else {
                continue;
            };
            for cell in runtime.header {
                self.sync_runtime_context_for_block(&cell, base_dir.as_deref(), cx);
            }
            for row in runtime.rows {
                for cell in row {
                    self.sync_runtime_context_for_block(&cell, base_dir.as_deref(), cx);
                }
            }
        }
    }

    pub(super) fn focusable_entity_by_id(&self, entity_id: EntityId) -> Option<Entity<Block>> {
        self.document
            .block_entity_by_id(entity_id)
            .or_else(|| {
                self.table_cells
                    .get(&entity_id)
                    .map(|binding| binding.cell.clone())
            })
            .or_else(|| {
                self.html_wrapper_children
                    .get(&entity_id)
                    .map(|binding| binding.child.clone())
            })
    }

    /// The currently focused block entity id, for toolbar state detection and
    /// execution helpers that fire from click handlers (no Window required).
    pub(super) fn active_entity_id_for_toolbar(&self) -> Option<EntityId> {
        self.active_entity_id.or(self.pending_focus)
    }

    pub(super) fn first_focusable_entity_id(&self, cx: &App) -> Option<EntityId> {
        let first_root = self.document.first_root()?.clone();
        if first_root.read(cx).kind() == BlockKind::Table {
            return first_root
                .read(cx)
                .table_runtime
                .as_ref()
                .and_then(|runtime| runtime.header.first())
                .map(|cell| cell.entity_id())
                .or_else(|| Some(first_root.entity_id()));
        }

        // Auto-focusing a "source-raw" block (HtmlBlock/MathBlock/MermaidBlock/
        // RawMarkdown/Comment) flips it into raw-source display mode. That is
        // wrong on file open: a README that starts with `<div align=center>`
        // would show as raw `<div>` source instead of the rendered wrapper. So
        // prefer the first block that stays rendered when focused (paragraph,
        // heading, list, …). Fall back to the first root if every block is a
        // source-raw type so the editor still gets an anchor.
        let mut fallback = first_root.entity_id();
        for visible in self.document.visible_blocks() {
            let kind = visible.entity.read(cx).kind();
            if !matches!(
                kind,
                BlockKind::HtmlBlock
                    | BlockKind::MathBlock
                    | BlockKind::MermaidBlock
                    | BlockKind::RawMarkdown
                    | BlockKind::Comment
                    | BlockKind::CodeBlock { .. }
            ) {
                return Some(visible.entity.entity_id());
            }
            fallback = visible.entity.entity_id();
        }
        Some(fallback)
    }

    pub(super) fn focused_edit_target_entity_id(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<EntityId> {
        self.document
            .focused_block_entity_id(window, cx)
            .or_else(|| {
                self.table_cells
                    .values()
                    .find(|binding| binding.cell.read(cx).focus_handle.is_focused(window))
                    .map(|binding| binding.cell.entity_id())
            })
            .or_else(|| {
                self.html_wrapper_children
                    .values()
                    .find(|binding| binding.child.read(cx).focus_handle.is_focused(window))
                    .map(|binding| binding.child.entity_id())
            })
    }

    pub(super) fn focused_edit_target(&self, window: &Window, cx: &App) -> Option<Entity<Block>> {
        self.focused_edit_target_entity_id(window, cx)
            .and_then(|entity_id| self.focusable_entity_by_id(entity_id))
    }

    pub(super) fn table_cell_binding(&self, entity_id: EntityId) -> Option<TableCellBinding> {
        self.table_cells.get(&entity_id).cloned()
    }

    pub(super) fn table_block_by_id(&self, entity_id: EntityId, cx: &App) -> Option<Entity<Block>> {
        self.document
            .block_entity_by_id(entity_id)
            .filter(|block| block.read(cx).kind() == BlockKind::Table)
    }

    /// The HTML-wrapper-child binding for `entity_id`, if it is a child of a
    /// focused wrapper block.
    pub(super) fn html_wrapper_child_binding(
        &self,
        entity_id: EntityId,
    ) -> Option<HtmlWrapperChildBinding> {
        self.html_wrapper_children.get(&entity_id).cloned()
    }

    /// The wrapper (parent) HtmlBlock owning `child_id`, if it is a wrapper child.
    pub(super) fn html_wrapper_block_for_child(&self, child_id: EntityId) -> Option<Entity<Block>> {
        self.html_wrapper_children
            .get(&child_id)
            .map(|binding| binding.wrapper_block.clone())
    }
}
