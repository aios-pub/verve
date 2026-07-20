//! A standalone settings window.
//!
//! The environment/cookie/global managers open in their own OS-level window
//! (not an in-app dialog), so confirmation dialogs stack correctly and the
//! layout isn't constrained by the dialog chrome. This view is the window's
//! root; it attaches the gpui-component overlay layers (dialog/sheet/
//! notification) so nested confirm dialogs render.
//!
//! The window has a left secondary nav with two sections:
//! - **通用设置** — home-view (首页指向) selector and other app preferences.
//! - **环境管理** — the full environment/global manager (two-column layout).

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Disableable as _, Icon, IconName, Sizable as _,
};

use crate::assets::REFRESH_CW;
use crate::state::persistence;
use crate::state::AppState;
use crate::ui::app::SideView;
use crate::ui::environments_view::EnvironmentsView;

/// Which kind of settings window to open.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsKind {
    /// The full environment/global manager (two-column sidebar layout).
    Environments,
    /// The general settings page (home view, etc.).
    General,
}

/// A selectable section in the settings window's secondary nav.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    /// App-level preferences (home view, etc.).
    General,
    /// Environment / global variable / cookie managers.
    Environments,
    /// About / version / update check.
    About,
}

impl SettingsSection {
    pub fn label(self) -> String {
        match self {
            SettingsSection::General => rust_i18n::t!("settings.general").to_string(),
            SettingsSection::Environments => rust_i18n::t!("settings.environments").to_string(),
            SettingsSection::About => rust_i18n::t!("settings.about").to_string(),
        }
    }
}

pub struct SettingsWindow {
    pub state: Entity<AppState>,
    pub kind: SettingsKind,
    pub envs_view: Entity<EnvironmentsView>,
    /// Active section in the secondary nav.
    pub active_section: SettingsSection,
    /// The current home-view setting (首页指向), read from layout on each
    /// render so external changes are picked up.
    pub home_view: SideView,
    /// Whether an update check is in progress (drives the settings-page spinner).
    pub update_checking: bool,
    /// The result of the last update check (shown in the settings page).
    pub update_check_result: Option<crate::updater::UpdateCheckResult>,
}

impl SettingsWindow {
    pub fn new(
        state: Entity<AppState>,
        kind: SettingsKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let envs_view = cx.new(|cx| EnvironmentsView::new(state.clone(), window, cx));
        let active_section = match kind {
            SettingsKind::Environments => SettingsSection::Environments,
            SettingsKind::General => SettingsSection::General,
        };
        let home_view = persistence::load_layout()
            .and_then(|l| l.home_view.as_deref().map(SideView::parse))
            .unwrap_or(SideView::Api);
        Self {
            state,
            kind,
            envs_view,
            active_section,
            home_view,
            update_checking: false,
            update_check_result: None,
        }
    }

    /// Persist a new home-view choice to `layout.json`.
    fn save_home_view(view: SideView) {
        let mut layout = persistence::load_layout().unwrap_or_default();
        layout.home_view = Some(view.name().to_string());
        let _ = persistence::save_layout(&layout);
    }

    /// Persist the UI locale to `layout.json`.
    fn save_locale(locale: &str) {
        let mut layout = persistence::load_layout().unwrap_or_default();
        layout.locale = Some(locale.to_string());
        let _ = persistence::save_layout(&layout);
    }

