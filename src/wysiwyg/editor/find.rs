//! In-editor find bar: query input, match computation, and navigation.
//!
//! Opened by the `Find` action (Cmd/Ctrl+F). The bar holds its own key context
//! (`EditorFindBar`) so `FindNext` / `FindPrevious` can drive navigation via
//! Cmd-G / F3 without clashing with block editing. Matches are computed over
//! each block's visible (rendered) text; navigating to a match focuses the
//! owning block and selects the matched range so the user sees it highlighted.

use std::ops::Range;

use gpui::*;
use gpui_component::input::{InputEvent, InputState};
use gpui::EntityInputHandler as _;

use super::Editor;
use crate::wysiwyg::components::{Find, FindNext, FindPrevious, Replace, ReplaceAll};

/// One query match, located inside a specific block's visible text.
#[derive(Clone, Debug)]
pub(super) struct FindMatch {
    /// Entity id of the block that contains the match.
    pub block: EntityId,
    /// Byte range of the match inside the block's visible text.
    pub range: Range<usize>,
}

/// State backing an open find bar.
pub(super) struct FindState {
    /// The query text input.
    pub input: Entity<InputState>,
    /// The replacement text input (shown when the bar is in replace mode).
    pub replace_input: Entity<InputState>,
    /// Whether the replacement row is visible (toggled by Cmd/Ctrl+H).
    pub replace_mode: bool,
    /// Matches for the current query, in document order.
    pub matches: Vec<FindMatch>,
    /// Index into `matches` of the currently highlighted match.
    pub current: usize,
}

impl Editor {
    /// Recompute matches for `query` across all visible blocks.  Matching is
    /// case-insensitive when the query is all-lowercase (the common
    /// editor convention); otherwise it is case-sensitive.  Returns the
    /// matches in visible-block order.
    pub(super) fn compute_find_matches(&self, query: &str, cx: &App) -> Vec<FindMatch> {
        if query.is_empty() {
            return Vec::new();
        }
        let case_sensitive = query.chars().any(|c| c.is_uppercase());
        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        let mut matches = Vec::new();
        for visible in self.document.visible_blocks() {
            let block = visible.entity.read(cx);
            // Skip non-text blocks (separators, images, empty containers) that
            // have no searchable visible text.
            let text = block.display_text();
            if text.is_empty() {
                continue;
            }
            let haystack = if case_sensitive {
                text.to_string()
            } else {
                text.to_lowercase()
            };
            let mut from = 0;
            while let Some(rel) = haystack[from..].find(&needle) {
                let start = from + rel;
                let end = start + needle.len();
                matches.push(FindMatch {
                    block: visible.entity.entity_id(),
                    range: start..end,
                });
                from = end.max(start + 1);
            }
        }
        matches
    }

    /// Refresh `find_state`'s matches from its current query. The current
    /// pointer is re-anchored at the first match at/after the active block so
    /// re-typing keeps the caret nearby instead of jumping back to the top.
    fn refresh_find_matches(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.find_state.as_ref() else {
            return;
        };
        let query = state.input.read(cx).value().to_string();
        let active_index = self
            .active_entity_id
            .and_then(|id| self.document.visible_index_for_entity_id(id));
        let matches = self.compute_find_matches(&query, cx);
        let current = match (active_index, matches.first()) {
            (Some(_), Some(_)) => matches
                .iter()
                .position(|m| {
                    self.document
                        .visible_index_for_entity_id(m.block)
                        .is_some_and(|i| i >= active_index.unwrap_or(0))
                })
                .unwrap_or(0),
            _ => 0,
        };
        if let Some(state) = self.find_state.as_mut() {
            state.matches = matches;
            state.current = current;
        }
        self.goto_current_find_match(false, cx);
    }

    /// Focus the block owning `find_state.matches[current]` and select the
    /// matched range.  `advance` moves to the next match first (with wrap).
    fn goto_current_find_match(&mut self, advance: bool, cx: &mut Context<Self>) {
        let Some(state) = self.find_state.as_ref() else {
            return;
        };
        if state.matches.is_empty() {
            return;
        }
        if advance {
            let next = (state.current + 1) % state.matches.len();
            if let Some(state) = self.find_state.as_mut() {
                state.current = next;
            }
        }
        let m = self
            .find_state
            .as_ref()
            .and_then(|state| state.matches.get(state.current).cloned());
        let Some(m) = m else {
            return;
        };
        if let Some(block) = self.document.block_entity_by_id(m.block) {
            self.focus_block_range(&block, m.range, cx);
        }
    }

