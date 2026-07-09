//! Verve — an offline-first API co-development platform built with gpui-component.
//!
//! Scope of this build: the **core HTTP debugging client** (PRD §4, §5.1–5.5)
//! with local JSON-file persistence. Features requiring a backend (team
//! collaboration, cloud sync, AI, DB tools, WebSocket/gRPC) are documented as
//! roadmap items and intentionally stubbed.

// Roadmap features and helpers produce dead-code warnings until wired up.
#![allow(dead_code)]
// The GPUI element-builder style produces some complex closures.
#![allow(clippy::type_complexity)]
// Some serde data structs are fine to derive-impl via macros.
#![allow(clippy::derivable_impls)]

use gpui::*;
use gpui_component::{button::*, *};
use gpui_component_assets::Assets;

use verve::ui::VerveApp;
use verve::{mock, state, ui};

fn main() {
    // Initialise logging before anything else. Default to `info` so the git
    // sync flow's step-by-step diagnostics print to the terminal; override
    // with the standard RUST_LOG env var.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let _ = env_logger::Builder::from_default_env()
        .format_timestamp_secs()
        .try_init();

    // Apply the persisted UI locale (defaults to zh-CN).
    if let Some(locale) = verve::state::persistence::load_layout()
        .and_then(|l| l.locale)
    {
        rust_i18n::set_locale(&locale);
    } else {
        rust_i18n::set_locale("zh-CN");
    }

    let app = gpui_platform::application().with_assets(Assets);

    app.run(move |cx| {
        // This must be called before using any GPUI Component features.
        gpui_component::init(cx);

        // Load all built-in themes (Catppuccin, Gruvbox, Tokyo Night, etc.).
        ui::themes::load_builtin_themes(cx);

        // Default to the dark theme to match the Apipost reference design.
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);

        // Disable the framework's built-in active-highlight border overlay —
        // its rounded border conflicts with the row's rounding and leaves a
        // crescent gap. We render our own clean selection highlight instead.
        gpui_component::Theme::global_mut(cx).list.active_highlight = false;

        // Register a reqwest-backed HTTP client so the app can make requests.
        if let Ok(http_client) = reqwest_client::ReqwestClient::user_agent("verve/0.1.0") {
            cx.set_http_client(std::sync::Arc::new(http_client));
        }

        // Load persistent workspace state.
        let app_state = state::AppState::init(cx);

        // Start the local Mock server with the active project's rules, if any.
        if let Some(project) = app_state.read(cx).active_project() {
            let rules = std::sync::Arc::new(mock::rule_map(project));
            if !rules.is_empty() {
                mock::serve(mock::DEFAULT_PORT, rules).detach();
            }
        }

        // Keybindings (PRD §6). macOS uses Cmd, others use Ctrl.
        #[cfg(target_os = "macos")]
        cx.bind_keys([
            KeyBinding::new("cmd-enter", ui::request_panel::SendRequest, None),
            KeyBinding::new("cmd-s", ui::app::SaveWorkspace, None),
            KeyBinding::new("cmd-n", ui::app::NewRequest, None),
        ]);
        #[cfg(not(target_os = "macos"))]
        cx.bind_keys([
            KeyBinding::new("ctrl-enter", ui::request_panel::SendRequest, None),
            KeyBinding::new("ctrl-s", ui::app::SaveWorkspace, None),
            KeyBinding::new("ctrl-n", ui::app::NewRequest, None),
        ]);

        // Centered, reasonable default size. Computed here (pre-spawn) where cx
        // is a &mut App; Bounds::centered needs a synchronous App reference.
        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);

        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    // Hide the native titlebar so our custom top bar is flush
                    // with the top edge (no gap). On macOS the traffic-light
                    // buttons overlay onto our bar.
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("Verve".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.), px(16.))),
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(960.), px(600.))),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| VerveApp::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx).bg(cx.theme().background))
                },
            )
            .expect("Failed to open window");
        })
        .detach();
    });

    // Silence unused import warnings for the placeholder below.
    let _ = Button::new("noop");
}
