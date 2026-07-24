//! Left pane: a searchable project navigation tree.
//!
//! Renders the active project's folders and requests using the gpui-component
//! `tree`. Each request shows a colored method badge; folders are collapsible.
//! A context menu offers new folder/request, rename, delete, duplicate.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tree::{TreeItem, TreeState, tree};
use gpui_component::{
    ActiveTheme, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    popover::Popover,
    v_flex,
};

use crate::state::models::{MoveTarget, Protocol, RequestMethod, new_id};
use crate::state::{AppEvent, AppState};

/// Tag embedded in a `TreeItem.id` so we can tell what a node refers to.
/// Format: `folder:<id>` or `request:<id>`.
fn folder_node(id: &str) -> String {
    format!("folder:{id}")
}
fn request_node(id: &str) -> String {
    format!("request:{id}")
}

/// Events emitted by the project tree panel.
#[derive(Clone, Debug)]
pub enum TreeEvent {
    /// User clicked "Share" on a request: (request_id, request_name)
    ShareRequest(String, String),
}

/// The payload carried during a tree drag. Implements `Render` so GPUI can
/// show a floating preview under the cursor.
#[derive(Clone, Debug)]
pub struct TreeDrag {
    /// "request:<id>" or "folder:<id>".
    pub node: String,
    /// Human-readable label for the drag preview.
    pub label: String,
}

impl Render for TreeDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .bg(theme.primary.opacity(0.9))
            .text_color(gpui::white())
            .text_xs()
            .shadow_md()
            .child(self.label.clone())
    }
}

pub struct ProjectTreePanel {
    pub state: Entity<AppState>,
    pub tree: Entity<TreeState>,
    pub search: Entity<InputState>,
    pub rename_input: Entity<InputState>,
    /// The tree node ("request:<id>" / "folder:<id>") whose "..." action menu
    /// is currently open, if any.
    pub open_menu_node: Option<String>,
    /// The window Y-coordinate (px) of the click that opened the "..." menu,
    /// used to decide whether the menu appears above or below the row.
    pub menu_click_y: Option<f32>,
    /// Whether the "+" new-request popover is open (controlled).
    pub new_popover_open: bool,
    /// Pending folder id for the per-folder "+" add popover (None = root).
    pub pending_folder_add: Option<String>,
    /// Whether the per-folder "+" add popover is open (controlled).
    pub folder_add_popover_open: bool,
    /// Folder node-ids ("folder:<id>") the user has collapsed; preserved across
    /// tree rebuilds so collapse state isn't lost on selection changes.
    pub collapsed_folders: std::collections::HashSet<String>,
    /// The tree node id currently being dragged (e.g. "request:abc").
    pub dragging_node: Option<String>,
    /// The tree node id the cursor is hovering over while dragging (drop
    /// target highlight). Cleared on drop.
    pub drop_target: Option<String>,
    _subs: Vec<gpui::Subscription>,
    focus_handle: FocusHandle,
}

impl ProjectTreePanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search requests..."));

        let mut panel = Self {
            state: state.clone(),
            tree,
            search,
            rename_input,
            open_menu_node: None,
            menu_click_y: None,
            new_popover_open: false,
            pending_folder_add: None,
            folder_add_popover_open: false,
            collapsed_folders: std::collections::HashSet::new(),
            dragging_node: None,
            drop_target: None,
            _subs: Vec::new(),
            focus_handle: cx.focus_handle(),
        };
        panel.rebuild_tree(cx);
        let sub_search = cx.subscribe(&panel.search, Self::on_search_change);
        let sub_state = cx.subscribe(&panel.state, Self::on_state_event);
        let sub_tree = cx.observe(&panel.tree, Self::on_tree_notify);
        panel._subs = vec![sub_search, sub_state, sub_tree];
        panel
    }

    fn on_search_change(
        &mut self,
        _src: Entity<InputState>,
        _ev: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_tree(cx);
    }

    /// Called whenever the tree entity notifies (on_entry_click sets
    /// selected_ix + notify, right-click sets right_clicked_ix + notify).
    /// Sync the framework's selected_ix → AppState. The dedup check prevents
    /// loops: if AppState already matches, no action is taken.
    fn on_tree_notify(&mut self, _tree: Entity<TreeState>, cx: &mut Context<Self>) {
        let item = match self.tree.read(cx).selected_item() {
            Some(item) => item.clone(),
            None => return,
        };
        let node = item.id.to_string();
        if let Some(req_id) = parse_request_id(&node) {
            let req_id = req_id.to_string();
            // Always call on_select_request; open_or_focus_tab is idempotent:
            // if the tab is already open and active it's a no-op, if open but
            // not active it focuses, if not open it opens.
            self.on_select_request(req_id, cx);
        } else if let Some(fid) = parse_folder_id(&node) {
            let fid = fid.to_string();
            if self.state.read(cx).selected_folder.as_deref() != Some(&fid) {
                self.on_select_folder(fid, cx);
            }
        }
    }

    fn on_state_event(&mut self, _src: Entity<AppState>, ev: &AppEvent, cx: &mut Context<Self>) {
        match ev {
            AppEvent::WorkspaceChanged | AppEvent::RequestEdited => {
                self.rebuild_tree(cx);
            }
            AppEvent::SelectionChanged => {
                cx.notify();
            }
            AppEvent::LocateActive => {
                self.locate_active(cx);
            }
            _ => {}
        }
    }

    /// Rebuild the tree items from the active project, filtered by the search
    /// query.
    pub fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_lowercase();
        let project = match self.state.read(cx).active_project() {
            Some(p) => p,
            None => {
                self.tree.update(cx, |t, cx| t.set_items(Vec::new(), cx));
                return;
            }
        };

        let mut items: Vec<TreeItem> = Vec::new();

        // Folder items.
        let collapsed = self.collapsed_folders.clone();
        for folder in &project.folders {
            if let Some(item) = folder_tree_item(folder, &query, &collapsed) {
                items.push(item);
            }
        }
        // Root-level requests.
        for req in &project.requests {
            if matches_query(&req.name, &query) {
                items.push(TreeItem::new(request_node(&req.id), req.name.clone()));
            }
        }

        self.tree.update(cx, |t, cx| t.set_items(items, cx));
    }

    fn on_select_request(&mut self, request_id: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.open_or_focus_tab(&request_id, cx);
        });
    }

    /// Select a folder node: clears the request selection and emits
    /// SelectionChanged so the center panel swaps to the folder detail view.
    fn on_select_folder(&mut self, folder_id: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.selected_folder = Some(folder_id);
            s.selected_request = None;
            s.active_tab_id = None;
            cx.emit(AppEvent::SelectionChanged);
        });
    }

    /// Apply a drag-and-drop: move the dragged node (`source`) relative to the
    /// `target` node. Folders receive the node; requests reorder beside them.
    pub fn apply_drop(&mut self, source: &str, target: &str, cx: &mut Context<Self>) {
        if source == target {
            self.dragging_node = None;
            self.drop_target = None;
            cx.notify();
            return;
        }
        let moved = self.state.update(cx, |s, cx| {
            let src_req = parse_request_id(source).map(|s| s.to_string());
            let src_folder = parse_folder_id(source).map(|s| s.to_string());
            let tgt_req = parse_request_id(target).map(|s| s.to_string());
            let tgt_folder = parse_folder_id(target).map(|s| s.to_string());

            let project = match s.active_project_mut() {
                Some(p) => p,
                None => return false,
            };
            let ok = if let Some(req_id) = src_req {
                // Dragging a request.
                let dest = if let Some(fid) = tgt_folder {
                    MoveTarget::IntoFolder(fid)
                } else if let Some(t) = tgt_req {
                    // Default to dropping before the target request.
                    MoveTarget::BeforeRequest(t)
                } else {
                    MoveTarget::ToRoot
                };
                project.move_request(&req_id, &dest)
            } else if let Some(fid) = src_folder {
                // Dragging a folder — only into another folder / root.
                let dest = if let Some(t) = tgt_folder {
                    MoveTarget::IntoFolder(t)
                } else {
                    MoveTarget::ToRoot
                };
                project.move_folder(&fid, &dest)
            } else {
                false
            };
            if ok {
                s.notify_workspace(cx);
            }
            ok
        });
        let _ = moved;
        self.dragging_node = None;
        self.drop_target = None;
        cx.notify();
    }
}

