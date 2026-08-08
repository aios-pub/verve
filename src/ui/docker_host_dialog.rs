//! Docker host configuration dialog (new / edit).
//!
//! Collects name, connection mode (direct DOCKER_HOST URL or tunnel through
//! an existing Verve SSH host), TLS verify, and cert path. Saved hosts are
//! persisted to `~/.verve/docker_hosts.json`.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::select::{Select, SelectState};
use gpui_component::{ActiveTheme, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};

use crate::docker::{DockerHost, host_store};
use crate::ssh::host_store as ssh_host_store;

/// Connection mode selected in the dialog.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnMode {
    /// User enters a DOCKER_HOST URL directly (tcp://, https://, unix://).
    Direct,
    /// Tunnel through an existing Verve-managed SSH host.
    SshTunnel,
}

impl ConnMode {
    fn label(&self) -> &'static str {
        match self {
            ConnMode::Direct => "自定义 DOCKER_HOST URL",
            ConnMode::SshTunnel => "通过 Verve SSH 主机",
        }
    }
}

/// Build the pick-list item for an SSH host: "name — username@host:port".
fn ssh_host_label(name: &str, username: &str, host: &str, port: u16) -> String {
    format!("{name} — {username}@{host}:{port}")
}

/// Mutable state backing the dialog.
pub struct DockerHostDialogState {
    pub editing_id: Option<String>,
    pub name: Entity<InputState>,
    // Mode-specific fields.
    pub(crate) mode: ConnMode,
    pub host_url: Entity<InputState>,
    pub ssh_host_select: Entity<SelectState<Vec<String>>>,
    /// Map from select label -> ssh host id (built on construction).
    pub ssh_label_to_id: HashMap<String, String>,
    pub tls_verify: bool,
    pub cert_path: Entity<InputState>,
}

