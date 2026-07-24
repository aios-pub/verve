//! WYSIWYG editor runtime settings (ported from velotype `config::preferences`).
//!
//! velotype stores these in an on-disk `config.toml` and exposes them through
//! a standalone preferences *window*. In verve the editor is embedded, so we
//! keep only the in-memory [`EditorSettings`] global that the block render
//! path reads (`show_table_headers`); disk persistence and the preferences UI
//! are intentionally dropped — verve owns its own settings surface.

use gpui::{App, Global};
use serde::{Deserialize, Serialize};

/// A custom button advertised on the status bar.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
pub(crate) struct StatusBarButton {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) tooltip: Option<String>,
}

/// User-tunable status-bar visibility flags.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct StatusBarPreferences {
    #[serde(default = "default_status_bar_enabled")]
    pub(crate) enabled: bool,
    #[serde(default = "default_true")]
    pub(crate) show_word_count: bool,
    #[serde(default = "default_true")]
    pub(crate) show_cursor_position: bool,
    #[serde(default = "default_true")]
    pub(crate) show_sidebar_toggle: bool,
    #[serde(default = "default_true")]
    pub(crate) show_mode_switch: bool,
    #[serde(default)]
    pub(crate) custom_buttons: Vec<StatusBarButton>,
}

fn default_status_bar_enabled() -> bool {
    true
}

fn default_true() -> bool {
    true
}

/// Controls how pasted images are stored when the editor is backed by a file.
///
/// verve's embedded editor has no file path of its own, so the behavior is
/// effectively always [`ImagePasteBehavior::None`] (images are kept inline as
/// data URLs or remote references). The enum is kept for source compatibility
/// with velotype's paste handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
pub(crate) enum ImagePasteBehavior {
    #[default]
    None,
    CopyToDocumentFolder,
    CopyToAssetsFolder,
    CopyToNamedAssetsFolder,
}

impl Default for StatusBarPreferences {
    fn default() -> Self {
        Self {
            enabled: default_status_bar_enabled(),
            show_word_count: true,
            show_cursor_position: true,
            show_sidebar_toggle: true,
            show_mode_switch: true,
            custom_buttons: Vec::new(),
        }
    }
}

/// Mirrored status-bar flags stored on the runtime global.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StatusBarSettings {
    status_bar_enabled: bool,
    status_bar_show_word_count: bool,
    status_bar_show_cursor_position: bool,
    status_bar_show_sidebar_toggle: bool,
    status_bar_show_mode_switch: bool,
}

/// Runtime-accessible editor settings.
///
/// In the embedded verve build this is a pure in-memory global: callers must
/// call [`EditorSettings::init`] once at startup (verve does this when the
/// notes panel is first constructed) before the block render path reads
/// [`EditorSettings::show_table_headers`].
pub struct EditorSettings {
    show_table_headers: bool,
    status_bar_settings: StatusBarSettings,
}

impl Global for EditorSettings {}

impl EditorSettings {
    /// Installs the runtime global. `show_table_headers` controls whether the
    /// top row of native tables is rendered with header styling.
    pub fn init(cx: &mut App, show_table_headers: bool) {
        cx.set_global(Self::build(
            show_table_headers,
            StatusBarPreferences::default(),
        ));
    }

    fn build(show_table_headers: bool, status_bar: StatusBarPreferences) -> Self {
        Self {
            show_table_headers,
            status_bar_settings: StatusBarSettings {
                status_bar_enabled: status_bar.enabled,
                status_bar_show_word_count: status_bar.show_word_count,
                status_bar_show_cursor_position: status_bar.show_cursor_position,
                status_bar_show_sidebar_toggle: status_bar.show_sidebar_toggle,
                status_bar_show_mode_switch: status_bar.show_mode_switch,
            },
        }
    }

    /// Whether table top rows are styled as headers. Defaults to `true` when
    /// the global has not been installed (e.g. in unit tests).
    pub fn show_table_headers(cx: &App) -> bool {
        cx.try_global::<Self>()
            .map(|settings| settings.show_table_headers)
            .unwrap_or(true)
    }

    /// Updates the table-header flag and re-installs the global.
    pub fn set_show_table_headers(cx: &mut App, show_table_headers: bool) {
        let status_bar = cx
            .try_global::<Self>()
            .map(|s| StatusBarPreferences {
                enabled: s.status_bar_settings.status_bar_enabled,
                show_word_count: s.status_bar_settings.status_bar_show_word_count,
                show_cursor_position: s.status_bar_settings.status_bar_show_cursor_position,
                show_sidebar_toggle: s.status_bar_settings.status_bar_show_sidebar_toggle,
                show_mode_switch: s.status_bar_settings.status_bar_show_mode_switch,
                custom_buttons: Vec::new(),
            })
            .unwrap_or_default();
        cx.set_global(Self::build(show_table_headers, status_bar));
    }

    /// Status-bar visibility flags as of the last install.
    pub(crate) fn status_bar_preferences(cx: &App) -> StatusBarPreferences {
        cx.try_global::<Self>()
            .map(|s| StatusBarPreferences {
                enabled: s.status_bar_settings.status_bar_enabled,
                show_word_count: s.status_bar_settings.status_bar_show_word_count,
                show_cursor_position: s.status_bar_settings.status_bar_show_cursor_position,
                show_sidebar_toggle: s.status_bar_settings.status_bar_show_sidebar_toggle,
                show_mode_switch: s.status_bar_settings.status_bar_show_mode_switch,
                custom_buttons: Vec::new(),
            })
            .unwrap_or_default()
    }
}