fn folder_tree_item(
    folder: &crate::state::models::Folder,
    query: &str,
    collapsed: &std::collections::HashSet<String>,
) -> Option<TreeItem> {
    let node = folder_node(&folder.id);
    let is_expanded = !collapsed.contains(&node);
    // Always build children so is_folder() returns true even when collapsed
    // (the tree framework hides children when expanded=false, but needs them
    // to identify the node as a folder for chevron/icon rendering).
    let mut children: Vec<TreeItem> = Vec::new();
    for sub in &folder.folders {
        if let Some(item) = folder_tree_item(sub, query, collapsed) {
            children.push(item);
        }
    }
    for req in &folder.requests {
        if matches_query(&req.name, query) {
            children.push(TreeItem::new(request_node(&req.id), req.name.clone()));
        }
    }
    // With a search query, hide folders that don't match and have no matching
    // children. Without a query, always show every folder.
    if !query.is_empty() && children.is_empty() && !matches_query(&folder.name, query) {
        return None;
    }
    Some(
        TreeItem::new(node, folder.name.clone())
            .children(children)
            .expanded(is_expanded),
    )
}

fn matches_query(label: &str, query: &str) -> bool {
    query.is_empty() || label.to_lowercase().contains(query)
}

/// Extract the node-id string payload from a node-carrying TreeAction.
fn node_of(action: &TreeAction) -> String {
    match action {
        TreeAction::Delete(n)
        | TreeAction::Rename(n)
        | TreeAction::Share(n)
        | TreeAction::CopyAsCurl(n)
        | TreeAction::CopyToBranch(n)
        | TreeAction::MoveTo(n)
        | TreeAction::Copy(n)
        | TreeAction::Clone(n)
        | TreeAction::DuplicateRequest(n) => n.clone(),
        TreeAction::AddRequest(_) | TreeAction::AddFolder(_) => String::new(),
    }
}

/// Parse a `request:<id>` node id out of a tree item id.
fn parse_request_id(node: &str) -> Option<&str> {
    node.strip_prefix("request:")
}

fn parse_folder_id(node: &str) -> Option<&str> {
    node.strip_prefix("folder:")
}

