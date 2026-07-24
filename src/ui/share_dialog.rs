//! The share configuration dialog (postman-style, screenshots 2 & 3).
//!
//! Opened via `window.open_dialog` from two entry points: the title-bar
//! "share project" button (scope = Project) and the request-panel "share single
//! API" button (scope = Request). The dialog owns its mutable state in a
//! [`ShareDialogState`] entity — title/password `InputState`, plus plain fields
//! for the dropdown/radio/checkbox selections — and on confirm emits a
//! [`ShareConfig`] that the caller persists + serves.
//!
//! Sections (matching postman exactly):
//! - 有效期 (expiration): dropdown — 永久有效 / 1 / 7 / 30 / 90 / 180 / 365 天
//! - 分享方式 (share methods): checkboxes — 链接 / 二维码 / 导出 HTML
//! - 访问限制 (access control): radio — 公开 / 密码 (+ password input)
//! - 开发环境 (environment): dropdown of the project's environments
//! - 字段展示控制 (field display): checkboxes for each doc section
//! - 文档 logo (document logo): file picker + thumbnail

use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::{
    ActiveTheme, IconName, Sizable as _, Theme, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    popover::Popover,
    v_flex,
};

use crate::share::models::{
    AccessControl, Expiration, FieldDisplay, ShareConfig, ShareMethod, ShareScope,
};
use crate::state::AppState;
use crate::state::models::Environment;

/// Mutable state backing the share dialog. Rendered as the dialog's content;
/// the footer reads it back on confirm to build the final [`ShareConfig`].
pub struct ShareDialogState {
    /// The in-progress config (id/project/scope/target are fixed at open time;
    /// the dialog edits the rest).
    pub config: ShareConfig,
    /// The project's environments, for the environment dropdown.
    pub environments: Vec<Environment>,
    /// Title input.
    pub title_input: Entity<InputState>,
    /// Password input (only used when access is password-protected).
    pub password_input: Entity<InputState>,
    /// Popover open states for the two dropdowns.
    pub expire_popover: bool,
    pub env_popover: bool,
    /// Logo preview path (mirrors `config.logo_path`; kept for re-render).
    pub logo_path: Option<PathBuf>,
}

impl ShareDialogState {
    /// Build the dialog state for a new share with the given scope/target,
    /// pre-filling sensible defaults.
    pub fn new(
        project_id: &str,
        project_name: &str,
        scope: ShareScope,
        target_id: Option<String>,
        target_name: Option<String>,
        environments: Vec<Environment>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let mut config = ShareConfig::new(project_id.to_string(), project_name.to_string());
        config.scope = scope;
        // Default title reflects the scope (computed before the move below).
        config.title = match (&scope, &target_name) {
            (ShareScope::Request, Some(n)) | (ShareScope::Folder, Some(n)) => {
                format!("{} · {}", project_name, n)
            }
            _ => project_name.to_string(),
        };
        config.target_id = target_id;
        config.target_name = target_name;

        let title_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("文档标题");
            s.set_value(&config.title, window, cx);
            s
        });
        let password_input = cx.new(|cx| InputState::new(window, cx).placeholder("访问密码"));

        cx.new(|_cx| Self {
            config,
            environments,
            title_input,
            password_input,
            expire_popover: false,
            env_popover: false,
            logo_path: None,
        })
    }

    /// Read the title/password inputs back into `config` and return it. Call
    /// this on confirm.
    pub fn finalize(&self, cx: &App) -> ShareConfig {
        let mut cfg = self.config.clone();
        cfg.title = self.title_input.read(cx).value().to_string();
        if !cfg.access.public {
            let pw = self.password_input.read(cx).value().to_string();
            cfg.access = AccessControl::password(pw);
        }
        cfg.logo_path = self.logo_path.clone();
        cfg
    }
}

// The entity needs a Render impl to be constructable via `cx.new`; we never
// render it directly (the dialog content is built by `render_content`).
impl Render for ShareDialogState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
    }
}

