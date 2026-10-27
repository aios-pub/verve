//! Request-tab rendering: the active-tab body (params/headers/body/scripts/docs),
//! the mock tab, and the auth tab.
use super::{ReqTab, RequestPanel};
use crate::state::models::*;
use crate::ui::kv_table::KvRow;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::WindowExt as _;
use gpui_component::input::{Editor, Input, InputState, Textarea};
use gpui_component::select::Select;
use gpui_component::{
    ActiveTheme, Disableable as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    v_flex,
};
use std::sync::Arc;

impl RequestPanel {
    pub(super) fn render_active_tab(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        match self.active_tab {
            ReqTab::Headers => self
                .render_kv(&self.headers_rows.clone(), false, false, cx)
                .into_any_element(),
            ReqTab::Query => self
                .render_kv(&self.params_rows.clone(), true, false, cx)
                .into_any_element(),
            ReqTab::Path => {
                v_flex()
                    .size_full()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(
                                "路径变量：在 URL 中用 {{key}} 引用，此处填写的值会在发送时替换。",
                            ),
                    )
                    .child(self.render_kv(&self.path_rows.clone(), false, false, cx))
                    .into_any_element()
            }
            ReqTab::Cookie => self
                .render_kv(&self.cookie_rows.clone(), false, false, cx)
                .into_any_element(),
            ReqTab::Body => {
                let bt = self.body_type_select.clone();
                let lang = self.body_lang_select.clone();
                let body_type = self.body_type;
                let rows = self.body_rows.clone();
                v_flex()
                    .size_full()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(200.))
                                    .child(Select::new(&bt).small().placeholder("Body type")),
                            )
                            .when(body_type == BodyType::Raw, |this| {
                                this.child(
                                    div().w(px(140.)).child(Select::new(&lang).small()),
                                )
                            }),
                    )
                    .child(match body_type {
                        BodyType::None => div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.muted_foreground)
                            .child("该请求没有请求体。")
                            .into_any_element(),
                        BodyType::Raw => {
                            let lang_val = self
                                .body_lang_select
                                .read(cx)
                                .selected_value()
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            let is_json = lang_val == "json";
                            let visual = self.body_visual_mode && is_json;
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                // Toolbar: visual/code toggle (JSON only).
                                .when(is_json, |col| {
                                    col.child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Button::new("body-mode-code")
                                                    .ghost()
                                                    .xsmall()
                                                    .selected(!visual)
                                                    .label("代码编辑")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if this.body_visual_mode {
                                                            // Sync fields → raw before switching.
                                                            this.sync_visual_to_raw(cx);
                                                        }
                                                        this.body_visual_mode = false;
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("body-mode-visual")
                                                    .ghost()
                                                    .xsmall()
                                                    .selected(visual)
                                                    .label("可视化编辑")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if !this.body_visual_mode {
                                                            // Parse raw → fields.
                                                            this.sync_raw_to_visual(cx);
                                                        }
                                                        this.body_visual_mode = true;
                                                        cx.notify();
                                                    })),
                                            )
                                            // Built-in JSON actions for the code editor
                                            // (the visual table has no raw text to format).
                                            .when(!visual, |bar| {
                                                bar.child(
                                                    Button::new("body-json-format")
                                                        .ghost()
                                                        .xsmall()
                                                        .label("格式化")
                                                        .tooltip("美化 JSON 缩进")
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.transform_json_body(
                                                                    false, window, cx,
                                                                );
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    Button::new("body-json-simplify")
                                                        .ghost()
                                                        .xsmall()
                                                        .label("简化")
                                                        .tooltip("每个数组仅保留首项（递归）")
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.transform_json_body(
                                                                    true, window, cx,
                                                                );
                                                            },
                                                        )),
                                                )
                                            }),
                                    )
                                })
                                .child(if visual {
                                    // Visual field table for the parsed JSON.
                                    self.render_raw_visual(cx).into_any_element()
                                } else {
                                    // Plain code editor + JSON validation error.
                                    let raw_text = self.body_editor.read(cx).text().to_string();
                                    let json_error = if is_json && !raw_text.trim().is_empty() {
                                        match serde_json::from_str::<serde_json::Value>(&raw_text) {
                                            Ok(_) => None,
                                            Err(e) => Some(e.to_string()),
                                        }
                                    } else {
                                        None
                                    };
                                    v_flex()
                                        .flex_1()
                                        .min_h_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .child(
                                                    Editor::new(&self.body_editor)
                                                        .h_full()
                                                        .font_family(theme.mono_font_family.clone())
                                                        .text_size(theme.mono_font_size)
                                                        // Apply a red border when JSON is invalid.
                                                        .when_some(json_error.as_ref(), |inp, _| {
                                                            inp.border_color(theme.danger)
                                                        }),
                                                ),
                                        )
                                        // Show JSON parse error below the editor.
                                        .when_some(json_error, |col, err| {
                                            col.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.danger)
                                                    .child(format!("JSON 解析错误: {err}")),
                                            )
                                        })
                                        .into_any_element()
                                })
                                .into_any_element()
                        }
                        BodyType::FormData | BodyType::Urlencoded => self
                            .render_kv(
                                &rows,
                                true,
                                self.body_type == BodyType::FormData,
                                cx,
                            )
                            .into_any_element(),
                    })
                    .into_any_element()
            }
            ReqTab::Auth => self.render_auth_tab(cx).into_any_element(),
            ReqTab::PreRequest => v_flex()
                .size_full()
                .min_h_0()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            "预执行脚本（发送前运行，可设置变量）。apt.setVariable(k,v) · apt.getVariable(k) · apt.echo(...)",
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(
                            Textarea::new(&self.pre_script_editor)
                                .h_full()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.mono_font_size),
                        ),
                )
                .into_any_element(),
            ReqTab::PostRequest => v_flex()
                .size_full()
                .min_h_0()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            "后执行脚本（响应后运行，可读取响应并断言）。response.{status,body,json,headers,time} · apt.assert(cond,msg)",
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(
                            Textarea::new(&self.tests_editor)
                                .h_full()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.mono_font_size),
                        ),
                )
                .into_any_element(),
            ReqTab::Mock => {
                self.render_mock_tab(&theme, cx)
            }
            ReqTab::Curl => {
                let curl = self.generate_curl(cx);
                let curl_for_btn = curl.clone();
                let theme_c = theme.clone();
                v_flex()
                    .size_full()
                    .min_h_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("生成的 curl 命令，可直接复制到终端使用"),
                            )
                            .child(
                                Button::new("copy-curl")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .label("复制")
                                    .tooltip("复制到剪贴板")
                                    .on_click(move |_ev, window, cx: &mut App| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(curl_for_btn.clone()));
                                        window.push_notification(
                                            gpui_component::notification::Notification::new()
                                                .title("已复制")
                                                .message("cURL 命令已复制到剪贴板。")
                                                .autohide(true),
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("curl-code")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_3()
                            .bg(theme_c.muted)
                            .rounded_md()
                            .child(
                                div()
                                    .font_family(theme_c.mono_font_family.clone())
                                    .text_size(theme_c.mono_font_size)
                                    .text_color(theme_c.foreground)
                                    .whitespace_nowrap()                                    .child(curl),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    /// Render the Mock configuration tab.
    pub(super) fn render_mock_tab(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = theme.clone();

        // 如果没有选中任何接口，显示全局引导页面
        if self.request_id.is_none() {
            return v_flex()
                .size_full()
                .p_6()
                .gap_4()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(24.))
                        .child("🧪")
                )
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Mock 服务")
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .text_center()
                        .max_w(px(400.))
                        .child("本地运行的接口模拟服务，支持自定义状态码、延迟、响应内容，帮助你快速开发和测试异常场景。")
                )
                .child(
                    v_flex()
                        .gap_2()
                        .p_4()
                        .w(px(400.))
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.muted)
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("快速开始")
                        )
                        .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("1. 在左侧接口树中选择一个接口"))
                        .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("2. 开启Mock开关，配置状态码、延迟、响应体"))
                        .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("3. 直接在URL中使用 {{mock_server}}/api/path，系统会自动替换为当前Mock服务地址（本地/远程）"))
                        .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("💡 点击左侧活动栏的Mock图标，可以一键为所有接口生成规则、批量模拟异常场景"))
                )
                .into_any_element();
        }

        let enabled = self.mock_enabled;
        let ent = cx.entity();
        let ent_toggle = ent.clone();
        let ent_copy = ent.clone();
        let ent_del1 = ent.clone();
        let ent_add1 = ent.clone();
        let ent_del2 = ent.clone();
        let ent_add2 = ent.clone();
        let ent_del3 = ent.clone();
        let ent_add3 = ent.clone();
        let ent_template = ent.clone();
        let copy_url = format!(
            "http://127.0.0.1:{}",
            crate::share::server::DEFAULT_PORT
        );
        let is_remote = false;

        v_flex()
            .size_full()
            .gap_3()
            .p_2()
            // 全局Mock引导卡片
            .child(
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.primary.opacity(0.3))
                    .bg(theme.primary.opacity(0.05))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("💡 Mock 服务使用指南")
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(if is_remote {
                                format!("当前使用远程Mock服务：{}，开启规则后，你可以：", copy_url)
                            } else {
                                format!("本地Mock服务运行在 {}，开启规则后，你可以：", copy_url)
                            })
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .px_2()
                            .child(div().text_size(px(12.)).child("1. 将前端/测试用例的API基础地址替换为上面的服务地址"))
                            .child(div().text_size(px(12.)).child("2. 配置状态码、延迟、响应体模拟各种异常场景"))
                            .child(div().text_size(px(12.)).child("3. 在左侧Mock活动栏可以一键生成所有规则、批量切换异常场景"))
                    )
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .id("mock-enable-toggle")
                                    .w(px(36.))
                                    .h(px(20.))
                                    .rounded(px(10.))
                                    .flex()
                                    .items_center()
                                    .px(px(2.))
                                    .cursor_pointer()
                                    .when(enabled, |d| d.bg(theme.primary).justify_end())
                                    .when(!enabled, |d| d.bg(theme.muted_foreground.opacity(0.4)).justify_start())
                                    .child(
                                        div()
                                            .w(px(16.))
                                            .h(px(16.))
                                            .rounded_full()
                                            .bg(theme.background),
                                    )
                                    .on_click(move |_, _w, cx: &mut App| {
                                        let _ = ent_toggle.update(cx, |this, cx| {
                                            this.mock_enabled = !this.mock_enabled;
                                            this.commit_to_model(cx);
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("启用 Mock"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child("服务地址:"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_family(theme.mono_font_family.clone())
                                    .child(copy_url.clone()),
                            )
                            .child(
                                Button::new("copy-mock-url")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .tooltip("复制服务地址")
                                    .on_click(move |_, window, cx: &mut App| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_url.clone()));
                                        window.push_notification(
                                            gpui_component::notification::Notification::new()
                                                .title("已复制")
                                                .message("Mock服务地址已复制到剪贴板")
                                                .autohide(true),
                                            cx,
                                        );
                                    }),
                            ),
                    ),
            )
            .when(enabled, |c| {
                c.child(
                    v_flex()
                        .gap_3()
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .w(px(120.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("响应状态码"),
                                        )
                                        .child(Input::new(&self.mock_status_input).small()),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .w(px(120.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("响应延迟(ms)"),
                                        )
                                        .child(Input::new(&self.mock_delay_input).small()),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .w(px(160.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("匹配方法"),
                                        )
                                        .child(Select::new(&self.mock_match_method_select).small().appearance(true)),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("路径匹配模式"),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .w(px(100.))
                                                        .child(Select::new(&self.mock_path_pattern_select).small().appearance(true)),
                                                )
                                                .child(Input::new(&self.mock_match_path_input).small().flex_1()),
                                        ),
                                ),
                        )
                        .child(div().h(px(1.)).w_full().bg(theme.border))
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("响应头"),
                                )
                                .child(
                                    crate::ui::kv_table::KvTable::new(
                                        "mock-headers",
                                        self.mock_headers_rows.clone(),
                                        crate::ui::kv_table::KvHandlers {
                                            on_toggle: Arc::new(|_, _, _, _| {}),
                                            on_delete: Arc::new(move |i, _, cx| {
                                                let _ = ent_del1.update(cx, |this, cx| {
                                                    this.mock_headers_rows.remove(i);
                                                    this.commit_to_model(cx);
                                                    cx.notify();
                                                });
                                            }),
                                            on_add: Arc::new(move |window, cx| {
                                                let _ = ent_add1.update(cx, |this, cx| {
                                                    this.mock_headers_rows.push(KvRow::empty(this.state.clone(), window, cx));
                                                    this.commit_to_model(cx);
                                                    cx.notify();
                                                });
                                            }),
                                            on_type_change: Arc::new(|_, _, _, _| {}),
                                            on_required_toggle: Arc::new(|_, _, _, _| {}),
                                            on_file_pick: Arc::new(|_, _, _| {}),
                                        },
                                    )
                                ),
                        )
                        .child(div().h(px(1.)).w_full().bg(theme.border))
                        .child(
                            v_flex()
                                .gap_1()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("响应体"),
                                )
                                .child(
                                    Editor::new(&self.mock_body_editor)
                                        .flex_1()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_size(theme.mono_font_size),
                                ),
                        )
                        .child(div().h(px(1.)).w_full().bg(theme.border))
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("mock-template-toggle")
                                                .w(px(28.))
                                                .h(px(16.))
                                                .rounded(px(8.))
                                                .flex()
                                                .items_center()
                                                .px(px(2.))
                                                .cursor_pointer()
                                                .when(self.mock_enable_templates, |d| d.bg(theme.primary).justify_end())
                                                .when(!self.mock_enable_templates, |d| d.bg(theme.muted_foreground.opacity(0.4)).justify_start())
                                                .child(
                                                    div()
                                                        .w(px(12.))
                                                        .h(px(12.))
                                                        .rounded_full()
                                                        .bg(theme.background),
                                                )
                                                .on_click(move |_, _w, cx: &mut App| {
                                                    let _ = ent_template.update(cx, |this, cx| {
                                                        this.mock_enable_templates = !this.mock_enable_templates;
                                                        this.commit_to_model(cx);
                                                        cx.notify();
                                                    });
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .child("启用动态模板"),
                                        ),
                                )
                                .when(self.mock_enable_templates, |c| {
                                    c.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child("可用变量: {{mock.request.path}} · {{mock.request.method}} · {{mock.request.query.参数名}} · {{mock.request.header.请求头名}}"),
                                    )
                                }),
                        )
                        .child(div().h(px(1.)).w_full().bg(theme.border))
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("高级匹配条件（可选）"),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("必须匹配的查询参数"),
                                        )
                                        .child(
                                            crate::ui::kv_table::KvTable::new(
                                                "mock-match-query",
                                                self.mock_match_query_rows.clone(),
                                                crate::ui::kv_table::KvHandlers {
                                                    on_toggle: Arc::new(|_, _, _, _| {}),
                                                    on_delete: Arc::new(move |i, _, cx| {
                                                        let _ = ent_del2.update(cx, |this, cx| {
                                                            this.mock_match_query_rows.remove(i);
                                                            this.commit_to_model(cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                                    on_add: Arc::new(move |window, cx| {
                                                        let _ = ent_add2.update(cx, |this, cx| {
                                                            this.mock_match_query_rows.push(KvRow::empty(this.state.clone(), window, cx));
                                                            this.commit_to_model(cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                                    on_type_change: Arc::new(|_, _, _, _| {}),
                                                    on_required_toggle: Arc::new(|_, _, _, _| {}),
                                                    on_file_pick: Arc::new(|_, _, _| {}),
                                                },
                                            )
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("必须匹配的请求头"),
                                        )
                                        .child(
                                            crate::ui::kv_table::KvTable::new(
                                                "mock-match-headers",
                                                self.mock_match_header_rows.clone(),
                                                crate::ui::kv_table::KvHandlers {
                                                    on_toggle: Arc::new(|_, _, _, _| {}),
                                                    on_delete: Arc::new(move |i, _, cx| {
                                                        let _ = ent_del3.update(cx, |this, cx| {
                                                            this.mock_match_header_rows.remove(i);
                                                            this.commit_to_model(cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                                    on_add: Arc::new(move |window, cx| {
                                                        let _ = ent_add3.update(cx, |this, cx| {
                                                            this.mock_match_header_rows.push(KvRow::empty(this.state.clone(), window, cx));
                                                            this.commit_to_model(cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                                    on_type_change: Arc::new(|_, _, _, _| {}),
                                                    on_required_toggle: Arc::new(|_, _, _, _| {}),
                                                    on_file_pick: Arc::new(|_, _, _| {}),
                                                },
                                            )
                                        ),
                                ),
                        ),
                )
            })
            .when(!enabled, |c| {
                c.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("💡 开启后，本地Mock服务会按照上方配置返回模拟响应，支持自定义状态码、延迟、响应内容，方便你测试前端异常处理逻辑。"),
                )
            })
            .into_any_element()
    }

    /// Render the 认证 (Auth) tab: a type selector + per-type inputs.
    pub(super) fn render_auth_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let auth_type = self.auth_type;
        // Field label + input row helper.
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(Input::new(input).small())
        };

        v_flex()
            .size_full()
            .gap_3()
            .p_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(180.))
                            .child(Select::new(&self.auth_type_select).small().appearance(true)),
                    )
                    .when(auth_type == AuthType::ApiKey, |row| {
                        row.child(
                            div().w(px(140.)).child(
                                Select::new(&self.auth_target_select)
                                    .small()
                                    .appearance(true),
                            ),
                        )
                    }),
            )
            .child(match auth_type {
                AuthType::None => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child("该请求不需要认证。")
                    .into_any_element(),
                AuthType::Bearer => h_flex()
                    .child(field("Token", &self.auth_token))
                    .into_any_element(),
                AuthType::Basic => h_flex()
                    .gap_2()
                    .child(field("用户名", &self.auth_username))
                    .child(field("密码", &self.auth_password))
                    .into_any_element(),
                AuthType::ApiKey => h_flex()
                    .gap_2()
                    .child(field("Key", &self.auth_key))
                    .child(field("Value", &self.auth_value))
                    .into_any_element(),
            })
    }
}