impl Render for ProjectTreePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity();
        let _ = view;
        let _selected_id = self.state.read(cx).selected_request.clone();
        v_flex()
            .id("project-tree-root")
            .key_context("ProjectTree")
            .size_full()
            .bg(theme.muted)
            .border_r_1()
            .border_color(theme.border)
            .on_action(cx.listener(|this, action: &TreeAction, window, cx| {
                // Any action closes the row menu.
                this.open_menu_node = None;
                match action.clone() {
                    TreeAction::AddRequest(folder_id) => {
                        this.add_request(folder_id, Protocol::Http, window, cx)
                    }
                    TreeAction::AddFolder(folder_id) => this.add_folder(folder_id, window, cx),
                    TreeAction::DuplicateRequest(id) => this.duplicate_request(id, cx),
                    // The "..." menu items + Delete/Rename all go through here.
                    other => {
                        let node = node_of(&other);
                        this.run_menu_action(other, &node, window, cx);
                    }
                }
                cx.notify();
            }))
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .items_center()
                    .gap_1()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.search)
                                .small()
                                .prefix(IconName::Search)
                                .appearance(false),
                        ),
                    )
                    // Single toggle: expand-all when collapsed, collapse-all
                    // when expanded. Icon reflects the current state.
                    .child({
                        let all_expanded = self.collapsed_folders.is_empty();
                        Button::new("toggle-folders")
                            .ghost()
                            .small()
                            .icon(if all_expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .tooltip(if all_expanded {
                                "全部收起"
                            } else {
                                "全部展开"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.collapsed_folders.is_empty() {
                                    this.collapse_all(cx);
                                } else {
                                    this.expand_all(cx);
                                }
                            }))
                    })
                    .child({
                        let view = cx.entity();
                        let open = self.new_popover_open;
                        // TopCenter: the popover's top-center anchors at the
                        // button's top-center and extends DOWNWARD, so it
                        // appears centered under the "+" button.
                        Popover::new("new-request-popover")
                            .anchor(gpui::Anchor::TopCenter)
                            .open(open)
                            .on_open_change(cx.listener(|this, open, _, cx| {
                                this.new_popover_open = *open;
                                cx.notify();
                            }))
                            .trigger(
                                Button::new("new-request")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Plus)
                                    .tooltip("新建"),
                            )
                            .p(px(6.))
                            .max_w(px(460.))
                            .child(new_request_picker(self.state.clone(), view, None))
                    }),
            )
            .child(
                div()
                    .id("tree-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        tree(&self.tree, move |ix, entry, _selected, _window, cx| {
                            let item = entry.item();
                            // Snapshot drag/drop state + theme for this row.
                            let (self_ref_drag, self_ref_drop, theme) = {
                                let p = view.read(cx);
                                let theme = cx.theme().clone();
                                (p.dragging_node.clone(), p.drop_target.clone(), theme)
                            };
                            // IMPORTANT: derive is_folder from the node-id prefix
                            // (folder:...), NOT from entry.is_folder(). The tree
                            // framework defines is_folder() as children.len() > 0, so
                            // an EMPTY folder would be treated as a leaf request,
                            // breaking its selection highlight, chevron, icon, and
                            // context-menu routing. The prefix is the only reliable
                            // signal.
                            let request_id = parse_request_id(&item.id).map(|s| s.to_string());
                            let folder_id = parse_folder_id(&item.id).map(|s| s.to_string());
                            let is_folder = folder_id.is_some();
                            let icon_name = if is_folder {
                                if entry.is_expanded() {
                                    IconName::FolderOpen
                                } else {
                                    IconName::Folder
                                }
                            } else {
                                IconName::File
                            };

                            // For requests, look up the method to render a
                            // colored badge (postman-style). For folders show
                            // the folder icon instead.
                            let method = if !is_folder {
                                request_id.as_ref().and_then(|id| {
                                    view.read(cx)
                                        .state
                                        .read(cx)
                                        .active_project()
                                        .and_then(|p| p.find_request(id).map(|(_, r)| r.method))
                                })
                            } else {
                                None
                            };

                            // Is this row the currently-selected request or folder?
                            // Both ids must be Some and equal.
                            let (selected_request, selected_folder) = {
                                let s = view.read(cx).state.read(cx);
                                (s.selected_request.clone(), s.selected_folder.clone())
                            };
                            let is_selected = match (&request_id, &selected_request) {
                                (Some(id), Some(sel)) => id == sel,
                                _ => {
                                    // Folder selection: a folder row is selected
                                    // when its folder id matches.
                                    is_folder && folder_id.as_deref() == selected_folder.as_deref()
                                }
                            };

                            let node_for_more = item.id.to_string();
                            let view_for_more = view.clone();
                            // Tree entity + folder node for the chevron toggle.
                            let tree_for_chevron = view.read(cx).tree.clone();
                            let folder_node_id = if is_folder {
                                item.id.to_string()
                            } else {
                                String::new()
                            };
                            let is_expanded = entry.is_expanded();
                            let view_for_add = view.clone();
                            let badge_color =
                                method.map(|m| crate::ui::method_colors::badge_color(m, cx));
                            let badge_label = method.map(|m| m.badge_label().to_string());

                            // Each row gets a unique group so the "..." button can
                            // reveal only when this row is hovered.
                            let group = format!("tree-row-{ix}");

                            // Selection colors: the ListItem already paints a
                            // `theme.accent` (neutral-800 gray) base on selected
                            // rows when `active_highlight = false`, so we lay a
                            // strongly-opaque blue over the top to overpower it.
                            // 70% opacity is enough to make the row vividly blue
                            // rather than gray-blue.
                            let selection_bg = theme.selection.opacity(0.70);
                            let selection_icon =
                                theme.selection.blend(theme.foreground.opacity(0.4));

                            let list_item = ListItem::new(ix)
                                .w_full()
                                // .selected(true) marks the row active so the
                                // framework's internal hover (list_hover) is
                                // suppressed — the selection highlight therefore
                                // stays visible while hovering. With the global
                                // `list.active_highlight = false` (set in main.rs),
                                // the framework adds NO border overlay.
                                .selected(is_selected)
                                .rounded(theme.radius)
                                .overflow_hidden()
                                .px_2()
                                .pl(px(16.) * entry.depth() + px(8.))
                                // Selection highlight: a solid rounded blue
                                // background filling the whole row (no left-edge
                                // bar, which kept looking like a text caret even
                                // in color). 70% opacity overpowers the ListItem's
                                // built-in neutral accent base so the row reads
                                // as vividly blue on near-black sidebars.
                                .when(is_selected, |item| {
                                    item.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .right_0()
                                            .bottom_0()
                                            .bg(selection_bg),
                                    )
                                    .text_color(theme.foreground)
                                })
                                .child(
                                    h_flex()
                                        .group(group.clone())
                                        .gap_1p5()
                                        .items_center()
                                        .w_full()
                                        // Chevron (folders only): click toggles expand.
                                        .when(is_folder, |this| {
                                            let _tree = tree_for_chevron.clone();
                                            let fid = folder_node_id.clone();
                                            let view = view_for_more.clone();
                                            let chevron = if is_expanded {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            };
                                            this.child(
                                                div()
                                                    .id(("tree-chevron", ix))
                                                    .w(px(16.))
                                                    .h(px(16.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_color(if is_selected {
                                                        theme.foreground
                                                    } else {
                                                        theme.muted_foreground
                                                    })
                                                    .hover(|t| t.text_color(theme.foreground))
                                                    .child(chevron)
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, _window, cx: &mut App| {
                                                            cx.stop_propagation();
                                                            let _ = view.update(cx, |this, cx| {
                                                                // Toggle the collapsed-folders set; the tree
                                                                // rebuild is driven by this set.
                                                                if this
                                                                    .collapsed_folders
                                                                    .contains(&fid)
                                                                {
                                                                    this.collapsed_folders
                                                                        .remove(&fid);
                                                                } else {
                                                                    this.collapsed_folders
                                                                        .insert(fid.clone());
                                                                }
                                                                this.rebuild_tree(cx);
                                                                cx.notify();
                                                            });
                                                        },
                                                    ),
                                            )
                                        })
                                        // Spacer for request rows (aligns with chevron width).
                                        .when(!is_folder, |this| this.child(div().w(px(16.))))
                                        .when_some(method, |this, _| {
                                            // Colored method badge (right-aligned, fixed width).
                                            this.child(
                                                div()
                                                    .w(px(34.))
                                                    .text_size(px(10.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(
                                                        badge_color
                                                            .unwrap_or(theme.muted_foreground),
                                                    )
                                                    .child(badge_label.unwrap_or_default()),
                                            )
                                        })
                                        .when(is_folder, |this| {
                                            this.child(
                                                div()
                                                    .text_color(if is_selected {
                                                        selection_icon
                                                    } else {
                                                        theme.muted_foreground
                                                    })
                                                    .child(icon_name),
                                            )
                                        })
                                        .child(item.label.clone())
                                        .child(div().flex_1())
                                        // "+" add button (folders only): opens the protocol
                                        // picker popover; the chosen protocol creates a request
                                        // under this folder.
                                        .when_some(folder_id.clone(), |_this, fid| {
                                            let view = view_for_add.clone();
                                            let fid = fid.clone();
                                            _this.child(
                                                div()
                                                    .id(("tree-add", ix))
                                                    .px_1()
                                                    .opacity(0.0)
                                                    .text_color(theme.muted_foreground)
                                                    .group_hover(group.clone(), |this| {
                                                        this.opacity(1.0)
                                                            .text_color(theme.foreground)
                                                    })
                                                    .hover(|this| this.bg(theme.border))
                                                    .child(IconName::Plus)
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, _window, cx: &mut App| {
                                                            cx.stop_propagation();
                                                            let _ = view.update(cx, |this, cx| {
                                                                this.pending_folder_add =
                                                                    Some(fid.clone());
                                                                this.folder_add_popover_open = true;
                                                                cx.notify();
                                                            });
                                                        },
                                                    ),
                                            )
                                        })
                                        // "..." actions button: hidden until the row
                                        // is hovered (group_hover), opens a menu on click.
                                        .child(
                                            div()
                                                .id(("tree-more", ix))
                                                .px_1()
                                                .opacity(0.0)
                                                .text_color(theme.muted_foreground)
                                                .group_hover(group.clone(), |this| {
                                                    this.opacity(1.0).text_color(theme.foreground)
                                                })
                                                .hover(|this| this.bg(theme.border))
                                                .child(IconName::EllipsisVertical)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    move |_event, _window, cx: &mut App| {
                                                        cx.stop_propagation();
                                                        let _ =
                                                            view_for_more.update(cx, |this, cx| {
                                                                if this.open_menu_node.as_deref()
                                                                    == Some(&node_for_more)
                                                                {
                                                                    this.open_menu_node = None;
                                                                    this.menu_click_y = None;
                                                                } else {
                                                                    this.open_menu_node =
                                                                        Some(node_for_more.clone());
                                                                    // Approximate Y from the window cursor.
                                                                    this.menu_click_y = None;
                                                                }
                                                                cx.notify();
                                                            });
                                                    },
                                                ),
                                        ),
                                );

                            // --- Drag & drop wiring (attached directly to the
                            // ListItem, which forwards to its base div). ---
                            let drag_node = item.id.to_string();
                            let is_drop_target =
                                self_ref_drop.as_deref() == Some(drag_node.as_str());
                            let is_dragging = self_ref_drag.as_deref() == Some(drag_node.as_str());
                            let view_drag = view.clone();
                            let view_hover = view.clone();
                            let view_drop = view.clone();
                            let drag_value = TreeDrag {
                                node: drag_node.clone(),
                                label: item.label.to_string(),
                            };
                            let drag_node_for_hover = drag_node.clone();
                            let drag_node_for_drop = drag_node.clone();

                            list_item
                                .when(is_dragging, |row| row.opacity(0.5))
                                .when(is_drop_target, |row| {
                                    row.border_l_2().border_color(theme.selection)
                                })
                                // Make the row a drag source (carries TreeDrag).
                                .on_drag(drag_value, move |drag: &TreeDrag, _pos, _window, cx| {
                                    let _ = view_drag.update(cx, |this, cx| {
                                        this.dragging_node = Some(drag.node.clone());
                                        cx.notify();
                                    });
                                    // Return a drag-preview entity.
                                    cx.new(|_| drag.clone())
                                })
                                // Highlight while dragging over this row.
                                .on_drag_hover(move |hovering: &bool, _window, cx: &mut App| {
                                    let _ = view_hover.update(cx, |this, cx| {
                                        if this.dragging_node.is_some() && *hovering {
                                            this.drop_target = Some(drag_node_for_hover.clone());
                                            cx.notify();
                                        } else if !*hovering
                                            && this.drop_target.as_deref()
                                                == Some(drag_node_for_hover.as_str())
                                        {
                                            this.drop_target = None;
                                            cx.notify();
                                        }
                                    });
                                })
                                // Drop handler: move source relative to target.
                                .on_drop(move |drag: &TreeDrag, _window, cx: &mut App| {
                                    let _ = view_drop.update(cx, |this, cx| {
                                        this.apply_drop(&drag.node, &drag_node_for_drop, cx);
                                    });
                                })
                        })
                        .context_menu(|_ix, entry, menu, _window, _cx| {
                            let node: String = entry.item().id.to_string();
                            let is_folder = parse_folder_id(&node).is_some();
                            let request_id = parse_request_id(&node).map(|s| s.to_string());
                            let folder_id = parse_folder_id(&node).map(|s| s.to_string());
                            menu.when(is_folder, |m| {
                                m.menu_with_enable(
                                    "New Request",
                                    Box::new(TreeAction::AddRequest(folder_id.clone())),
                                    true,
                                )
                                .menu_with_enable(
                                    "New Folder",
                                    Box::new(TreeAction::AddFolder(folder_id.clone())),
                                    true,
                                )
                            })
                            .when_some(request_id.clone(), |m, id| {
                                m.menu_with_enable(
                                    "Duplicate",
                                    Box::new(TreeAction::DuplicateRequest(id)),
                                    true,
                                )
                            })
                            .separator()
                            .menu_with_enable(
                                "Rename",
                                Box::new(TreeAction::Rename(node.clone())),
                                true,
                            )
                            .menu_with_enable(
                                "Delete",
                                Box::new(TreeAction::Delete(node.clone())),
                                true,
                            )
                        })
                        .p_1(),
                    ),
            )
            // Floating "..." action menu overlay (shown when a row's menu is open).
            .when_some(self.open_menu_node.clone(), |this, node| {
                this.child(render_action_menu(
                    node,
                    self.menu_click_y,
                    cx.entity(),
                    _window,
                    cx,
                ))
            })
            // Folder "+" add overlay (centered picker, shown when folder_add_popover_open).
            .when(self.folder_add_popover_open, |this| {
                let panel = cx.entity();
                let state = self.state.clone();
                let folder_id = self.pending_folder_add.clone();
                this.child(render_folder_add_overlay(panel, state, folder_id, cx))
            })
    }
}

