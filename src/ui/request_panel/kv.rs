//! Key-value table rendering and editing: request & folder KV tables,
//! the folder detail view (incl. the interface table), raw/visual field
//! sync, and all toggle/add/delete/commit handlers.
use super::folder_helpers::{IfaceEntry, collect_iface_entries, folder_tab_label, set_folder_base_url,
};
use super::{FolderKvSection, FolderTab, ReqTab, RequestPanel};
use crate::state::AppEvent;
use crate::state::models::*;
use crate::ui::kv_table::{self, KvRow};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::WindowExt as _;
use gpui_component::input::Input;
use gpui_component::{
    ActiveTheme, Disableable as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    popover::Popover,
    v_flex,
};
use std::sync::Arc;

impl RequestPanel {
    /// Render the kv editor for params/headers rows.
    pub(super) fn render_kv(
        &self,
        rows: &[KvRow],
        show_type: bool,
        allow_files: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let view_toggle = cx.entity();
        let view_delete = cx.entity();
        let view_add = cx.entity();
        let view_type = cx.entity();
        let view_req = cx.entity();
        let view_file = cx.entity();
        let handlers = crate::ui::kv_table::KvHandlers {
            on_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_toggle.update(cx, |this, cx| {
                    this.toggle_kv(ix, val, cx);
                });
            }),
            on_delete: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_delete.update(cx, |this, cx| {
                    this.delete_kv(ix, cx);
                });
            }),
            on_add: Arc::new(move |_window, cx: &mut App| {
                let _ = view_add.update(cx, |this, cx| {
                    this.pending_kv_add = true;
                    cx.notify();
                });
            }),
            on_type_change: Arc::new(move |ix, ft, _window, cx: &mut App| {
                let _ = view_type.update(cx, |this, cx| {
                    this.change_kv_type(ix, ft, cx);
                });
            }),
            on_required_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_req.update(cx, |this, cx| {
                    this.toggle_required(ix, val, cx);
                });
            }),
            on_file_pick: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_file.update(cx, |this, cx| {
                    this.pending_file_pick = Some(ix);
                    cx.notify();
                });
            }),
        };
        crate::ui::kv_table::KvTable::new("req-kv", rows.to_vec(), handlers)
            .show_type(show_type)
            .show_required(true)
            .show_description(true)
            .allow_files(allow_files)
            .into_any_element()
    }

    /// Render the kv editor for a folder section (params/headers/variables).
    pub(super) fn render_folder_kv(
        &self,
        section: FolderKvSection,
        show_type: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let rows = self.folder_rows(section).to_vec();
        let view_toggle = cx.entity();
        let view_delete = cx.entity();
        let view_add = cx.entity();
        let view_type = cx.entity();
        let view_req = cx.entity();
        let handlers = crate::ui::kv_table::KvHandlers {
            on_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_toggle.update(cx, |this, cx| {
                    this.folder_toggle_kv(section, ix, val, cx);
                });
            }),
            on_delete: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_delete.update(cx, |this, cx| {
                    this.folder_delete_kv(section, ix, cx);
                });
            }),
            on_add: Arc::new(move |_window, cx: &mut App| {
                let _ = view_add.update(cx, |this, cx| {
                    this.folder_add_kv(section, cx);
                });
            }),
            on_type_change: Arc::new(move |ix, ft, _window, cx: &mut App| {
                let _ = view_type.update(cx, |this, cx| {
                    this.folder_change_kv_type(section, ix, ft, cx);
                });
            }),
            on_required_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_req.update(cx, |this, cx| {
                    this.folder_toggle_required(section, ix, val, cx);
                });
            }),
            // File picking isn't meaningful at the folder level.
            on_file_pick: Arc::new(move |_, _, _| {}),
        };
        let id = match section {
            FolderKvSection::Params => "folder-kv-params",
            FolderKvSection::Headers => "folder-kv-headers",
            FolderKvSection::Variables => "folder-kv-vars",
        };
        crate::ui::kv_table::KvTable::new(id, rows, handlers)
            .show_type(show_type)
            .show_required(true)
            .into_any_element()
    }

    /// Render the full folder detail view (directory settings / params /
    /// interface list). Shown in the center column when a folder is selected.
    pub(super) fn render_folder_detail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let folder_id = self.folder_id.clone().unwrap_or_default();
        let active_tab = self.folder_tab;

        // Collect the requests in this folder for the interface list.
        let iface_list = self
            .state
            .read(cx)
            .active_project()
            .map(|p| {
                p.find_folder(&folder_id)
                    .map(|(_, f)| {
                        // (id, name, method, protocol) for each request.
                        collect_iface_entries(f)
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let selected_request = self.state.read(cx).selected_request.clone();
        let view_for_open = cx.entity();

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .child(
                // Header: folder icon + name + tab strip.
                v_flex()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .px_3()
                            .h(px(36.))
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(IconName::FolderOpen),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("目录"),
                            ),
                    )
                    .child(
                        h_flex()
                            .px_3()
                            .gap_1()
                            .child(folder_tab_label(
                                "目录设置",
                                active_tab == FolderTab::Settings,
                                FolderTab::Settings,
                                &theme,
                                cx,
                            ))
                            .child(folder_tab_label(
                                "目录参数",
                                active_tab == FolderTab::Params,
                                FolderTab::Params,
                                &theme,
                                cx,
                            ))
                            .child(folder_tab_label(
                                "接口列表",
                                active_tab == FolderTab::InterfaceList,
                                FolderTab::InterfaceList,
                                &theme,
                                cx,
                            )),
                    ),
            )
            .child(
                // Active tab content (scrollable).
                div()
                    .id("folder-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(match active_tab {
                        FolderTab::Settings => v_flex()
                            .gap_3()
                            .max_w(px(720.))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("目录名称"),
                                    )
                                    .child(
                                        Input::new(&self.folder_name).small(),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("前置 URL（Base URL）"),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "设置后，该目录下所有接口的 URL 会自动拼接此前置地址。留空则不注入。",
                                        ),
                                    )
                                    .child(
                                        // Base URL dropdown: the trigger shows the resolved
                                        // current base_url; the menu offers clear + env-var options.
                                        {
                                            // Resolve the trigger label from the stored base_url.
                                            let proj = self.state.read(cx).active_project();
                                            let raw_base = proj
                                                .and_then(|p| {
                                                    self.folder_id.as_deref().and_then(|fid| p.find_folder(fid))
                                                })
                                                .and_then(|(_, f)| f.base_url.clone())
                                                .unwrap_or_default();
                                            let label_text = {
                                                let raw = raw_base.trim();
                                                if raw.is_empty() {
                                                    "前置URL".to_string()
                                                } else if raw.contains("{{") {
                                                    let vars = proj
                                                        .map(|p| p.base_url_subst_vars())
                                                        .unwrap_or_default();
                                                    let resolved = crate::http::variable::substitute(raw, &vars);
                                                    if resolved.trim().is_empty() {
                                                        "前置URL".to_string()
                                                    } else {
                                                        resolved
                                                    }
                                                } else {
                                                    raw.to_string()
                                                }
                                            };

                                            let active_env = self.state.read(cx).active_project()
                                                .and_then(|p| p.active_environment.as_ref())
                                                .and_then(|eid| {
                                                    self.state.read(cx).active_project()
                                                        .and_then(|p| p.environments.iter().find(|e| &e.id == eid))
                                                });
                                            let env_vars: Vec<(String, String)> = active_env
                                                .map(|env| {
                                                    env.variables.iter()
                                                        .filter(|kv| kv.enabled && (kv.value.starts_with("http://") || kv.value.starts_with("https://")))
                                                        .map(|kv| (kv.key.clone(), kv.value.clone()))
                                                        .collect()
                                                })
                                                .unwrap_or_default();

                                            let open = self.folder_baseurl_open;
                                            let folder_id = self.folder_id.clone();
                                            let state = self.state.clone();
                                            let theme_c = theme.clone();
                                            let panel_entity = cx.entity();

                                            Popover::new("folder-baseurl-popover")
                                                .anchor(gpui::Anchor::BottomLeft)
                                                .open(open)
                                                .on_open_change(cx.listener(|this, open, _, cx| {
                                                    this.folder_baseurl_open = *open;
                                                    cx.notify();
                                                }))
                                                .trigger(
                                                    Button::new("folder-baseurl-trigger")
                                                        .outline()
                                                        .small()
                                                        .w(px(280.))
                                                        .icon(IconName::ChevronDown)
                                                        .label(label_text),
                                                )
                                                .p(px(4.))
                                                .child(
                                                    v_flex()
                                                        .w(px(300.))
                                                        .gap(px(2.))
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(theme_c.muted_foreground)
                                                                .px_2()
                                                                .py_1()
                                                                .child("选择前置URL"),
                                                        )
                                                        .child({
                                                            let fid = folder_id.clone();
                                                            let st = state.clone();
                                                            let panel = panel_entity.clone();
                                                            div()
                                                                .id("folder-baseurl-clear")
                                                                .px_2()
                                                                .py_1()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme_c.muted))
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(theme_c.muted_foreground)
                                                                        .child("（空）不使用前置URL"),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    move |_ev, _window, cx: &mut App| {
                                                                        let fid_clone = fid.clone().unwrap_or_default();
                                                                        let _ = st.update(cx, |s, cx| {
                                                                            if let Some(p) = s.active_project_mut() {
                                                                                set_folder_base_url(&mut p.folders, &fid_clone, None);
                                                                            }
                                                                            s.notify_edited(cx);
                                                                        });
                                                                        let _ = panel.update(cx, |this, cx| {
                                                                            this.folder_baseurl_open = false;
                                                                            cx.notify();
                                                                        });
                                                                    },
                                                                )
                                                        })
                                                        .child(if env_vars.is_empty() {
                                                            div()
                                                                .text_xs()
                                                                .text_color(theme_c.muted_foreground)
                                                                .px_2()
                                                                .py_1()
                                                                .child("（当前环境没有 URL 变量）")
                                                                .into_any_element()
                                                        } else {
                                                            div()
                                                                .id("folder-baseurl-scroll")
                                                                .max_h(px(300.))
                                                                .overflow_y_scroll()
                                                                .children(env_vars.iter().enumerate().map(|(i, (k, v))| {
                                                                    let val_display = v.trim_end_matches('/').to_string();
                                                                    let key = k.clone();
                                                                    // Store the {{key}} placeholder so the stored
                                                                    // value stays in sync when the env var changes;
                                                                    // the literal value is shown as a preview.
                                                                    let placeholder = format!("{{{{{}}}}}", key);
                                                                    let fid = folder_id.clone();
                                                                    let st = state.clone();
                                                                    let tc = theme_c.clone();
                                                                    let panel = panel_entity.clone();

                                                                    div()
                                                                        .id(("baseurl-opt", i))
                                                                        .px_2()
                                                                        .py_1()
                                                                        .rounded_md()
                                                                        .cursor_pointer()
                                                                        .hover(|s| s.bg(tc.muted))
                                                                        .child(
                                                                            v_flex()
                                                                                .gap(px(1.))
                                                                                .child(
                                                                                    div()
                                                                                        .text_sm()
                                                                                        .font_weight(FontWeight::SEMIBOLD)
                                                                                        .child(key.clone()),
                                                                                )
                                                                                .child(
                                                                                    div()
                                                                                        .text_xs()
                                                                                        .text_color(tc.muted_foreground)
                                                                                        .child(val_display),
                                                                                ),
                                                                        )
                                                                        .on_mouse_down(MouseButton::Left, move |_ev, window, cx: &mut App| {
                                                                            let fid_clone = fid.clone().unwrap_or_default();
                                                                            let val_set = placeholder.clone();
                                                                            let _ = st.update(cx, |s, cx| {
                                                                                if let Some(p) = s.active_project_mut() {
                                                                                    set_folder_base_url(&mut p.folders, &fid_clone, Some(val_set.clone()));
                                                                                }
                                                                                s.notify_edited(cx);
                                                                            });
                                                                            let _ = panel.update(cx, |this, cx| {
                                                                                this.folder_baseurl_open = false;
                                                                                cx.notify();
                                                                            });
                                                                            window.refresh();
                                                                        })
                                                                }))
                                                                .into_any_element()
                                                        }),
                                                )
                                                .into_any_element()
                                        },
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("目录描述"),
                                            ),
                                    )
                                    .child(
                                        Input::new(&self.folder_desc)
                                            .small(),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("目录变量"),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "该目录下的所有接口共享这些变量。同名变量时，接口级变量覆盖目录级变量。",
                                        ),
                                    )
                                    .child(self.render_folder_kv(FolderKvSection::Variables, true, cx)),
                            )
                            .into_any_element(),
                        FolderTab::Params => v_flex()
                            .gap_4()
                            .max_w(px(960.))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("公共 Query 参数"),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "该目录下接口自动合并这些 query 参数（接口级同名参数覆盖）。",
                                        ),
                                    )
                                    .child(self.render_folder_kv(FolderKvSection::Params, true, cx)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("公共 Header"),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "该目录下接口自动合并这些请求头（接口级同名头覆盖）。",
                                        ),
                                    )
                                    .child(self.render_folder_kv(FolderKvSection::Headers, false, cx)),
                            )
                            .into_any_element(),
                        FolderTab::InterfaceList => {
                            let theme2 = theme.clone();
                            let selected_req = selected_request.clone();
                            let is_empty = iface_list.is_empty();
                            let total = iface_list.len();
                            // The visible columns (cloned so the closure can move
                            // a snapshot; the source of truth is self.iface_columns).
                            let columns = self.iface_columns.clone();
                            // Pagination: a fixed page size. Clamp the current
                            // page to the valid range (handles deletions).
                            let page_size = 20usize;
                            let last_page = if total == 0 {
                                0
                            } else {
                                (total - 1) / page_size
                            };
                            let page = self.iface_page.min(last_page);
                            let start = page * page_size;
                            let end = (start + page_size).min(total);
                            let page_entries: Vec<IfaceEntry> =
                                iface_list[start..end].to_vec();
                            let has_prev = page > 0;
                            let has_next = page < last_page;
                            let view_paging = view_for_open.clone();
                            let view_cols = view_for_open.clone();
                            let columns_popover_open = self.iface_columns_popover_open;

                            // Build a single table row (header or data) from a
                            // list of (column, text, color_override, is_header)
                            // cell specs.
                            let build_row = |cells: Vec<(IfaceColumn, String, Option<Hsla>, bool)>,
                                             row_id: String,
                                             is_sel: bool,
                                             theme2: &gpui_component::Theme| {
                                let mut row = h_flex()
                                    .id(row_id.clone())
                                    .px_2()
                                    .py(px(4.))
                                    .gap_2()
                                    .rounded(theme2.radius)
                                    .items_center();
                                if is_sel {
                                    row = row.bg(theme2.primary.opacity(0.18));
                                }
                                for (col, text, color_override, is_header) in cells {
                                    let weight = col.width_weight();
                                    let is_method = col == IfaceColumn::Method;
                                    let default_color = if is_header {
                                        theme2.muted_foreground
                                    } else {
                                        theme2.foreground
                                    };
                                    let text_color = color_override.unwrap_or(default_color);
                                    let cell = div()
                                        .flex_shrink_0()
                                        .w(px(96. * weight))
                                        .min_w(px(48.))
                                        .overflow_hidden()
                                        .text_color(text_color)
                                        .text_size(if is_header || is_method {
                                            px(11.)
                                        } else {
                                            px(13.)
                                        })
                                        .font_weight(if is_header || is_method {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .child(text);
                                    row = row.child(cell);
                                }
                                row
                            };

                            v_flex()
                                .gap_1()
                                .max_w(px(1100.))
                                // Toolbar: count on the left, column-picker on the right.
                                .child(
                                    h_flex()
                                        .mb_1()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(format!("共 {} 个接口", total)),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            Popover::new("iface-columns")
                                                .anchor(gpui::Anchor::BottomRight)
                                                .open(columns_popover_open)
                                                .on_open_change(cx.listener(
                                                    |this, open, _, cx| {
                                                        this.iface_columns_popover_open = *open;
                                                        cx.notify();
                                                    },
                                                ))
                                                .trigger(
                                                    Button::new("iface-cols-btn")
                                                        .ghost()
                                                        .xsmall()
                                                        .icon(IconName::Settings2)
                                                        .label("列设置"),
                                                )
                                                .p(px(6.))
                                                .child({
                                                    // Column picker: a checkbox list
                                                    // of every available column.
                                                    let theme_p = theme2.clone();
                                                    let view = view_cols.clone();
                                                    let active: Vec<IfaceColumn> =
                                                        self.iface_columns.clone();
                                                    v_flex()
                                                        .w(px(180.))
                                                        .gap(px(2.))
                                                        .child(
                                                            div()
                                                                .px_2()
                                                                .py_1()
                                                                .text_xs()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(theme_p.foreground)
                                                                .child("显示的列"),
                                                        )
                                                        .children(IfaceColumn::all().iter().map(
                                                            move |col| {
                                                                let checked = active.contains(col);
                                                                let col = *col;
                                                                let view = view.clone();
                                                                h_flex()
                                                                    .id(format!(
                                                                        "col-toggle-{}",
                                                                        col.as_key()
                                                                    ))
                                                                    .px_2()
                                                                    .py(px(3.))
                                                                    .gap_2()
                                                                    .items_center()
                                                                    .rounded(px(4.))
                                                                    .hover(|d| {
                                                                        d.bg(theme_p
                                                                            .accent
                                                                            .opacity(0.3))
                                                                    })
                                                                    .child(
                                                                        Checkbox::new(
                                                                            "col-cb-".to_string() + col.as_key(),
                                                                        )
                                                                        .checked(checked)
                                                                        .on_click(move |val: &bool, _window, cx: &mut App| {
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.toggle_iface_column(col, *val, cx);
                                                                            });
                                                                        }),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_sm()
                                                                            .text_color(theme_p.foreground)
                                                                            .child(col.label()),
                                                                    )
                                                            },
                                                        ))
                                                }),
                                        ),
                                )
                                .when(!columns.is_empty(), |col| {
                                    // Table header row.
                                    let header_cells: Vec<(IfaceColumn, String, Option<Hsla>, bool)> =
                                        columns.iter().map(|c| (*c, c.label().to_string(), None, true)).collect();
                                    col.child(
                                        build_row(
                                            header_cells,
                                            "iface-header".to_string(),
                                            false,
                                            &theme2,
                                        )
                                        .border_b_1()
                                        .border_color(theme2.border),
                                    )
                                })
                                .when(is_empty, |col| {
                                    col.child(
                                        div()
                                            .py_4()
                                            .text_color(theme.muted_foreground)
                                            .text_sm()
                                            .child("该目录下暂无接口，点击目录旁的 + 新建。"),
                                    )
                                })
                                .children(page_entries.iter().enumerate().map(|(i, e)| {
                                    let is_sel = selected_req.as_deref() == Some(&e.id);
                                    // Pre-compute the method badge color for this row.
                                    let method_color = Some(crate::ui::method_colors::badge_color(e.method, cx));
                                    let method_str = e.method.as_str().to_string();
                                    let cells: Vec<(IfaceColumn, String, Option<Hsla>, bool)> = columns
                                        .iter()
                                        .map(|c| {
                                            let text = if *c == IfaceColumn::Method {
                                                method_str.clone()
                                            } else {
                                                e.cell_text(*c)
                                            };
                                            let color = if *c == IfaceColumn::Method {
                                                method_color
                                            } else {
                                                None
                                            };
                                            (*c, text, color, false)
                                        })
                                        .collect();
                                    let view = view_for_open.clone();
                                    let id_for_open = e.id.clone();
                                    let theme_r = theme2.clone();
                                    let mut row = build_row(
                                        cells,
                                        format!("iface-row-{i}"),
                                        is_sel,
                                        &theme_r,
                                    );
                                    row = row
                                        .hover(|d| d.bg(theme_r.accent.opacity(0.4)))
                                        .on_click(move |_, _window, cx: &mut App| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.state.update(cx, |s, cx| {
                                                    s.open_or_focus_tab(&id_for_open, cx);
                                                });
                                            });
                                        });
                                    row
                                }))
                                // Pagination footer: prev / "page x of n" / next.
                                .when(!is_empty && last_page > 0, |col| {
                                    let view_prev = view_paging.clone();
                                    let view_next = view_paging.clone();
                                    col.child(
                                        h_flex()
                                            .mt_2()
                                            .items_center()
                                            .justify_center()
                                            .gap_2()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(
                                                Button::new("iface-prev")
                                                    .ghost()
                                                    .xsmall()
                                                    .icon(IconName::ChevronLeft)
                                                    .disabled(!has_prev)
                                                    .on_click(move |_, _window, cx: &mut App| {
                                                        let _ = view_prev.update(cx, |this, cx| {
                                                            if this.iface_page > 0 {
                                                                this.iface_page -= 1;
                                                                cx.notify();
                                                            }
                                                        });
                                                    }),
                                            )
                                            .child(div().child(format!(
                                                "{}/{} · 共 {} 个",
                                                page + 1,
                                                last_page + 1,
                                                total
                                            )))
                                            .child(
                                                Button::new("iface-next")
                                                    .ghost()
                                                    .xsmall()
                                                    .icon(IconName::ChevronRight)
                                                    .disabled(!has_next)
                                                    .on_click(move |_, _window, cx: &mut App| {
                                                        let _ = view_next.update(cx, |this, cx| {
                                                            this.iface_page += 1;
                                                            cx.notify();
                                                        });
                                                    }),
                                            ),
                                    )
                                })
                                .into_any_element()
                        }
                    }),
            )
    }
}

