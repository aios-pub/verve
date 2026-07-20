//! Kubernetes panel — the full-bleed "K8s" exclusive view.
//!
//! Shows the current kubeconfig context/cluster and lets users switch context
//! (calls `kubectl config use-context <name>`). Future iterations (tasks 18-20)
//! will add pod lists, logs, exec, and port-forward by shelling out to kubectl.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _};

use crate::k8s::{kubeconfig_path, load_default, KubeConfig};

pub struct K8sPanel {
    cfg: KubeConfig,
    error: Option<String>,
}

impl K8sPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            cfg: load_default(),
            error: None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.cfg = load_default();
        self.error = None;
        cx.notify();
    }

    fn switch_context(&mut self, name: String, cx: &mut Context<Self>) {
        let out = std::process::Command::new("kubectl")
            .args(["config", "use-context", &name])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                self.refresh(cx);
            }
            Ok(o) => {
                self.error = Some(format!(
                    "kubectl: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ));
                cx.notify();
            }
            Err(e) => {
                self.error = Some(format!("执行 kubectl 失败: {e}. 请确认 kubectl 已安装。"));
                cx.notify();
            }
        }
    }
}

impl Render for K8sPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;

        let active_ctx = self.cfg.active_context().cloned();
        let active_server = active_ctx
            .as_ref()
            .and_then(|c| self.cfg.cluster_for(&c.name))
            .map(|cl| cl.server.clone())
            .unwrap_or_else(|| "(未配置)".into());
        let active_ns = active_ctx
            .as_ref()
            .and_then(|c| c.context.namespace.clone())
            .unwrap_or_else(|| "default".into());
        let active_name = self.cfg.current_context.clone();

        let error_msg = self.error.clone();

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(44.))
                    .px_4()
                    .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Kubernetes"))
                    .child(div().flex_1())
                    .child(
                        Button::new("k8s-refresh")
                            .ghost()
                            .small()
                            .icon(IconName::Loader)
                            .label("刷新")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .id("k8s-scroll")
                    .overflow_y_scroll()
                    .p_4()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_size(px(12.)).text_color(muted).child("当前 Context"))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if active_name.is_empty() { "(无)".into() } else { active_name }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_6()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_size(px(11.)).text_color(muted).child("Server"))
                                    .child(div().text_size(px(12.)).child(active_server)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_size(px(11.)).text_color(muted).child("Namespace"))
                                    .child(div().text_size(px(12.)).child(active_ns)),
                            ),
                    )
                    .children(error_msg.map(|e| {
                        div().text_size(px(12.)).text_color(theme.danger).child(e)
                    }))
                    .child(div().h(px(1.)).w_full().bg(border))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("可用 Context (点击切换)"),
                    )
                    .child(if self.cfg.contexts.is_empty() {
                        div().text_size(px(12.)).text_color(muted).child(format!(
                            "未找到 kubeconfig ({}). 请先配置 kubectl。",
                            crate::k8s::kubeconfig_path().display()
                        ))
                        .into_any_element()
                    } else {
                        v_flex()
                            .gap_1()
                            .children(self.cfg.contexts.iter().map(|c| {
                                let is_active = c.name == self.cfg.current_context;
                                let name = c.name.clone();
                                let server = self
                                    .cfg
                                    .cluster_for(&c.name)
                                    .map(|cl| cl.server.clone())
                                    .unwrap_or_default();
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .id(c.name.clone())
                                    .p_2()
                                    .border_1()
                                    .border_color(border)
                                    .rounded_md()
                                    .bg(if is_active { theme.accent.opacity(0.15) } else { Hsla::transparent_black() })
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(if is_active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                                            .flex_1()
                                            .child(c.name.clone()),
                                    )
                                    .child(div().text_size(px(11.)).text_color(muted).child(server))
                                    .when(!is_active, |d| {
                                        d.child(
                                            Button::new(format!("k8s-switch-{}", c.name))
                                                .xsmall()
                                                .ghost()
                                                .label("切换")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.switch_context(name.clone(), cx);
                                                })),
                                        )
                                    })
                            }))
                            .into_any_element()
                    }),
            )
    }
}