/// A centered overlay showing the new-request picker for a folder "+" add.
/// Dismissed on outside-click or after a protocol is chosen.
fn render_folder_add_overlay(
    panel: gpui::Entity<ProjectTreePanel>,
    state: gpui::Entity<AppState>,
    folder_id: Option<String>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let panel_close = panel.clone();
    deferred(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(gpui::black().opacity(0.3))
            .on_mouse_down(MouseButton::Left, move |_, _window, cx: &mut App| {
                let _ = panel_close.update(cx, |this, cx| {
                    this.folder_add_popover_open = false;
                    this.pending_folder_add = None;
                    cx.notify();
                });
            })
            .child(
                v_flex()
                    .absolute()
                    .top(px(120.))
                    .left(px(0.))
                    .right(px(0.))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w(px(380.))
                            .max_w(px(460.))
                            .p(px(8.))
                            .rounded(px(8.))
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .shadow_md()
                            // Stop propagation so clicking the card doesn't close.
                            .on_mouse_down(MouseButton::Left, |_, _window, cx: &mut App| {
                                cx.stop_propagation();
                            })
                            .child(new_request_picker(state, panel, folder_id)),
                    ),
            ),
    )
}

/// Render the floating action menu for a tree node. A full-pane backdrop closes
/// the menu on outside click; the menu lists the row actions. The menu opens
/// BELOW the clicked row if there's room, otherwise ABOVE it.
fn render_action_menu(
    node: String,
    click_y: Option<f32>,
    view: gpui::Entity<ProjectTreePanel>,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let is_request = parse_request_id(&node).is_some();
    let node_for_item = node.clone();
    let view_close = view.clone();

    // Estimated menu height (9 items ~26px + padding) for space checking.
    let menu_height = 260.0_f32;
    let viewport_h = window.viewport_size().height.as_f32();
    // Default to opening at the top if we don't know the click position.
    let y = click_y.unwrap_or(40.0);
    // If the menu would overflow the bottom of the window, open it ABOVE the
    // row (anchored by its bottom edge to the row's y) instead of below.
    let open_below = y + menu_height < viewport_h;

    // A deferred backdrop that captures outside clicks to close the menu.
    deferred(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(MouseButton::Left, move |_, _window, cx: &mut App| {
                let _ = view_close.update(cx, |this, cx| {
                    this.open_menu_node = None;
                    this.menu_click_y = None;
                    cx.notify();
                });
            })
            .child(
                // The menu card. Horizontally flush-right (next to the "..."
                // button); vertically either below the row or above it.
                div()
                    .absolute()
                    .right(px(8.))
                    .w(px(190.))
                    .py_1()
                    .rounded(px(6.))
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_md()
                    .when(open_below, |card| {
                        // Open below: top edge at the clicked row's y.
                        card.top(px(y + 18.0))
                    })
                    .when(!open_below, |card| {
                        // Open above: bottom edge at the clicked row's y.
                        card.bottom(px(viewport_h - y + 4.0))
                    })
                    .children(action_menu_items(
                        &node_for_item,
                        is_request,
                        view.clone(),
                        &theme,
                    )),
            ),
    )
}

/// A menu item that runs the given action on the owning panel when clicked.
fn menu_item(
    label: &'static str,
    icon: IconName,
    action: TreeAction,
    node: String,
    view: gpui::Entity<ProjectTreePanel>,
    theme: &gpui_component::Theme,
) -> gpui::AnyElement {
    div()
        .id(format!("menu-item-{label}"))
        .w_full()
        .px_3()
        .py(px(6.))
        .flex()
        .flex_row()
        .gap_2()
        .items_center()
        .text_sm()
        .text_color(if matches!(action, TreeAction::Delete(_)) {
            theme.danger
        } else {
            theme.foreground
        })
        .hover(|this| this.bg(theme.accent.opacity(0.5)))
        .child(div().text_color(theme.muted_foreground).child(icon))
        .child(label)
        // Use on_mouse_down (not on_click) so the action fires on press, and
        // stop_propagation so the backdrop's on_mouse_down (which would close
        // the menu and trigger a re-render that destroys this element) doesn't
        // race ahead and eat the click. With on_click the element must survive
        // from mouse-down to mouse-up; the backdrop's close→re-render breaks
        // that invariant, which is why the first click did nothing.
        .on_mouse_down(MouseButton::Left, move |_, window, cx: &mut App| {
            cx.stop_propagation();
            let action = action.clone();
            let node = node.clone();
            let _ = view.update(cx, |this, cx| {
                this.open_menu_node = None;
                this.run_menu_action(action, &node, window, cx);
                cx.notify();
            });
        })
        .into_any_element()
}

