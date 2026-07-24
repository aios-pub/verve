//! Bridges between verve's theme/locale state and the WYSIWYG editor's own
//! globals (`ThemeManager`, `I18nManager`).
//!
//! The editor (ported from velotype) reads colors from `ThemeManager` and UI
//! strings from `I18nManager` — both separate from gpui-component's theme and
//! `rust-i18n`. These helpers keep the two in sync so the embedded editor
//! matches verve's active theme mode and UI language.

use gpui::{App, Global};

use crate::wysiwyg::i18n::I18nManager;
use crate::wysiwyg::theme::ThemeManager;

/// velotype builtin theme id for dark mode.
const WYSIWYG_THEME_DARK: &str = "velotype";
/// velotype builtin theme id for light mode.
const WYSIWYG_THEME_LIGHT: &str = "velotype-light";

/// Maps a verve/rust-i18n locale code (e.g. `"zh-CN"`, `"en"`, `"en-US"`) to the
/// editor's builtin language id (`"zh-CN"` or `"en-US"`). Anything not
/// Chinese falls back to English.
pub fn locale_to_editor_language_id(locale: &str) -> &'static str {
    if locale.to_lowercase().starts_with("zh") {
        "zh-CN"
    } else {
        "en-US"
    }
}

/// Maps a verve theme mode to the editor's builtin theme id.
pub fn theme_mode_to_editor_theme_id(is_dark: bool) -> &'static str {
    if is_dark {
        WYSIWYG_THEME_DARK
    } else {
        WYSIWYG_THEME_LIGHT
    }
}

/// Synchronizes the editor's `I18nManager` to the given locale. No-op if the
/// editor language is already the target, or if the global isn't installed yet.
///
/// Call this at startup (after verve applies its persisted locale) and whenever
/// the user switches UI language in settings.
pub fn sync_editor_language(locale: &str, cx: &mut App) {
    let target = locale_to_editor_language_id(locale);
    sync_global::<I18nManager>(cx, |mgr| {
        if mgr.current_language_id() != target {
            mgr.set_language_by_id(target);
        }
    });
}

/// Synchronizes the editor's `ThemeManager` to the given mode. No-op if the
/// editor theme is already the target, or if the global isn't installed yet.
///
/// Call this at startup and whenever verve's theme mode changes.
pub fn sync_editor_theme(is_dark: bool, cx: &mut App) {
    let target = theme_mode_to_editor_theme_id(is_dark);
    sync_global::<ThemeManager>(cx, |mgr| {
        if mgr.current_theme_id() != target {
            mgr.set_theme_by_id(target);
        }
    });
}

/// Clones a `Global` type, mutates the clone, and re-installs it. verve's gpui
/// (zed fork) has no `update_global`, so this is the canonical way to mutate a
/// global in place.
fn sync_global<G>(cx: &mut App, mutate: impl FnOnce(&mut G))
where
    G: Global + Clone,
{
    if let Some(existing) = cx.try_global::<G>() {
        let mut next = G::clone(&existing);
        mutate(&mut next);
        cx.set_global(next);
    }
}
