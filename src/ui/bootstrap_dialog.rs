//! First-run bootstrap dialog — choose local mode or clone config from git.
//!
//! On completion the dialog sets a global `BootstrapComplete` marker which
//! `main.rs` can detect to know whether to proceed to the main window.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme, Disableable as _, Sizable as _, h_flex, v_flex};

use crate::git::ops::{self, GitAuth};
use crate::state::persistence;

/// Global marker set when the bootstrap flow completes.
pub struct BootstrapComplete;
impl Global for BootstrapComplete {}

/// Result of the bootstrap flow.
#[derive(Clone)]
pub enum BootstrapResult {
    Local,
    Cloned,
}

pub struct BootstrapDialog {
    url_input: Entity<InputState>,
    username_input: Entity<InputState>,
    token_input: Entity<InputState>,
    status: BootstrapStatus,
}

#[derive(Clone)]
enum BootstrapStatus {
    Choosing,
    Cloning,
    Error(String),
}

impl BootstrapDialog {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://github.com/your/verve-config.git")
        });
        let username_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("username (optional)"));
        let token_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("token/password (optional)"));

        cx.new(|_cx| Self {
            url_input,
            username_input,
            token_input,
            status: BootstrapStatus::Choosing,
        })
    }

    fn choose_local(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        persistence::mark_bootstrap_done();
        cx.set_global(BootstrapComplete);
        cx.quit();
    }

    fn start_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.url_input.read(cx).value().to_string();
        if url.is_empty() {
            self.status = BootstrapStatus::Error("请输入 Git 仓库地址".to_string());
            cx.notify();
            return;
        }

        let username = self.username_input.read(cx).value().to_string();
        let token = self.token_input.read(cx).value().to_string();

        self.status = BootstrapStatus::Cloning;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let data_dir =
                        persistence::data_dir().map_err(|e| format!("数据目录创建失败: {}", e))?;

                    let tmp_dir = data_dir.with_file_name(".verve-clone-tmp");
                    if tmp_dir.exists() {
                        let _ = std::fs::remove_dir_all(&tmp_dir);
                    }

                    let auth = GitAuth { username, token };
                    ops::clone(&tmp_dir, &url, &auth).map_err(|e| format!("克隆失败: {}", e))?;

                    // Move cloned contents into data_dir.
                    for entry in std::fs::read_dir(&tmp_dir).map_err(|e| e.to_string())? {
                        let entry = entry.map_err(|e| e.to_string())?;
                        let dest = data_dir.join(entry.file_name());
                        if dest.exists() {
                            let _ = std::fs::remove_dir_all(&dest);
                        }
                        std::fs::rename(entry.path(), &dest).map_err(|e| e.to_string())?;
                    }
                    let _ = std::fs::remove_dir_all(&tmp_dir);

                    // Save git config.
                    let mut cfg = persistence::load_git_config();
                    cfg.remote = Some(url);
                    cfg.username = auth.username.clone();
                    cfg.token = auth.token.clone();
                    persistence::save_git_config(&cfg);

                    persistence::mark_bootstrap_done();
                    Ok(())
                })
                .await;

            match result {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = BootstrapStatus::Choosing;
                        cx.set_global(BootstrapComplete);
                        cx.quit();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = BootstrapStatus::Error(e);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

impl Render for BootstrapDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let is_cloning = matches!(&self.status, BootstrapStatus::Cloning);
        let url_error = matches!(&self.status, BootstrapStatus::Error(_));

        v_flex()
            .w(px(520.))
            .gap(px(16.))
            .p(px(24.))
            .bg(theme.background)
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(FontWeight::BOLD)
                    .child(rust_i18n::t!("git_bootstrap.title").to_string()),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .child(rust_i18n::t!("git_bootstrap.welcome").to_string()),
            )
            .child(
                Button::new("bootstrap-local")
                    .w_full()
                    .disabled(is_cloning)
                    .child(
                        v_flex()
                            .w_full()
                            .items_start()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(rust_i18n::t!("git_bootstrap.start_fresh").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        rust_i18n::t!("git_bootstrap.start_fresh_desc").to_string(),
                                    ),
                            ),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_local(window, cx);
                    })),
            )
            .child(div().w_full().h(px(1.)).my(px(4.)).bg(theme.border))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(rust_i18n::t!("git_bootstrap.clone_existing").to_string()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(rust_i18n::t!("git_bootstrap.clone_existing_desc").to_string()),
            )
            .child(Input::new(&self.url_input).w_full())
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .child(Input::new(&self.username_input).w_full().flex_1())
                    .child(Input::new(&self.token_input).w_full().flex_1()),
            )
            .child(match &self.status {
                BootstrapStatus::Choosing => Button::new("bootstrap-clone")
                    .w_full()
                    .primary()
                    .child(rust_i18n::t!("git_bootstrap.clone_existing").to_string())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start_clone(window, cx);
                    }))
                    .into_any_element(),
                BootstrapStatus::Cloning => div()
                    .w_full()
                    .p_2()
                    .text_center()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .child(rust_i18n::t!("git_bootstrap.cloning").to_string())
                    .into_any_element(),
                BootstrapStatus::Error(e) => v_flex()
                    .w_full()
                    .gap(px(8.))
                    .child(
                        div()
                            .w_full()
                            .p_2()
                            .rounded_md()
                            .bg(theme.danger.opacity(0.1))
                            .text_color(theme.danger)
                            .text_size(px(12.))
                            .child(e.clone()),
                    )
                    .child(
                        Button::new("bootstrap-retry")
                            .w_full()
                            .primary()
                            .child(rust_i18n::t!("git_bootstrap.retry").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_clone(window, cx);
                            })),
                    )
                    .into_any_element(),
            })
    }
}