/// Build the list of menu item rows for the action menu.
fn action_menu_items(
    node: &str,
    is_request: bool,
    view: gpui::Entity<ProjectTreePanel>,
    theme: &gpui_component::Theme,
) -> Vec<gpui::AnyElement> {
    let mut items: Vec<gpui::AnyElement> = Vec::new();
    if is_request {
        items.push(menu_item(
            "分享",
            IconName::ExternalLink,
            TreeAction::Share(node.into()),
            node.into(),
            view.clone(),
            theme,
        ));
        items.push(menu_item(
            "复制为 cURL",
            IconName::Copy,
            TreeAction::CopyAsCurl(node.into()),
            node.into(),
            view.clone(),
            theme,
        ));
        items.push(menu_item(
            "复制到其它分支",
            IconName::Copy,
            TreeAction::CopyToBranch(node.into()),
            node.into(),
            view.clone(),
            theme,
        ));
        items.push(menu_item(
            "移动至",
            IconName::ArrowRight,
            TreeAction::MoveTo(node.into()),
            node.into(),
            view.clone(),
            theme,
        ));
        items.push(separator(theme));
        items.push(menu_item(
            "复制",
            IconName::Copy,
            TreeAction::Copy(node.into()),
            node.into(),
            view.clone(),
            theme,
        ));
        items.push(menu_item(
            "克隆",
            IconName::Redo,
            TreeAction::Clone(node.into()),
            node.into(),
            view.clone(),
            theme,
        ));
        items.push(separator(theme));
        items.push(menu_item(
            "删除",
            IconName::Delete,
            TreeAction::Delete(node.into()),
            node.into(),
            view,
            theme,
        ));
    } else {
        items.push(menu_item(
            "重命名",
            IconName::File,
            TreeAction::Rename(node.into()),
            node.into(),
            view.clone(),
            theme,
        ));
        items.push(menu_item(
            "删除",
            IconName::Delete,
            TreeAction::Delete(node.into()),
            node.into(),
            view,
            theme,
        ));
    }
    items
}

fn separator(theme: &gpui_component::Theme) -> gpui::AnyElement {
    div()
        .h(px(1.))
        .my(px(2.))
        .mx_2()
        .bg(theme.border)
        .into_any_element()
}

/// A colored icon tile for a new-request card.
fn proto_icon(bg: gpui::Hsla, label: &'static str) -> impl IntoElement {
    div()
        .size_5()
        .rounded(px(4.))
        .bg(bg)
        .text_color(gpui::white())
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(7.))
        .font_weight(FontWeight::BOLD)
        .child(label.to_string())
}

/// One selectable card in the new-request picker. On click it creates a
/// request of the given protocol and closes the popover.
fn picker_card(
    id: &'static str,
    icon_bg: gpui::Hsla,
    icon_label: &'static str,
    title: &'static str,
    beta: bool,
    state: gpui::Entity<AppState>,
    panel: gpui::Entity<ProjectTreePanel>,
    folder_id: Option<String>,
    full_width: bool,
) -> impl IntoElement {
    let state_for_click = state.clone();
    let panel_for_close = panel.clone();
    // Determine the protocol from the id.
    let protocol = match id {
        "http" => Protocol::Http,
        "sse" => Protocol::Sse,
        "websocket" => Protocol::WebSocket,
        "socketio" => Protocol::SocketIo,
        "grpc" => Protocol::Grpc,
        "graphql" => Protocol::Graphql,
        "tcp" => Protocol::Tcp,
        "markdown" => Protocol::Markdown,
        "directory" => Protocol::Directory,
        _ => Protocol::Http,
    };
    div()
        .id("picker-card-".to_string() + id)
        .when(full_width, |this| this.w_full())
        .flex_1()
        .min_w_0()
        .items_center()
        .gap_1p5()
        .px(px(7.))
        .py(px(5.))
        .rounded(px(5.))
        .bg(gpui::hsla(0., 0., 0.12, 1.0))
        .hover(|this| this.bg(gpui::hsla(0., 0., 0.20, 1.0)))
        .child(proto_icon(icon_bg, icon_label))
        .child(div().text_size(px(12.)).child(title.to_string()))
        .when(beta, |this| {
            this.child(
                div()
                    .px(px(4.))
                    .py(px(0.))
                    .rounded(px(3.))
                    .bg(gpui::hsla(0., 0., 0.25, 1.0))
                    .text_size(px(8.))
                    .text_color(gpui::hsla(0., 0., 0.75, 1.0))
                    .child("Beta"),
            )
        })
        .on_click(move |_, _window, cx: &mut App| {
            let proto = protocol;
            // Build the request directly against the shared state.
            let name = match proto {
                Protocol::Http => "New Request",
                Protocol::Sse => "New SSE",
                Protocol::WebSocket => "New WebSocket",
                Protocol::SocketIo => "New Socket.IO",
                Protocol::Grpc => "New gRPC",
                Protocol::Graphql => "New GraphQL",
                Protocol::Tcp => "New TCP",
                Protocol::Markdown => "New Markdown",
                Protocol::Directory => "New Folder",
            };
            if matches!(proto, Protocol::Directory) {
                // Open a name-input dialog instead of hardcoding "New Folder".
                let _ = panel_for_close.update(cx, |this, cx| {
                    this.add_folder(folder_id.clone(), _window, cx);
                    cx.notify();
                });
            } else {
                let mut req =
                    crate::state::models::ApiRequest::new(name, RequestMethod::Get, "{{baseUrl}}");
                req.protocol = proto;
                let req_id = req.id.clone();
                let fid = folder_id.clone();
                let _ = state_for_click.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        match &fid {
                            Some(fid) => {
                                if let Some(f) = find_folder_mut(&mut project.folders, fid) {
                                    f.requests.push(req);
                                }
                            }
                            None => project.requests.push(req),
                        }
                    }
                    s.notify_workspace(cx);
                    s.open_or_focus_tab(&req_id, cx);
                });
            }
            // Close the popover after creating the request.
            let _ = panel_for_close.update(cx, |this, cx| {
                this.new_popover_open = false;
                this.folder_add_popover_open = false;
                cx.notify();
            });
        })
}

/// An import-source card. Clicking shows a "coming soon" note and closes the
/// dialog (import flows are wired separately via the top toolbar).
fn import_card(
    id: &'static str,
    icon_bg: gpui::Hsla,
    icon_label: &'static str,
    title: &'static str,
    full_width: bool,
) -> impl IntoElement {
    div()
        .id("import-card-".to_string() + id)
        .when(full_width, |this| this.w_full())
        .flex_1()
        .min_w_0()
        .items_center()
        .gap_1p5()
        .px(px(7.))
        .py(px(5.))
        .rounded(px(5.))
        .bg(gpui::hsla(0., 0., 0.12, 1.0))
        .hover(|this| this.bg(gpui::hsla(0., 0., 0.20, 1.0)))
        .child(proto_icon(icon_bg, icon_label))
        .child(div().text_size(px(12.)).child(title.to_string()))
        .on_click(move |_, window, cx: &mut App| {
            log::info!("导入功能「{title}」开发中");
            window.close_dialog(cx);
        })
}

