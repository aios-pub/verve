//! Docker host configuration dialog (new / edit).
//!
//! Collects name, DOCKER_HOST endpoint, TLS verify, and cert path. Saved hosts
//! are persisted to `~/.verve/docker_hosts.json`.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Selectable as _, Sizable as _, WindowExt as _};

use crate::docker::{host_store, DockerHost};

/// Mutable state backing the dialog.
pub struct DockerHostDialogState {
    pub editing_id: Option<String>,
    pub name: Entity<InputState>,
    pub host: Entity<InputState>,
    pub tls_verify: bool,
    pub cert_path: Entity<InputState>,
}

impl DockerHostDialogState {
    pub fn new(
        editing: Option<&DockerHost>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let existing = editing.cloned();
        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("名称, 如 生产环境");
            if let Some(e) = &existing {
                s.set_value(&e.name, window, cx);
            }
            s
        });
        let host = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("tcp://host:2376 或 ssh://user@host 或 unix:///path");
            if let Some(e) = &existing {
                s.set_value(&e.host, window, cx);
            }
            s
        });
        let cert_path = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("证书目录 (可选)");
            if let Some(e) = &existing {
                if let Some(cp) = &e.cert_path {
                    s.set_value(cp, window, cx);
                }
            }
            s
        });
        let tls_verify = existing.as_ref().map(|e| e.tls_verify).unwrap_or(false);
        cx.new(|_cx| Self {
            editing_id: existing.map(|e| e.id),
            name,
            host,
            tls_verify,
            cert_path,
        })
    }
}

impl Render for DockerHostDialogState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
    }
}

/// Open the host dialog. `on_save` receives the finalized `DockerHost`.
pub fn open_dialog<F>(
    editing: Option<&DockerHost>,
    on_save: F,
    window: &mut Window,
    cx: &mut App,
) where
    F: Fn(DockerHost, &mut Window, &mut App) + 'static,
{
    let on_save: std::sync::Arc<F> = std::sync::Arc::new(on_save);
    let state = DockerHostDialogState::new(editing, window, cx);
    let title_text = if editing.is_some() {
        "编辑 Docker 主机"
    } else {
        "添加 Docker 主机"
    };
    window.open_dialog(cx, move |dialog, _window, cx| {
        let state_for_content = state.clone();
        let state_for_footer = state.clone();
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let on_save = on_save.clone();

        dialog
            .title(title_text)
            .content(move |content, _window, cx| {
                let s = state_for_content.read(cx);
                let tls_verify = s.tls_verify;
                let body = v_flex()
                    .p_4()
                    .w(px(480.))
                    .gap_3()
                    // Name
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("名称"),
                            )
                            .child(Input::new(&s.name).small()),
                    )
                    // Host endpoint
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Host 地址 (DOCKER_HOST)"),
                            )
                            .child(Input::new(&s.host).small())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("例如: tcp://192.168.1.10:2376, ssh://user@server, unix:///var/run/docker.sock"),
                            ),
                    )
                    // TLS verify toggle
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("docker-tls-toggle")
                                    .ghost()
                                    .small()
                                    .label(if tls_verify { "TLS 验证: 开启" } else { "TLS 验证: 关闭" })
                                    .selected(tls_verify)
                                    .when(tls_verify, |b| {
                                        b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground)
                                    })
                                    .on_click({
                                        let state = state_for_content.clone();
                                        move |_ev, _window, cx: &mut App| {
                                            let _ = state.update(cx, |s, cx| {
                                                s.tls_verify = !s.tls_verify;
                                                cx.notify();
                                            });
                                        }
                                    }),
                            ),
                    )
                    // Cert path (shown when TLS is on, but always editable)
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("证书路径 (DOCKER_CERT_PATH, 可选)"),
                            )
                            .child(Input::new(&s.cert_path).small())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("包含 ca.pem / cert.pem / key.pem 的目录"),
                            ),
                    );
                content.child(body)
            })
            .footer({
                let on_save = on_save.clone();
                let state = state_for_footer.clone();
                Button::new("docker-host-save")
                    .primary()
                    .small()
                    .label("保存")
                    .on_click(move |_ev, window, cx| {
                        let s = state.read(cx);
                        let name_val = s.name.read(cx).value().to_string();
                        let host_val = s.host.read(cx).value().to_string();
                        let cert_val = s.cert_path.read(cx).value().to_string();

                        if host_val.trim().is_empty() {
                            return;
                        }

                        let name = if name_val.trim().is_empty() {
                            host_val.clone()
                        } else {
                            name_val
                        };

                        let mut host = if let Some(id) = &s.editing_id {
                            host_store::find_host(id)
                                .unwrap_or_else(|| DockerHost::new(&name, &host_val))
                        } else {
                            DockerHost::new(&name, &host_val)
                        };
                        host.name = name;
                        host.host = host_val;
                        host.tls_verify = s.tls_verify;
                        host.cert_path = if cert_val.trim().is_empty() {
                            None
                        } else {
                            Some(cert_val)
                        };

                        host_store::upsert_host(host.clone());

                        (*on_save.clone())(host, window, cx);
                        window.close_dialog(cx);
                    })
            })
    });
}