impl RequestPanel {
    /// Return the active kv row list mutably, based on the current tab.
    pub(super) fn active_rows_mut(&mut self) -> Option<&mut Vec<KvRow>> {
        match self.active_tab {
            ReqTab::Query => Some(&mut self.params_rows),
            ReqTab::Headers => Some(&mut self.headers_rows),
            ReqTab::Path => Some(&mut self.path_rows),
            ReqTab::Cookie => Some(&mut self.cookie_rows),
            ReqTab::Body => match self.body_type {
                BodyType::FormData | BodyType::Urlencoded => Some(&mut self.body_rows),
                _ => None,
            },
            _ => None,
        }
    }

    /// Borrow the kv rows for a folder section.
    pub(super) fn folder_rows(&self, section: FolderKvSection) -> &[KvRow] {
        match section {
            FolderKvSection::Params => &self.folder_param_rows,
            FolderKvSection::Headers => &self.folder_header_rows,
            FolderKvSection::Variables => &self.folder_var_rows,
        }
    }

    /// Mutably borrow the kv rows for a folder section.
    pub(super) fn folder_rows_mut(&mut self, section: FolderKvSection) -> &mut Vec<KvRow> {
        match section {
            FolderKvSection::Params => &mut self.folder_param_rows,
            FolderKvSection::Headers => &mut self.folder_header_rows,
            FolderKvSection::Variables => &mut self.folder_var_rows,
        }
    }