/// Build the new-request picker body (新建 + 导入 sections).
fn new_request_picker(
    state: gpui::Entity<AppState>,
    panel: gpui::Entity<ProjectTreePanel>,
    folder_id: Option<String>,
) -> impl IntoElement {
    let card_row = |left: gpui::AnyElement, right: gpui::AnyElement| {
        h_flex().w_full().gap_1().child(left).child(right)
    };

    v_flex()
        .gap_1()
        .w(px(380.))
        // Section: 新建
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::hsla(0., 0., 0.6, 1.0))
                .child("新建"),
        )
        .child(
            v_flex()
                .gap_1()
                .child(card_row(
                    picker_card(
                        "http",
                        gpui::hsla(0.0, 0.72, 0.52, 1.0),
                        "HTTP",
                        "HTTP",
                        false,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        false,
                    )
                    .into_any_element(),
                    picker_card(
                        "sse",
                        gpui::hsla(0.58, 0.7, 0.55, 1.0),
                        "SSE",
                        "Event Stream",
                        false,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        false,
                    )
                    .into_any_element(),
                ))
                .child(card_row(
                    picker_card(
                        "websocket",
                        gpui::hsla(0.11, 0.78, 0.5, 1.0),
                        "ws",
                        "WebSocket",
                        false,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        false,
                    )
                    .into_any_element(),
                    picker_card(
                        "socketio",
                        gpui::hsla(0.45, 0.6, 0.45, 1.0),
                        "SIO",
                        "Socket.IO",
                        false,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        false,
                    )
                    .into_any_element(),
                ))
                .child(card_row(
                    picker_card(
                        "grpc",
                        gpui::hsla(0.07, 0.8, 0.5, 1.0),
                        "gRPC",
                        "gRPC",
                        true,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        false,
                    )
                    .into_any_element(),
                    picker_card(
                        "graphql",
                        gpui::hsla(0.83, 0.6, 0.55, 1.0),
                        "GQL",
                        "GraphQL",
                        true,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        false,
                    )
                    .into_any_element(),
                ))
                .child(card_row(
                    picker_card(
                        "tcp",
                        gpui::hsla(0.50, 0.6, 0.45, 1.0),
                        "TCP",
                        "TCP 客户端",
                        false,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        false,
                    )
                    .into_any_element(),
                    picker_card(
                        "markdown",
                        gpui::hsla(0.60, 0.7, 0.5, 1.0),
                        "M",
                        "Markdown",
                        false,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        false,
                    )
                    .into_any_element(),
                ))
                .child(
                    picker_card(
                        "directory",
                        gpui::hsla(0.75, 0.5, 0.6, 1.0),
                        "DIR",
                        "目录",
                        false,
                        state.clone(),
                        panel.clone(),
                        folder_id.clone(),
                        true,
                    )
                    .into_any_element(),
                ),
        )
        // Section: 导入 (import sources — not protocol requests).
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::hsla(0., 0., 0.6, 1.0))
                .child("导入"),
        )
        .child(
            v_flex()
                .gap_1()
                .child(card_row(
                    import_card(
                        "ai",
                        gpui::hsla(0.83, 0.6, 0.5, 1.0),
                        "AI",
                        "AI 智能提取 API 文档",
                        false,
                    )
                    .into_any_element(),
                    import_card(
                        "curl",
                        gpui::hsla(0.07, 0.78, 0.5, 1.0),
                        "{}",
                        "cURL 导入",
                        false,
                    )
                    .into_any_element(),
                ))
                .child(card_row(
                    import_card(
                        "paste",
                        gpui::hsla(0.07, 0.78, 0.5, 1.0),
                        "PST",
                        "粘贴接口/文本",
                        false,
                    )
                    .into_any_element(),
                    import_card(
                        "idea",
                        gpui::hsla(0., 0., 0.1, 1.0),
                        "IJ",
                        "IDEA 上传",
                        false,
                    )
                    .into_any_element(),
                ))
                .child(
                    import_card(
                        "file",
                        gpui::hsla(0.07, 0.78, 0.5, 1.0),
                        "↓",
                        "其他文件导入",
                        true,
                    )
                    .into_any_element(),
                ),
        )
}

/// Actions emitted by the tree context menu. They carry payloads (folder id).
/// `#[action(no_json)]` skips serde since these are dispatched only from menus.
#[derive(Clone, PartialEq, Eq, gpui::Action)]
#[action(namespace = verve, no_json)]
pub enum TreeAction {
    AddRequest(Option<String>),
    AddFolder(Option<String>),
    DuplicateRequest(String),
    Rename(String),
    Delete(String),
    // Row "..." action menu items.
    Share(String),
    CopyAsCurl(String),
    CopyToBranch(String),
    MoveTo(String),
    Copy(String),
    Clone(String),
}

