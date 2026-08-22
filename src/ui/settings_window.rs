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
use gpui_component::input::{Input, InputEvent, InputState};
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
    /// The AI providers / assistant-behaviour page.
    Ai,
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
    /// Remote document-sharing server settings.
    Remote,
    /// WYSIWYG editor keyboard-shortcut mapping + customization.
    Shortcuts,
    /// About / version / update check.
    About,
    /// Media storage (local + S3) for note images and videos.
    Media,
    /// AI providers (OpenAI-compatible) and assistant behaviour.
    Ai,
}

impl SettingsSection {
    pub fn label(self) -> String {
        match self {
            SettingsSection::General => rust_i18n::t!("settings.general").to_string(),
            SettingsSection::Environments => rust_i18n::t!("settings.environments").to_string(),
            SettingsSection::AfterSend => "发送后设置".to_string(),
            SettingsSection::Remote => rust_i18n::t!("settings.remote").to_string(),
            SettingsSection::Shortcuts => rust_i18n::t!("settings.shortcuts").to_string(),
            SettingsSection::About => rust_i18n::t!("settings.about").to_string(),
            SettingsSection::Media => rust_i18n::t!("settings.media.title").to_string(),
            SettingsSection::Ai => rust_i18n::t!("settings.ai.title").to_string(),
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
    /// Remote document-sharing server configuration.
    pub remote: crate::state::persistence::RemoteConfig,
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
    /// Text input for the publicly-advertised remote service address.
    pub remote_address_input: Entity<InputState>,
    /// Shortcut id currently being rebound (capture mode), or None.
    pub editing_shortcut: Option<String>,
    /// Pending keystroke captured during edit, before confirm.
    pub pending_keystroke: Option<String>,
    /// In-memory draft of custom shortcuts (id → keys), saved on confirm.
    pub shortcut_draft: std::collections::BTreeMap<String, Vec<String>>,
    /// Text inputs bound to `media_storage.toml` fields (saved on change/blur).
    media_inputs: MediaInputs,
    /// Last S3 connectivity-test result (Idle / Testing / Ok / Err).
    media_test: MediaTestState,
    /// AI global: max agent loop iterations (1-100).
    ai_iterations_input: Entity<InputState>,
    /// AI global: max 429 rate-limit retries (0-100, stepped backoff).
    ai_retries_input: Entity<InputState>,
    /// AI global: default agent mode dropdown.
    ai_mode_select: Entity<SelectState<Vec<String>>>,
    /// "Add provider" preset dropdown.
    ai_preset_select: Entity<SelectState<Vec<String>>>,
    _subs: Vec<gpui::Subscription>,
}

/// Stable `InputState`/`SelectState` entities for one AI provider card.
/// Persisted on change via `SettingsWindow::on_ai_provider_input_change`.
/// State of the S3 "Test connection" button.
#[derive(Clone, Debug, Default)]
enum MediaTestState {
    #[default]
    Idle,
    Testing,
    Ok,
    Err(String),
}

/// Stable `InputState` entities for the Media settings cards. Each is subscribed
/// to [`SettingsWindow::on_media_input_change`], which dispatches by entity
/// identity to the matching `media_storage.toml` field.
struct MediaInputs {
    image_prefix: Entity<InputState>,
    image_max_mb: Entity<InputState>,
    video_prefix: Entity<InputState>,
    video_threshold_mb: Entity<InputState>,
    video_part_mb: Entity<InputState>,
    s3_endpoint: Entity<InputState>,
    s3_bucket: Entity<InputState>,
    s3_region: Entity<InputState>,
    s3_access_key: Entity<InputState>,
    s3_secret_key: Entity<InputState>,
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
            SettingsKind::Ai => SettingsSection::Ai,
        };
        let home_view = persistence::load_layout()
            .and_then(|l| l.home_view.as_deref().map(SideView::parse))
            .unwrap_or(SideView::Api);
        let hidden_rails = persistence::load_hidden_rails();
        let sync_interval_minutes = persistence::load_sync_interval_minutes();
        let remote = persistence::load_remote_config();
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

        let initial_addr = remote.public_host.clone().unwrap_or_default();
        let remote_address_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rust_i18n::t!("settings.remote_address_ph").to_string())
                .default_value(&initial_addr)
        });

        // Media storage inputs — each bound to a `media_storage.toml` field and
        // persisted via `on_media_input_change`.
        let media_cfg = crate::media::config::load_media_config();
        let media_inputs = MediaInputs {
            image_prefix: cx.new(|cx| {
                InputState::new(window, cx).default_value(&media_cfg.image.s3_prefix)
            }),
            image_max_mb: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(&media_cfg.image.max_size_mb.to_string())
            }),
            video_prefix: cx.new(|cx| {
                InputState::new(window, cx).default_value(&media_cfg.video.s3_prefix)
            }),
            video_threshold_mb: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(&media_cfg.video.multipart_threshold_mb.to_string())
            }),
            video_part_mb: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(&media_cfg.video.multipart_part_mb.to_string())
            }),
            s3_endpoint: cx.new(|cx| {
                InputState::new(window, cx).default_value(&media_cfg.s3.endpoint)
            }),
            s3_bucket: cx.new(|cx| {
                InputState::new(window, cx).default_value(&media_cfg.s3.bucket)
            }),
            s3_region: cx.new(|cx| {
                InputState::new(window, cx).default_value(&media_cfg.s3.region)
            }),
            s3_access_key: cx.new(|cx| {
                InputState::new(window, cx).default_value(&media_cfg.s3.access_key)
            }),
            s3_secret_key: cx.new(|cx| {
                InputState::new(window, cx).default_value(&media_cfg.s3.secret_key)
            }),
        };
        let media_subs: Vec<gpui::Subscription> = [
            media_inputs.image_prefix.clone(),
            media_inputs.image_max_mb.clone(),
            media_inputs.video_prefix.clone(),
            media_inputs.video_threshold_mb.clone(),
            media_inputs.video_part_mb.clone(),
            media_inputs.s3_endpoint.clone(),
            media_inputs.s3_bucket.clone(),
            media_inputs.s3_region.clone(),
            media_inputs.s3_access_key.clone(),
            media_inputs.s3_secret_key.clone(),
        ]
        .iter()
        .map(|inp| cx.subscribe(inp, Self::on_media_input_change))
        .collect();

        // AI settings inputs — global fields + provider cards reconciled
        // against ai_providers.json.
        let ai_cfg = crate::ai::config::load_ai_settings();
        let ai_iterations_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(ai_cfg.clamped_iterations().to_string())
        });
        let ai_retries_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(ai_cfg.clamped_rate_retries().to_string())
        });
        let ai_mode_options = vec![
            rust_i18n::t!("settings.ai.mode_plan").to_string(),
            rust_i18n::t!("settings.ai.mode_full").to_string(),
        ];
        let ai_mode_idx = match ai_cfg.agent_mode {
            crate::ai::config::AgentMode::FullAccess => 1,
            crate::ai::config::AgentMode::Plan => 0,
        };
        let ai_mode_select = cx.new(|cx| {
            SelectState::new(
                ai_mode_options,
                Some(gpui_component::IndexPath::new(ai_mode_idx)),
                window,
                cx,
            )
        });
        let ai_preset_options = crate::ai::config::presets()
            .iter()
            .map(|p| p.name.to_string())
            .collect::<Vec<_>>();
        let ai_preset_select = cx.new(|cx| {
            SelectState::new(
                ai_preset_options,
                Some(gpui_component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        let sub_ai_iters = cx.subscribe(&ai_iterations_input, Self::on_ai_global_input_change);
        let sub_ai_retries = cx.subscribe(&ai_retries_input, Self::on_ai_global_input_change);
        let sub_ai_mode = cx.subscribe(&ai_mode_select, Self::on_ai_mode_change);

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

        let autosave_options: Vec<String> = AUTOSAVE_MODES
            .iter()
            .map(|m| m.label().to_string())
            .collect();
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
        let sub_addr = cx.subscribe(&remote_address_input, Self::on_remote_address_change);
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
            remote,
            autosave_mode,
            home_view_select,
            sync_interval_select,
            language_select,
            autosave_select,
            remote_address_input,
            media_inputs,
            media_test: MediaTestState::Idle,
            ai_iterations_input,
            ai_retries_input,
            ai_mode_select,
            ai_preset_select,
            editing_shortcut: None,
            pending_keystroke: None,
            shortcut_draft: crate::state::persistence::load_shortcuts(),
            _subs: {
                let mut v = vec![sub_home, sub_sync, sub_lang, sub_addr, sub_autosave];
                v.extend(media_subs);
                v.push(sub_ai_iters);
                v.push(sub_ai_retries);
                v.push(sub_ai_mode);
                v
            },
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
                // Keep the WYSIWYG editor's I18nManager in sync so its context
                // menus / dialogs follow the new language immediately.
                crate::wysiwyg::sync::sync_editor_language(code, cx);
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

    /// React to the remote address input changing / losing focus — persist
    /// the typed value as `public_host`.
    fn on_remote_address_change(
        &mut self,
        src: Entity<InputState>,
        ev: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(ev, InputEvent::Change | InputEvent::Blur) {
            return;
        }
        let val = src.read(cx).value().trim().to_string();
        let mut r = persistence::load_remote_config();
        if val.is_empty() {
            r.public_host = None;
        } else {
            r.public_host = Some(val);
        }
        persistence::save_remote_config(&r);
        self.remote = r;
        // No cx.notify() on every keystroke to avoid re-render loops; the
        // render path already re-reads persisted config if it changed on disk.
    }

    /// Persist a media-storage text field on change/blur. Dispatches by entity
    /// identity to the matching `media_storage.toml` field; numeric fields fall
    /// back to the existing value when the input is not a valid integer.
    fn on_media_input_change(
        &mut self,
        src: Entity<InputState>,
        ev: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(ev, InputEvent::Change | InputEvent::Blur) {
            return;
        }
        let val = src.read(cx).value().trim().to_string();
        let mut cfg = crate::media::config::load_media_config();
        let m = &self.media_inputs;
        let changed = if src == m.image_prefix {
            cfg.image.s3_prefix = val;
            true
        } else if src == m.image_max_mb {
            if let Ok(n) = val.parse::<u64>() {
                cfg.image.max_size_mb = n;
            }
            true
        } else if src == m.video_prefix {
            cfg.video.s3_prefix = val;
            true
        } else if src == m.video_threshold_mb {
            if let Ok(n) = val.parse::<u64>() {
                cfg.video.multipart_threshold_mb = n;
            }
            true
        } else if src == m.video_part_mb {
            if let Ok(n) = val.parse::<u64>() {
                cfg.video.multipart_part_mb = n;
            }
            true
        } else if src == m.s3_endpoint {
            cfg.s3.endpoint = val;
            true
        } else if src == m.s3_bucket {
            cfg.s3.bucket = val;
            true
        } else if src == m.s3_region {
            cfg.s3.region = val;
            true
        } else if src == m.s3_access_key {
            cfg.s3.access_key = val;
            true
        } else if src == m.s3_secret_key {
            cfg.s3.secret_key = val;
            true
        } else {
            false
        };
        if changed {
            let _ = crate::media::config::save_media_config(&cfg);
        }
    }

    // -- AI section ---------------------------------------------------------

    /// Load → mutate → save `ai_providers.json` in one step.
    fn update_ai_settings(cx: &mut Context<Self>, f: impl FnOnce(&mut crate::ai::config::AiSettings)) {
        let mut s = crate::ai::config::load_ai_settings();
        f(&mut s);
        if let Err(e) = crate::ai::config::save_ai_settings(&s) {
            log::error!("save ai settings: {e}");
        }
        cx.notify();
    }

    /// Create the input entities for one provider card.
    fn on_ai_global_input_change(
        &mut self,
        src: Entity<InputState>,
        ev: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(ev, InputEvent::Change | InputEvent::Blur) {
            return;
        }
        let val = src.read(cx).value().trim().to_string();
        if src == self.ai_iterations_input {
            let n = val.parse::<usize>().unwrap_or(50);
            Self::update_ai_settings(cx, |s| {
                s.max_agent_iterations = n.clamp(1, 100);
            });
        } else if src == self.ai_retries_input {
            let n = val.parse::<usize>().unwrap_or(20);
            Self::update_ai_settings(cx, |s| {
                s.max_rate_limit_retries = n.clamp(0, 100);
            });
        }
    }

    /// Persist the default agent-mode dropdown.
    fn on_ai_mode_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let Some(idx) = src.read(cx).selected_index(cx) else {
            return;
        };
        let mode = match idx.row {
            1 => crate::ai::config::AgentMode::FullAccess,
            _ => crate::ai::config::AgentMode::Plan,
        };
        Self::update_ai_settings(cx, |s| s.agent_mode = mode);
    }

    /// Persist one provider-card text field. Dispatches by entity identity.
    fn add_ai_provider(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let preset_idx = self
            .ai_preset_select
            .read(cx)
            .selected_index(cx)
            .map(|i| i.row)
            .unwrap_or(0);
        let Some(preset) = crate::ai::config::presets().get(preset_idx) else {
            return;
        };
        let provider = crate::ai::config::AiProviderConfig::from_preset(preset);
        let id = provider.id.clone();
        let is_first = crate::ai::config::load_ai_settings()
            .providers
            .is_empty();
        Self::update_ai_settings(cx, |s| {
            s.providers.push(provider);
            if is_first || s.active_provider_id.is_none() {
                s.active_provider_id = Some(id.clone());
            }
        });
        // Jump straight into the edit dialog so the key/model get filled.
        let settings = crate::ai::config::load_ai_settings();
        if let Some(p) = settings.providers.iter().find(|p| p.id == id) {
            crate::ui::ai_provider_dialog::AiProviderDialog::open(
                Some(p),
                settings.active_provider_id.as_deref(),
                cx,
            );
        }
    }

    /// Delete a provider (and its vault key). Keeps another provider active.
    fn set_ai_active_provider(&mut self, id: &str, cx: &mut Context<Self>) {
        Self::update_ai_settings(cx, |s| {
            s.active_provider_id = Some(id.to_string());
        });
    }

    /// Toggle a provider bool flag (enabled / vision / tools).
    fn render_ai_section(
        &mut self,
        _window: &mut Window,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = theme.muted_foreground;
        let settings = crate::ai::config::load_ai_settings();
        let active_id = settings.active_provider_id.clone();

        let section = v_flex()
            .size_full()
            .id("settings-ai-scroll")
            .overflow_y_scroll()
            .p_6()
            .gap_6()
            // ---- Global behaviour card ----
            .child(media_card(
                rust_i18n::t!("settings.ai.card_global").to_string(),
                rust_i18n::t!("settings.ai.card_global_desc").to_string(),
                muted,
                v_flex()
                    .gap_3()
                    .child(media_field(&self.ai_iterations_input, "最大循环次数（1-100）", muted))
                    .child(media_field(
                        &self.ai_retries_input,
                        "429 限流最大重试次数（每 10s 重试一次，0-100）",
                        muted,
                    ))
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(340.))
                            .child(div().text_sm().text_color(muted).child("默认执行模式"))
                            .child(
                                div().w(px(340.)).child(
                                    Select::new(&self.ai_mode_select).small().appearance(true),
                                ),
                            ),
                    )
                    .into_any_element(),
            ))
            // ---- Providers list card ----
            .child(media_card(
                rust_i18n::t!("settings.ai.card_providers").to_string(),
                rust_i18n::t!("settings.ai.card_providers_desc").to_string(),
                muted,
                v_flex()
                    .gap_2()
                    // Add-from-preset row.
                    .child(
                        h_flex()
                            .gap_2()
                            .w(px(420.))
                            .child(
                                div().flex_1().child(
                                    Select::new(&self.ai_preset_select).small().appearance(true),
                                ),
                            )
                            .child(
                                Button::new("ai-add-provider")
                                    .primary()
                                    .small()
                                    .label(rust_i18n::t!("settings.ai.add").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_ai_provider(window, cx);
                                    })),
                            ),
                    )
                    .children(if settings.providers.is_empty() {
                        vec![div()
                            .text_sm()
                            .text_color(muted)
                            .child(rust_i18n::t!("settings.ai.empty").to_string())
                            .into_any_element()]
                    } else {
                        Vec::new()
                    })
                    // Compact provider rows: click to edit in a dialog.
                    .children(settings.providers.iter().map(|p| {
                        let id = p.id.clone();
                        let is_active = active_id.as_deref() == Some(p.id.as_str());
                        let theme_c = theme.clone();
                        let name_model = format!("{} · {}", p.name, p.model);
                        let (dot_color, dim) = if p.enabled {
                            (theme.accent, 1.0)
                        } else {
                            (theme.muted_foreground, 0.55)
                        };
                        h_flex()
                            .id(format!("ai-provider-row-{}", p.id))
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .opacity(dim)
                            .cursor_pointer()
                            .hover(move |d| d.bg(theme_c.muted))
                            .gap_2()
                            .items_center()
                            .child(div().size(px(8.)).rounded_full().bg(dot_color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .child(name_model),
                            )
                            .child(if p.api_key_set {
                                div().text_xs().text_color(muted).child("🔑")
                            } else {
                                div()
                                    .text_xs()
                                    .text_color(theme.danger)
                                    .child("未设密钥")
                            })
                            .child(if is_active {
                                div()
                                    .text_xs()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .bg(theme.accent.opacity(0.15))
                                    .child("✓ 默认")
                            } else {
                                div().text_xs().text_color(muted).child("　")
                            })
                            .when(!is_active, |row| {
                                row.child(
                                    div()
                                        .id(format!("ai-row-default-{}", p.id))
                                        .text_xs()
                                        .text_color(muted)
                                        .cursor_pointer()
                                        .child("设为默认")
                                        .on_click({
                                            let id = id.clone();
                                            move |_, _, cx: &mut App| {
                                                cx.stop_propagation();
                                                let mut s =
                                                    crate::ai::config::load_ai_settings();
                                                s.active_provider_id = Some(id.clone());
                                                let _ = crate::ai::config::save_ai_settings(&s);
                                            }
                                        }),
                                )
                            })
                            .on_click(move |_, _window, cx: &mut App| {
                                let settings = crate::ai::config::load_ai_settings();
                                let provider =
                                    settings.providers.iter().find(|p| p.id == id).cloned();
                                crate::ui::ai_provider_dialog::AiProviderDialog::open(
                                    provider.as_ref(),
                                    settings.active_provider_id.as_deref(),
                                    cx,
                                );
                            })
                            .into_any_element()
                    }))
                    .into_any_element(),
            ));
        section
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
    // -- Shortcuts section --------------------------------------------------

    /// Renders the keyboard-shortcut mapping viewer + editor.
    fn render_shortcuts_section(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_component::h_flex;
        use gpui_component::kbd::Kbd;

        let definitions = crate::wysiwyg::components::shortcut_definitions();
        let draft = self.shortcut_draft.clone();
        let editing = self.editing_shortcut.clone();
        let pending = self.pending_keystroke.clone();
        let muted = theme.muted_foreground;
        let accent = theme.accent;
        let border = theme.border;
        let fg = theme.foreground;

        // Only show shortcuts that are actually registered (BlockEditor context
        // + viewport nav + chrome toggles). Skip window-level actions filtered
        // out by resolved_keybindings.
        let is_visible = |def: &crate::wysiwyg::components::ShortcutDefinition| {
            def.context.is_some()
                || crate::wysiwyg::components::actions::is_viewport_navigation(def.id)
                || crate::wysiwyg::components::actions::is_editor_chrome_toggle(def.id)
        };

        // Group by category, preserving definition order.
        let mut groups: Vec<(
            crate::wysiwyg::components::ShortcutCategory,
            Vec<&crate::wysiwyg::components::ShortcutDefinition>,
        )> = Vec::new();
        for def in definitions.iter().filter(|d| is_visible(d)) {
            if let Some((_, items)) = groups.iter_mut().find(|(c, _)| *c == def.category) {
                items.push(def);
            } else {
                groups.push((def.category, vec![def]));
            }
        }

        let mut content = v_flex().size_full().gap_6().p_6();

        // Header hint.
        content = content.child(
            v_flex().gap_1().child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(rust_i18n::t!("settings.shortcuts_hint").to_string()),
            ),
        );

        // Each category group.
        for (category, items) in &groups {
            let cat_label = crate::wysiwyg::components::shortcut_category_label(*category);
            let mut group = v_flex().gap_2();
            group = group.child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg)
                    .child(cat_label.to_string()),
            );

            for def in items {
                let id = def.id;
                let label = crate::wysiwyg::components::shortcut_label(id);
                if label.is_empty() {
                    continue;
                }
                let keys: Vec<String> = draft
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| def.default_keys.iter().map(|s| s.to_string()).collect());
                let is_editing = editing.as_deref() == Some(id);
                let is_custom = draft.contains_key(id);

                let mut row = h_flex()
                    .w_full()
                    .py(px(6.))
                    .px(px(8.))
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .gap(px(12.))
                    .items_center();

                // Label.
                row = row.child(div().flex_1().min_w_0().text_sm().child(label.to_string()));

                // Key display or capture input.
                if is_editing {
                    let display = pending.as_deref().unwrap_or("按快捷键...");
                    row = row.child(
                        div()
                            .id(gpui::ElementId::Name(
                                format!("shortcut-capture-{id}").into(),
                            ))
                            .px(px(8.))
                            .py(px(3.))
                            .rounded_md()
                            .bg(accent.opacity(0.3))
                            .text_sm()
                            .text_color(fg)
                            .child(display.to_string())
                            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                                let ks = &event.keystroke;
                                // Ignore lone modifier presses.
                                if matches!(
                                    ks.key.as_ref(),
                                    "cmd" | "ctrl" | "alt" | "shift" | "super" | "fn"
                                ) {
                                    return;
                                }
                                let mut parts: Vec<&str> = Vec::new();
                                if ks.modifiers.platform {
                                    parts.push("cmd");
                                }
                                if ks.modifiers.control {
                                    parts.push("ctrl");
                                }
                                if ks.modifiers.alt {
                                    parts.push("alt");
                                }
                                if ks.modifiers.shift {
                                    parts.push("shift");
                                }
                                parts.push(ks.key.as_ref());
                                let combo = parts.join("-");
                                this.pending_keystroke = Some(combo);
                                cx.notify();
                            })),
                    );
                } else {
                    // Show resolved keys as Kbd keycaps.
                    let mut key_row = h_flex().gap(px(4.));
                    for k in &keys {
                        if let Ok(ks) = gpui::Keystroke::parse(k) {
                            key_row = key_row.child(Kbd::new(ks));
                        }
                    }
                    row = row.child(key_row);
                }

                // Edit / Confirm / Cancel buttons.
                let id_owned = id.to_string();
                let command = def.command;
                if is_editing {
                    // Confirm.
                    let id_c = id_owned.clone();
                    let pending_c = pending.clone();
                    row = row.child(
                        Button::new(gpui::ElementId::Name(
                            format!("shortcut-confirm-{id_owned}").into(),
                        ))
                        .small()
                        .primary()
                        .label("✓")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(combo) = pending_c.as_deref() {
                                let keys = vec![combo.to_string()];
                                // Validate.
                                if crate::wysiwyg::components::normalize_shortcut_keys(&keys)
                                    .is_some()
                                {
                                    // Conflict check.
                                    let conflict =
                                        crate::wysiwyg::components::shortcut_conflict_for(
                                            command,
                                            &keys,
                                            &this.shortcut_draft,
                                        );
                                    if conflict.is_none() {
                                        this.shortcut_draft.insert(id_c.clone(), keys);
                                        crate::state::persistence::save_shortcuts(
                                            &this.shortcut_draft,
                                        );
                                    }
                                }
                            }
                            this.editing_shortcut = None;
                            this.pending_keystroke = None;
                            cx.notify();
                        })),
                    );
                    // Cancel.
                    row = row.child(
                        Button::new(gpui::ElementId::Name(
                            format!("shortcut-cancel-{id_owned}").into(),
                        ))
                        .small()
                        .ghost()
                        .label("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.editing_shortcut = None;
                            this.pending_keystroke = None;
                            cx.notify();
                        })),
                    );
                } else {
                    // Edit button.
                    let id_e = id_owned.clone();
                    row = row.child(
                        Button::new(gpui::ElementId::Name(
                            format!("shortcut-edit-{id_owned}").into(),
                        ))
                        .small()
                        .ghost()
                        .label(rust_i18n::t!("settings.shortcuts_edit").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.editing_shortcut = Some(id_e.clone());
                            this.pending_keystroke = None;
                            cx.notify();
                        })),
                    );
                    // Reset (only if customized).
                    if is_custom {
                        let id_r = id_owned.clone();
                        row = row.child(
                            Button::new(gpui::ElementId::Name(
                                format!("shortcut-reset-{id_owned}").into(),
                            ))
                            .small()
                            .ghost()
                            .label(rust_i18n::t!("settings.shortcuts_reset").to_string())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.shortcut_draft.remove(&id_r);
                                    crate::state::persistence::save_shortcuts(&this.shortcut_draft);
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }

                group = group.child(row);
            }
            content = content.child(group);
        }

        v_flex()
            .size_full()
            .id("settings-shortcuts-scroll")
            .overflow_y_scroll()
            .child(content)
    }

    /// Render the Media storage settings: four cards (global local, image,
    /// video, unified S3). Reads `media_storage.toml` on each render; inputs and
    /// toggles persist on change/blur via the subscribed handlers.
    fn render_media_section(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = theme.muted_foreground;
        let cfg = crate::media::config::load_media_config();
        let m = &self.media_inputs;

        v_flex()
            .size_full()
            .id("settings-media-scroll")
            .overflow_y_scroll()
            .p_6()
            .gap_6()
            // ---- Card 1: global local ----
            .child(media_card(
                rust_i18n::t!("settings.media.card_global").to_string(),
                rust_i18n::t!("settings.media.card_global_desc").to_string(),
                muted,
                v_flex()
                    .gap_3()
                    .child(self.media_root_picker(theme, cx))
                    .child(media_toggle(
                        "media-git-sync",
                        rust_i18n::t!("settings.media.git_sync").as_ref(),
                        cfg.global.git_sync,
                        theme,
                        |c| c.global.git_sync = !c.global.git_sync,
                    ))
                    .into_any_element(),
            ))
            // ---- Card 2: image ----
            .child(media_card(
                rust_i18n::t!("settings.media.card_image").to_string(),
                rust_i18n::t!("settings.media.card_image_desc").to_string(),
                muted,
                v_flex()
                    .gap_3()
                    .child(media_storage_select(
                        "media-img-storage",
                        cfg.image.storage,
                        theme,
                        |c, v| c.image.storage = v,
                    ))
                    .child(media_field(
                        &m.image_prefix,
                        rust_i18n::t!("settings.media.field_img_prefix").as_ref(),
                        muted,
                    ))
                    .child(media_field(
                        &m.image_max_mb,
                        rust_i18n::t!("settings.media.field_img_max").as_ref(),
                        muted,
                    ))
                    .into_any_element(),
            ))
            // ---- Card 3: video ----
            .child(media_card(
                rust_i18n::t!("settings.media.card_video").to_string(),
                rust_i18n::t!("settings.media.card_video_desc").to_string(),
                muted,
                v_flex()
                    .gap_3()
                    .child(media_storage_select(
                        "media-vid-storage",
                        cfg.video.storage,
                        theme,
                        |c, v| c.video.storage = v,
                    ))
                    .child(media_field(
                        &m.video_prefix,
                        rust_i18n::t!("settings.media.field_vid_prefix").as_ref(),
                        muted,
                    ))
                    .child(media_field(
                        &m.video_threshold_mb,
                        rust_i18n::t!("settings.media.field_vid_threshold").as_ref(),
                        muted,
                    ))
                    .child(media_field(
                        &m.video_part_mb,
                        rust_i18n::t!("settings.media.field_vid_part").as_ref(),
                        muted,
                    ))
                    .into_any_element(),
            ))
            // ---- Card 4: unified S3 ----
            .child(media_card(
                rust_i18n::t!("settings.media.card_s3").to_string(),
                rust_i18n::t!("settings.media.card_s3_desc").to_string(),
                muted,
                v_flex()
                    .gap_3()
                    .child(media_field(
                        &m.s3_endpoint,
                        rust_i18n::t!("settings.media.field_endpoint").as_ref(),
                        muted,
                    ))
                    .child(media_field(
                        &m.s3_bucket,
                        rust_i18n::t!("settings.media.field_bucket").as_ref(),
                        muted,
                    ))
                    .child(media_field(
                        &m.s3_region,
                        rust_i18n::t!("settings.media.field_region").as_ref(),
                        muted,
                    ))
                    .child(media_field(
                        &m.s3_access_key,
                        rust_i18n::t!("settings.media.field_ak").as_ref(),
                        muted,
                    ))
                    .child(media_field(
                        &m.s3_secret_key,
                        rust_i18n::t!("settings.media.field_sk").as_ref(),
                        muted,
                    ))
                    .child(media_toggle(
                        "media-s3-ssl",
                        rust_i18n::t!("settings.media.use_ssl").as_ref(),
                        cfg.s3.use_ssl,
                        theme,
                        |c| c.s3.use_ssl = !c.s3.use_ssl,
                    ))
                    .child(media_toggle(
                        "media-s3-pathstyle",
                        rust_i18n::t!("settings.media.path_style").as_ref(),
                        cfg.s3.path_style,
                        theme,
                        |c| c.s3.path_style = !c.s3.path_style,
                    ))
                    .child(self.media_s3_test_row(theme, cx))
                    .into_any_element(),
            ))
    }

    /// Build the S3 "Test connection" button + last-result status row.
    fn media_s3_test_row(
        &self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = self.media_test.clone();
        let testing = matches!(status, MediaTestState::Testing);
        let accent = theme.accent;
        let danger = theme.danger;
        let muted_f = theme.muted_foreground;
        let (status_text, status_color, show) = match &status {
            MediaTestState::Idle => (String::new(), muted_f, false),
            MediaTestState::Testing => (
                rust_i18n::t!("settings.media.test_testing").to_string(),
                muted_f,
                true,
            ),
            MediaTestState::Ok => (
                rust_i18n::t!("settings.media.test_ok").to_string(),
                accent,
                true,
            ),
            MediaTestState::Err(e) => (
                format!("{}: {}", rust_i18n::t!("settings.media.test_failed"), e),
                danger,
                true,
            ),
        };
        v_flex()
            .gap_2()
            .w(px(340.))
            .child(
                Button::new("media-s3-test")
                    .small()
                    .ghost()
                    .disabled(testing)
                    .child(rust_i18n::t!("settings.media.test_button").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_media_s3_test(cx);
                    })),
            )
            .when(show, |p| {
                p.child(div().text_sm().text_color(status_color).child(status_text))
            })
            .into_any_element()
    }

    /// Run an S3 HEAD-bucket connectivity test on the media runtime.
    fn run_media_s3_test(&mut self, cx: &mut Context<Self>) {
        self.media_test = MediaTestState::Testing;
        cx.notify();
        let cfg = crate::media::config::load_media_config().s3;
        let (tx, rx) = tokio::sync::oneshot::channel();
        crate::media::runtime::media_runtime().spawn(async move {
            let res = crate::media::s3::test_connection(&cfg).await;
            let _ = tx.send(res);
        });
        cx.spawn(async move |this, cx| match rx.await {
            Ok(Ok(())) => {
                let _ = this.update(cx, |this, cx| {
                    this.media_test = MediaTestState::Ok;
                    cx.notify();
                });
            }
            Ok(Err(e)) => {
                let _ = this.update(cx, |this, cx| {
                    this.media_test = MediaTestState::Err(e.to_string());
                    cx.notify();
                });
            }
            Err(_) => {}
        })
        .detach();
    }

    /// Media-root directory picker: shows the resolved current root and a
    /// "Browse" button that opens a native folder picker (no manual path entry).
    fn media_root_picker(&self, theme: &gpui_component::Theme, cx: &mut Context<Self>) -> AnyElement {
        let cfg = crate::media::config::load_media_config();
        let current = crate::media::config::resolved_media_root(&cfg)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| cfg.global.media_root.clone());
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        v_flex()
            .gap_1()
            .w(px(340.))
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(rust_i18n::t!("settings.media.field_root").to_string()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().text_sm().text_color(fg).child(current))
                    .child(
                        Button::new("media-root-browse")
                            .small()
                            .ghost()
                            .child(rust_i18n::t!("settings.media.browse").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.pick_media_root(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Open a native folder picker for the media root and persist the choice.
    fn pick_media_root(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let pick = rfd::FileDialog::new().pick_folder();
            let _ = tx.send(pick);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Some(path)) = rx.await {
                let mut cfg = crate::media::config::load_media_config();
                cfg.global.media_root = path.to_string_lossy().to_string();
                let _ = crate::media::config::save_media_config(&cfg);
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }
}

fn media_card(title: String, desc: String, muted: gpui::Hsla, controls: AnyElement) -> AnyElement {
    v_flex()
        .gap_2()
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(div().text_sm().text_color(muted).child(desc))
        .child(div().mt_2().child(controls))
        .into_any_element()
}

/// A labeled single-line text input bound to an `InputState`.
fn media_field(state: &Entity<InputState>, label: &str, muted: gpui::Hsla) -> AnyElement {
    v_flex()
        .gap_1()
        .w(px(340.))
        .child(div().text_sm().text_color(muted).child(label.to_string()))
        .child(Input::new(state).small())
        .into_any_element()
}

/// A boolean toggle chip that runs a caller-provided click handler.
fn ai_toggle(
    id: &str,
    label: &str,
    on: bool,
    theme: &gpui_component::Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let accent = theme.accent;
    let border = theme.border;
    let fg = theme.foreground;
    let mf = theme.muted_foreground;
    let hover_bg = theme.muted;
    div()
        .id(id.to_string())
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .flex()
        .items_center()
        .gap_2()
        .when(on, move |d| d.border_color(accent).bg(accent.opacity(0.1)))
        .when(!on, move |d| d.border_color(border).hover(move |s| s.bg(hover_bg)))
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
        .on_click(move |ev, window, cx| on_click(ev, window, cx))
        .into_any_element()
}

fn media_toggle(
    id: &str,
    label: &str,
    on: bool,
    theme: &gpui_component::Theme,
    apply: impl Fn(&mut crate::media::config::MediaStorageConfig) + Send + 'static,
) -> AnyElement {
    let accent = theme.accent;
    let border = theme.border;
    let fg = theme.foreground;
    let mf = theme.muted_foreground;
    let hover_bg = theme.muted;
    let label_text = label.to_string();
    let state_text = if on {
        rust_i18n::t!("settings.media.state_on").to_string()
    } else {
        rust_i18n::t!("settings.media.state_off").to_string()
    };
    div()
        .id(id.to_string())
        .w(px(340.))
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .flex()
        .items_center()
        .gap_2()
        .when(on, move |d| d.border_color(accent).bg(accent.opacity(0.1)))
        .when(!on, move |d| d.border_color(border).hover(move |s| s.bg(hover_bg)))
        .child(div().text_sm().text_color(if on { fg } else { mf }).child(state_text))
        .child(div().flex_1())
        .child(div().text_xs().text_color(mf).child(label_text))
        .on_click(move |_, _, _: &mut App| {
            let mut c = crate::media::config::load_media_config();
            apply(&mut c);
            let _ = crate::media::config::save_media_config(&c);
            // Re-sync .gitignore so the media-config file is tracked (git_sync
            // on) or machine-local (git_sync off). Idempotent for other toggles.
            crate::state::persistence::ensure_gitignore();
        })
        .into_any_element()
}

/// A two-option (Local / S3) storage-method selector for image or video.
fn media_storage_select(
    id_prefix: &str,
    current: crate::media::config::StorageMethod,
    theme: &gpui_component::Theme,
    set: fn(
        &mut crate::media::config::MediaStorageConfig,
        crate::media::config::StorageMethod,
    ),
) -> AnyElement {
    use crate::media::config::StorageMethod;
    let accent = theme.accent;
    let border = theme.border;
    let fg = theme.foreground;
    let mf = theme.muted_foreground;
    let hover_bg = theme.muted;
    let label_local = rust_i18n::t!("settings.media.storage_local").to_string();
    let label_s3 = rust_i18n::t!("settings.media.storage_s3").to_string();

    let make = move |method: StorageMethod, label: String, is_active: bool| -> AnyElement {
        div()
            .id(format!("{}-{}", id_prefix, method.label()))
            .flex_1()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .when(is_active, move |d| d.border_color(accent).bg(accent.opacity(0.1)))
            .when(!is_active, move |d| {
                d.border_color(border).hover(move |s| s.bg(hover_bg))
            })
            .child(
                div()
                    .text_sm()
                    .text_color(if is_active { fg } else { mf })
                    .child(label),
            )
            .on_click(move |_, _, _: &mut App| {
                let mut c = crate::media::config::load_media_config();
                set(&mut c, method);
                let _ = crate::media::config::save_media_config(&c);
            })
            .into_any_element()
    };

    v_flex()
        .gap_1()
        .w(px(340.))
        .child(
            div()
                .text_sm()
                .text_color(mf)
                .child(rust_i18n::t!("settings.media.storage_label").to_string()),
        )
        .child(
            h_flex()
                .gap_2()
                .child(make(
                    StorageMethod::Local,
                    label_local,
                    current == StorageMethod::Local,
                ))
                .child(make(StorageMethod::S3, label_s3, current == StorageMethod::S3)),
        )
        .into_any_element()
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
        let new_remote = persistence::load_remote_config();
        if new_remote != self.remote {
            self.remote = new_remote;
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
                s.set_selected_index(
                    Some(gpui_component::IndexPath::new(autosave_idx)),
                    window,
                    cx,
                );
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
                            // Remote nav item.
                            .child({
                                let is_active = active == SettingsSection::Remote;
                                let fg = theme.foreground;
                                let accent = theme.accent;
                                let mf = muted;
                                div()
                                    .id("settings-nav-remote")
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
                                    .child(SettingsSection::Remote.label().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_section = SettingsSection::Remote;
                                        cx.notify();
                                    }))
                            })
                            // Shortcuts nav item.
                            .child({
                                let is_active = active == SettingsSection::Shortcuts;
                                let fg = theme.foreground;
                                let accent = theme.accent;
                                let mf = muted;
                                div()
                                    .id("settings-nav-shortcuts")
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
                                    .child(SettingsSection::Shortcuts.label().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_section = SettingsSection::Shortcuts;
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
                            })
                            // Media nav item.
                            .child({
                                let is_active = active == SettingsSection::Media;
                                let fg = theme.foreground;
                                let accent = theme.accent;
                                let mf = muted;
                                div()
                                    .id("settings-nav-media")
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
                                    .child(SettingsSection::Media.label().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_section = SettingsSection::Media;
                                        cx.notify();
                                    }))
                            })
                            // AI nav item.
                            .child({
                                let is_active = active == SettingsSection::Ai;
                                let fg = theme.foreground;
                                let accent = theme.accent;
                                let mf = muted;
                                div()
                                    .id("settings-nav-ai")
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
                                    .child(SettingsSection::Ai.label().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_section = SettingsSection::Ai;
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
                                SettingsSection::Remote => {
                                    let remote = self.remote.clone();
                                    let tc = theme.clone();
                                    let addr_input = self.remote_address_input.clone();
                                    v_flex()
                                        .size_full()
                                        .id("settings-remote-scroll")
                                        .overflow_y_scroll()
                                        .p_6()
                                        .gap_6()
                                        // Enable toggle
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(rust_i18n::t!("settings.remote_enable").to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(muted)
                                                        .child(rust_i18n::t!("settings.remote_enable_desc").to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .mt_2()
                                                        .w(px(340.))
                                                        .child(
                                                            div()
                                                                .id("remote-enable-toggle")
                                                                .px_3()
                                                                .py_2()
                                                                .rounded_md()
                                                                .border_1()
                                                                .cursor_pointer()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .when(remote.enabled, |d| {
                                                                    d.border_color(tc.accent)
                                                                        .bg(tc.accent.opacity(0.1))
                                                                })
                                                                .when(!remote.enabled, |d| {
                                                                    d.border_color(tc.border)
                                                                        .hover(|s| s.bg(tc.muted))
                                                                })
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(if remote.enabled { tc.foreground } else { tc.muted_foreground })
                                                                        .child(if remote.enabled { "✓ 已启用" } else { "已禁用" }),
                                                                )
                                                                .child(div().flex_1())
                                                                .on_click(move |_: &gpui::ClickEvent, _window, _cx: &mut App| {
                                                                    let mut r = persistence::load_remote_config();
                                                                    r.enabled = !r.enabled;
                                                                    persistence::save_remote_config(&r);
                                                                })
                                                        )
                                                )
                                        )
                                        // Bind address
                                        .when(remote.enabled, |p| {
                                            p.child(
                                                v_flex()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_base()
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .child(rust_i18n::t!("settings.remote_host").to_string()),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .mt_2()
                                                            .gap(px(2.))
                                                            .w(px(340.))
                                                            .children([
                                                                ("127.0.0.1", "仅本地 (127.0.0.1)"),
                                                                ("0.0.0.0", "所有网络接口 (0.0.0.0)"),
                                                            ].iter().map(|&(addr, label)| {
                                                                let is_active = remote.host == addr;
                                                                let tc2 = tc.clone();
                                                                div()
                                                                    .id(format!("remote-host-{}", addr))
                                                                    .px_3()
                                                                    .py_2()
                                                                    .rounded_md()
                                                                    .border_1()
                                                                    .cursor_pointer()
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap_2()
                                                                    .when(is_active, |d| {
                                                                        d.border_color(tc2.accent)
                                                                            .bg(tc2.accent.opacity(0.1))
                                                                    })
                                                                    .when(!is_active, |d| {
                                                                        d.border_color(tc2.border)
                                                                            .hover(|s| s.bg(tc2.muted))
                                                                    })
                                                                    .child(
                                                                        div()
                                                                            .text_sm()
                                                                            .text_color(if is_active { tc2.foreground } else { tc2.muted_foreground })
                                                                            .child(label),
                                                                    )
                                                                    .child(div().flex_1())
                                                                    .when(is_active, |d| {
                                                                        d.child(
                                                                            div()
                                                                                .text_xs()
                                                                                .text_color(tc2.accent)
                                                                                .child(format!("✓ {}", rust_i18n::t!("settings.current"))),
                                                                        )
                                                                    })
                                                                    .on_click(move |_, _window, _cx: &mut App| {
                                                                        let mut r = persistence::load_remote_config();
                                                                        r.host = addr.to_string();
                                                                        persistence::save_remote_config(&r);
                                                                    })
                                                            }))
                                                    )
                                            )
                                        })
                                        // Remote service address (public host) input
                                        .when(remote.enabled, |p| {
                                            p.child(
                                                v_flex()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_base()
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .child(rust_i18n::t!("settings.remote_address").to_string()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(muted)
                                                            .child(rust_i18n::t!("settings.remote_address_desc").to_string()),
                                                    )
                                                    .child(
                                                        div()
                                                            .mt_2()
                                                            .w(px(340.))
                                                            .child(Input::new(&addr_input).small())
                                                    )
                                            )
                                        })
                                        // Help text
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(muted)
                                                .child(rust_i18n::t!("settings.remote_restart_hint").to_string())
                                        )
                                        .into_any_element()
                                }
                                SettingsSection::Shortcuts => {
                                    self.render_shortcuts_section(&theme, cx)
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
                                SettingsSection::Media => {
                                    self.render_media_section(&theme, cx)
                                        .into_any_element()
                                }
                                SettingsSection::Ai => {
                                    self.render_ai_section(window, &theme, cx)
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