    /// Trigger a manual update check from the settings page.
    fn check_updates(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        cx.notify();

        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let result = crate::updater::run_check(client).await;
            let _ = this.update_in(cx, |this, _window, cx| {
                this.update_checking = false;
                this.update_check_result = Some(result);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let sheet_layer = gpui_component::Root::render_sheet_layer(window, cx);
        let dialog_layer = gpui_component::Root::render_dialog_layer(window, cx);
        let notification_layer = gpui_component::Root::render_notification_layer(window, cx);

        let active = self.active_section;
        let current_home = self.home_view;

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .text_color(theme.foreground)
            // ---- Title bar ----
            .child(
                h_flex()
                    .h(px(38.))
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .bg(theme.muted)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(rust_i18n::t!("settings.title").to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        gpui_component::button::Button::new("settings-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(rust_i18n::t!("settings.close").to_string())
                            .on_click(|_, window, _cx: &mut App| {
                                window.remove_window();
                            }),
                    ),
            )
            // ---- Body: secondary nav (left) + content (right) ----
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    // Secondary nav.
                    .child(
                        v_flex()
                            .w(px(160.))
                            .flex_shrink_0()
                            .h_full()
                            .border_r_1()
                            .border_color(border)
                            .bg(theme.muted)
                            .py_2()
                            .gap_1()
                            .px_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .px_2()
                                    .py_1()
                                    .child(rust_i18n::t!("settings.title").to_string()),
                            )
                            // General nav item.
                            .child({
                                let is_active = active == SettingsSection::General;
                                let fg = theme.foreground;
                                let accent = theme.accent;
                                let mf = muted;
                                div()
                                    .id("settings-nav-general")
                                    .px_2()
                                    .py_1p5()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_sm()
                                    .when(is_active, |d| {
                                        d.bg(accent.opacity(0.5)).text_color(fg)
                                    })
                                    .when(!is_active, |d| {
                                        d.text_color(mf).hover(|s| s.bg(accent.opacity(0.2)))
                                    })
                                    .child(SettingsSection::General.label().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_section = SettingsSection::General;
                                        cx.notify();
                                    }))
                            })
                            // Environments nav item.
                            .child({
                                let is_active = active == SettingsSection::Environments;
                                let fg = theme.foreground;
                                let accent = theme.accent;
                                let mf = muted;
                                div()
                                    .id("settings-nav-envs")
                                    .px_2()
                                    .py_1p5()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_sm()
                                    .when(is_active, |d| {
                                        d.bg(accent.opacity(0.5)).text_color(fg)
                                    })
                                    .when(!is_active, |d| {
                                        d.text_color(mf).hover(|s| s.bg(accent.opacity(0.2)))
                                    })
                                    .child(SettingsSection::Environments.label().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_section = SettingsSection::Environments;
                                        cx.notify();
                                    }))
                            })
                            // About nav item.
                            .child({
                                let is_active = active == SettingsSection::About;
                                let fg = theme.foreground;
                                let accent = theme.accent;
                                let mf = muted;
                                div()
                                    .id("settings-nav-about")
                                    .px_2()
                                    .py_1p5()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_sm()
                                    .when(is_active, |d| {
                                        d.bg(accent.opacity(0.5)).text_color(fg)
                                    })
                                    .when(!is_active, |d| {
                                        d.text_color(mf).hover(|s| s.bg(accent.opacity(0.2)))
                                    })
                                    .child(SettingsSection::About.label().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_section = SettingsSection::About;
                                        cx.notify();
                                    }))
                            }),
                    )
                    // Content.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .child(match active {
                                SettingsSection::General => {
                                    let theme_clone = theme.clone();
                                    v_flex()
                                        .size_full()
                                        .id("settings-general-scroll")
                                        .overflow_y_scroll()
                                        .p_6()
                                        .gap_6()
                                        // Section: 首页指向
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(rust_i18n::t!("settings.home_view").to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(muted)
                                                        .child(
                                                            rust_i18n::t!("settings.home_view_desc").to_string(),
                                                        ),
                                                )
                                                .child(
                                                    v_flex()
                                                        .mt_2()
                                                        .gap(px(2.))
                                                        .w(px(340.))
                                                        .children(SideView::ALL.iter().map(|&view| {
                                                            let is_current = view == current_home;
                                                            let tc = theme_clone.clone();
                                                            let tc2 = theme_clone.clone();
                                                            div()
                                                                .id(format!("general-home-{}", view.name()))
                                                                .px_3()
                                                                .py_2()
                                                                .rounded_md()
                                                                .border_1()
                                                                .cursor_pointer()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .when(is_current, |d| {
                                                                    d.border_color(tc.accent)
                                                                        .bg(tc.accent.opacity(0.1))
                                                                })
                                                                .when(!is_current, |d| {
                                                                    d.border_color(tc.border)
                                                                        .hover(|s| s.bg(tc.muted))
                                                                })
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(if is_current {
                                                                            tc.foreground
                                                                        } else {
                                                                            tc.muted_foreground
                                                                        })
                                                                        .child(view.label().to_string()),
                                                                )
                                                                .child(div().flex_1())
                                                                .when(is_current, |d| {
                                                                    d.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(tc.accent)
                                                                            .child(format!("✓ {}", rust_i18n::t!("settings.current"))),
                                                                    )
                                                                })
                                                                .on_click(move |_, _window, _cx: &mut App| {
                                                                    Self::save_home_view(view);
                                                                    let _ = tc2;
                                                                })
                                                        })),
                                                ),
                                        )
                                        // Section: 界面语言 / Interface Language
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(rust_i18n::t!("settings.language").to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(muted)
                                                        .child(rust_i18n::t!("settings.language_desc").to_string()),
                                                )
                                                .child(
                                                    v_flex()
                                                        .mt_2()
                                                        .gap(px(2.))
                                                        .w(px(340.))
                                                        .child({
                                                            let cur = rust_i18n::locale().to_string();
                                                            let is_zh = cur == "zh-CN" || cur.starts_with("zh");
                                                            let tc3 = theme_clone.clone();
                                                            div()
                                                                .id("lang-zh")
                                                                .px_3()
                                                                .py_2()
                                                                .rounded_md()
                                                                .border_1()
                                                                .cursor_pointer()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .when(is_zh, |d| {
                                                                    d.border_color(tc3.accent)
                                                                        .bg(tc3.accent.opacity(0.1))
                                                                })
                                                                .when(!is_zh, |d| {
                                                                    d.border_color(tc3.border)
                                                                        .hover(|s| s.bg(tc3.muted))
                                                                })
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(if is_zh {
                                                                            tc3.foreground
                                                                        } else {
                                                                            tc3.muted_foreground
                                                                        })
                                                                        .child("简体中文"),
                                                                )
                                                                .child(div().flex_1())
                                                                .when(is_zh, |d| {
                                                                    d.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(tc3.accent)
                                                                            .child(format!("✓ {}", rust_i18n::t!("settings.current"))),
                                                                    )
                                                                })
                                                                .on_click(cx.listener(|_this, _event, _window, cx| {
                                                                    rust_i18n::set_locale("zh-CN");
                                                                    Self::save_locale("zh-CN");
                                                                    cx.notify();
                                                                }))
                                                        })
                                                        .child({
                                                            let cur = rust_i18n::locale().to_string();
                                                            let is_en = cur == "en" || cur.starts_with("en");
                                                            let tc4 = theme_clone.clone();
                                                            div()
                                                                .id("lang-en")
                                                                .px_3()
                                                                .py_2()
                                                                .rounded_md()
                                                                .border_1()
                                                                .cursor_pointer()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .when(is_en, |d| {
                                                                    d.border_color(tc4.accent)
                                                                        .bg(tc4.accent.opacity(0.1))
                                                                })
                                                                .when(!is_en, |d| {
                                                                    d.border_color(tc4.border)
                                                                        .hover(|s| s.bg(tc4.muted))
                                                                })
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(if is_en {
                                                                            tc4.foreground
                                                                        } else {
                                                                            tc4.muted_foreground
                                                                        })
                                                                        .child("English"),
                                                                )
                                                                .child(div().flex_1())
                                                                .when(is_en, |d| {
                                                                    d.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(tc4.accent)
                                                                            .child(format!("✓ {}", rust_i18n::t!("settings.current"))),
                                                                    )
                                                                })
                                                                .on_click(cx.listener(|_this, _event, _window, cx| {
                                                                    rust_i18n::set_locale("en");
                                                                    Self::save_locale("en");
                                                                    cx.notify();
                                                                }))
                                                        }),
                                                ),
                                        )
                                        .into_any_element()
                                }
                                SettingsSection::Environments => {
                                    div()
                                        .size_full()
                                        .child(self.envs_view.clone())
                                        .into_any_element()
                                }
                                SettingsSection::About => {
                                    let theme_clone = theme.clone();
                                    let checking = self.update_checking;
                                    let result = self.update_check_result.clone();
                                    let accent = theme.accent;
                                    let muted_f = muted;

                                    v_flex()
                                        .size_full()
                                        .id("settings-about-scroll")
                                        .overflow_y_scroll()
                                        .p_6()
                                        .gap_6()
                                        // App name + version.
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child("Verve"),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(muted_f)
                                                        .child(format!(
                                                            "版本 v{}",
                                                            crate::updater::CURRENT_VERSION
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(muted_f)
                                                        .child(format!(
                                                            "GitHub: {}",
                                                            crate::updater::REPO
                                                        )),
                                                ),
                                        )
                                        // Check for updates.
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(rust_i18n::t!("update.title").to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(muted_f)
                                                        .child(rust_i18n::t!("update.desc").to_string()),
                                                )
                                                .child(
                                                    h_flex()
                                                        .mt_2()
                                                        .gap_2()
                                                        .items_center()
                                                        .child(
                                                            Button::new("about-check-update")
                                                                .small()
                                                                .label(if checking {
                                                                    rust_i18n::t!("update.checking").to_string()
                                                                } else {
                                                                    rust_i18n::t!("update.check_now").to_string()
                                                                })
                                                                .icon(Icon::from(IconName::Redo).path(REFRESH_CW))
                                                                .disabled(checking)
                                                                .on_click(cx.listener(|this, _ev, window, cx| {
                                                                    this.check_updates(window, cx);
                                                                })),
                                                        ),
                                                )
                                                // Result display.
                                                .child(
                                                    div().mt_2().child(match &result {
                                                        Some(crate::updater::UpdateCheckResult::UpdateAvailable(info)) => {
                                                            let release_url = info.release_url.clone();
                                                            let version = info.version.clone();
                                                            let notes = info.notes.clone();
                                                            v_flex()
                                                                .gap_2()
                                                                .p_3()
                                                                .rounded_md()
                                                                .border_1()
                                                                .border_color(accent.opacity(0.5))
                                                                .bg(accent.opacity(0.05))
                                                                .child(
                                                                    h_flex()
                                                                        .gap_2()
                                                                        .items_center()
                                                                        .child(
                                                                            div()
                                                                                .text_sm()
                                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                                .text_color(accent)
                                                                                .child(format!("🎉 发现新版本 v{}", version)),
                                                                        )
                                                                        .child(
                                                                            Button::new("about-download")
                                                                                .xsmall()
                                                                                .label(rust_i18n::t!("update.download").to_string())
                                                                                .icon(IconName::ExternalLink)
                                                                                .on_click(move |_, _window, cx: &mut App| {
                                                                                    cx.open_url(&release_url);
                                                                                }),
                                                                        ),
                                                                )
                                                                .when(!notes.is_empty(), |d| {
                                                                    d.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(muted_f)
                                                                            .max_w(px(600.))
                                                                            .child(notes),
                                                                    )
                                                                })
                                                                .into_any_element()
                                                        }
                                                        Some(crate::updater::UpdateCheckResult::UpToDate) => {
                                                            div()
                                                                .text_sm()
                                                                .text_color(muted_f)
                                                                .child(format!("✓ {}", rust_i18n::t!("update.up_to_date")))
                                                                .into_any_element()
                                                        }
                                                        Some(crate::updater::UpdateCheckResult::Error(e)) => {
                                                            div()
                                                                .text_sm()
                                                                .text_color(theme_clone.danger)
                                                                .child(format!("❌ {}", e))
                                                                .into_any_element()
                                                        }
                                                        None => div().into_any_element(),
                                                    }),
                                                ),
                                        )
                                        .into_any_element()
                                }
                            }),
                    ),
            )
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}
