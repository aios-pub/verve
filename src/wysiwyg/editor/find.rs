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

use super::Editor;
use crate::wysiwyg::components::{Find, FindNext, FindPrevious};

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
