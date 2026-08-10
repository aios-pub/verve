//! SSH host configuration dialog (new / edit).
//!
//! Opened via `window.open_dialog` from the SSH panel's "+ 新建主机" or a
//! card's "edit" button. Collects all non-secret fields; the password / key
//! passphrase is stored separately via the credential API on confirm.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::{
    ActiveTheme, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::ssh::models::{SshAuthType, SshHost};
use crate::ssh::{credentials, host_store};

/// Mutable state backing the host dialog.
pub struct SshHostDialogState {
    pub editing_id: Option<String>, // Some = editing existing, None = new
    pub name: Entity<InputState>,
    pub host: Entity<InputState>,
    pub port: Entity<InputState>,
    pub username: Entity<InputState>,
    pub auth_type: SshAuthType,
    pub key_path: Entity<InputState>,
    pub password: Entity<InputState>,
    pub proxy_jump: Option<String>, // selected bastion host id
}

impl SshHostDialogState {
    pub fn new(
        editing: Option<&SshHost>,
        available_hosts: &[SshHost], // for jump-host selection
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let existing = editing.cloned();
        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("名称");
            if let Some(e) = &existing {
                s.set_value(&e.name, window, cx);
            }
            s
        });
        let host = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("主机地址");
            if let Some(e) = &existing {
                s.set_value(&e.host, window, cx);
            }
            s
        });
        let port = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("端口");
            s.set_value(
                &existing
                    .as_ref()
                    .map(|e| e.port.to_string())
                    .unwrap_or_else(|| "22".into()),
                window,
                cx,
            );
            s
        });
        let username = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("用户名");
            if let Some(e) = &existing {
                s.set_value(&e.username, window, cx);
            }
            s
        });
        let key_path = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("~/.ssh/id_rsa");
            if let Some(e) = &existing {
                if let Some(kp) = &e.key_path {
                    s.set_value(kp, window, cx);
                }
            }
            s
        });
        let password = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("密码").masked(true);
            // Editing an existing host: pre-fill the stored password so the
            // field is not left blank. (Leaving it blank would also be fine —
            // the save path only writes when non-empty — but pre-filling lets
            // the user verify / edit the existing secret.)
            if let Some(e) = &existing {
                if let Some(stored) = credentials::load_secret(&e.id, "password") {
                    s.set_value(&stored, window, cx);
                }
                if let Some(stored) = credentials::load_secret(&e.id, "key_passphrase") {
                    // Prefer whichever matches the host's auth type, but if the
                    // user switches modes the field will just show what we set
                    // last; the save path re-binds kind by auth_type anyway.
                    if s.value().is_empty() {
                        s.set_value(&stored, window, cx);
                    }
                }
            }
            s
        });
        let auth_type = existing.as_ref().map(|e| e.auth_type).unwrap_or_default();
        let proxy_jump = existing.as_ref().and_then(|e| e.bastion_id.clone());
        let _ = available_hosts; // used by caller for dropdown population
        cx.new(|_cx| Self {
            editing_id: existing.map(|e| e.id),
            name,
            host,
            port,
            username,
            auth_type,
            key_path,
            password,
            proxy_jump,
        })
    }
}

impl Render for SshHostDialogState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
    }
}