    pub(super) fn on_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_state.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find..."));
            let replace_input =
                cx.new(|cx| InputState::new(window, cx).placeholder("Replace..."));
            let input_for_sub = input.clone();
            // Recompute matches on every keystroke so the bar stays live.
            cx.subscribe(
                &input_for_sub,
                move |this, _src, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.refresh_find_matches(cx);
                        cx.notify();
                    }
                },
            )
            .detach();
            self.find_state = Some(FindState {
                input,
                replace_input,
                replace_mode: false,
                matches: Vec::new(),
                current: 0,
            });
        }
        // Focus the query input so the user can type immediately.
        if let Some(state) = self.find_state.as_ref() {
            state.input.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.refresh_find_matches(cx);
        cx.notify();
    }

    /// Toggle the replace row open (Cmd/Ctrl+H). Opens the find bar if needed
    /// and focuses the replacement field.
    pub(super) fn on_replace(&mut self, _: &Replace, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_state.is_none() {
            // Open the bar first (without stealing focus to the query field).
            self.on_find(&Find, window, cx);
        }
        if let Some(state) = self.find_state.as_mut() {
            state.replace_mode = true;
        }
        if let Some(state) = self.find_state.as_ref() {
            state
                .replace_input
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        }
        cx.notify();
    }

    /// Replace the current match with the replacement text and advance.
    pub(super) fn on_replace_one(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (block, range, replacement) = match self.find_state.as_ref() {
            Some(state) if !state.matches.is_empty() => {
                let m = &state.matches[state.current];
                let replacement = state.replace_input.read(cx).value().to_string();
                let Some(block) = self.document.block_entity_by_id(m.block) else {
                    return;
                };
                (block, m.range.clone(), replacement)
            }
            _ => return,
        };
        // Select the match, then replace the selection with the replacement.
        self.focus_block_range(&block, range.clone(), cx);
        block.update(cx, |block, cx| {
            block.selected_range = range;
            cx.notify();
        });
        // Drive the replacement through the block's own text-replace path so
        // undo/history stay consistent.
        block.update(cx, |block, cx| {
            block.replace_text_in_range(None, &replacement, _window, cx);
        });
        self.refresh_find_matches(cx);
        cx.notify();
    }

    /// Replace every match in the document, then close replace mode.
    pub(super) fn on_replace_all(
        &mut self,
        _: &ReplaceAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Snapshot the matches + replacement up front, since each replacement
        // mutates block text and shifts offsets. We re-derive matches per block
        // rather than trusting stale ranges.
        let Some(state) = self.find_state.as_ref() else {
            return;
        };
        let query = state.input.read(cx).value().to_string();
        let replacement = state.replace_input.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }
        let case_sensitive = query.chars().any(|c| c.is_uppercase());
        let needle = if case_sensitive {
            query.clone()
        } else {
            query.to_lowercase()
        };

        // Collect the distinct blocks that contain at least one match, then
        // replace within each from the end so earlier ranges stay valid.
        let mut blocks: Vec<gpui::Entity<crate::wysiwyg::components::Block>> = Vec::new();
        for visible in self.document.visible_blocks() {
            if self
                .document
                .visible_index_for_entity_id(visible.entity.entity_id())
                .is_some()
            {
                blocks.push(visible.entity.clone());
            }
        }
        for block in blocks {
            let text = block.read(cx).display_text().to_string();
            let haystack = if case_sensitive {
                text.clone()
            } else {
                text.to_lowercase()
            };
            // Collect match ranges (byte offsets into visible text) from the end.
            let mut ranges: Vec<Range<usize>> = Vec::new();
            let mut from = 0;
            while let Some(rel) = haystack[from..].find(&needle) {
                let start = from + rel;
                let end = start + needle.len();
                ranges.push(start..end);
                from = end.max(start + 1);
            }
            if ranges.is_empty() {
                continue;
            }
            // Apply replacements from right to left so byte offsets stay valid.
            // We rebuild the visible text, then splice it back as one edit.
            let mut new_text = text.clone();
            for range in ranges.iter().rev() {
                new_text.replace_range(range.clone(), &replacement);
            }
            block.update(cx, |b, cx| {
                b.selected_range = 0..b.display_text().len();
                cx.notify();
            });
            block.update(cx, |block, cx| {
                block.replace_text_in_range(None, &new_text, window, cx);
            });
        }
        self.refresh_find_matches(cx);
        cx.notify();
    }

    pub(super) fn on_find_next(
        &mut self,
        _: &FindNext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goto_current_find_match(true, cx);
        cx.notify();
    }

    pub(super) fn on_find_previous(
        &mut self,
        _: &FindPrevious,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.find_state.as_ref() else {
            return;
        };
        if state.matches.is_empty() {
            return;
        }
        let prev = if state.current == 0 {
            state.matches.len() - 1
        } else {
            state.current - 1
        };
        if let Some(state) = self.find_state.as_mut() {
            state.current = prev;
        }
        self.goto_current_find_match(false, cx);
        cx.notify();
    }

    /// Close the find bar (Esc / close button). Restores focus to the active
    /// block so editing resumes.
    pub(super) fn close_find_bar(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.find_state.take().is_some() {
            if let Some(active) = self.active_entity_id {
                self.focus_block(active);
            }
            cx.notify();
        }
    }
}