    // --- folder kv row handlers (parallel to the request kv handlers) ---

    pub fn folder_toggle_kv(
        &mut self,
        section: FolderKvSection,
        ix: usize,
        val: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.folder_rows_mut(section).get_mut(ix) {
            row.enabled = val;
        }
        self.commit_folder(cx);
        cx.notify();
    }

    pub fn folder_delete_kv(
        &mut self,
        section: FolderKvSection,
        ix: usize,
        cx: &mut Context<Self>,
    ) {
        let rows = self.folder_rows_mut(section);
        if ix < rows.len() {
            rows.remove(ix);
        }
        self.commit_folder(cx);
        cx.notify();
    }

    pub fn folder_add_kv(&mut self, section: FolderKvSection, cx: &mut Context<Self>) {
        // Defer the actual row creation to render where a Window is available.
        self.pending_folder_kv_add = Some(section);
        cx.notify();
    }

    pub fn folder_change_kv_type(
        &mut self,
        section: FolderKvSection,
        ix: usize,
        ft: FieldType,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.folder_rows_mut(section).get_mut(ix) {
            row.field_type = ft;
        }
        self.commit_folder(cx);
        cx.notify();
    }

    pub fn folder_toggle_required(
        &mut self,
        section: FolderKvSection,
        ix: usize,
        req: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.folder_rows_mut(section).get_mut(ix) {
            row.required = req;
        }
        self.commit_folder(cx);
        cx.notify();
    }

