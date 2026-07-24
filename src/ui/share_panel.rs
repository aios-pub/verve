//! Document-sharing management panel — the full-bleed "文档管理" view.
//!
//! Shown when the activity rail's "文档管理" button is active (an exclusive
//! view, like `ProjectManage`). Lists every [`ShareConfig`] for the active
//! project with status (active/expired), visit count, and row actions: open
//! link / copy link / delete. Hosts the "新建分享" entry point which opens the
//! [`crate::ui::share_dialog`] with `ShareScope::Project`.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    ActiveTheme, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::share::models::{ShareConfig, ShareScope, now_ts};
use crate::state::{AppEvent, AppState};

/// Events the share panel emits upward (consumed by `VerveApp`).
#[derive(Clone, Debug)]
pub enum ShareEvent {
    /// Open the share-config dialog (scope decided by caller).
    NewShare,
    /// Open the link for the given share in the system browser.
    Open(String),
    /// Copy the link for the given share to the clipboard.
    Copy(String),
    /// Delete the given share (after the panel's confirm dialog).
    Delete(String),
}

pub struct SharePanel {
    pub state: Entity<AppState>,
    /// Cached shares for the active project, reloaded on render when stale.
    pub shares: Vec<ShareConfig>,
    /// Set true when the active project changes; triggers a reload on render.
    pub stale: bool,
    _subs: Vec<gpui::Subscription>,
}

impl EventEmitter<ShareEvent> for SharePanel {}

impl SharePanel {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Reload shares when the project/workspace changes.
        let sub = cx.subscribe(&state, |this, _src, ev: &AppEvent, cx| match ev {
            AppEvent::WorkspaceChanged | AppEvent::WorkspaceSwitched | AppEvent::Persisted => {
                this.stale = true;
                cx.notify();
            }
            _ => {}
        });

        let shares = load_active_project_shares(&state, cx);
        Self {
            state,
            shares,
            stale: false,
            _subs: vec![sub],
        }
    }

    /// Reload the cached shares from disk, filtered to the active project.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.shares = load_active_project_shares(&self.state, cx);
        self.stale = false;
        cx.notify();
    }
}

impl Render for SharePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.stale {
            self.reload(cx);
        }
        let theme = cx.theme().clone();
        let bg = theme.background;
        let border = theme.border;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(bg)
            .text_color(fg)
            // Header bar.
            .child(
                h_flex()
                    .h(px(44.))
                    .px_4()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("文档管理"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(muted)
                            .text_sm()
                            .pl_3()
                            .child(format!("当前项目共 {} 个分享", self.shares.len())),
                    )
                    .child(
                        Button::new("share-new")
                            .primary()
                            .small()
                            .icon(IconName::Plus)
                            .label("新建分享")
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                cx.emit(ShareEvent::NewShare);
                                let _ = this; // no-op
                            })),
                    ),
            )
            // Body: the shares table.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("share-list-scroll")
                    .overflow_y_scroll()
                    .child(self.render_table(cx, theme.clone())),
            )
    }
}

impl SharePanel {
    fn render_table(
        &self,
        cx: &mut Context<Self>,
        theme: gpui_component::Theme,
    ) -> impl IntoElement {
        let border = theme.border;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;

        if self.shares.is_empty() {
            return v_flex()
                .p_8()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_color(muted)
                        .child("还没有分享文档。点击右上角「新建分享」创建一个。"),
                )
                .into_any_element();
        }

        let now = now_ts();

        // Table header.
        let header = h_flex()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(border)
            .bg(theme.muted)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(muted)
            .child(div().w(px(40.)).child("状态"))
            .child(div().w(px(220.)).child("标题"))
            .child(div().w(px(90.)).child("范围"))
            .child(div().w(px(90.)).child("有效期"))
            .child(div().w(px(70.)).child("访问"))
            .child(div().w(px(70.)).child("访问次数"))
            .child(div().w(px(150.)).child("创建时间"))
            .child(div().flex_1().child("操作"));

        // Rows.
        let rows = self.shares.iter().enumerate().map(|(ix, share)| {
            let expired = !share.is_valid_at(now);
            let row_muted = if expired { muted.opacity(0.6) } else { fg };
            let status = if expired { "已过期" } else { "生效中" };
            let status_color = if expired {
                gpui::red()
            } else {
                gpui::hsla(0.33, 0.62, 0.45, 1.0)
            };
            let created = format_ts(share.created_at);

            let mut actions = h_flex().gap_1();
            // Open link.
            {
                let id = share.id.clone();
                actions = actions.child(
                    Button::new(("share-open", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ExternalLink)
                        .tooltip("在浏览器中打开")
                        .on_click(cx.listener(move |this, _ev, _window, cx| {
                            cx.emit(ShareEvent::Open(id.clone()));
                            let _ = this;
                        })),
                );
            }
            // Copy link.
            {
                let id = share.id.clone();
                actions = actions.child(
                    Button::new(("share-copy", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Copy)
                        .tooltip("复制链接")
                        .on_click(cx.listener(move |this, _ev, _window, cx| {
                            cx.emit(ShareEvent::Copy(id.clone()));
                            let _ = this;
                        })),
                );
            }
            // Delete.
            {
                let id = share.id.clone();
                actions = actions.child(
                    Button::new(("share-delete", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Delete)
                        .tooltip("删除分享")
                        .text_color(gpui::red())
                        .on_click(cx.listener(move |_this, _ev, _window, cx| {
                            cx.emit(ShareEvent::Delete(id.clone()));
                        })),
                );
            }

            h_flex()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(border)
                .hover(|s| s.bg(theme.muted))
                .child(
                    div()
                        .w(px(40.))
                        .text_xs()
                        .text_color(status_color)
                        .child(status),
                )
                .child(
                    div()
                        .w(px(220.))
                        .text_sm()
                        .text_color(row_muted)
                        .overflow_hidden()
                        .child(div().truncate().child(share.display_title())),
                )
                .child(
                    div()
                        .w(px(90.))
                        .text_sm()
                        .text_color(row_muted)
                        .child(share.scope_label()),
                )
                .child(
                    div()
                        .w(px(90.))
                        .text_sm()
                        .text_color(row_muted)
                        .child(share.expire_label()),
                )
                .child(
                    div()
                        .w(px(70.))
                        .text_sm()
                        .text_color(row_muted)
                        .child(share.access_label()),
                )
                .child(
                    div()
                        .w(px(70.))
                        .text_sm()
                        .text_color(row_muted)
                        .child(share.visits.to_string()),
                )
                .child(div().w(px(150.)).text_xs().text_color(muted).child(created))
                .child(div().flex_1().child(actions))
        });

        v_flex()
            .w_full()
            .child(header)
            .children(rows)
            .into_any_element()
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Load shares for the active project from disk.
fn load_active_project_shares(state: &Entity<AppState>, cx: &App) -> Vec<ShareConfig> {
    let active_id = state
        .read(cx)
        .active_project()
        .map(|p| p.id.clone())
        .unwrap_or_default();
    crate::share::persist::load_shares()
        .into_iter()
        .filter(|s| s.project_id == active_id)
        .collect()
}

/// Format a Unix timestamp into a readable `YYYY-MM-DD HH:MM` string.
fn format_ts(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}

// Silence unused warnings for fields/events wired but consumed by the parent.
#[allow(dead_code)]
fn _silence(_scope: ShareScope) {}