impl DockerHostDialogState {
    pub fn new(editing: Option<&DockerHost>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let existing = editing.cloned();

        // Determine initial mode from the host being edited.
        let (init_mode, init_ssh_id, init_url) = match &existing {
            Some(h) if h.ssh_host_id.is_some() => {
                (ConnMode::SshTunnel, h.ssh_host_id.clone(), String::new())
            }
            Some(h) => (ConnMode::Direct, None, h.host.clone()),
            None => (ConnMode::Direct, None, String::new()),
        };

        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("名称, 如 生产环境");
            if let Some(e) = &existing {
                s.set_value(&e.name, window, cx);
            }
            s
        });
        let host_url = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("tcp://host:2376, https://host:2376 或 unix:///path");
            if !init_url.is_empty() {
                s.set_value(&init_url, window, cx);
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

        // Load SSH hosts for the picker.
        let ssh_hosts = ssh_host_store::load_hosts();
        let mut label_to_id = HashMap::new();
        let mut options: Vec<String> = Vec::new();
        for h in &ssh_hosts {
            let label = ssh_host_label(&h.name, &h.username, &h.host, h.port);
            label_to_id.insert(label.clone(), h.id.clone());
            options.push(label);
        }
        // Find initial selected label by id (if editing an ssh-mode host).
        let init_label = init_ssh_id.and_then(|id| {
            ssh_hosts
                .iter()
                .find(|h| h.id == id)
                .map(|h| ssh_host_label(&h.name, &h.username, &h.host, h.port))
        });
        let initial_index = init_label
            .as_deref()
            .and_then(|lbl| options.iter().position(|s| s == lbl))
            .unwrap_or(0);
        let ssh_host_select = cx.new(|cx| {
            SelectState::new(
                options,
                Some(gpui_component::IndexPath::new(initial_index)),
                window,
                cx,
            )
        });
        if let Some(lbl) = init_label {
            ssh_host_select.update(cx, |s, cx| {
                s.set_selected_value(&lbl, window, cx);
            });
        }

        let tls_verify = existing.as_ref().map(|e| e.tls_verify).unwrap_or(false);

        cx.new(|_cx| Self {
            editing_id: existing.map(|e| e.id),
            name,
            mode: init_mode,
            host_url,
            ssh_host_select,
            ssh_label_to_id: label_to_id,
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
pub fn open_dialog<F>(editing: Option<&DockerHost>, on_save: F, window: &mut Window, cx: &mut App)
where
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
        let on_save = on_save.clone();

        dialog
            .title(title_text)
            .w(px(600.))
            .content(move |content, window, cx| {
                let s = state_for_content.read(cx);
                let tls_verify = s.tls_verify;
                let mode = s.mode;
                let ssh_hosts_empty = s.ssh_label_to_id.is_empty();
                let body = v_flex()
                    .p_4()
                    .w_full()
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
                    // Connection mode toggle
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("连接方式"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("docker-mode-direct")
                                            .ghost()
                                            .small()
                                            .label(ConnMode::Direct.label())
                                            .selected(mode == ConnMode::Direct)
                                            .when(mode == ConnMode::Direct, |b| {
                                                b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground)
                                            })
                                            .on_click({
                                                let state = state_for_content.clone();
                                                move |_ev, _window, cx: &mut App| {
                                                    let _ = state.update(cx, |s, cx| {
                                                        s.mode = ConnMode::Direct;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    )
                                    .child(
                                        Button::new("docker-mode-ssh")
                                            .ghost()
                                            .small()
                                            .label(ConnMode::SshTunnel.label())
                                            .selected(mode == ConnMode::SshTunnel)
                                            .when(mode == ConnMode::SshTunnel, |b| {
                                                b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground)
                                            })
                                            .on_click({
                                                let state = state_for_content.clone();
                                                move |_ev, _window, cx: &mut App| {
                                                    let _ = state.update(cx, |s, cx| {
                                                        s.mode = ConnMode::SshTunnel;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    ),
                            ),
                    )
                    // Mode-specific content
                    .child(match mode {
                        ConnMode::Direct => v_flex()
                            .gap_1()
                            .child(Input::new(&s.host_url).small())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("例如: tcp://192.168.1.10:2376, unix:///var/run/docker.sock (SSH 连接请用「SSH 隧道」模式)"),
                            )
                            .into_any_element(),
                        ConnMode::SshTunnel => {
                            if ssh_hosts_empty {
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.danger)
                                            .child("尚未配置任何 Verve SSH 主机。请先在 \"主机\" 面板添加 SSH 主机。"),
                                    )
                                    .into_any_element()
                            } else {
                                v_flex()
                                    .gap_1()
                                    .child(Select::new(&s.ssh_host_select).small())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child("通过 Verve 建立 SSH 隧道转发远端 /var/run/docker.sock，复用 Verve 保存的凭证。"),
                                    )
                                    .into_any_element()
                            }
                        }
                    })
                    // TLS verify toggle (only relevant for direct tcp://)
                    .when(mode == ConnMode::Direct, |b| {
                        b.child(
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
                    })
                    // Cert path
                    .when(mode == ConnMode::Direct, |b| {
                        b.child(
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
                        )
                    });
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

                        let (host_val, ssh_host_id) = match s.mode {
                            ConnMode::SshTunnel => {
                                let selected_label = s.ssh_host_select.read(cx).selected_value();
                                let Some(label) = selected_label else { return };
                                let Some(id) = s.ssh_label_to_id.get(label.as_str()) else { return };
                                (String::new(), Some(id.clone()))
                            }
                            ConnMode::Direct => {
                                let url = s.host_url.read(cx).value().to_string();
                                if url.trim().is_empty() {
                                    return;
                                }
                                (url, None)
                            }
                        };

                        let cert_val = s.cert_path.read(cx).value().to_string();

                        let name = if name_val.trim().is_empty() {
                            if s.mode == ConnMode::SshTunnel {
                                "SSH Docker".to_string()
                            } else {
                                host_val.clone()
                            }
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
                        host.ssh_host_id = ssh_host_id;
                        host.tls_verify = s.tls_verify;
                        host.cert_path = if cert_val.trim().is_empty() || s.mode == ConnMode::SshTunnel {
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