    /// Process pending folder kv additions now that a Window is available.
    pub fn reconcile_folder_kv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(section) = self.pending_folder_kv_add.take() {
            let st = self.state.clone();
            self.folder_rows_mut(section).push(KvRow::empty(st, window, cx));
            cx.notify();
        }
    }

    /// Toggle a column's visibility in the interface list and persist the
    /// customized set. Always keeps at least one column.
    pub fn toggle_iface_column(&mut self, col: IfaceColumn, on: bool, cx: &mut Context<Self>) {
        let idx = self.iface_columns.iter().position(|c| *c == col);
        match (on, idx) {
            (true, None) => {
                // Add it (after the same relative order as IfaceColumn::all()).
                self.iface_columns.push(col);
                self.iface_columns
                    .sort_by_key(|c| IfaceColumn::all().iter().position(|x| x == c).unwrap_or(0));
            }
            (false, Some(i)) => {
                // Never remove the last column.
                if self.iface_columns.len() > 1 {
                    self.iface_columns.remove(i);
                }
            }
            _ => {}
        }
        crate::state::persistence::save_iface_columns(&self.iface_columns);
        cx.notify();
    }

    pub fn toggle_kv(&mut self, ix: usize, val: bool, cx: &mut Context<Self>) {
        if let Some(rows) = self.active_rows_mut() {
            if let Some(row) = rows.get_mut(ix) {
                row.enabled = val;
            }
        }
        cx.notify();
    }

    pub fn delete_kv(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(rows) = self.active_rows_mut() {
            if ix < rows.len() {
                rows.remove(ix);
            }
        }
        cx.notify();
    }

    /// Process a pending kv add now that a Window is available (called from render).
    pub fn reconcile_pending_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_kv_add {
            self.pending_kv_add = false;
            let st = self.state.clone();
            if let Some(rows) = self.active_rows_mut() {
                rows.push(KvRow::empty(st, window, cx));
            }
        }
        // Visual-mode add row (needs a Window to create InputState entities).
        if self.pending_visual_add {
            self.pending_visual_add = false;
            self.raw_visual_rows.push(KvRow::empty(self.state.clone(), window, cx));
        }
        // File picker for form-data file fields (needs a Window).
        if let Some(ix) = self.pending_file_pick.take() {
            let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择文件".into()),
            });
            let entity = cx.entity();
            cx.spawn(async move |_this, cx| {
                if let Ok(Ok(Some(paths))) = prompt.await {
                    if let Some(path) = paths.first() {
                        let path_str = path.to_string_lossy().to_string();
                        let _ = entity.update(cx, |this, cx| {
                            if let Some(rows) = this.active_rows_mut() {
                                if let Some(row) = rows.get_mut(ix) {
                                    row.file_path = Some(path_str);
                                    row.field_type = FieldType::File;
                                }
                            }
                            this.commit_to_model(cx);
                            cx.notify();
                        });
                    }
                }
            })
            .detach();
        }
    }

    /// Change the field type of a kv row (form-data/query).
    pub fn change_kv_type(&mut self, ix: usize, ft: FieldType, cx: &mut Context<Self>) {
        // Track whether we should auto-open the file picker: switching a row
        // *to* File (in a form-data context) with no path yet selected.
        let mut open_picker = false;
        if let Some(rows) = self.active_rows_mut() {
            if let Some(row) = rows.get_mut(ix) {
                row.field_type = ft;
                // Switching away from File clears the file path.
                if ft != FieldType::File {
                    row.file_path = None;
                } else if row.file_path.is_none() {
                    // Only auto-pick for form-data rows (the only scope that
                    // supports files); urlencoded etc. have no file concept.
                    open_picker =
                        self.body_type == BodyType::FormData && self.active_tab == ReqTab::Body;
                }
            }
        }
        self.commit_to_model(cx);
        if open_picker {
            // Defer to reconcile_pending_add, which runs in render where a
            // Window is available (required by prompt_for_paths).
            self.pending_file_pick = Some(ix);
        }
        cx.notify();
    }

    /// Toggle the "required" flag of a kv row.
    pub fn toggle_required(&mut self, ix: usize, val: bool, cx: &mut Context<Self>) {
        if let Some(rows) = self.active_rows_mut() {
            if let Some(row) = rows.get_mut(ix) {
                row.required = val;
            }
        }
        self.commit_to_model(cx);
        cx.notify();
    }

    // ---- Raw JSON visual editing ----

    /// Parse the current Raw JSON body into `raw_parameter` fields. Called when
    /// switching to visual mode. Triggers a reload so `raw_visual_rows` are
    /// rebuilt from the freshly parsed model.
    pub fn sync_raw_to_visual(&mut self, cx: &mut Context<Self>) {
        let id = self.request_id.clone();
        if let Some(req) = self.state.read(cx).active_project().and_then(|p| {
            id.as_ref()
                .and_then(|id| p.find_request(id).map(|(_, r)| r.clone()))
        }) {
            let mut body = req.body.clone();
            body.sync_raw_to_fields();
            self.state.update(cx, |s, cx| {
                if let Some(p) = s.active_project_mut() {
                    if let Some((_, r)) = p.find_request_mut(&id.as_ref().unwrap()) {
                        r.body.raw_parameter = body.raw_parameter.clone();
                    }
                }
                s.dirty = true;
                s.schedule_save(cx);
            });
            // 注意：必须在可变借用 AppState 释放后再 emit 事件，避免双重借用 panic。
            // spawn异步任务在当前同步流程结束后执行，借用已释放。
            let state = self.state.clone();
            cx.spawn(async move |_, cx| {
                let _ = state.update(cx, |s, cx| {
                    let _ = s;
                    cx.emit(AppEvent::RequestEdited);
                });
            })
            .detach();
        }
        // Rebuild the persistent row entities on next render.
        self.pending_reload = true;
    }

    /// Serialize the `raw_parameter` fields back into the Raw JSON body. Called
    /// when switching back to code mode. First commits the current row inputs
    /// to the model, then serializes.
    pub fn sync_visual_to_raw(&mut self, cx: &mut Context<Self>) {
        // Commit current row inputs → model, then serialize to raw.
        self.commit_visual_to_model(cx);
        // Reload picks up the updated `body.raw` into the code editor.
        self.pending_reload = true;
    }

    /// Render the Raw body as a visual field table (KvTable over
    /// `raw_visual_rows`). Uses persistent InputState entities so typing works
    /// normally — the rows are built once at reload and only re-synced from the
    /// model when the visual mode is entered.
    pub(super) fn render_raw_visual(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view_toggle = cx.entity();
        let view_delete = cx.entity();
        let view_add = cx.entity();
        let view_type = cx.entity();
        let view_req = cx.entity();
        let handlers = crate::ui::kv_table::KvHandlers {
            on_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_toggle.update(cx, |this, cx| {
                    this.update_raw_field(ix, |kv| kv.enabled = val, cx);
                });
            }),
            on_delete: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_delete.update(cx, |this, cx| {
                    this.remove_raw_field(ix, cx);
                });
            }),
            on_add: Arc::new(move |_window, cx: &mut App| {
                let _ = view_add.update(cx, |this, cx| {
                    this.add_raw_field(cx);
                });
            }),
            on_type_change: Arc::new(move |ix, ft, _window, cx: &mut App| {
                let _ = view_type.update(cx, |this, cx| {
                    this.update_raw_field(ix, |kv| kv.field_type = ft, cx);
                });
            }),
            on_required_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_req.update(cx, |this, cx| {
                    this.update_raw_field(ix, |kv| kv.required = val, cx);
                });
            }),
            on_file_pick: Arc::new(|_, _, _| {}),
        };

        v_flex().size_full().min_h_0().gap_1().child(
            crate::ui::kv_table::KvTable::new("raw-visual", self.raw_visual_rows.clone(), handlers)
                .show_type(true)
                .show_required(true)
                .show_description(true),
        )
    }

    /// Sync the visual-mode row inputs back to the model's `raw_parameter`,
    /// then serialize to `raw` JSON. Called after any visual-mode edit.
    pub(super) fn commit_visual_to_model(&mut self, cx: &mut Context<Self>) {
        let id = match self.request_id.clone() {
            Some(id) => id,
            None => return,
        };
        let fields = kv_table::pairs_from_rows(&self.raw_visual_rows, cx);
        self.state.update(cx, |s, cx| {
            if let Some(p) = s.active_project_mut() {
                if let Some((_, r)) = p.find_request_mut(&id) {
                    r.body.raw_parameter = fields;
                    r.body.sync_fields_to_raw();
                }
            }
            s.dirty = true;
            s.schedule_save(cx);
        });
        // 注意：必须在可变借用 AppState 释放后再 emit 事件，避免双重借用 panic。
        // spawn异步任务在当前同步流程结束后执行，借用已释放。
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::RequestEdited);
            });
        })
        .detach();
    }

    /// Update a single visual-mode row's plain fields (enabled/type/required),
    /// then commit everything to the model. Key/value/description are edited
    /// in-place via the persistent InputState entities and picked up by the
    /// commit.
    pub(super) fn update_raw_field(
        &mut self,
        ix: usize,
        f: impl Fn(&mut kv_table::KvRow),
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.raw_visual_rows.get_mut(ix) {
            f(row);
        }
        self.commit_visual_to_model(cx);
        cx.notify();
    }

    pub(super) fn remove_raw_field(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.raw_visual_rows.len() {
            self.raw_visual_rows.remove(ix);
        }
        self.commit_visual_to_model(cx);
        cx.notify();
    }

    pub(super) fn add_raw_field(&mut self, cx: &mut Context<Self>) {
        // Flag for rebuild on next render (KvRow needs a Window to create its
        // InputState entities; the handler runs on &mut App without one).
        self.pending_visual_add = true;
        let id = match self.request_id.clone() {
            Some(id) => id,
            None => return,
        };
        self.state.update(cx, |s, cx| {
            if let Some(p) = s.active_project_mut() {
                if let Some((_, r)) = p.find_request_mut(&id) {
                    r.body.raw_parameter.push(KeyValue::new("new_field", ""));
                    r.body.sync_fields_to_raw();
                }
            }
            s.dirty = true;
            s.schedule_save(cx);
        });
        // 注意：必须在可变借用 AppState 释放后再 emit 事件，避免双重借用 panic。
        // spawn异步任务在当前同步流程结束后执行，借用已释放。
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::RequestEdited);
            });
        })
        .detach();
        cx.notify();
    }
}
