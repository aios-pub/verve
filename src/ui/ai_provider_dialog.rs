//! Provider edit dialog — its own OS window with an explicit 保存 button.
//!
//! Opened from the AI settings section's provider list (click a row) or right
//! after adding a provider from a preset. All fields are drafts until 保存
//! writes them to `ai_providers.json` in one atomic pass; 取消/关闭 discards.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::select::{Select, SelectState};
use gpui_component::{
    ActiveTheme, Disableable as _, IconName, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::ai::config::{self, AiProviderConfig, PriceUnit};

/// Connectivity-test state for the dialog.
#[derive(Clone, Debug, Default)]
enum TestState {
    #[default]
    Idle,
    Testing,
    Ok(String),
    Err(String),
}

/// Price-unit dropdown options (order maps to `PriceUnit::{Per1M, Per1K}`).
fn unit_options() -> Vec<String> {
    vec!["每 1M tokens".to_string(), "每 1K tokens".to_string()]
}

pub struct AiProviderDialog {
    /// Existing provider id (edit mode); `None` for a brand-new draft.
    provider_id: Option<String>,
    name: Entity<InputState>,
    base_url: Entity<InputState>,
    model: Entity<InputState>,
    /// All model ids, one per line (multi-line input).
    models: Entity<InputState>,
    api_key: Entity<InputState>,
    input_price: Entity<InputState>,
    output_price: Entity<InputState>,
    currency: Entity<InputState>,
    unit_select: Entity<SelectState<Vec<String>>>,
    enabled: bool,
    supports_vision: bool,
    set_default: bool,
    key_was_set: bool,
    test: TestState,
}

impl AiProviderDialog {
    /// Open the dialog window for an existing provider (by id) or a preset
    /// draft (`provider` = None + preset filled for new).
    pub fn open(
        provider: Option<&AiProviderConfig>,
        active_provider_id: Option<&str>,
        cx: &mut App,
    ) {
        let window_open = cx.open_window(
            {
                let bounds = crate::ui::app::centered_window_bounds(px(560.), px(660.), cx);
                let mut options = gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                    window_min_size: Some(gpui::size(px(480.), px(540.))),
                    titlebar: None,
                    is_minimizable: true,
                    ..Default::default()
                };
                crate::ui::app::apply_window_chrome_fixes(&mut options, true);
                options
            },
            |window, cx| {
                let view = cx.new(|cx| {
                    Self::new(provider, active_provider_id, window, cx)
                });
                cx.new(|cx| {
                    gpui_component::Root::new(view, window, cx).bg(cx.theme().background)
                })
            },
        );
        if let Ok(handle) = window_open {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
    }

    fn new(
        provider: Option<&AiProviderConfig>,
        active_provider_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let draft = provider.cloned();
        let name = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                draft.as_ref().map(|p| p.name.as_str()).unwrap_or_default(),
            )
        });
        let base_url = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                draft.as_ref().map(|p| p.base_url.as_str()).unwrap_or_default(),
            )
        });
        let model = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                draft.as_ref().map(|p| p.model.as_str()).unwrap_or_default(),
            )
        });
        let models_init = draft
            .as_ref()
            .map(|p| {
                if p.models.is_empty() {
                    p.model.clone()
                } else {
                    p.models.join("\n")
                }
            })
            .unwrap_or_default();
        let models = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(models_init)
                .multi_line(true)
                .auto_grow(3, 8)
        });
        let key_was_set = draft.as_ref().map(|p| p.api_key_set).unwrap_or(false);
        let api_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(if key_was_set { "已设置（输入以更换）" } else { "API Key" }.to_string())
        });
        let input_price = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                draft
                    .as_ref()
                    .and_then(|p| p.input_price)
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            )
        });
        let output_price = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                draft
                    .as_ref()
                    .and_then(|p| p.output_price)
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            )
        });
        let currency = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                draft
                    .as_ref()
                    .and_then(|p| p.currency.clone())
                    .unwrap_or_default(),
            )
        });
        let unit_idx = match draft.as_ref().map(|p| p.price_unit) {
            Some(PriceUnit::Per1K) => 1,
            _ => 0,
        };
        let unit_select = cx.new(|cx| {
            SelectState::new(
                unit_options(),
                Some(gpui_component::IndexPath::new(unit_idx)),
                window,
                cx,
            )
        });
        Self {
            provider_id: provider.map(|p| p.id.clone()),
            name,
            base_url,
            model,
            models,
            api_key,
            input_price,
            output_price,
            currency,
            unit_select,
            enabled: draft.as_ref().map(|p| p.enabled).unwrap_or(true),
            supports_vision: draft.as_ref().map(|p| p.supports_vision).unwrap_or(false),
            set_default: match (provider, active_provider_id) {
                (Some(p), Some(active)) => p.id == active,
                (None, _) => true,
                _ => false,
            },
            key_was_set,
            test: TestState::Idle,
        }
    }

    /// Assemble the draft provider from the dialog fields.
    fn collect(&self, cx: &App) -> AiProviderConfig {
        let unit = match self.unit_select.read(cx).selected_index(cx).map(|i| i.row) {
            Some(1) => PriceUnit::Per1K,
            _ => PriceUnit::Per1M,
        };
        AiProviderConfig {
            id: self
                .provider_id
                .clone()
                .unwrap_or_else(crate::state::models::new_id),
            name: self.name.read(cx).value().trim().to_string(),
            base_url: self.base_url.read(cx).value().trim().to_string(),
            model: self.model.read(cx).value().trim().to_string(),
            models: {
                let mut list: Vec<String> = self
                    .models
                    .read(cx)
                    .value()
                    .split([ '\n', ',', '，', ';', '；' ])
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .map(str::to_string)
                    .collect();
                list.dedup();
                // The active model must always be selectable.
                let active = self.model.read(cx).value().trim().to_string();
                if !active.is_empty() && !list.contains(&active) {
                    list.push(active);
                }
                list
            },
            enabled: self.enabled,
            supports_vision: self.supports_vision,
            api_key_set: self.key_was_set
                || !self.api_key.read(cx).value().trim().is_empty(),
            input_price: self.input_price.read(cx).value().trim().parse::<f64>().ok(),
            output_price: self.output_price.read(cx).value().trim().parse::<f64>().ok(),
            price_unit: unit,
            currency: {
                let c = self.currency.read(cx).value().trim().to_string();
                if c.is_empty() { None } else { Some(c) }
            },
        }
    }

    /// 保存: upsert the draft into settings, store the key, close.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.collect(cx);
        if draft.name.is_empty() {
            self.test = TestState::Err("名称不能为空".into());
            cx.notify();
            return;
        }
        let key = self.api_key.read(cx).value().trim().to_string();
        if !key.is_empty() {
            config::store_provider_api_key(&draft.id, &key);
        }
        let mut settings = config::load_ai_settings();
        match settings
            .providers
            .iter_mut()
            .find(|p| p.id == draft.id)
        {
            Some(existing) => *existing = draft.clone(),
            None => settings.providers.push(draft.clone()),
        }
        if self.set_default || settings.active_provider_id.is_none() {
            settings.active_provider_id = Some(draft.id);
        }
        if let Err(e) = config::save_ai_settings(&settings) {
            log::error!("save ai provider: {e}");
            self.test = TestState::Err(format!("保存失败: {e}"));
            cx.notify();
            return;
        }
        window.remove_window();
    }

    /// Persist the capability flags immediately (edit mode only). Toggles
    /// look instant — requiring a separate 保存 click loses them on
    /// restart, which is exactly what users don't expect.
    fn autosave_capabilities(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.provider_id.clone() else {
            // Add mode: the provider doesn't exist yet; 保存 creates it.
            return;
        };
        let mut settings = config::load_ai_settings();
        let Some(p) = settings.providers.iter_mut().find(|p| p.id == id) else {
            return;
        };
        p.enabled = self.enabled;
        p.supports_vision = self.supports_vision;
        let label = if self.set_default {
            settings.active_provider_id = Some(id.clone());
            "已保存（并设为默认）"
        } else {
            "已自动保存 ✓"
        };
        if let Err(e) = config::save_ai_settings(&settings) {
            log::error!("autosave capabilities: {e}");
            self.test = TestState::Err(format!("自动保存失败: {e}"));
        } else {
            self.test = TestState::Ok(label.into());
        }
        cx.notify();
    }

    /// 删除 the provider (edit mode only) and close.
    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.provider_id.clone() else {
            window.remove_window();
            return;
        };
        config::delete_provider_api_key(&id);
        let mut settings = config::load_ai_settings();
        settings.providers.retain(|p| p.id != id);
        if settings.active_provider_id.as_deref() == Some(id.as_str()) {
            settings.active_provider_id = settings.providers.first().map(|p| p.id.clone());
        }
        let _ = config::save_ai_settings(&settings);
        window.remove_window();
    }

    /// Clear the stored key (keeps the provider).
    fn clear_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.provider_id.clone() {
            config::delete_provider_api_key(&id);
        }
        self.key_was_set = false;
        self.api_key
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.test = TestState::Ok("已清除密钥".into());
        cx.notify();
    }

    /// 测试连接 using the CURRENT dialog draft (no save needed first).
    fn test_connection(&mut self, cx: &mut Context<Self>) {
        if matches!(self.test, TestState::Testing) {
            return;
        }
        let draft = self.collect(cx);
        let key = self.api_key.read(cx).value().trim().to_string();
        if !key.is_empty() {
            config::store_provider_api_key(&draft.id, &key);
        }
        let Ok(client) = crate::ai::client::AiClient::for_provider(&draft, cx) else {
            self.test = TestState::Err("构造客户端失败".into());
            cx.notify();
            return;
        };
        self.test = TestState::Testing;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.ping().await;
            let _ = this.update_in(cx, |this, _window, cx| {
                this.test = match result {
                    Ok(u) => TestState::Ok(format!(
                        "OK · 输入 {} tok · 输出 {} tok",
                        u.prompt_tokens, u.completion_tokens
                    )),
                    Err(e) => TestState::Err(format!("{e}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_chip(
        &self,
        id: &'static str,
        label: &str,
        on: bool,
        cx: &mut Context<Self>,
        toggle: fn(&mut Self),
    ) -> AnyElement {
        let theme = cx.theme();
        let accent = theme.accent;
        let border = theme.border;
        let fg = theme.foreground;
        let mf = theme.muted_foreground;
        div()
            .id(id)
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .when(on, move |d| d.border_color(accent).bg(accent.opacity(0.1)))
            .when(!on, move |d| d.border_color(border))
            .child(
                div()
                    .text_sm()
                    .text_color(if on { fg } else { mf })
                    .child(label.to_string()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(if on { fg } else { mf })
                    .child(if on { "✓" } else { "—" }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this);
                // Capability toggles look instant, so they must BE instant:
                // persist them right away in edit mode (a restart must not
                // lose them). Text fields still follow the explicit 保存.
                this.autosave_capabilities(cx);
            }))
            .into_any_element()
    }
}

impl Render for AiProviderDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let field = |state: &Entity<InputState>, label: &str| {
            v_flex()
                .gap_1()
                .child(div().text_sm().text_color(muted).child(label.to_string()))
                .child(Input::new(state).small())
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                TitleBar::new()
                    .h(px(38.))
                    .pl_3()
                    .bg(theme.muted)
                    .border_color(theme.border)
                    .on_close_window(|_, window, _cx| window.remove_window())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("供应商设置"),
                    ),
            )
            .child(
                v_flex()
                    .id("ai-provider-dialog-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_3()
                    .child(field(&self.name, "名称"))
                    .child(field(&self.base_url, "Base URL（OpenAI 兼容）"))
                    .child(field(&self.model, "模型（当前使用，对话中可切换）"))
                    .child(field(&self.models, "模型列表（每行一个；文本/图像/视频模型均可填，切换即时生效）"))
                    .child(field(&self.api_key, "API Key（加密存储于本地保险库）"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("clear-key")
                                    .ghost()
                                    .xsmall()
                                    .label("清除已存密钥")
                                    .disabled(!self.key_was_set)
                                    .on_click(cx.listener(|this, _, window, cx| this.clear_key(window, cx))),
                            ),
                    )
                    // Pricing.
                    .child(
                        h_flex()
                            .gap_3()
                            .items_end()
                            .flex_wrap()
                            .child(field(&self.input_price, "输入价格（可选）"))
                            .child(field(&self.output_price, "输出价格（可选）"))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .w(px(150.))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(muted)
                                            .child("计价单位"),
                                    )
                                    .child(
                                        Select::new(&self.unit_select).small().appearance(true),
                                    ),
                            )
                            .child(field(&self.currency, "币种（如 ¥/$）")),
                    )
                    // Capability / default toggles.
                    .child(
                        h_flex().gap_2().flex_wrap().children([
                            self.toggle_chip("dlg-enabled", "启用", self.enabled, cx, |s| {
                                s.enabled = !s.enabled
                            }),
                            self.toggle_chip(
                                "dlg-vision",
                                "视觉输入",
                                self.supports_vision,
                                cx,
                                |s| s.supports_vision = !s.supports_vision,
                            ),
                            self.toggle_chip(
                                "dlg-default",
                                "设为默认供应商",
                                self.set_default,
                                cx,
                                |s| s.set_default = !s.set_default,
                            ),
                        ]),
                    )
                    // Test connection.
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("dlg-test")
                                    .small()
                                    .label(
                                        if matches!(self.test, TestState::Testing) {
                                            "测试中…"
                                        } else {
                                            "测试连接"
                                        },
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.test_connection(cx)
                                    })),
                            )
                            .when_some(
                                match &self.test {
                                    TestState::Idle => None,
                                    TestState::Testing => {
                                        Some((muted, "测试中…".to_string()))
                                    }
                                    TestState::Ok(msg) => {
                                        Some((theme.success, format!("✓ {msg}")))
                                    }
                                    TestState::Err(e) => {
                                        Some((theme.danger, format!("❌ {e}")))
                                    }
                                },
                                |bar, (color, text)| {
                                    bar.child(div().text_sm().text_color(color).child(text))
                                },
                            ),
                    ),
            )
            // Footer: explicit save.
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .gap_2()
                    .justify_end()
                    .when(self.provider_id.is_some(), |bar| {
                        bar.child(
                            Button::new("dlg-delete")
                                .danger()
                                .small()
                                .label("删除供应商")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.delete(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("dlg-cancel")
                            .small()
                            .label("取消")
                            .on_click(|_, window, _cx| window.remove_window()),
                    )
                    .child(
                        Button::new("dlg-save")
                            .primary()
                            .small()
                            .label("保存")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save(window, cx)
                            })),
                    ),
            )
    }
}
