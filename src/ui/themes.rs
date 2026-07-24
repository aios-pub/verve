//! Built-in theme loading.
//!
//! Embeds all theme JSON files from the gpui-component `themes/` directory via
//! `include_str!` and registers them with the `ThemeRegistry` at startup.

use gpui_component::{ActiveTheme, ThemeRegistry};

/// All embedded theme JSON files (copied into the project's `themes/` dir).
const THEME_FILES: &[(&str, &str)] = &[
    ("adventure", include_str!("../../themes/adventure.json")),
    ("alduin", include_str!("../../themes/alduin.json")),
    ("asciinema", include_str!("../../themes/asciinema.json")),
    ("aurora", include_str!("../../themes/aurora.json")),
    ("ayu", include_str!("../../themes/ayu.json")),
    ("catppuccin", include_str!("../../themes/catppuccin.json")),
    ("everforest", include_str!("../../themes/everforest.json")),
    ("fahrenheit", include_str!("../../themes/fahrenheit.json")),
    ("flexoki", include_str!("../../themes/flexoki.json")),
    ("gruvbox", include_str!("../../themes/gruvbox.json")),
    ("harper", include_str!("../../themes/harper.json")),
    ("hybrid", include_str!("../../themes/hybrid.json")),
    ("jellybeans", include_str!("../../themes/jellybeans.json")),
    ("kibble", include_str!("../../themes/kibble.json")),
    (
        "macos-classic",
        include_str!("../../themes/macos-classic.json"),
    ),
    ("matrix", include_str!("../../themes/matrix.json")),
    ("mellifluous", include_str!("../../themes/mellifluous.json")),
    ("molokai", include_str!("../../themes/molokai.json")),
    ("solarized", include_str!("../../themes/solarized.json")),
    ("spaceduck", include_str!("../../themes/spaceduck.json")),
    ("tokyonight", include_str!("../../themes/tokyonight.json")),
    ("twilight", include_str!("../../themes/twilight.json")),
];

/// Load all embedded themes into the global `ThemeRegistry`.
pub fn load_builtin_themes(cx: &mut gpui::App) {
    let registry = ThemeRegistry::global_mut(cx);
    let mut loaded = 0;
    for (name, content) in THEME_FILES {
        match registry.load_themes_from_str(content) {
            Ok(_) => loaded += 1,
            Err(e) => log::warn!("Failed to load theme '{name}': {e}"),
        }
    }
    let total = registry.themes().len();
    log::info!("Loaded {loaded} theme files, {total} themes registered");
}

/// Return a sorted list of all available theme display names.
pub fn theme_names(cx: &gpui::App) -> Vec<String> {
    let registry = ThemeRegistry::global(cx);
    registry
        .sorted_themes()
        .iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// Apply a named theme at runtime: look it up in the registry, install it into
/// the matching mode slot, and re-apply.
pub fn apply_theme(name: &str, cx: &mut gpui::App) {
    let registry = ThemeRegistry::global(cx);
    if let Some(config) = registry.themes().get(name).cloned() {
        let mode = config.mode;
        let theme = gpui_component::Theme::global_mut(cx);
        if mode.is_dark() {
            theme.dark_theme = config;
        } else {
            theme.light_theme = config;
        }
        gpui_component::Theme::change(mode, None, cx);

        // Keep the WYSIWYG editor's ThemeManager in sync with verve's mode
        // (dark → velotype, light → velotype-light). No-op if the editor global
        // isn't installed yet.
        crate::wysiwyg::sync::sync_editor_theme(mode.is_dark(), cx);

        cx.refresh_windows();
    }
}

/// Return the currently active theme name.
pub fn current_theme_name(cx: &gpui::App) -> String {
    cx.theme().theme_name().to_string()
}