/// Render the dialog's content (sections) for `window.open_dialog`.
pub fn render_content(
    state: Entity<ShareDialogState>,
    theme: Theme,
    cx: &mut App,
) -> impl IntoElement {
    let s = state.read(cx);
    let border = theme.border;
    let muted = theme.muted_foreground;

    v_flex()
        .p_4()
        .w_full()
        .gap_4()
        // Title input (always first).
        .child(section_label("文档标题", muted))
        .child(Input::new(&s.title_input).small())
        // 有效期
        .child(divider(border))
        .child(render_expire_section(state.clone(), theme.clone(), cx))
        // 分享方式
        .child(divider(border))
        .child(render_methods_section(state.clone(), theme.clone(), cx))
        // 访问限制
        .child(divider(border))
        .child(render_access_section(state.clone(), theme.clone(), cx))
        // 开发环境
        .child(divider(border))
        .child(render_env_section(state.clone(), theme.clone(), cx))
        // 字段展示控制
        .child(divider(border))
        .child(render_field_display_section(
            state.clone(),
            theme.clone(),
            cx,
        ))
        // 文档 logo
        .child(divider(border))
        .child(render_logo_section(state.clone(), theme.clone(), cx))
}

fn section_label(text: &str, muted: Hsla) -> Div {
    div()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(muted)
        .child(text.to_string())
}

fn divider(border: Hsla) -> Div {
    div().h(px(1.)).w_full().bg(border)
}

// ===========================================================================
// Section: 有效期 (expiration dropdown)
// ===========================================================================

fn render_expire_section(state: Entity<ShareDialogState>, theme: Theme, cx: &App) -> Div {
    let s = state.read(cx);
    let muted = theme.muted_foreground;
    let current_label = s.config.expire.label();
    let popover_open = s.expire_popover;

    v_flex()
        .gap_2()
        .child(section_label("有效期", muted))
        .child(
            Popover::new("share-expire-pop")
                .anchor(gpui::Anchor::BottomLeft)
                .open(popover_open)
                .on_open_change({
                    let state = state.clone();
                    move |open: &bool, _window, cx| {
                        let _ = state.update(cx, |s, cx| {
                            s.expire_popover = *open;
                            cx.notify();
                        });
                    }
                })
                .trigger(
                    Button::new("share-expire-trigger")
                        .ghost()
                        .small()
                        .w(px(200.))
                        .label(current_label)
                        .icon(IconName::ChevronDown),
                )
                .child(
                    v_flex().p_1().w(px(200.)).gap(px(1.)).children(
                        Expiration::PRESETS
                            .iter()
                            .enumerate()
                            .map(|(ix, (exp, label))| {
                                let selected = s.config.expire == *exp;
                                let state_for_click = state.clone();
                                let exp_val = *exp;
                                h_flex()
                                    .id(("expire-opt", ix))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|st| st.bg(theme.muted))
                                    .when(selected, |d| d.bg(theme.accent.opacity(0.5)))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(if selected {
                                                theme.foreground
                                            } else {
                                                muted
                                            })
                                            .child(*label),
                                    )
                                    .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                        let _ = state_for_click.update(cx, |s, cx| {
                                            s.config.expire = exp_val;
                                            s.expire_popover = false;
                                            window.refresh();
                                            cx.notify();
                                        });
                                    })
                            }),
                    ),
                ),
        )
}

// ===========================================================================
// Section: 分享方式 (share methods checkboxes)
// ===========================================================================

fn render_methods_section(state: Entity<ShareDialogState>, theme: Theme, cx: &App) -> Div {
    let s = state.read(cx);
    let muted = theme.muted_foreground;

    v_flex()
        .gap_2()
        .child(section_label("分享方式", muted))
        .child(
            h_flex()
                .gap_4()
                .flex_wrap()
                .children(ShareMethod::ALL.iter().map(|&method| {
                    let checked = s.config.share_methods.contains(&method);
                    let state_for_click = state.clone();
                    let id = format!("share-method-{}", method.label());
                    Checkbox::new(SharedString::from(id))
                        .checked(checked)
                        .label(method.label())
                        .on_click(move |c: &bool, _window, cx| {
                            let _ = state_for_click.update(cx, |s, cx| {
                                if *c {
                                    if !s.config.share_methods.contains(&method) {
                                        s.config.share_methods.push(method);
                                    }
                                } else {
                                    s.config.share_methods.retain(|m| *m != method);
                                }
                                cx.notify();
                            });
                        })
                })),
        )
}

// ===========================================================================
// Section: 访问限制 (access control: public / password radio)
// ===========================================================================

