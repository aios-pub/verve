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

use verve::PendingMarkdownFiles;
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

    // Collect .md file paths from argv (e.g. `verve path/to/note.md`).
    let initial_md_files: Vec<std::path::PathBuf> = std::env::args()
        .skip(1)
        .filter(|a| {
            let lower = a.to_ascii_lowercase();
            lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".txt")
        })
        .map(std::path::PathBuf::from)
        .collect();

    // Shared pending-files list: populated from argv at launch and from macOS
    // "open file" events while running. Consumed by VerveApp.
    let pending = std::sync::Arc::new(std::sync::Mutex::new(initial_md_files.clone()))
        as std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>;

    // macOS "open file" (Finder double-click, drag onto dock icon): the OS
    // delivers file:// URLs. Registered on the Application before run().
    {
        let pending = pending.clone();
        app.on_open_urls(move |urls| {
            for url in &urls {
                if let Ok(parsed) = url::Url::parse(url) {
                    if let Ok(path) = parsed.to_file_path() {
                        let lower = path.to_string_lossy().to_ascii_lowercase();
                        if lower.ends_with(".md")
                            || lower.ends_with(".markdown")
                            || lower.ends_with(".txt")
                        {
                            if let Ok(mut guard) = pending.lock() {
                                guard.push(path);
                            }
                        }
                    }
                }
            }
        });
    }

    app.run(move |cx| {
        gpui_component::init(cx);

        // Stash the pending-files global so VerveApp can consume it.
        cx.set_global(PendingMarkdownFiles(pending.clone()));

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
        // and never collide with verve's global bindings. Custom overrides
        // persisted in layout.json (set via Settings → Shortcuts) are applied
        // here; they take effect on the next app restart.
        let shortcut_config = verve::state::persistence::load_shortcuts();
        verve::wysiwyg::components::init_with_keybindings(cx, &shortcut_config);

        // Apply locale early.
        if let Some(locale) = persistence::load_layout().and_then(|l| l.locale) {
            rust_i18n::set_locale(&locale);
            // Keep the WYSIWYG editor's I18nManager in sync with the persisted
            // locale (it defaults to the OS-detected language otherwise).
            verve::wysiwyg::sync::sync_editor_language(&locale, cx);
        } else {
            rust_i18n::set_locale("zh-CN");
            verve::wysiwyg::sync::sync_editor_language("zh-CN", cx);
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
        // New API request moved off cmd-n (now "new file"); use cmd-shift-n.
        KeyBinding::new("cmd-shift-n", ui::app::NewRequest, None),
        KeyBinding::new("cmd-n", ui::app::NewFile, None),
        KeyBinding::new("cmd-w", ui::app::CloseFile, None),
        // Open a Markdown file in a new tab of the markdown editor view.
        KeyBinding::new("cmd-o", ui::app::OpenMarkdownFile, None),
        KeyBinding::new("cmd-shift-o", ui::app::OpenMarkdownFileNewTab, None),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-enter", ui::request_panel::SendRequest, None),
        KeyBinding::new("ctrl-s", ui::app::SaveWorkspace, None),
        KeyBinding::new("ctrl-shift-n", ui::app::NewRequest, None),
        KeyBinding::new("ctrl-n", ui::app::NewFile, None),
        KeyBinding::new("ctrl-w", ui::app::CloseFile, None),
        KeyBinding::new("ctrl-o", ui::app::OpenMarkdownFile, None),
        KeyBinding::new("ctrl-shift-o", ui::app::OpenMarkdownFileNewTab, None),
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
