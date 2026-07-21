//! Dialog to manually add a Kubernetes cluster.
//!
//! Collects name, server URL, namespace, auth type (token / client cert / basic),
//! the credential fields, and a skip-TLS toggle. On save the entry is written
//! to `~/.verve/kube_clusters.json` and merged into `~/.verve/kube_config.yaml`.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Selectable as _, Sizable as _, WindowExt as _};

use crate::k8s::clusters::{upsert_manual_cluster, ClusterAuth, ManualCluster};

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

pub struct K8sClusterDialogState {
    pub name: Entity<InputState>,
    pub server: Entity<InputState>,
    pub namespace: Entity<InputState>,
    pub auth_kind: AuthKind,
    pub token: Entity<InputState>,
    pub cert_data: Entity<InputState>,
    pub key_data: Entity<InputState>,
    pub username: Entity<InputState>,
    pub password: Entity<InputState>,
    pub insecure: bool,
}

impl K8sClusterDialogState {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Context 名称, 如 prod-cluster"));
        let server = cx.new(|cx| InputState::new(window, cx).placeholder("https://k8s.example.com:6443"));
        let namespace = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("default");
            s.set_value("default", window, cx);
            s
        });
        let token = cx.new(|cx| InputState::new(window, cx).placeholder("Bearer token"));
        let cert_data = cx.new(|cx| InputState::new(window, cx).placeholder("cert-data (base64 PEM)"));
        let key_data = cx.new(|cx| InputState::new(window, cx).placeholder("key-data (base64 PEM)"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("用户名"));
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("密码"));
        cx.new(|_cx| Self {
            name,
            server,
            namespace,
            auth_kind: AuthKind::Token,
            token,
            cert_data,
            key_data,
            username,
            password,
            insecure: false,
        })
    }
}

impl Render for K8sClusterDialogState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
    }
}

pub fn open_dialog(window: &mut Window, cx: &mut App) {
    let state = K8sClusterDialogState::new(window, cx);
    window.open_dialog(cx, move |dialog, _window, cx| {
        let state_for_content = state.clone();
        let state_for_footer = state.clone();
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;

        dialog
            .title("添加 Kubernetes 集群")
            .content(move |content, _window, cx| {
                let s = state_for_content.read(cx);
                let auth_kind = s.auth_kind;
                let insecure = s.insecure;
                let body = v_flex()
                    .p_4()
                    .w(px(520.))
                    .gap_3()
                    // Name
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("名称"))
                            .child(Input::new(&s.name).small()),
                    )
                    // Server
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Server URL"))
                            .child(Input::new(&s.server).small())
                            .child(div().text_xs().text_color(muted).child("API server 地址，例如 https://1.2.3.4:6443")),
                    )
                    // Namespace
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("默认 Namespace"))
                            .child(Input::new(&s.namespace).small()),
                    )
                    // Auth type
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("认证方式"))
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
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Token"))
                            .child(Input::new(&s.token).small()))
                    })
                    .when(auth_kind == AuthKind::ClientCert, |col| {
                        col.child(v_flex().gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Client Certificate Data (base64)"))
                            .child(Input::new(&s.cert_data).small()))
                           .child(v_flex().gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Client Key Data (base64)"))
                            .child(Input::new(&s.key_data).small()))
                    })
                    .when(auth_kind == AuthKind::BasicAuth, |col| {
                        col.child(h_flex().gap_2()
                            .child(v_flex().gap_1().flex_1()
                                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("用户名"))
                                .child(Input::new(&s.username).small()))
                            .child(v_flex().gap_1().flex_1()
                                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("密码"))
                                .child(Input::new(&s.password).small())))
                    })
                    // Insecure TLS toggle
                    .child(
                        Button::new("k8s-insecure-toggle")
                            .ghost()
                            .small()
                            .label(if insecure { "跳过 TLS 验证: 开" } else { "跳过 TLS 验证: 关" })
                            .selected(insecure)
                            .when(insecure, |b| b.bg(theme.accent.opacity(0.5)).text_color(theme.foreground))
                            .on_click({
                                let st = state_for_content.clone();
                                move |_ev, _window, cx: &mut App| {
                                    let _ = st.update(cx, |s, cx| { s.insecure = !s.insecure; cx.notify(); });
                                }
                            })
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
                        let s = state.read(cx);
                        let name = s.name.read(cx).value().to_string();
                        let server = s.server.read(cx).value().to_string();
                        let namespace = s.namespace.read(cx).value().to_string();
                        if name.trim().is_empty() || server.trim().is_empty() {
                            return;
                        }
                        let auth = match s.auth_kind {
                            AuthKind::Token => ClusterAuth::Token {
                                token: s.token.read(cx).value().to_string(),
                            },
                            AuthKind::ClientCert => ClusterAuth::ClientCert {
                                cert_data: s.cert_data.read(cx).value().to_string(),
                                key_data: s.key_data.read(cx).value().to_string(),
                            },
                            AuthKind::BasicAuth => ClusterAuth::BasicAuth {
                                username: s.username.read(cx).value().to_string(),
                                password: s.password.read(cx).value().to_string(),
                            },
                        };
                        let mut mc = ManualCluster::new(&name, &server);
                        mc.namespace = if namespace.trim().is_empty() { "default".into() } else { namespace };
                        mc.auth = auth;
                        mc.insecure_skip_tls = s.insecure;
                        upsert_manual_cluster(mc);
                        window.close_dialog(cx);
                    })
            })
    });
}