fn render_access_section(state: Entity<ShareDialogState>, theme: Theme, cx: &App) -> Div {
    let s = state.read(cx);
    let muted = theme.muted_foreground;
    let is_public = s.config.access.public;

    let mut col = v_flex().gap_2().child(section_label("访问限制", muted));
    let mut radios = h_flex().gap_6();

    // Public radio.
    {
        let state_for_click = state.clone();
        radios = radios.child(
            gpui_component::radio::Radio::new("access-public")
                .checked(is_public)
                .label("公开")
                .on_click(move |_checked: &bool, _window, cx| {
                    let _ = state_for_click.update(cx, |s, cx| {
                        s.config.access = AccessControl::public();
                        cx.notify();
                    });
                }),
        );
    }
    // Password radio.
    {
        let state_for_click = state.clone();
        radios = radios.child(
            gpui_component::radio::Radio::new("access-password")
                .checked(!is_public)
                .label("密码")
                .on_click(move |_checked: &bool, _window, cx| {
                    let _ = state_for_click.update(cx, |s, cx| {
                        if s.config.access.public {
                            s.config.access = AccessControl::password(String::new());
                        }
                        cx.notify();
                    });
                }),
        );
    }
    col = col.child(radios);

    // Password input (shown only when password mode is selected).
    if !is_public {
        col = col.child(
            div()
                .pl_6()
                .w(px(220.))
                .child(Input::new(&s.password_input).small()),
        );
    }
    col
}

// ===========================================================================
// Section: 开发环境 (environment dropdown)
// ===========================================================================

fn render_env_section(state: Entity<ShareDialogState>, theme: Theme, cx: &App) -> Div {
    let s = state.read(cx);
    let muted = theme.muted_foreground;

    // If the project has no environments, show a disabled hint.
    if s.environments.is_empty() {
        return v_flex()
            .gap_2()
            .child(section_label("开发环境", muted))
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("当前项目没有环境变量"),
            );
    }

    let selected_id = s.config.environment_id.clone();
    let selected_label = s
        .environments
        .iter()
        .find(|e| Some(&e.id) == selected_id.as_ref())
        .map(|e| e.name.as_str())
        .unwrap_or("无环境");
    let popover_open = s.env_popover;

    // Build options: "无环境" + each environment.
    let mut options: Vec<(Option<String>, String)> = vec![(None, "无环境".to_string())];
    for env in &s.environments {
        options.push((Some(env.id.clone()), env.name.clone()));
    }

    v_flex()
        .gap_2()
        .child(section_label("开发环境", muted))
        .child(
            Popover::new("share-env-pop")
                .anchor(gpui::Anchor::BottomLeft)
                .open(popover_open)
                .on_open_change({
                    let state = state.clone();
                    move |open: &bool, _window, cx| {
                        let _ = state.update(cx, |s, cx| {
                            s.env_popover = *open;
                            cx.notify();
                        });
                    }
                })
                .trigger(
                    Button::new("share-env-trigger")
                        .ghost()
                        .small()
                        .w(px(220.))
                        .label(selected_label)
                        .icon(IconName::ChevronDown),
                )
                .child(v_flex().p_1().w(px(220.)).gap(px(1.)).children(
                    options.into_iter().enumerate().map(|(ix, (id, label))| {
                        let selected = id == selected_id;
                        let state_for_click = state.clone();
                        h_flex()
                            .id(("env-opt", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|st| st.bg(theme.muted))
                            .when(selected, |d| d.bg(theme.accent.opacity(0.5)))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(if selected { theme.foreground } else { muted })
                                    .child(label),
                            )
                            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                let _ = state_for_click.update(cx, |s, cx| {
                                    s.config.environment_id = id.clone();
                                    s.env_popover = false;
                                    window.refresh();
                                    cx.notify();
                                });
                            })
                    }),
                )),
        )
}

// ===========================================================================
// Section: 字段展示控制 (field display checkboxes)
// ===========================================================================

fn render_field_display_section(state: Entity<ShareDialogState>, theme: Theme, cx: &App) -> Div {
    let s = state.read(cx);
    let muted = theme.muted_foreground;

    v_flex()
        .gap_2()
        .child(section_label("字段展示控制", muted))
        .child(
            h_flex().gap_x(px(24.)).gap_y(px(8.)).flex_wrap().children(
                FieldDisplay::FIELDS
                    .iter()
                    .enumerate()
                    .map(|(ix, (key, label))| {
                        let checked = s.config.field_display.get(key);
                        let state_for_click = state.clone();
                        let key_owned = key.to_string();
                        Checkbox::new(("field-display", ix))
                            .checked(checked)
                            .label(*label)
                            .on_click(move |c: &bool, _window, cx| {
                                let _ = state_for_click.update(cx, |s, cx| {
                                    s.config.field_display.set(&key_owned, *c);
                                    cx.notify();
                                });
                            })
                    }),
            ),
        )
}

