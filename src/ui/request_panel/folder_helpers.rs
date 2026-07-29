//! Folder/interface helper types and functions: the interface-table entry
//! model + collector, folder tab labels, and folder base-URL resolution.
use std::collections::BTreeMap;
use std::sync::Arc;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Icon;
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::{ActiveTheme, Disableable as _, IconName, Selectable as _, Sizable as _, button::{Button, ButtonVariants as _}, h_flex, popover::Popover, v_flex};
use crate::http;
use crate::state::models::*;
use crate::state::AppState;
use crate::ui::kv_table::{self, KvRow};
use super::{RequestPanel, FolderKvSection, FolderTab, ReqTab};

/// A flattened request entry in a folder's interface list. Carries all the
/// column data (name/method/path/audit metadata) so the table renders any
/// combination of columns.
#[derive(Clone)]
pub(super) struct IfaceEntry {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) method: RequestMethod,
    pub(super) protocol: Protocol,
    pub(super) url: String,
    /// Owning folder name (immediate parent).
    pub(super) folder_name: String,
    pub(super) created_by: String,
    pub(super) created_at: String,
    pub(super) updated_by: String,
    pub(super) updated_at: String,
    pub(super) status: String,
    pub(super) tags: Vec<String>,
}

impl IfaceEntry {
    /// Render the cell text for a column.
    pub(super) fn cell_text(&self, col: IfaceColumn) -> String {
        match col {
            IfaceColumn::Name => self.name.clone(),
            IfaceColumn::Method => self.method.as_str().to_string(),
            IfaceColumn::Path => self.url.clone(),
            IfaceColumn::Folder => self.folder_name.clone(),
            IfaceColumn::CreatedBy => self.created_by.clone(),
            IfaceColumn::CreatedAt => self.created_at.clone(),
            IfaceColumn::UpdatedBy => self.updated_by.clone(),
            IfaceColumn::UpdatedAt => self.updated_at.clone(),
            IfaceColumn::Status => self.status.clone(),
            IfaceColumn::Tags => self.tags.join(", "),
        }
    }
}

/// Collect an entry for every request that is a **direct child** of the
/// folder (its own `requests` only — not those of nested sub-folders). The
/// 接口目录 column shows this folder's name, since all listed interfaces live
/// in it directly.
pub(super) fn collect_iface_entries(folder: &Folder) -> Vec<IfaceEntry> {
    folder
        .requests
        .iter()
        .map(|req| IfaceEntry {
            id: req.id.clone(),
            name: req.name.clone(),
            method: req.method,
            protocol: req.protocol,
            url: req.url.clone(),
            folder_name: folder.name.clone(),
            created_by: req.created_by.clone(),
            created_at: req.created_at.clone(),
            updated_by: req.updated_by.clone(),
            updated_at: req.updated_at.clone(),
            status: req.status.clone(),
            tags: req.tags.clone(),
        })
        .collect()
}

/// Build a clickable folder-tab label.
pub(super) fn folder_tab_label(
    label: &'static str,
    is_active: bool,
    tab: FolderTab,
    theme: &gpui_component::Theme,
    cx: &mut Context<RequestPanel>,
) -> impl IntoElement {
    div()
        .id(label.to_string())
        .px_3()
        .py_2()
        .text_sm()
        .text_color(if is_active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .when(is_active, |this| {
            this.border_b_2().border_color(theme.primary)
        })
        .hover(|this| this.bg(theme.accent.opacity(0.5)))
        .child(label.to_string())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.folder_tab = tab;
            cx.notify();
        }))
}


// Silence unused import warnings for items used only conditionally.
#[allow(dead_code)]
pub(super) fn _unused(_a: Arc<()>, _t: Task<()>) {}

/// Recursively set `base_url` on the folder with the given id.
pub(super) fn set_folder_base_url(
    folders: &mut [crate::state::models::Folder],
    id: &str,
    url: Option<String>,
) -> bool {
    for f in folders.iter_mut() {
        if f.id == id {
            f.base_url = url.clone();
            return true;
        }
        if set_folder_base_url(&mut f.folders, id, url.clone()) {
            return true;
        }
    }
    false
}

/// Resolve the effective base URL for a request by walking up the folder
/// chain. Returns the first non-empty base_url found in the folder hierarchy.
///
/// The stored `base_url` may contain `{{var}}` placeholders (when it was set
/// by picking an environment variable from the dropdown). These are
/// substituted against the active environment + global variables so the
/// resolved value always reflects the current environment.
pub(super) fn resolve_folder_base_url(
    project: &crate::state::models::Project,
    chain: &[String],
) -> Option<String> {
    // Build a variable map for substitution: globals + active env + folder
    // variables along the chain (so a folder's own variables can also be
    // referenced in a base_url set on that same folder or an ancestor).
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for kv in &project.global_variables {
        if kv.enabled && !kv.key.trim().is_empty() {
            vars.insert(kv.key.clone(), kv.value.clone());
        }
    }
    for kv in project.active_env_variables() {
        if kv.enabled && !kv.key.trim().is_empty() {
            vars.insert(kv.key.clone(), kv.value.clone());
        }
    }
    // Walk the chain from the deepest folder up to find a base_url.
    for depth in (0..chain.len()).rev() {
        let id = &chain[depth];
        if let Some((_, folder)) = project.find_folder(id) {
            if let Some(url) = &folder.base_url {
                if !url.trim().is_empty() {
                    // Substitute any {{var}} placeholders against the vars
                    // collected so far (incl. this folder's own variables).
                    for kv in &folder.variables {
                        if kv.enabled && !kv.key.trim().is_empty() {
                            vars.insert(kv.key.clone(), kv.value.clone());
                        }
                    }
                    let resolved = crate::http::variable::substitute(url, &vars);
                    if !resolved.trim().is_empty() {
                        return Some(resolved.trim_end_matches('/').to_string());
                    }
                }
            }
        }
    }
    None
}

/// Apply autosave logic: if the global autosave setting is enabled, save the
/// response as a success or failure example on the request.
///
/// - Success responses (2xx/3xx, no error) → stored as the single `success_example` (overwrite).
/// - Failure responses (4xx/5xx/network error) → appended to `fail_examples`, deduplicated by
///   (status + body) so identical responses don't accumulate duplicates.
pub(super) fn apply_autosave_example(req: &mut ApiRequest, resp: &Response) {
    use crate::state::persistence::AutoSaveMode;

    let mode = crate::state::persistence::load_autosave_mode();
    if mode == AutoSaveMode::Off {
        return;
    }

    let is_success = ResponseExample::is_success_status(resp.status, resp.error.as_deref());

    // Route by outcome: success → single success example (overwrite);
    // failure → deduplicated failure examples list. A "both" mode handles each.
    if is_success && mode.saves_success() {
        req.success_example = Some(ResponseExample::from_response(resp));
    }
    if !is_success && mode.saves_failure() {
        let example = ResponseExample::from_response(resp);
        // Deduplicate: remove any existing example with the same
        // (status + body) key, then push the new one at the front.
        let key = example.dedup_key();
        req.fail_examples.retain(|e| e.dedup_key() != key);
        req.fail_examples.insert(0, example);
        // Cap failure examples at 20 to prevent unbounded growth.
        if req.fail_examples.len() > 20 {
            req.fail_examples.truncate(20);
        }
    }
}