impl ProjectTreePanel {
    /// Open a rename dialog for the given tree node (`request:<id>` or
    /// `folder:<id>`), pre-filled with the current name.
    ///
    /// Before opening the dialog, this syncs the tree's `selected_ix` to the
    /// renamed node and updates AppState's selection so the highlight follows
    /// the right-clicked node (right-click only sets `right_clicked_ix`, which
    /// is invisible to the render callback's `selected` flag).
    pub fn start_rename(&mut self, node: String, window: &mut Window, cx: &mut Context<Self>) {
        // 1. Sync AppState selection to the renamed node (so the detail panel
        //    and highlight both follow).
        if let Some(req_id) = parse_request_id(&node) {
            let req_id = req_id.to_string();
            if self.state.read(cx).selected_request.as_deref() != Some(&req_id) {
                self.on_select_request(req_id, cx);
            }
        } else if let Some(fid) = parse_folder_id(&node) {
            let fid = fid.to_string();
            if self.state.read(cx).selected_folder.as_deref() != Some(&fid) {
                self.on_select_folder(fid, cx);
            }
        }
        // 2. Sync the tree framework's selected_ix to the renamed node so the
        //    row highlight follows. set_selected_item() also expands collapsed
        //    ancestors so the node is visible.
        self.tree.update(cx, |t, cx| {
            // Build a minimal TreeItem to match by id.
            let dummy = TreeItem::new(node.as_str(), "");
            t.set_selected_item(Some(&dummy), cx);
        });

        // Look up the current name and pre-fill the rename Input.
        let current_name = self.current_name_for_node(&node, cx).unwrap_or_default();
        self.rename_input
            .update(cx, |s, cx| s.set_value(current_name, window, cx));
        let project = self.state.clone();
        let rename_input = self.rename_input.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let node = node.clone();
            let project = project.clone();
            let input_for_content = rename_input.clone();
            let input_for_ok = rename_input.clone();
            dialog
                .title("重命名")
                .w(px(420.))
                .content(move |content, _, _| {
                    let input = input_for_content.clone();
                    content.child(
                        v_flex()
                            .p_4()
                            .gap_2()
                            .child("请输入新名称：")
                            .child(Input::new(&input)),
                    )
                })
                .on_ok(move |_, _window, cx| {
                    let new_name = input_for_ok.read(cx).value().to_string();
                    if !new_name.trim().is_empty() {
                        let _ = project.update(cx, |s, cx| {
                            if let Some(project) = s.active_project_mut() {
                                apply_rename(project, &node, &new_name);
                                s.notify_workspace(cx);
                            }
                        });
                    }
                    true
                })
        });
    }

    /// Resolve the current display name for a tree node id.
    fn current_name_for_node(&self, node: &str, cx: &mut Context<Self>) -> Option<String> {
        let project = self.state.read(cx).active_project()?;
        if let Some(id) = node.strip_prefix("request:") {
            project.find_request(id).map(|(_, r)| r.name.clone())
        } else if let Some(id) = node.strip_prefix("folder:") {
            find_folder(&project.folders, id).map(|f| f.name.clone())
        } else {
            None
        }
    }

    /// Add a new request under the active project (root or given folder id)
    /// with the given protocol. Directory protocol creates a folder instead.
    pub fn add_request(
        &mut self,
        folder_id: Option<String>,
        protocol: Protocol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 「目录」 creates a folder, not a request.
        if matches!(protocol, Protocol::Directory) {
            self.add_folder(folder_id, window, cx);
            return;
        }
        let name = match protocol {
            Protocol::Http => "New Request",
            Protocol::Sse => "New SSE",
            Protocol::WebSocket => "New WebSocket",
            Protocol::SocketIo => "New Socket.IO",
            Protocol::Grpc => "New gRPC",
            Protocol::Graphql => "New GraphQL",
            Protocol::Tcp => "New TCP",
            Protocol::Markdown => "New Markdown",
            Protocol::Directory => "New Folder",
        };
        let mut req =
            crate::state::models::ApiRequest::new(name, RequestMethod::Get, "{{baseUrl}}");
        req.protocol = protocol;
        let req_id = req.id.clone();
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                match &folder_id {
                    Some(fid) => {
                        if let Some(f) = find_folder_mut(&mut project.folders, fid) {
                            f.requests.push(req);
                        }
                    }
                    None => project.requests.push(req),
                }
            }
            s.notify_workspace(cx);
            s.open_or_focus_tab(&req_id, cx);
        });
    }

    /// Locate (scroll to + reveal) the currently-selected request in the tree.
    pub fn locate_active(&mut self, cx: &mut Context<Self>) {
        let sel = match self.state.read(cx).selected_request.clone() {
            Some(id) => id,
            None => return,
        };
        let node = request_node(&sel);
        // Clear collapsed state for ancestors so the target is revealed.
        // (Simple approach: expand all folders, then scroll.)
        self.collapsed_folders.clear();
        self.rebuild_tree(cx);
        self.tree.update(cx, |t, cx| {
            t.ensure_visible(&node.clone().into(), cx);
        });
        cx.notify();
    }

    /// Expand all folders in the tree.
    pub fn expand_all(&mut self, cx: &mut Context<Self>) {
        self.collapsed_folders.clear();
        self.rebuild_tree(cx);
        cx.notify();
    }

    /// Collapse all folders in the tree.
    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        // Collect all folder node-ids from the project.
        if let Some(project) = self.state.read(cx).active_project() {
            let mut ids = Vec::new();
            collect_folder_ids(&project.folders, &mut ids);
            self.collapsed_folders = ids.into_iter().collect();
        }
        self.rebuild_tree(cx);
        cx.notify();
    }

    pub fn add_folder(
        &mut self,
        folder_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_input = self.rename_input.clone();
        name_input.update(cx, |s, cx| s.set_value("New Folder", window, cx));
        let project = self.state.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let folder_id = folder_id.clone();
            let project = project.clone();
            let name_for_content = name_input.clone();
            let name_for_ok = name_input.clone();
            dialog
                .title("新建目录")
                .w(px(420.))
                .content(move |content, _, _| {
                    let name_input = name_for_content.clone();
                    content.child(
                        v_flex()
                            .p_4()
                            .gap_2()
                            .child("请输入目录名称：")
                            .child(Input::new(&name_input)),
                    )
                })
                .on_ok(move |_, _window, cx| {
                    let name = name_for_ok.read(cx).value().to_string();
                    let name = if name.trim().is_empty() {
                        "New Folder".to_string()
                    } else {
                        name.trim().to_string()
                    };
                    let folder = crate::state::models::Folder::new(&name);
                    let _ = project.update(cx, |s, cx| {
                        if let Some(project) = s.active_project_mut() {
                            match &folder_id {
                                Some(fid) => {
                                    if let Some(f) = find_folder_mut(&mut project.folders, fid) {
                                        f.folders.push(folder);
                                    }
                                }
                                None => project.folders.push(folder),
                            }
                        }
                        s.notify_workspace(cx);
                    });
                    true
                })
        });
    }

    pub fn delete_node(&mut self, node: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let mut needs_emit = false;
            if let Some(project) = s.active_project_mut() {
                if let Some(id) = node.strip_prefix("request:") {
                    remove_request(&mut project.folders, &mut project.requests, id);
                    // Remove the deleted request from open tabs and close the tab if active.
                    if s.open_request_ids.iter().any(|x| x == id) {
                        s.close_tab(id, cx); // close_tab emits SelectionChanged itself
                    } else if s.selected_request.as_deref() == Some(id) {
                        s.selected_request = None;
                        needs_emit = true;
                    }
                } else if let Some(id) = node.strip_prefix("folder:") {
                    remove_folder(&mut project.folders, id);
                    if s.selected_folder.as_deref() == Some(id) {
                        s.selected_folder = None;
                        needs_emit = true;
                    }
                }
            }
            s.notify_workspace(cx);
            if needs_emit {
                cx.emit(AppEvent::SelectionChanged);
            }
        });
    }

    pub fn duplicate_request(&mut self, id: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let mut opened: Option<String> = None;
            if let Some(project) = s.active_project_mut() {
                if let Some(new_id) =
                    duplicate_request_in_place(&mut project.folders, &mut project.requests, &id)
                {
                    opened = Some(new_id);
                }
            }
            s.notify_workspace(cx);
            if let Some(new_id) = opened {
                s.open_or_focus_tab(&new_id, cx);
            }
        });
    }

    /// Handle a "..." row-menu action for a request node. Most options are
    /// stubbed with a status response (share/branch/move require more UI); the
    /// actionable ones (copy curl, copy, clone, delete) do real work.
    pub fn run_menu_action(
        &mut self,
        action: TreeAction,
        node: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Resolve request id + model snapshot for the actions that need it.
        let req_id = parse_request_id(node).map(|s| s.to_string());
        let req_snapshot = req_id.as_deref().and_then(|id| {
            self.state
                .read(cx)
                .active_project()
                .and_then(|p| p.find_request(id).map(|(_, r)| r.clone()))
        });

        match action {
            TreeAction::Share(_) => {
                // Share the selected request's documentation.
                if let Some((id, name)) =
                    req_id.and_then(|id| req_snapshot.as_ref().map(|r| (id, r.name.clone())))
                {
                    // Emit event up to VerveApp to open the share dialog.
                    cx.emit(crate::ui::project_tree_panel::TreeEvent::ShareRequest(
                        id, name,
                    ));
                }
            }
            TreeAction::CopyAsCurl(_) => {
                if let Some(r) = req_snapshot {
                    let curl = build_curl(&r);
                    self.copy_to_clipboard(curl, cx);
                    self.show_info("复制为 cURL", "已复制到剪贴板。", window, cx);
                }
            }
            TreeAction::CopyToBranch(_) => {
                self.show_info("复制到其它分支", "分支功能开发中。", window, cx);
            }
            TreeAction::MoveTo(_) => {
                self.show_info("移动至", "请拖拽或右键选择目标文件夹。", window, cx);
            }
            TreeAction::Copy(_) => {
                if let Some(id) = req_id {
                    // Copy = duplicate but keep selection on the original.
                    self.duplicate_request_keep_selection(id, cx);
                    self.show_info("复制", "已创建副本。", window, cx);
                }
            }
            TreeAction::Clone(_) => {
                if let Some(id) = req_id {
                    self.duplicate_request(id, cx);
                }
            }
            TreeAction::Delete(_) => {
                self.delete_node(node.to_string(), cx);
            }
            TreeAction::Rename(_) => {
                // Defer the rename dialog to the next event loop tick so the
                // popup menu has fully closed before we open the dialog.
                let node = node.to_string();
                cx.defer_in(window, move |this, window, cx| {
                    this.start_rename(node, window, cx);
                });
            }
            _ => {}
        }
    }

    /// Duplicate a request but leave the selection on the original (the "复制"
    /// / copy action, vs. "克隆"/clone which selects the new one).
    fn duplicate_request_keep_selection(&mut self, id: String, cx: &mut Context<Self>) {
        let selected = self.state.read(cx).selected_request.clone();
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                let _ =
                    duplicate_request_in_place(&mut project.folders, &mut project.requests, &id);
            }
            s.notify_workspace(cx);
        });
        // Restore selection.
        self.state.update(cx, |s, cx| {
            s.selected_request = selected;
            cx.emit(crate::state::AppEvent::SelectionChanged);
        });
    }

    /// Show a transient info notification (toast) that auto-hides.
    fn show_info(&self, title: &str, message: &str, window: &mut Window, cx: &mut Context<Self>) {
        log::info!("[{title}] {message}");
        window.push_notification(
            gpui_component::notification::Notification::new()
                .title(title)
                .message(message)
                .autohide(true),
            cx,
        );
    }

    /// Copy text to the system clipboard.
    fn copy_to_clipboard(&self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }
}