// ===========================================================================
// Section: 文档 logo (file picker + thumbnail)
// ===========================================================================

fn render_logo_section(state: Entity<ShareDialogState>, theme: Theme, cx: &App) -> Div {
    let s = state.read(cx);
    let muted = theme.muted_foreground;

    let mut col = v_flex().gap_2().child(section_label("文档 logo", muted));
    let mut row = h_flex().gap_3().items_center();

    // Thumbnail or placeholder.
    if let Some(path) = &s.logo_path {
        row = row.child(
            div()
                .w(px(48.))
                .h(px(48.))
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_xs().text_color(muted).child("已选")),
        );
        let path_display = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("logo")
            .to_string();
        row = row.child(
            div()
                .flex_1()
                .text_sm()
                .text_color(muted)
                .child(path_display),
        );
    } else {
        row = row.child(
            div()
                .w(px(48.))
                .h(px(48.))
                .rounded_md()
                .bg(theme.muted)
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_lg().text_color(muted).child("V")),
        );
        row = row.child(
            div()
                .flex_1()
                .text_sm()
                .text_color(muted)
                .child("未选择 logo（可选）"),
        );
    }

    // Choose button.
    {
        let state_for_click = state.clone();
        row = row.child(
            Button::new("share-logo-pick")
                .ghost()
                .small()
                .label("选择图片")
                .on_click(move |_ev, window, cx| {
                    let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
                        files: true,
                        directories: false,
                        multiple: false,
                        prompt: Some("选择文档 logo".into()),
                    });
                    let state_clone = state_for_click.clone();
                    cx.spawn(async move |cx| {
                        if let Ok(Ok(Some(paths))) = prompt.await {
                            if let Some(path) = paths.first() {
                                let _ = state_clone.update(cx, |s, cx| {
                                    s.logo_path = Some(path.clone());
                                    cx.notify();
                                });
                            }
                        }
                    })
                    .detach();
                    window.refresh();
                }),
        );
    }

    col = col.child(row);
    col
}

// ===========================================================================
// Public entry: build a full dialog (content + footer) for open_dialog.
// ===========================================================================

/// Build a complete dialog via `window.open_dialog`. The `on_confirm` closure
/// receives the finalized [`ShareConfig`] and is responsible for persisting it,
/// starting/refreshing the server, and opening the link.
pub fn open_dialog<F>(
    app_state: Entity<AppState>,
    scope: ShareScope,
    target_id: Option<String>,
    target_name: Option<String>,
    on_confirm: F,
    window: &mut Window,
    cx: &mut App,
) where
    F: Fn(ShareConfig, &mut Window, &mut App) + 'static,
{
    // Resolve the active project + its environments to pre-fill the dialog.
    let (project_id, project_name, environments) = app_state
        .read(cx)
        .active_project()
        .map(|p| (p.id.clone(), p.name.clone(), p.environments.clone()))
        .unwrap_or_else(|| (String::new(), "未命名项目".to_string(), Vec::new()));

    let state = ShareDialogState::new(
        &project_id,
        &project_name,
        scope,
        target_id,
        target_name,
        environments,
        window,
        cx,
    );

    // Wrap once outside the `Fn` closure so it can be cloned per build.
    let on_confirm = std::sync::Arc::new(on_confirm);

    window.open_dialog(cx, move |dialog, _window, cx| {
        let state_for_content = state.clone();
        let state_for_footer = state.clone();
        let theme = cx.theme().clone();

        dialog
            .title("分享文档")
            .w(px(580.))
            .content(move |content, _window, cx| {
                let body = render_content(state_for_content.clone(), theme.clone(), cx);
                content.child(body)
            })
            .footer({
                let on_confirm = on_confirm.clone();
                Button::new("share-confirm")
                    .primary()
                    .small()
                    .label("创建分享")
                    .on_click(move |_ev, window, cx| {
                        let cfg = state_for_footer.read(cx).finalize(cx);
                        (*on_confirm.clone())(cfg, window, cx);
                        window.close_dialog(cx);
                    })
            })
    });
}
