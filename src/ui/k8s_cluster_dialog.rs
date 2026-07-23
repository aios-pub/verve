//! Dialog to add/edit a Kubernetes cluster.
//!
//! Provides two tabs:
//! 1. **导入 KubeConfig** - Select kubeconfig file or paste content, then select a context to import
//! 2. **手动配置** - Enter API URL + credentials manually
//!
//! Both paths support connection mode selection: Direct or SSH tunnel.
//! SSH tunnel mode can be toggled at any time without re-importing config.
//! insecure-skip-tls-verify skips ALL certificate validation (CA + hostname).

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::select::{Select, SelectState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Selectable as _, Sizable as _, WindowExt as _};

use crate::k8s::clusters::{
    cluster_from_kubeconfig, parse_kubeconfig_contexts, upsert_manual_cluster, ClusterAuth,
    ClusterConnectionType, KubeconfigContextInfo, ManualCluster,
};
use crate::ssh::host_store as ssh_host_store;

/// Which tab is active in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DialogTab {
    #[default]
    ImportKubeconfig,
    ManualEntry,
}

impl DialogTab {
    fn label(self) -> &'static str {
        match self {
            DialogTab::ImportKubeconfig => "导入 KubeConfig",
            DialogTab::ManualEntry => "手动配置",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthKind {
    #[default]
    Token,
    ClientCert,
    BasicAuth,
}

impl AuthKind {
    fn label(self) -> &'static str {
        match self {
            AuthKind::Token => "Token",
            AuthKind::ClientCert => "客户端证书",
            AuthKind::BasicAuth => "用户名/密码",
        }
    }
    const ALL: &'static [AuthKind] = &[AuthKind::Token, AuthKind::ClientCert, AuthKind::BasicAuth];
}

/// Build the pick-list item for an SSH host: "name — username@host:port".
fn ssh_host_label(name: &str, username: &str, host: &str, port: u16) -> String {
    format!("{name} — {username}@{host}:{port}")
}

pub struct K8sClusterDialogState {
    pub editing_id: Option<String>,
    // When editing an existing cluster, store its original data for edit flow
    editing_cluster: Option<ManualCluster>,
    // True if this cluster was imported from kubeconfig (has raw_kubeconfig),
    // so we hide auth fields (credentials come from imported config, can't be edited here)
    is_imported_cluster: bool,
    // --- Tab selection ---
    pub active_tab: DialogTab,
    // --- Common fields ---
    pub name: Entity<InputState>,
    pub namespace: Entity<InputState>,
    pub insecure: bool,
    // --- Connection mode ---
    pub connection_type: ClusterConnectionType,
    pub ssh_host_select: Entity<SelectState<Vec<String>>>,
    /// Map from select label -> ssh host id (built on construction).
    pub ssh_label_to_id: HashMap<String, String>,
    // --- Tab 1: Import kubeconfig ---
    pub kubeconfig_text: Entity<InputState>,
    pub parsed_contexts: Vec<KubeconfigContextInfo>,
    pub selected_context: Option<usize>,
    // --- Tab 2: Manual entry ---
    pub server: Entity<InputState>,
    pub auth_kind: AuthKind,
    pub token: Entity<InputState>,
    pub cert_data: Entity<InputState>,
    pub key_data: Entity<InputState>,
    pub username: Entity<InputState>,
    pub password: Entity<InputState>,
    /// Validation error message (displayed at top of form when non-empty).
    pub error: Option<String>,
}

impl K8sClusterDialogState {
    pub fn new(
        editing: Option<&ManualCluster>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let existing = editing.cloned();
        let is_editing = existing.is_some();
        let is_imported = existing.as_ref().map(|e| e.raw_kubeconfig.is_some()).unwrap_or(false);

        // Determine initial tab: new → import tab; editing → manual tab (simplified view with conn mode switch)
        let init_tab = DialogTab::ManualEntry;

        // Determine initial auth kind from existing cluster
        let (init_auth_kind, init_token, init_cert, init_key, init_user, init_pass) = match &existing {
            Some(c) => match &c.auth {
                ClusterAuth::Token { token } => (AuthKind::Token, token.clone(), String::new(), String::new(), String::new(), String::new()),
                ClusterAuth::ClientCert { cert_data, key_data } => (AuthKind::ClientCert, String::new(), cert_data.clone(), key_data.clone(), String::new(), String::new()),
                ClusterAuth::BasicAuth { username, password } => (AuthKind::BasicAuth, String::new(), String::new(), String::new(), username.clone(), password.clone()),
            },
            None => (AuthKind::Token, String::new(), String::new(), String::new(), String::new(), String::new()),
        };

        // Determine initial connection mode
        let (init_conn_type, init_ssh_host_id, init_server, init_insecure) = match &existing {
            Some(c) => (c.connection_type, c.ssh_host_id.clone(), c.server.clone(), c.insecure_skip_tls),
            None => (ClusterConnectionType::Direct, None, String::new(), false),
        };

        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("集群名称, 如 生产集群");
            if let Some(e) = &existing {
                s.set_value(&e.name, window, cx);
            }
            s
        });
        let server = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("https://k8s.example.com:6443");
            if !init_server.is_empty() {
                s.set_value(&init_server, window, cx);
            }
            s
        });
        let namespace = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("default");
            let ns = existing.as_ref().map(|e| e.namespace.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "default".into());
            s.set_value(&ns, window, cx);
            s
        });
        let token = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("Bearer token");
            if !init_token.is_empty() { s.set_value(&init_token, window, cx); }
            s
        });
        let cert_data = cx.new(|cx| {
            let mut s = InputState::new(window, cx).multi_line(true).placeholder("base64 编码的 PEM 证书数据");
            if !init_cert.is_empty() { s.set_value(&init_cert, window, cx); }
            s
        });
        let key_data = cx.new(|cx| {
            let mut s = InputState::new(window, cx).multi_line(true).placeholder("base64 编码的 PEM 私钥数据");
            if !init_key.is_empty() { s.set_value(&init_key, window, cx); }
            s
        });
        let username = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("用户名");
            if !init_user.is_empty() { s.set_value(&init_user, window, cx); }
            s
        });
        let password = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("密码");
            if !init_pass.is_empty() { s.set_value(&init_pass, window, cx); }
            s
        });
        let kubeconfig_text = cx.new(|cx| {
            let mut s = InputState::new(window, cx).multi_line(true).placeholder("粘贴 kubeconfig YAML 内容，或点击\"选择文件\"按钮导入");
            // If editing and has raw kubeconfig, preload it
            if let Some(e) = &existing {
                if let Some(raw) = &e.raw_kubeconfig {
                    s.set_value(raw, window, cx);
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
        // Find initial selected SSH host by id (if editing an ssh-tunnel cluster)
        let init_label = init_ssh_host_id.and_then(|id| {
            ssh_hosts.iter()
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
                if label_to_id.is_empty() { None } else { Some(gpui_component::IndexPath::new(initial_index)) },
                window,
                cx,
            )
        });
        if let Some(lbl) = init_label {
            ssh_host_select.update(cx, |s, cx| {
                s.set_selected_value(&lbl, window, cx);
            });
        }

        // Pre-parse kubeconfig if editing has raw config
        let mut parsed_contexts = Vec::new();
        let mut selected_context = None;
        if let Some(e) = &existing {
            if let Some(raw) = &e.raw_kubeconfig {
                if let Ok(contexts) = parse_kubeconfig_contexts(raw) {
                    if let Some(orig) = &e.original_context {
                        selected_context = contexts.iter().position(|c| &c.name == orig);
                    }
                    parsed_contexts = contexts;
                }
            }
        }

        cx.new(|_cx| Self {
            editing_id: existing.as_ref().map(|e| e.id.clone()),
            editing_cluster: existing,
            is_imported_cluster: is_imported,
            active_tab: init_tab,
            name,
            namespace,
            insecure: init_insecure,
            connection_type: init_conn_type,
            ssh_host_select,
            ssh_label_to_id: label_to_id,
            kubeconfig_text,
            parsed_contexts,
            selected_context,
            server,
            auth_kind: init_auth_kind,
            token,
            cert_data,
            key_data,
            username,
            password,
            error: None,
        })
    }
}

impl Render for K8sClusterDialogState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
    }
}