/// Build a `curl` command string for a request (best-effort, variables left
/// as-is since they are unresolved at copy time).
fn build_curl(r: &crate::state::models::ApiRequest) -> String {
    use crate::state::models::BodyType;
    let mut parts: Vec<String> = vec!["curl".into(), "-X".into(), r.method.to_string()];
    parts.push(format!("'{}'", r.url));

    // Track whether user already set a Content-Type so we don't override it
    // when auto-injecting one for the body.
    let mut has_content_type = false;
    for h in r.headers.iter().filter(|h| h.enabled && !h.is_empty()) {
        if h.key.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        parts.push("-H".into());
        parts.push(format!("'{}: {}'", h.key, h.value));
    }

    // Auth headers (Bearer/Basic).
    use crate::state::models::AuthType;
    match r.auth.auth_type {
        AuthType::Bearer if !r.auth.token.is_empty() => {
            parts.push("-H".into());
            parts.push(format!("'Authorization: Bearer {}'", r.auth.token));
        }
        AuthType::Basic if !r.auth.username.is_empty() => {
            parts.push("-u".into());
            parts.push(format!("'{}:{}'", r.auth.username, r.auth.password));
        }
        _ => {}
    }

    match r.body.body_type {
        BodyType::Raw if !r.body.raw.is_empty() => {
            if !has_content_type {
                let ct = r.body.raw_language.content_type();
                parts.push("-H".into());
                parts.push(format!("'Content-Type: {}'", ct));
            }
            parts.push("-d".into());
            parts.push(format!("'{}'", r.body.raw.replace('\'', "'\\''")));
        }
        BodyType::Urlencoded => {
            let kv: Vec<String> = r
                .body
                .urlencoded
                .iter()
                .filter(|kv| kv.enabled && !kv.is_empty())
                .map(|kv| format!("{}={}", kv.key, kv.value))
                .collect();
            if !kv.is_empty() {
                if !has_content_type {
                    parts.push("-H".into());
                    parts.push("'Content-Type: application/x-www-form-urlencoded'".into());
                }
                parts.push("-d".into());
                parts.push(format!("'{}'", kv.join("&")));
            }
        }
        BodyType::FormData => {
            for kv in r
                .body
                .form_data
                .iter()
                .filter(|kv| kv.enabled && !kv.is_empty())
            {
                parts.push("-F".into());
                parts.push(format!("'{}={}'", kv.key, kv.value));
            }
            // curl auto-sets multipart/form-data with boundary when using -F.
        }
        _ => {}
    }
    parts.join(" \\\n  ")
}

/// Collect all folder node-ids ("folder:<id>") recursively.
fn collect_folder_ids(folders: &[crate::state::models::Folder], out: &mut Vec<String>) {
    for f in folders {
        out.push(folder_node(&f.id));
        collect_folder_ids(&f.folders, out);
    }
}

fn find_folder_mut<'a>(
    folders: &'a mut [crate::state::models::Folder],
    id: &str,
) -> Option<&'a mut crate::state::models::Folder> {
    for f in folders.iter_mut() {
        if f.id == id {
            return Some(f);
        }
        if let Some(found) = find_folder_mut(&mut f.folders, id) {
            return Some(found);
        }
    }
    None
}

/// Immutable folder lookup by id.
fn find_folder<'a>(
    folders: &'a [crate::state::models::Folder],
    id: &str,
) -> Option<&'a crate::state::models::Folder> {
    for f in folders.iter() {
        if f.id == id {
            return Some(f);
        }
        if let Some(found) = find_folder(&f.folders, id) {
            return Some(found);
        }
    }
    None
}

/// Apply a rename to a project by tree node id (`request:<id>` / `folder:<id>`).
fn apply_rename(project: &mut crate::state::models::Project, node: &str, new_name: &str) {
    if let Some(id) = node.strip_prefix("request:") {
        if let Some((_, req)) = project.find_request_mut(id) {
            req.name = new_name.to_string();
        }
    } else if let Some(id) = node.strip_prefix("folder:") {
        if let Some(f) = find_folder_mut(&mut project.folders, id) {
            f.name = new_name.to_string();
        }
    }
}

fn remove_request(
    folders: &mut [crate::state::models::Folder],
    root: &mut Vec<crate::state::models::ApiRequest>,
    id: &str,
) {
    root.retain(|r| r.id != id);
    for f in folders.iter_mut() {
        f.requests.retain(|r| r.id != id);
        remove_request(&mut f.folders, &mut Vec::new(), id);
    }
}

fn remove_folder(folders: &mut Vec<crate::state::models::Folder>, id: &str) {
    folders.retain(|f| f.id != id);
    for f in folders.iter_mut() {
        let mut subs = std::mem::take(&mut f.folders);
        remove_folder(&mut subs, id);
        f.folders = subs;
    }
}

/// Duplicate a request in place, keeping the original and inserting the copy
/// right after it in the same folder/root. Returns the new request id if successful.
fn duplicate_request_in_place(
    folders: &mut [crate::state::models::Folder],
    root: &mut Vec<crate::state::models::ApiRequest>,
    id: &str,
) -> Option<String> {
    // Check root requests first
    if let Some(pos) = root.iter().position(|r| r.id == id) {
        let original = &root[pos];
        let mut dup = original.clone();
        dup.id = new_id();
        dup.name = format!("{} copy", original.name);
        let new_id = dup.id.clone();
        root.insert(pos + 1, dup);
        return Some(new_id);
    }

    // Check folders recursively
    fn duplicate_in_folders(
        folders: &mut [crate::state::models::Folder],
        id: &str,
    ) -> Option<String> {
        for f in folders.iter_mut() {
            if let Some(pos) = f.requests.iter().position(|r| r.id == id) {
                let original = &f.requests[pos];
                let mut dup = original.clone();
                dup.id = new_id();
                dup.name = format!("{} copy", original.name);
                let new_id = dup.id.clone();
                f.requests.insert(pos + 1, dup);
                return Some(new_id);
            }
            if let Some(found) = duplicate_in_folders(&mut f.folders, id) {
                return Some(found);
            }
        }
        None
    }

    duplicate_in_folders(folders, id)
}

impl Focusable for ProjectTreePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<TreeEvent> for ProjectTreePanel {}
impl EventEmitter<()> for ProjectTreePanel {}
