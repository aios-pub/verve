//! Verve — an offline-first API co-development platform built with gpui-component.

// Roadmap features and helpers produce dead-code warnings until wired up.
#![allow(dead_code)]
// The GPUI element-builder style produces some complex closures.
#![allow(clippy::type_complexity)]
// Some serde data structs are fine to derive-impl via macros.
#![allow(clippy::derivable_impls)]

use std::sync::Arc;

use gpui::{img, *};
use gpui_component::button::*;
use gpui_component::*;

use verve::assets::VerveAssets;
use verve::state::persistence;
use verve::ui::VerveApp;
use verve::{mock, state, ui};

fn main() {
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        };
    }
    let _ = env_logger::Builder::from_default_env()
        .format_timestamp_secs()
        .try_init();

    let app = gpui_platform::application().with_assets(VerveAssets::new());

    app.run(move |cx| {
        gpui_component::init(cx);

        ui::themes::load_builtin_themes(cx);
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
        gpui_component::Theme::global_mut(cx).list.active_highlight = false;

        if let Ok(http_client) = reqwest_client::ReqwestClient::user_agent("verve/0.1.0") {
            cx.set_http_client(std::sync::Arc::new(http_client));
        }

        // Register the WYSIWYG editor globals (ported from velotype). The notes
        // panel's embedded block editor reads colors from `ThemeManager`, UI
        // strings from `I18nManager`, and the table-header flag from
        // `EditorSettings`. These are independent from gpui-component's own
        // theme system.
        verve::wysiwyg::theme::ThemeManager::init(cx);
        verve::wysiwyg::i18n::I18nManager::init(cx);
        verve::wysiwyg::config::EditorSettings::init(cx, true);
        // Register the WYSIWYG block-editor keybindings (Newline, DeleteBack,
        // BoldSelection, IndentBlock, ...). These are scoped to the
        // "BlockEditor" key context so they only fire when a block is focused
        // and never collide with verve's global bindings.
        verve::wysiwyg::components::init(cx);

        // Apply locale early.
        if let Some(locale) = persistence::load_layout().and_then(|l| l.locale) {
            rust_i18n::set_locale(&locale);
        } else {
            rust_i18n::set_locale("zh-CN");
        }

        // Decide whether to show bootstrap or main app.
        let is_first_run = persistence::is_first_run();

        if is_first_run {
            log::info!("First run detected, showing bootstrap dialog");
            let bounds = Bounds::centered(None, size(px(560.), px(560.)), cx);
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        titlebar: Some(gpui::TitlebarOptions {
                            title: Some("Verve - Welcome".into()),
                            appears_transparent: false,
                            traffic_light_position: Some(point(px(14.), px(16.))),
                        }),
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        window_min_size: Some(size(px(400.), px(400.))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let dialog = ui::bootstrap_dialog::BootstrapDialog::new(window, cx);
                        cx.new(|cx| Root::new(dialog, window, cx))
                    },
                )
                .expect("Failed to open bootstrap window");
            })
            .detach();

            persistence::mark_bootstrap_done();
        }

        launch_main_app(cx);
    });

    let _ = Button::new("noop");
}

fn launch_main_app(cx: &mut App) {
    let app_state = state::AppState::init(cx);

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

    let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);

    cx.spawn(async move |cx| {
        cx.open_window(
            WindowOptions {
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
}
