//! App-menu action dispatch stubs.
//!
//! velotype drives the editor from a native/in-window menu bar whose actions
//! are dispatched here (`dispatch_menu_action_for_editor`) and also records
//! recently opened files. verve owns its own menu and recent-file tracking, so
//! in the embedded build these are thin no-ops / forwarders. The editor's menu
//! action handlers (save, export, toggle view mode, ...) are still reachable
//! via verve's own UI and keybindings.

use std::path::Path;

use gpui::{App, Entity, WeakEntity, Window, actions};

use crate::wysiwyg::editor::Editor;

actions!(velotype_embedded, []);

/// Dispatches a menu action to the editor entity.
///
/// In velotype this owned the full menu-routing logic. For the embedded build
/// we simply forward the action onto the target editor so its `on_action`
/// handlers still run.
pub(crate) fn dispatch_menu_action_for_editor(
    action: &dyn gpui::Action,
    target: &WeakEntity<Editor>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(handle) = target.upgrade() {
        let _ = handle.update(cx, |_editor, cx| {
            window.dispatch_action(action.boxed_clone(), cx);
        });
    }
}

/// No-op: verve manages application lifecycle.
#[allow(dead_code)]
pub(crate) fn request_quit_application(_cx: &mut App) {}

/// No-op: verve has no CLI tool install flow.
#[allow(dead_code)]
pub(crate) fn install_cli_tool(_cx: &mut App) {}

/// No-op: verve has no CLI tool install flow.
#[allow(dead_code)]
pub(crate) fn uninstall_cli_tool(_cx: &mut App) {}

/// No-op: verve tracks recent files in its own project store.
#[allow(dead_code)]
pub(crate) fn record_recent_file_from_editor(_path: &Path, _cx: &mut App) {}

// Keep the Entity<Editor> import used so the stub compiles cleanly.
#[allow(dead_code)]
fn _ensure_editor_import(_e: Entity<Editor>) {}