/// Open the add/edit cluster dialog. `on_save` is invoked after a cluster is
/// successfully saved, so the caller can refresh its state.
pub fn open_dialog<F>(
    editing: Option<&ManualCluster>,
    on_save: F,
    window: &mut Window,
    cx: &mut App,
)
where
    F: Fn(&mut Window, &mut App) + 'static,
{
    let on_save: std::sync::Arc<F> = std::sync::Arc::new(on_save);
    let is_editing = editing.is_some();
    let state = K8sClusterDialogState::new(editing, window, cx);
    let title_text = if is_editing { "编辑 Kubernetes 集群" } else { "添加 Kubernetes 集群" };
    window.open_dialog(cx, move |dialog, _window, cx| {
        let state_for_content = state.clone();
        let state_for_footer = state.clone();
        let on_save = on_save.clone();
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;

        dialog
            .title(title_text)
            .w(px(680.))
            .content(move |content, window, cx| {
                let s = state_for_content.read(cx);
                let auth_kind = s.auth_kind;
                let insecure = s.insecure;
                let connection_type = s.connection_type;
                let active_tab = s.active_tab;
                let ssh_hosts_empty = s.ssh_label_to_id.is_empty();
                let is_editing_imported = s.editing_id.is_some() && s.is_imported_cluster;

                let mut body = v_flex()
                    .p_4()
                    .w_full()
                    .gap_3();

                // Error message
                if let Some(err) = &s.error {
                    body = body.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.danger)
                            .p_2()
                            .bg(theme.danger.opacity(0.1))
                            .rounded_md()
                            .child(err.clone())
                    );
                }

                // Tab selector only shown when adding new cluster (not editing)
                if s.editing_id.is_none() {
                    body = body.child(
                        h_flex()
                            .gap_1()
                            .p(px(3.))
                            .bg(theme.muted)
                            .rounded_md()
                            .children([DialogTab::ImportKubeconfig, DialogTab::ManualEntry].iter().map(|&tab| {
                                let is_selected = tab == active_tab;
                                Button::new(SharedString::from(format!("k8s-tab-{}", tab.label())))
                                    .ghost()
                                    .small()
                                    .label(tab.label())
                                    .selected(is_selected)
                                    .when(is_selected, |b| b.bg(theme.background).text_color(theme.foreground))
                                    .on_click({
                                        let state = state_for_content.clone();
                                        move |_ev, _window, cx: &mut App| {
                                            let _ = state.update(cx, |s, cx| {
                                                s.active_tab = tab;
                                                s.error = None;
                                                cx.notify();
                                            });
                                        }
                                    })
                            })),
                    );
                }

                // Info banner when editing an imported cluster
                if is_editing_imported {
                    body = body.child(
                        div()
                            .text_size(px(12.))
                            .p_2()
                            .bg(theme.primary.opacity(0.1))
                            .rounded_md()
                            .child("ℹ️ 凭证信息已从导入的 kubeconfig 中读取，此处仅可修改连接方式和 SSH 隧道配置")
                    );
                }

                // Common field: Display Name
                body = body.child(
                    v_flex()
                        .gap_1()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("集群显示名称 *"))
                        .child(Input::new(&s.name).small())
                );

                // Tab-specific content
                body = match active_tab {
                    DialogTab::ImportKubeconfig => {
                        // Import tab: paste/upload kubeconfig, select context
                        let contexts = s.parsed_contexts.clone();
                        let selected = s.selected_context;

                        let mut import_body = body
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("KubeConfig YAML"))
                                    .child(Input::new(&s.kubeconfig_text).small().h(px(140.)))
                                    .child(div().text_xs().text_color(muted).child("粘贴 kubeconfig YAML 内容或点击\"选择文件\"导入，然后点击\"解析\"列出可用 Context"))
                            );

                        // File picker + Parse buttons
                        let state_for_file = state_for_content.clone();
                        import_body = import_body.child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("k8s-select-kubeconfig-file")
                                        .small()
                                        .ghost()
                                        .label("选择文件…")
                                        .on_click(move |_ev, window, cx: &mut App| {
                                            let state = state_for_file.clone();
                                            let window_handle = window.window_handle();
                                            let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
                                            std::thread::spawn(move || {
                                                let res = rfd::FileDialog::new()
                                                    .add_filter("kubeconfig", &["yaml", "yml", "conf", "*"])
                                                    .pick_file();
                                                let _ = tx.send(res.map(|p| p.to_path_buf()));
                                            });
                                            cx.spawn(async move |cx| {
                                                loop {
                                                    match rx.try_recv() {
                                                        Ok(Some(path)) => {
                                                            let content = std::fs::read_to_string(&path)
                                                                .map_err(|e| format!("读取文件失败: {e}"));
                                                            let _ = window_handle.update(cx, |_this, window, cx| {
                                                                let _ = state.update(cx, |s, cx| {
                                                                    match content {
                                                                        Ok(text) => {
                                                                            s.kubeconfig_text.update(cx, |input, cx| {
                                                                                input.set_value(&text, window, cx);
                                                                            });
                                                                            s.error = None;
                                                                        }
                                                                        Err(e) => {
                                                                            s.error = Some(e);
                                                                        }
                                                                    }
                                                                    cx.notify();
                                                                });
                                                            });
                                                            break;
                                                        }
                                                        Ok(None) => break,
                                                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                                                            smol::Timer::after(std::time::Duration::from_millis(50)).await;
                                                        }
                                                        Err(_) => break,
                                                    }
                                                }
                                            }).detach();
                                        })
                                )
                                .child(
                                    Button::new("k8s-parse-kubeconfig")
                                        .small()
                                        .label("解析 kubeconfig")
                                        .on_click({
                                            let state = state_for_content.clone();
                                            move |_ev, window, cx: &mut App| {
                                                let text = state.read(cx).kubeconfig_text.read(cx).value().to_string();
                                                let (error, contexts, selected, name_val_for_auto) = if text.trim().is_empty() {
                                                    (Some("请粘贴 kubeconfig YAML 内容或选择文件".to_string()), Vec::new(), None, None)
                                                } else {
                                                    match parse_kubeconfig_contexts(&text) {
                                                        Ok(ctxs) => {
                                                            if ctxs.is_empty() {
                                                                (Some("kubeconfig 中未找到任何有效的 context".to_string()), Vec::new(), None, None)
                                                            } else {
                                                                let first_name = ctxs.first().unwrap().name.clone();
                                                                (None, ctxs, Some(0), Some(first_name))
                                                            }
                                                        }
                                                        Err(e) => {
                                                            (Some(format!("解析 kubeconfig 失败: {e}")), Vec::new(), None, None)
                                                        }
                                                    }
                                                };

                                                let _ = state.update(cx, |s, cx| {
                                                    s.error = error;
                                                    s.parsed_contexts = contexts;
                                                    s.selected_context = selected;
                                                    cx.notify();
                                                });

                                                if let Some(name_val) = name_val_for_auto {
                                                    let s = state.clone();
                                                    window.defer(cx, move |window, cx| {
                                                        let _ = s.update(cx, |s, cx| {
                                                            s.name.update(cx, |input, cx| {
                                                                input.set_value(&name_val, window, cx);
                                                            });
                                                        });
                                                    });
                                                }
                                            }
                                        })
                                )
                        );

                        if !contexts.is_empty() {
                            let state_for_ctx = state_for_content.clone();
                            let contexts_clone = contexts.clone();
                            import_body = import_body.child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("选择要导入的 Context"))
                                    .children(contexts.iter().enumerate().map(|(idx, ctx_info)| {
                                        let is_sel = selected == Some(idx);
                                        let label = format!(
                                            "{}  →  {}  ({})",
                                            ctx_info.name,
                                            ctx_info.server,
                                            if ctx_info.has_token { "Token" } else if ctx_info.has_client_cert { "客户端证书" } else { "其他" }
                                        );
                                        let ca_label = if ctx_info.has_ca { "CA ✓" } else { "无 CA" };
                                        let state_clone = state_for_ctx.clone();
                                        let ctx_name = ctx_info.name.clone();
                                        Button::new(SharedString::from(format!("k8s-ctx-{}", idx)))
                                            .ghost()
                                            .small()
                                            .label(format!("{label}  {ca_label}"))
                                            .selected(is_sel)
                                            .when(is_sel, |b| b.bg(theme.accent.opacity(0.3)))
                                            .on_click(move |_ev, window, cx: &mut App| {
                                                let name_to_set = ctx_name.clone();
                                                let _ = state_clone.update(cx, |s, cx| {
                                                    s.selected_context = Some(idx);
                                                    s.error = None;
                                                    s.name.update(cx, |input, cx| {
                                                        input.set_value(&name_to_set, window, cx);
                                                    });
                                                    cx.notify();
                                                });
                                            })
                                    }))
                            );
                        }

                        // Connection mode for import
                        let conn_state = state_for_content.clone();
                        let mut conn_section = v_flex()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("连接方式"))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("k8s-conn-direct-import")
                                            .ghost()
                                            .small()
                                            .label(ClusterConnectionType::Direct.label())
                                            .selected(connection_type == ClusterConnectionType::Direct)
                                            .when(connection_type == ClusterConnectionType::Direct, |b| {
                                                b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground)
                                            })
                                            .on_click({
                                                let state = conn_state.clone();
                                                move |_ev, _window, cx: &mut App| {
                                                    let _ = state.update(cx, |s, cx| {
                                                        s.connection_type = ClusterConnectionType::Direct;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    )
                                    .child(
                                        Button::new("k8s-conn-ssh-import")
                                            .ghost()
                                            .small()
                                            .label(ClusterConnectionType::SshTunnel.label())
                                            .selected(connection_type == ClusterConnectionType::SshTunnel)
                                            .when(connection_type == ClusterConnectionType::SshTunnel, |b| {
                                                b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground)
                                            })
                                            .on_click({
                                                let state = conn_state.clone();
                                                move |_ev, _window, cx: &mut App| {
                                                    let _ = state.update(cx, |s, cx| {
                                                        s.connection_type = ClusterConnectionType::SshTunnel;
                                                        s.insecure = true;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    ),
                            );
                        if connection_type == ClusterConnectionType::SshTunnel {
                            if ssh_hosts_empty {
                                conn_section = conn_section.child(
                                    div().text_sm().text_color(theme.danger).child("尚未配置任何 Verve SSH 主机。请先在 \"主机\" 面板添加 SSH 主机。")
                                );
                            } else {
                                conn_section = conn_section
                                    .child(v_flex().gap_1()
                                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("SSH 跳板机 *"))
                                        .child(Select::new(&s.ssh_host_select).small()))
                                    .child(div().text_xs().text_color(muted).child("💡 Verve 自动建立 SSH 端口转发，无需手动修改 kubeconfig"));
                            }
                        }

                        import_body = import_body
                            .child(div().h(px(1.)).w_full().bg(border))
                            .child(conn_section);

                        import_body
                    }
                    DialogTab::ManualEntry => {
                        // Manual entry tab
                        let mut manual_body = body
                            // Server URL - when editing imported cluster, show as read-only info
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(
                                        if is_editing_imported { "API Server 地址" } else { "API Server 地址 *" }
                                    ))
                                    .child(
                                        if is_editing_imported {
                                            div().text_sm().child(s.server.read(cx).value().to_string()).py_1().px_2().bg(theme.muted).rounded_md().into_any_element()
                                        } else {
                                            Input::new(&s.server).small().into_any_element()
                                        }
                                    )
                                    .when(!is_editing_imported, |b| {
                                        b.child(div().text_xs().text_color(muted).child("K8s API Server URL，例如 https://k8s.example.com:6443"))
                                    })
                            );

                        // Connection mode section
                        let conn_state = state_for_content.clone();
                        let mut conn_section = v_flex()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("连接方式"))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("k8s-conn-direct-manual")
                                            .ghost()
                                            .small()
                                            .label(ClusterConnectionType::Direct.label())
                                            .selected(connection_type == ClusterConnectionType::Direct)
                                            .when(connection_type == ClusterConnectionType::Direct, |b| {
                                                b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground)
                                            })
                                            .on_click({
                                                let state = conn_state.clone();
                                                move |_ev, _window, cx: &mut App| {
                                                    let _ = state.update(cx, |s, cx| {
                                                        s.connection_type = ClusterConnectionType::Direct;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    )
                                    .child(
                                        Button::new("k8s-conn-ssh-manual")
                                            .ghost()
                                            .small()
                                            .label(ClusterConnectionType::SshTunnel.label())
                                            .selected(connection_type == ClusterConnectionType::SshTunnel)
                                            .when(connection_type == ClusterConnectionType::SshTunnel, |b| {
                                                b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground)
                                            })
                                            .on_click({
                                                let state = conn_state.clone();
                                                move |_ev, _window, cx: &mut App| {
                                                    let _ = state.update(cx, |s, cx| {
                                                        s.connection_type = ClusterConnectionType::SshTunnel;
                                                        s.insecure = true;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    ),
                            );
                        if connection_type == ClusterConnectionType::SshTunnel {
                            if ssh_hosts_empty {
                                conn_section = conn_section.child(
                                    div().text_sm().text_color(theme.danger).child("尚未配置任何 Verve SSH 主机。请先在 \"主机\" 面板添加 SSH 主机。")
                                );
                            } else {
                                conn_section = conn_section
                                    .child(v_flex().gap_1()
                                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("SSH 跳板机 *"))
                                        .child(Select::new(&s.ssh_host_select).small()))
                                    .child(div().text_xs().text_color(muted).child("💡 Verve 自动建立 SSH 端口转发，无需手动修改 kubeconfig"));
                            }
                        }

                        manual_body = manual_body
                            .child(div().h(px(1.)).w_full().bg(border))
                            .child(conn_section);

                        // In SSH tunnel mode, override the server URL field label
                        // (server field is already shown above for direct mode, for tunnel we clarify)
                        if connection_type == ClusterConnectionType::SshTunnel && !ssh_hosts_empty {
                            // The server field is already rendered above, but we add clarifying text here
                            manual_body = manual_body.child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("💡 上方 API 地址填写跳板机可访问的内网地址，通常为 https://127.0.0.1:6443")
                            );
                        }

                        manual_body = manual_body
                            // Namespace
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("默认 Namespace (可选)"))
                                    .child(Input::new(&s.namespace).small()),
                            );

                        // Only show auth fields when NOT editing an imported cluster
                        if !is_editing_imported {
                            manual_body = manual_body
                                // Auth type
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("认证方式 *"))
                                        .child(h_flex().gap_2().children(AuthKind::ALL.iter().map(|&ak| {
                                            let is_selected = ak == auth_kind;
                                            Button::new(SharedString::from(format!("k8s-auth-{}", ak.label())))
                                                .ghost()
                                                .small()
                                                .label(ak.label())
                                                .selected(is_selected)
                                                .when(is_selected, |b| b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground))
                                                .on_click({
                                                    let st = state_for_content.clone();
                                                    move |_ev, _window, cx: &mut App| {
                                                        let _ = st.update(cx, |s, cx| { s.auth_kind = ak; cx.notify(); });
                                                    }
                                                })
                                        }))),
                                )
                                // Conditional auth fields
                                .when(auth_kind == AuthKind::Token, |col| {
                                    col.child(v_flex().gap_1()
                                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Token *"))
                                        .child(Input::new(&s.token).small()))
                                })
                                .when(auth_kind == AuthKind::ClientCert, |col| {
                                    col.child(v_flex().gap_1()
                                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Client Certificate Data (base64 PEM) *"))
                                        .child(Input::new(&s.cert_data).small().h(px(70.))))
                                       .child(v_flex().gap_1()
                                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Client Key Data (base64 PEM) *"))
                                        .child(Input::new(&s.key_data).small().h(px(70.))))
                                })
                                .when(auth_kind == AuthKind::BasicAuth, |col| {
                                    col.child(h_flex().gap_2()
                                        .child(v_flex().gap_1().flex_1()
                                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("用户名 *"))
                                            .child(Input::new(&s.username).small()))
                                        .child(v_flex().gap_1().flex_1()
                                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("密码 *"))
                                            .child(Input::new(&s.password).small())))
                                });
                        }

                        manual_body
                    }
                };

                // TLS insecure toggle (common to both tabs)
                body = body
                    .child(div().h(px(1.)).w_full().bg(border))
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Button::new("k8s-insecure-toggle")
                                    .ghost()
                                    .small()
                                    .label(if insecure { "跳过所有 TLS 证书校验: 开" } else { "跳过所有 TLS 证书校验: 关" })
                                    .selected(insecure)
                                    .when(insecure, |b| b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground))
                                    .on_click({
                                        let st = state_for_content.clone();
                                        move |_ev, _window, cx: &mut App| {
                                            let _ = st.update(cx, |s, cx| { s.insecure = !s.insecure; cx.notify(); });
                                        }
                                    })
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(if connection_type == ClusterConnectionType::SshTunnel {
                                        "SSH 隧道模式下默认开启：转发到本地 127.0.0.1 时，K8s API 证书的主机名通常不匹配。\n开启后忽略 CA 验证和主机名校验，相当于 kubectl --insecure-skip-tls-verify=true"
                                    } else {
                                        "开启后跳过所有 TLS 证书校验（CA验证、主机名匹配），请仅在测试环境或自签名证书场景使用"
                                    })
                            )
                    );

                content.child(body)
            })
            .footer({
                let state = state_for_footer.clone();
                Button::new("k8s-cluster-save")
                    .primary()
                    .small()
                    .label("保存")
                    .on_click(move |_ev, window, cx| {
                        // Collect ALL values first to avoid borrow conflicts
                        let (
                            active_tab,
                            editing_id,
                            editing_cluster,
                            is_imported_edit,
                            name,
                            server,
                            namespace,
                            connection_type,
                            selected_ssh_label,
                            ssh_label_to_id,
                            insecure,
                            auth_kind,
                            token_val,
                            cert_val,
                            key_val,
                            username_val,
                            password_val,
                            kubeconfig_raw,
                            parsed_contexts,
                            selected_context,
                        ) = {
                            let s = state.read(cx);
                            let name = s.name.read(cx).value().to_string();
                            let server = s.server.read(cx).value().to_string();
                            let namespace = s.namespace.read(cx).value().to_string();
                            let selected_label = match s.connection_type {
                                ClusterConnectionType::SshTunnel => {
                                    s.ssh_host_select.read(cx).selected_value().map(|s| s.to_string())
                                }
                                ClusterConnectionType::Direct => None,
                            };
                            (
                                s.active_tab,
                                s.editing_id.clone(),
                                s.editing_cluster.clone(),
                                s.is_imported_cluster,
                                name,
                                server,
                                namespace,
                                s.connection_type,
                                selected_label,
                                s.ssh_label_to_id.clone(),
                                s.insecure,
                                s.auth_kind,
                                s.token.read(cx).value().to_string(),
                                s.cert_data.read(cx).value().to_string(),
                                s.key_data.read(cx).value().to_string(),
                                s.username.read(cx).value().to_string(),
                                s.password.read(cx).value().to_string(),
                                s.kubeconfig_text.read(cx).value().to_string(),
                                s.parsed_contexts.clone(),
                                s.selected_context,
                            )
                        };

                        // Validate and build cluster based on active tab
                        let mut mc = match active_tab {
                            DialogTab::ImportKubeconfig => {
                                // Validate import tab fields
                                let mut error: Option<String> = None;
                                if name.trim().is_empty() {
                                    error = Some("请填写集群名称".into());
                                } else if selected_context.is_none() {
                                    error = Some("请选择要导入的 Context".into());
                                } else if kubeconfig_raw.trim().is_empty() {
                                    error = Some("请粘贴 kubeconfig 内容或点击\"选择文件\"导入文件".into());
                                } else if let ClusterConnectionType::SshTunnel = connection_type {
                                    if selected_ssh_label.is_none() {
                                        error = Some("请选择 SSH 跳板机".into());
                                    }
                                }

                                if let Some(err) = error {
                                    let _ = state.update(cx, |s, cx| {
                                        s.error = Some(err);
                                        cx.notify();
                                    });
                                    return;
                                }

                                // Build cluster from imported kubeconfig
                                let ctx_idx = selected_context.unwrap();
                                let ctx_info = &parsed_contexts[ctx_idx];
                                match cluster_from_kubeconfig(
                                    &name,
                                    &kubeconfig_raw,
                                    &ctx_info.name,
                                ) {
                                    Ok(mut c) => {
                                        c.connection_type = connection_type;
                                        c.insecure_skip_tls = insecure;
                                        // Resolve SSH host id for tunnel mode
                                        if let ClusterConnectionType::SshTunnel = connection_type {
                                            let label = selected_ssh_label.unwrap();
                                            match ssh_label_to_id.get(&label) {
                                                Some(id) => c.ssh_host_id = Some(id.clone()),
                                                None => {
                                                    let _ = state.update(cx, |s, cx| {
                                                        s.error = Some("无法找到选中的 SSH 主机".into());
                                                        cx.notify();
                                                    });
                                                    return;
                                                }
                                            }
                                        }
                                        // Override namespace if user changed it from default
                                        if !namespace.trim().is_empty() && namespace != "default" {
                                            c.namespace = namespace;
                                        }
                                        c
                                    }
                                    Err(e) => {
                                        let _ = state.update(cx, |s, cx| {
                                            s.error = Some(format!("导入集群失败: {e}"));
                                            cx.notify();
                                        });
                                        return;
                                    }
                                }
                            }
                            DialogTab::ManualEntry => {
                                // Validate manual entry fields
                                let mut error: Option<String> = None;
                                if name.trim().is_empty() {
                                    error = Some("请填写集群名称".into());
                                } else if server.trim().is_empty() {
                                    error = Some(match connection_type {
                                        ClusterConnectionType::Direct => "请填写 API Server 地址".to_string(),
                                        ClusterConnectionType::SshTunnel => "请填写跳板机侧 API 地址".to_string(),
                                    });
                                } else if !server.trim().starts_with("https://") && !server.trim().starts_with("http://") {
                                    error = Some("API 地址需以 http:// 或 https:// 开头".into());
                                } else if let ClusterConnectionType::SshTunnel = connection_type {
                                    if selected_ssh_label.is_none() {
                                        error = Some("请选择 SSH 跳板机".into());
                                    }
                                }

                                // Validate credential fields based on auth type
                                if error.is_none() {
                                    match auth_kind {
                                        AuthKind::Token => {
                                            if token_val.trim().is_empty() {
                                                error = Some("请填写 Token".into());
                                            }
                                        }
                                        AuthKind::ClientCert => {
                                            if cert_val.trim().is_empty() {
                                                error = Some("请填写 Client Certificate Data".into());
                                            } else if key_val.trim().is_empty() {
                                                error = Some("请填写 Client Key Data".into());
                                            }
                                        }
                                        AuthKind::BasicAuth => {
                                            if username_val.trim().is_empty() {
                                                error = Some("请填写用户名".into());
                                            } else if password_val.trim().is_empty() {
                                                error = Some("请填写密码".into());
                                            }
                                        }
                                    }
                                }

                                if let Some(err) = error {
                                    let _ = state.update(cx, |s, cx| {
                                        s.error = Some(err);
                                        cx.notify();
                                    });
                                    return;
                                }

                                // Resolve SSH host id
                                let ssh_host_id = match connection_type {
                                    ClusterConnectionType::SshTunnel => {
                                        let label = selected_ssh_label.unwrap();
                                        match ssh_label_to_id.get(&label) {
                                            Some(id) => Some(id.clone()),
                                            None => {
                                                let _ = state.update(cx, |s, cx| {
                                                    s.error = Some("无法找到选中的 SSH 主机".into());
                                                    cx.notify();
                                                });
                                                return;
                                            }
                                        }
                                    }
                                    ClusterConnectionType::Direct => None,
                                };

                                let auth = match auth_kind {
                                    AuthKind::Token => ClusterAuth::Token {
                                        token: token_val,
                                    },
                                    AuthKind::ClientCert => ClusterAuth::ClientCert {
                                        cert_data: cert_val,
                                        key_data: key_val,
                                    },
                                    AuthKind::BasicAuth => ClusterAuth::BasicAuth {
                                        username: username_val,
                                        password: password_val,
                                    },
                                };

                                let mut mc = ManualCluster::new(&name, &server);
                                mc.namespace = if namespace.trim().is_empty() { "default".into() } else { namespace };
                                mc.auth = auth;
                                mc.insecure_skip_tls = insecure;
                                mc.connection_type = connection_type;
                                mc.ssh_host_id = ssh_host_id;
                                mc
                            }
                        };

                        // Apply editing overrides
                        if let Some(id) = &editing_id {
                            mc.id = id.clone();
                            // When editing an imported cluster, preserve original auth and raw_kubeconfig
                            // (credentials from imported config should not be overwritten by empty form fields)
                            if is_imported_edit {
                                if let Some(original) = &editing_cluster {
                                    mc.auth = original.auth.clone();
                                    mc.raw_kubeconfig = original.raw_kubeconfig.clone();
                                    mc.original_context = original.original_context.clone();
                                }
                            }
                            // Close any existing SSH tunnel for this cluster
                            crate::k8s::kubectl::close_ssh_tunnel(id);
                        }

                        upsert_manual_cluster(mc);
                        (*on_save.clone())(window, cx);
                        window.close_dialog(cx);
                    })
            })
    });
}
