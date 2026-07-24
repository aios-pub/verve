//! Central application state, shared across all panels via a single GPUI entity.

use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};

use super::models::*;

/// Events broadcast by [`AppState`] so panels can react to changes.
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// The workspace data changed structurally (projects/folders/requests) and
    /// should be persisted + the tree refreshed.
    WorkspaceChanged,
    /// The active workspace was switched — the in-memory `data` was reloaded
    /// from a different git branch's `workspace.json`. Panels should fully
    /// re-seed (tree, switchers, inputs).
    WorkspaceSwitched,
    /// The selected request changed.
    SelectionChanged,
    /// A request's editable fields changed (debounced persist).
    RequestEdited,
    /// A response arrived for a request (payload = request id).
    ResponseUpdated(String),
    /// The active environment changed.
    EnvironmentChanged,
    /// Theme should toggle.
    ToggleTheme,
    /// Request to locate/scroll-to the active request in the tree.
    LocateActive,
    /// Request to share a single API document (payload = request id). Emitted
    /// by the request panel's "share single API" button; handled by VerveApp.
    ShareRequest(String),
    /// A persist-to-disk just completed successfully. Git auto-sync hooks off
    /// this so commits coalesce with the debounced save.
    Persisted,
}

pub struct AppState {
    pub data: WorkspaceData,
    /// Index of the active project in `data.projects`.
    pub active_project: usize,
    /// Id of the active workspace (from `workspaces.json`). Tracked here so
    /// the UI knows which workspace is current; the source of truth lives in
    /// `workspaces.json` on disk.
    pub active_workspace_id: Option<String>,
    /// Currently selected request id (across the active project).
    pub selected_request: Option<String>,
    /// Currently selected folder id (when a directory node is clicked).
    /// Mutually exclusive with `selected_request` (clicking a request clears
    /// this, and vice-versa).
    pub selected_folder: Option<String>,
    /// Ordered list of request ids currently open as editor tabs.
    pub open_request_ids: Vec<String>,
    /// Which open tab is currently focused (the one loaded into the editor).
    pub active_tab_id: Option<String>,
    /// Pending request id that is currently sending (drives the Send button spinner).
    pub sending: Option<String>,
    /// Dirty flag; cleared on persist.
    pub dirty: bool,
    /// Outstanding debounced-save timer; replaced on each edit so saves
    /// coalesce ~1s after the last keystroke.
    pub save_timer: Option<gpui::Task<()>>,
    /// Shared, hot-swappable mock rule set (written by the UI, read by the
    /// running mock server on every request). Populated at startup in main.rs.
    pub mock_rules: Option<crate::mock::SharedRules>,
}

/// Global wrapper holding the single shared [`AppState`] entity.
#[derive(Clone)]
pub struct AppStateGlobal(pub Entity<AppState>);

impl Global for AppStateGlobal {}

impl AppState {
    /// Create, register, and return the global app state entity.
    pub fn init(cx: &mut App) -> Entity<Self> {
        // Load the active workspace id from the cross-branch index, then load
        // that workspace's data (workspace.json on the checked-out branch).
        let ws_idx = super::persistence::load_workspaces_index();
        let active_workspace_id = ws_idx.active.clone();
        let data = super::persistence::load_or_default();
        let active_project = data
            .active_project_id
            .as_deref()
            .and_then(|id| data.projects.iter().position(|p| &p.id == id))
            .unwrap_or(0);
        let entity: Entity<Self> = cx.new(|_| Self {
            data,
            active_project,
            active_workspace_id,
            selected_request: None,
            selected_folder: None,
            open_request_ids: Vec::new(),
            active_tab_id: None,
            sending: None,
            dirty: false,
            save_timer: None,
            mock_rules: None,
        });
        cx.set_global(AppStateGlobal(entity.clone()));
        entity
    }

