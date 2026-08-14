//! Variable completion provider for kv-table value inputs.
//!
//! When the cursor sits inside an unclosed `{{` placeholder, the completion
//! menu offers the dynamic variables (`{{$uuid}}`, `{{$datetime}}`, …) plus
//! the project's global / active-environment user variables. Selecting an item
//! inserts `{{name}}` (replacing the partial `{{...` up to the cursor).

use anyhow::Result;
use gpui::{App, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse,
    CompletionTextEdit, Documentation, InsertReplaceEdit,
};
use ropey::Rope;

use crate::http::variable::dynamic_variable_names;
use crate::state::AppState;

/// Builds the LSP-style completion list for `{{...}}` variable references.
pub struct VarCompletionProvider {
    state: gpui::Entity<AppState>,
}

impl VarCompletionProvider {
    pub fn new(state: gpui::Entity<AppState>) -> Self {
        Self { state }
    }

    /// Collect `(name, description)` candidates: dynamic variables first, then
    /// the project's global + active-environment user variables.
    fn candidates(&self, cx: &App) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = dynamic_variable_names()
            .iter()
            .map(|(n, d)| (n.to_string(), d.to_string()))
            .collect();
        if let Some(p) = self.state.read(cx).active_project() {
            for kv in &p.global_variables {
                if kv.enabled && !kv.key.trim().is_empty() {
                    out.push((kv.key.clone(), format!("= {}", kv.value)));
                }
            }
            for kv in p.active_env_variables() {
                if kv.enabled && !kv.key.trim().is_empty() {
                    out.push((kv.key.clone(), format!("= {}", kv.value)));
                }
            }
        }
        out
    }
}

impl CompletionProvider for VarCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut gpui::Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        // Text before the cursor. `offset` is the cursor's byte offset (always a
        // UTF-8 boundary); copy to a String and slice with `.get` for boundary
        // safety (AGENTS.md §二), avoiding ropey byte-slicing pitfalls.
        let full = text.to_string();
        let before = full.get(..offset).unwrap_or(&full);

        // Find the last `{{` whose body (up to the cursor) is still open, i.e.
        // contains no closing `}}`. `rfind` returns a byte index; `{{` and the
        // following `+2` are ASCII boundaries, so `.get(..)` below is safe.
        let open = before.rfind("{{").filter(|&brace_pos| {
            match before.get(brace_pos + 2..) {
                Some(rest) => !rest.contains("}}"),
                None => true,
            }
        });

        let items: Vec<CompletionItem> = match open {
            None => Vec::new(),
            Some(brace_pos) => {
                // `{{` is ASCII so `brace_pos + 2` is a char boundary; `.get`
                // avoids a raw byte slice (AGENTS.md §二).
                let filter = before.get(brace_pos + 2..).map(|s| s.trim()).unwrap_or("");
                let range = lsp_types::Range::new(
                    text.offset_to_position(brace_pos),
                    text.offset_to_position(offset),
                );
                self.candidates(cx)
                    .into_iter()
                    .filter(|(name, _)| filter.is_empty() || name.contains(filter))
                    .map(|(name, doc)| CompletionItem {
                        label: name.clone(),
                        kind: Some(CompletionItemKind::VARIABLE),
                        text_edit: Some(CompletionTextEdit::InsertAndReplace(InsertReplaceEdit {
                            new_text: format!("{{{{{}}}}}", name),
                            insert: range,
                            replace: range,
                        })),
                        documentation: Some(Documentation::String(doc)),
                        ..Default::default()
                    })
                    .collect()
            }
        };

        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut gpui::Context<InputState>,
    ) -> bool {
        if new_text.is_empty() {
            return false;
        }
        // Trigger when typing `{`/`}` (open/close braces) or identifier chars
        // (so the menu keeps filtering as the user types the variable name).
        new_text == "{"
            || new_text == "}"
            || new_text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    }
}