/// Open the host dialog. `on_save` receives the finalized `SshHost`.
pub fn open_dialog<F>(
    editing: Option<&SshHost>,
    available_hosts: Vec<SshHost>,
    on_save: F,
    window: &mut Window,
    cx: &mut App,
) where
    F: Fn(SshHost, &mut Window, &mut App) + 'static,
{
    // Wrap in Arc so the Fn closure can clone it per build.
    let on_save: std::sync::Arc<F> = std::sync::Arc::new(on_save);
    let state = SshHostDialogState::new(editing, &available_hosts, window, cx);
    let title_text = if editing.is_some() {
        "编辑主机"
    } else {
        "新建主机"
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
            .w(px(520.))
            .content(move |content, _window, cx| {
                let s = state_for_content.read(cx);
                let auth_type = s.auth_type;
                let body =
                    v_flex()
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
                        // Host + Port (side by side)
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child("主机地址"),
                                        )
                                        .child(Input::new(&s.host).small()),
                                )
                                .child(
                                    v_flex()
                                        .w(px(100.))
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child("端口"),
                                        )
                                        .child(Input::new(&s.port).small()),
                                ),
                        )
                        // Username
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("用户名"),
                                )
                                .child(Input::new(&s.username).small()),
                        )
                        // Auth type selector (simple buttons)
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("认证方式"),
                                )
                                .child(h_flex().gap_2().children(SshAuthType::ALL.iter().map(
                                    |&at| {
                                        let is_selected = at == auth_type;
                                        let tc = theme.clone();
                                        Button::new(SharedString::from(format!(
                                            "auth-type-{}",
                                            at.label()
                                        )))
                                        .ghost()
                                        .small()
                                        .label(at.label())
                                        .selected(is_selected)
                                        .when(is_selected, |b| {
                                            b.bg(tc.accent.opacity(0.5)).text_color(tc.foreground)
                                        })
                                        .on_click({
                                            let state = state_for_content.clone();
                                            move |_ev, _window, cx: &mut App| {
                                                let _ = state.update(cx, |s, cx| {
                                                    s.auth_type = at;
                                                    cx.notify();
                                                });
                                            }
                                        })
                                    },
                                ))),
                        )
                        // Conditional: password or key path
                        .when(auth_type == SshAuthType::Password, |col| {
                            col.child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("密码"),
                                    )
                                    .child(Input::new(&s.password).small().mask_toggle()),
                            )
                        })
                        .when(auth_type == SshAuthType::PrivateKey, |col| {
                            col.child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("密钥路径"),
                                    )
                                    .child(Input::new(&s.key_path).small()),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("密钥密码（可选）"),
                                    )
                                    .child(Input::new(&s.password).small().mask_toggle()),
                            )
                        })
                        .when(auth_type == SshAuthType::KeyboardInteractive, |col| {
                            col.child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("密码（MFA 前置，可选）"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child("验证码（OTP/TOTP）将在连接时动态弹出，无需在此填写。许多 MFA 服务器要求“密码 + 验证码”，请填写账户密码。"),
                                    )
                                    .child(Input::new(&s.password).small().mask_toggle()),
                            )
                        })
                        // Jump host selection
                        .child(div().h(px(1.)).w_full().bg(border))
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("跳板机（可选）"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("选择一台已配置的主机作为跳板机（ProxyJump）"),
                                ),
                        );
                content.child(body)
            })
            .footer({
                let on_save = on_save.clone();
                let state = state_for_footer.clone();
                Button::new("host-save")
                    .primary()
                    .small()
                    .label("保存")
                    .on_click(move |_ev, window, cx| {
                        let s = state.read(cx);
                        let name_input = s.name.read(cx).value().to_string();
                        let host_addr = s.host.read(cx).value().to_string();
                        let port: u16 = s.port.read(cx).value().parse().unwrap_or(22);
                        let username = s.username.read(cx).value().to_string();
                        let key_path_val = s.key_path.read(cx).value().to_string();
                        let password_val = s.password.read(cx).value().to_string();

                        // Name defaults to the host address if left empty.
                        let name = if name_input.trim().is_empty() {
                            host_addr.clone()
                        } else {
                            name_input
                        };

                        let mut host = if let Some(id) = &s.editing_id {
                            host_store::find_host(id)
                                .unwrap_or_else(|| SshHost::new(&name, &host_addr))
                        } else {
                            SshHost::new(&name, &host_addr)
                        };
                        host.name = name;
                        host.host = host_addr;
                        host.port = port;
                        host.username = username;
                        host.auth_type = s.auth_type;
                        host.key_path =
                            if s.auth_type == SshAuthType::PrivateKey && !key_path_val.is_empty() {
                                Some(key_path_val)
                            } else {
                                None
                            };
                        host.bastion_id = s.proxy_jump.clone();

                        // Store the secret if a password was entered.
                        if !password_val.is_empty() {
                            let kind = match s.auth_type {
                                SshAuthType::Password => "password",
                                SshAuthType::PrivateKey => "key_passphrase",
                                // Keyboard-interactive (MFA) servers often pair
                                // the OTP with a password, stored as "password".
                                SshAuthType::KeyboardInteractive => "password",
                                SshAuthType::Agent => "",
                            };
                            if !kind.is_empty() {
                                let _ = credentials::store_secret(&host.id, kind, &password_val);
                            }
                        }

                        // Persist to disk before invoking the callback.
                        host_store::upsert_host(host.clone());

                        (*on_save.clone())(host, window, cx);
                        window.close_dialog(cx);
                    })
            })
    });
}
