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
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, IconName, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::assets::REFRESH_CW;
use crate::state::AppState;
use crate::state::persistence;
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
    /// After-send behavior (autosave examples, tab switching).
    AfterSend,
    /// About / version / update check.
    About,
}

impl SettingsSection {
    pub fn label(self) -> String {
        match self {
            SettingsSection::General => rust_i18n::t!("settings.general").to_string(),
            SettingsSection::Environments => rust_i18n::t!("settings.environments").to_string(),
            SettingsSection::AfterSend => "发送后设置".to_string(),
            SettingsSection::About => rust_i18n::t!("settings.about").to_string(),
        }
    }
}

/// Autosave example mode options shown in the dropdown.
const AUTOSAVE_MODES: [crate::state::persistence::AutoSaveMode; 4] = [
    crate::state::persistence::AutoSaveMode::Off,
    crate::state::persistence::AutoSaveMode::SaveSuccess,
    crate::state::persistence::AutoSaveMode::SaveFailure,
    crate::state::persistence::AutoSaveMode::SaveBoth,
];

/// Preset sync intervals (minutes) shown in the dropdown.
const SYNC_INTERVALS: [u32; 6] = [5, 10, 15, 30, 60, 120];

/// Available UI languages: (locale code, display label).
const LANGUAGES: [(&str, &str); 2] = [("zh-CN", "简体中文"), ("en", "English")];

pub struct SettingsWindow {
    pub state: Entity<AppState>,
    pub kind: SettingsKind,
    pub envs_view: Entity<EnvironmentsView>,
    /// Active section in the secondary nav.
    pub active_section: SettingsSection,
    /// The current home-view setting (首页指向), read from layout on each
    /// render so external changes are picked up.
    pub home_view: SideView,
    /// Which activity-rail items are currently hidden.
    pub hidden_rails: std::collections::HashSet<String>,
    /// Whether an update check is in progress (drives the settings-page spinner).
    pub update_checking: bool,
    /// The result of the last update check (shown in the settings page).
    pub update_check_result: Option<crate::updater::UpdateCheckResult>,
    /// Currently configured git auto-sync interval in minutes.
    pub sync_interval_minutes: u32,
    /// After-send response-example autosave mode.
    pub autosave_mode: crate::state::persistence::AutoSaveMode,
    /// Dropdown for the home-view (首页指向) selector.
    pub home_view_select: Entity<SelectState<Vec<String>>>,
    /// Dropdown for the git auto-sync interval selector.
    pub sync_interval_select: Entity<SelectState<Vec<String>>>,
    /// Dropdown for the interface language selector.
    pub language_select: Entity<SelectState<Vec<String>>>,
    /// Dropdown for the autosave response-examples setting.
    pub autosave_select: Entity<SelectState<Vec<String>>>,
    _subs: Vec<gpui::Subscription>,
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
        let hidden_rails = persistence::load_hidden_rails();
        let sync_interval_minutes = persistence::load_sync_interval_minutes();
        let autosave_mode = persistence::load_autosave_mode();

        // Build dropdowns.
        let home_options: Vec<String> = SideView::ALL.iter().map(|v| v.label()).collect();
        let home_idx = SideView::ALL
            .iter()
            .position(|&v| v == home_view)
            .unwrap_or(0);
        let home_view_select = cx.new(|cx| {
            SelectState::new(
                home_options,
                Some(gpui_component::IndexPath::new(home_idx)),
                window,
                cx,
            )
        });

        let min_suffix = rust_i18n::t!("settings.minutes").to_string();
        let sync_options: Vec<String> = SYNC_INTERVALS
            .iter()
            .map(|&m| format!("{} {}", m, min_suffix))
            .collect();
        let sync_idx = SYNC_INTERVALS
            .iter()
            .position(|&m| m == sync_interval_minutes)
            .unwrap_or(3); // default to 30
        let sync_interval_select = cx.new(|cx| {
            SelectState::new(
                sync_options,
                Some(gpui_component::IndexPath::new(sync_idx)),
                window,
                cx,
            )
        });

        let cur_locale = rust_i18n::locale().to_string();
        let lang_options: Vec<String> = LANGUAGES
            .iter()
            .map(|(_, label)| label.to_string())
            .collect();
        let lang_idx = LANGUAGES
            .iter()
            .position(|(code, _)| *code == cur_locale || cur_locale.starts_with(code))
            .unwrap_or(0);
        let language_select = cx.new(|cx| {
            SelectState::new(
                lang_options,
                Some(gpui_component::IndexPath::new(lang_idx)),
                window,
                cx,
            )
        });