    /// Access the shared state entity registered with the app.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<AppStateGlobal>().0.clone()
    }

    pub fn active_project(&self) -> Option<&Project> {
        self.data.projects.get(self.active_project)
    }

    /// Borrow the active project mutably.
    pub fn active_project_mut(&mut self) -> Option<&mut Project> {
        self.data.projects.get_mut(self.active_project)
    }

    /// Open a request as a tab: if already open, focus it; otherwise append.
    /// Keeps `selected_request` in sync with the active tab for backward
    /// compatibility (response panel, console, stress test, etc. all read it).
    pub fn open_or_focus_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected_folder = None;
        let is_new = !self.open_request_ids.contains(&id.to_string());
        if is_new {
            self.open_request_ids.push(id.to_string());
        }
        let already_active = self.active_tab_id.as_deref() == Some(id);
        self.active_tab_id = Some(id.to_string());
        self.selected_request = Some(id.to_string());
        if is_new || !already_active {
            cx.emit(AppEvent::SelectionChanged);
        }
    }

    /// Close an open tab and activate a neighbor if it was the active one.
    pub fn close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(pos) = self.open_request_ids.iter().position(|x| x == id) {
            self.open_request_ids.remove(pos);
            if self.active_tab_id.as_deref() == Some(id) {
                // Switch to neighbor: previous if it exists, else next.
                if pos > 0 {
                    self.active_tab_id = self.open_request_ids.get(pos - 1).cloned();
                } else {
                    self.active_tab_id = self.open_request_ids.get(0).cloned();
                }
                self.selected_request = self.active_tab_id.clone();
            }
        }
        // If no tabs remain, clear selection entirely.
        if self.open_request_ids.is_empty() {
            self.active_tab_id = None;
            self.selected_request = None;
        }
        cx.emit(AppEvent::SelectionChanged);
    }

    /// Activate an already-open tab by id (does not add it if missing).
    pub fn set_active_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.open_request_ids.iter().any(|x| x == id) {
            if self.active_tab_id.as_deref() != Some(id) {
                self.active_tab_id = Some(id.to_string());
                self.selected_request = Some(id.to_string());
                self.selected_folder = None;
                cx.emit(AppEvent::SelectionChanged);
            }
        }
    }

    /// Reload `workspace.json` from disk into `self.data`, resetting selection.
    /// Called after a workspace branch switch (git checkout rewrites
    /// `workspace.json` to the target workspace's content).
    pub fn reload_from_disk(&mut self, active_workspace_id: Option<String>) {
        self.data = super::persistence::load_or_default();
        self.active_workspace_id = active_workspace_id;
        // Resolve the saved active project id to an index; fall back to clamping
        // if the id isn't found (e.g. project was deleted on another branch).
        self.active_project = self
            .data
            .active_project_id
            .as_deref()
            .and_then(|id| self.data.projects.iter().position(|p| &p.id == id))
            .unwrap_or_else(|| self.data.projects.len().saturating_sub(1));
        self.selected_request = None;
        self.selected_folder = None;
        self.open_request_ids.clear();
        self.active_tab_id = None;
        self.sending = None;
        self.dirty = false;
    }

    /// Mark the workspace dirty and emit a [`AppEvent::WorkspaceChanged`].
    pub fn notify_workspace(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        // Hot-swap the mock server's rule set so newly-added/edited/toggled
        // rules take effect immediately without restart.
        self.refresh_mock_rules();
        cx.emit(AppEvent::WorkspaceChanged);
    }

    /// Rebuild the rule set from the active project and publish it to the
    /// running mock server via the SharedRules handle.
    pub fn refresh_mock_rules(&mut self) {
        if let (Some(shared), Some(p)) = (self.mock_rules.clone(), self.active_project()) {
            let entries =
                Arc::try_unwrap(crate::mock::rule_map(p)).unwrap_or_else(|arc| (*arc).clone());
            crate::mock::swap_rules(&shared, entries);
        }
    }

    pub fn notify_edited(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        cx.emit(AppEvent::RequestEdited);
        self.schedule_save(cx);
    }

    /// Schedule a debounced save ~1s out, replacing any pending one so rapid
    /// edits coalesce into a single write.
    pub fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        self.save_timer = Some(cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            let _ = weak.update(cx, |this, cx| this.persist(cx));
        }));
    }

    /// Persist to disk.
    pub fn persist(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = super::persistence::save(&self.data) {
            log::error!("persist failed: {e:?}");
        } else {
            self.dirty = false;
            cx.emit(AppEvent::Persisted);
        }
        let _ = cx;
    }
}

impl EventEmitter<AppEvent> for AppState {}