        let autosave_options: Vec<String> = AUTOSAVE_MODES.iter().map(|m| m.label().to_string()).collect();
        let autosave_idx = AUTOSAVE_MODES
            .iter()
            .position(|&m| m == autosave_mode)
            .unwrap_or(0);
        let autosave_select = cx.new(|cx| {
            SelectState::new(
                autosave_options,
                Some(gpui_component::IndexPath::new(autosave_idx)),
                window,
                cx,
            )
        });

        let sub_home = cx.subscribe(&home_view_select, Self::on_home_view_change);
        let sub_sync = cx.subscribe(&sync_interval_select, Self::on_sync_interval_change);
        let sub_lang = cx.subscribe(&language_select, Self::on_language_change);
        let sub_autosave = cx.subscribe(&autosave_select, Self::on_autosave_change);

        Self {
            state,
            kind,
            envs_view,
            active_section,
            home_view,
            hidden_rails,
            update_checking: false,
            update_check_result: None,
            sync_interval_minutes,
            autosave_mode,
            home_view_select,
            sync_interval_select,
            language_select,
            autosave_select,
            _subs: vec![sub_home, sub_sync, sub_lang, sub_autosave],
        }
    }

    /// Persist a new home-view choice to `layout.json`.
    fn save_home_view(view: SideView) {
        let mut layout = persistence::load_layout().unwrap_or_default();
        layout.home_view = Some(view.name().to_string());
        let _ = persistence::save_layout(&layout);
    }

    /// Toggle visibility of a single rail item.
    fn toggle_rail(&mut self, view: SideView) {
        let name = view.name();
        if self.hidden_rails.contains(name) {
            self.hidden_rails.remove(name);
        } else {
            self.hidden_rails.insert(name.to_string());
        }
        persistence::save_hidden_rails(&self.hidden_rails);
    }

    /// Persist the UI locale to `layout.json`.
    fn save_locale(locale: &str) {
        let mut layout = persistence::load_layout().unwrap_or_default();
        layout.locale = Some(locale.to_string());
        let _ = persistence::save_layout(&layout);
    }

    /// React to the home-view dropdown changing selection.
    fn on_home_view_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = src.read(cx).selected_index(cx) {
            if let Some(&view) = SideView::ALL.get(idx.row) {
                Self::save_home_view(view);
                self.home_view = view;
                cx.notify();
            }
        }
    }

    /// React to the sync-interval dropdown changing selection.
    fn on_sync_interval_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = src.read(cx).selected_index(cx) {
            if let Some(&mins) = SYNC_INTERVALS.get(idx.row) {
                persistence::save_sync_interval_minutes(mins);
                self.sync_interval_minutes = mins;
                cx.notify();
            }
        }
    }

    /// React to the language dropdown changing selection.
    fn on_language_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = src.read(cx).selected_index(cx) {
            if let Some(&(code, _)) = LANGUAGES.get(idx.row) {
                rust_i18n::set_locale(code);
                Self::save_locale(code);
                cx.refresh_windows();
                cx.notify();
            }
        }
    }

    /// React to the autosave-examples dropdown changing selection.
    fn on_autosave_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = src.read(cx).selected_index(cx) {
            if let Some(&mode) = AUTOSAVE_MODES.get(idx.row) {
                persistence::save_autosave_mode(mode);
                self.autosave_mode = mode;
                cx.notify();
            }
        }
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
        // Overlay layers are Root-owned since gpui-component 0.7.0.

        let active = self.active_section;
        // Re-read persisted settings on each render so external changes (or
        // click handlers that write directly to disk) are reflected instantly.
        if let Some(layout) = persistence::load_layout() {
            if let Some(name) = layout.home_view.as_deref() {
                let parsed = SideView::parse(name);
                if parsed != self.home_view {
                    self.home_view = parsed;
                }
            }
        }
        let new_hidden = persistence::load_hidden_rails();
        if new_hidden != self.hidden_rails {
            self.hidden_rails = new_hidden;
        }
        let new_sync = persistence::load_sync_interval_minutes();
        if new_sync != self.sync_interval_minutes {
            self.sync_interval_minutes = new_sync;
        }
        let new_autosave = persistence::load_autosave_mode();
        if new_autosave != self.autosave_mode {
            self.autosave_mode = new_autosave;
        }

        // Keep the dropdown selections in sync with persisted state when it
        // changes from outside this window.
        let home_idx = SideView::ALL
            .iter()
            .position(|&v| v == self.home_view)
            .unwrap_or(0);
        self.home_view_select.update(cx, |s, cx| {
            let cur = s.selected_index(cx).map(|p| p.row).unwrap_or(usize::MAX);
            if cur != home_idx {
                s.set_selected_index(Some(gpui_component::IndexPath::new(home_idx)), window, cx);
            }
        });
        let sync_idx = SYNC_INTERVALS
            .iter()
            .position(|&m| m == self.sync_interval_minutes)
            .unwrap_or(3);
        self.sync_interval_select.update(cx, |s, cx| {
            let cur = s.selected_index(cx).map(|p| p.row).unwrap_or(usize::MAX);
            if cur != sync_idx {
                s.set_selected_index(Some(gpui_component::IndexPath::new(sync_idx)), window, cx);
            }
        });
        let cur_locale = rust_i18n::locale().to_string();
        let lang_idx = LANGUAGES
            .iter()
            .position(|(code, _)| *code == cur_locale || cur_locale.starts_with(code))
            .unwrap_or(0);
        self.language_select.update(cx, |s, cx| {
            let cur = s.selected_index(cx).map(|p| p.row).unwrap_or(usize::MAX);
            if cur != lang_idx {
                s.set_selected_index(Some(gpui_component::IndexPath::new(lang_idx)), window, cx);
            }
        });
        let autosave_idx = AUTOSAVE_MODES
            .iter()
            .position(|&m| m == self.autosave_mode)
            .unwrap_or(0);
        self.autosave_select.update(cx, |s, cx| {
            let cur = s.selected_index(cx).map(|p| p.row).unwrap_or(usize::MAX);
            if cur != autosave_idx {
                s.set_selected_index(Some(gpui_component::IndexPath::new(autosave_idx)), window, cx);
            }
        });

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .text_color(theme.foreground)
            // ---- Title bar ----
            // TitleBar provides the drag region plus drawn min/max/close on
            // Windows/Linux. macOS gets no native chrome for this window
            // (titlebar: None), so the self-drawn close button stays there
            // only. Left padding is forced to 12px because the 80px default
            // assumes macOS traffic lights, which this window doesn't have.
            .child(
                TitleBar::new()
                    .h(px(38.))
                    .pl_3()
                    .bg(theme.muted)
                    .border_color(border)
                    .on_close_window(|_, window, _cx| window.remove_window())
                    .child(
                        h_flex()
                            .h_full()
                            .w_full()
                            .min_w_0()
                            .pr_3()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(rust_i18n::t!("settings.title").to_string()),
                            )
                            .child(div().flex_1())
                            .when(cfg!(target_os = "macos"), |row| {
                                row.child(
                                    gpui_component::button::Button::new("settings-close")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip(rust_i18n::t!("settings.close").to_string())
                                        .on_click(|_, window, _cx: &mut App| {
                                            window.remove_window();
                                        }),
                                )
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
                            // AfterSend nav item.
                            .child({
                                let is_active = active == SettingsSection::AfterSend;
                                let fg = theme.foreground;
                                let accent = theme.accent;
                                let mf = muted;
                                div()
                                    .id("settings-nav-after-send")
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
                                    .child(SettingsSection::AfterSend.label().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_section = SettingsSection::AfterSend;
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
                                    let home_sel = self.home_view_select.clone();
                                    let sync_sel = self.sync_interval_select.clone();
                                    let lang_sel = self.language_select.clone();
                                    v_flex()
                                        .size_full()
                                        .id("settings-general-scroll")
                                        .overflow_y_scroll()
                                        .p_6()
                                        .gap_6()
                                        // Section: 首页指向 (dropdown)
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
                                                    div()
                                                        .mt_2()
                                                        .w(px(340.))
                                                        .child(Select::new(&home_sel).small().appearance(true)),
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
                                                    div()
                                                        .mt_2()
                                                        .w(px(340.))
                                                        .child(Select::new(&lang_sel).small().appearance(true)),
                                                ),
                                        )
                                        // Section: 活动栏显示 / Activity Bar Items
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(rust_i18n::t!("settings.activity_bar").to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(muted)
                                                        .child(rust_i18n::t!("settings.activity_bar_desc").to_string()),
                                                )
                                                .child(
                                                    v_flex()
                                                        .mt_2()
                                                        .gap(px(4.))
                                                        .w(px(340.))
                                                        .children({
                                                            // Snapshot hidden set for use in click handlers.
                                                            let hidden_set: std::collections::HashSet<String> = self.hidden_rails.clone();
                                                            SideView::ALL.iter().map(move |&view| {
                                                                let name = view.name().to_string();
                                                                let is_hidden = hidden_set.contains(&name);
                                                                let tc = theme.clone();
                                                                div()
                                                                    .id(format!("rail-toggle-{}", view.name()))
                                                                    .px_3()
                                                                    .py_2()
                                                                    .rounded_md()
                                                                    .border_1()
                                                                    .cursor_pointer()
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap_2()
                                                                    .border_color(tc.border)
                                                                    .hover(|s| s.bg(tc.muted))
                                                                    .child(
                                                                        div()
                                                                            .text_sm()
                                                                            .text_color(tc.foreground)
                                                                            .child(view.label().to_string()),
                                                                    )
                                                                    .child(div().flex_1())
                                                                    .child(
                                                                        div()
                                                                            .px_2()
                                                                            .py(px(1.))
                                                                            .rounded(px(3.))
                                                                            .text_xs()
                                                                            .when(!is_hidden, |d| {
                                                                                d.bg(tc.accent.opacity(0.2))
                                                                                    .text_color(tc.accent)
                                                                            })
                                                                            .when(is_hidden, |d| {
                                                                                d.bg(tc.muted)
                                                                                    .text_color(tc.muted_foreground)
                                                                            })
                                                                            .child(if is_hidden {
                                                                                rust_i18n::t!("settings.hide").to_string()
                                                                            } else {
                                                                                rust_i18n::t!("settings.show").to_string()
                                                                            }),
                                                                    )
                                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                                        this.toggle_rail(view);
                                                                        cx.notify();
                                                                    }))
                                                            })
                                                        }),
                                                )
                                        )
                                        // Section: Git 自动同步间隔 (dropdown)
                                        .child({
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(rust_i18n::t!("settings.sync_interval").to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(muted)
                                                        .child(rust_i18n::t!("settings.sync_interval_desc").to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .mt_2()
                                                        .w(px(340.))
                                                        .child(Select::new(&sync_sel).small().appearance(true)),
                                                )
                                        })
                                        .into_any_element()
                                }
                                SettingsSection::Environments => {
                                    div()
                                        .size_full()
                                        .child(self.envs_view.clone())
                                        .into_any_element()
                                }
                                SettingsSection::AfterSend => {
                                    let autosave_sel = self.autosave_select.clone();
                                    v_flex()
                                        .size_full()
                                        .id("settings-after-send-scroll")
                                        .overflow_y_scroll()
                                        .p_6()
                                        .gap_6()
                                        // Section: 发送请求后结果保存响应示例
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child("发送请求后结果保存响应示例"),
                                                )
                                                .child(
                                                    div()
                                                        .mt_2()
                                                        .w(px(340.))
                                                        .child(Select::new(&autosave_sel).small().appearance(true)),
                                                )
                                        )
                                        .into_any_element()
                                }
                                SettingsSection::About => {
                                    let theme_clone = theme.clone();
                                    let checking = self.update_checking;
                                    let result = self.update_check_result.clone();
                                    let accent = theme.accent;
                                    let muted_f = muted;
                                    let repo_url = format!("https://github.com/{}", crate::updater::REPO);

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
                                                    h_flex()
                                                        .gap_2()
                                                        .items_center()
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .text_color(muted_f)
                                                                .child("GitHub: "),
                                                        )
                                                        .child(
                                                            Button::new("about-github-link")
                                                                .ghost()
                                                                .xsmall()
                                                                .label(crate::updater::REPO)
                                                                .icon(IconName::ExternalLink)
                                                                .on_click(move |_, _window, cx: &mut App| {
                                                                    cx.open_url(&repo_url);
                                                                }),
                                                        ),
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
    }
}
