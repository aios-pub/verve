//! WYSIWYG Markdown block editor, ported from velotype.
//!
//! Embedded inside verve's notes panel. The editor ships its own theme and
//! i18n globals (they predate gpui-component); the [`sync`] helpers keep them
//! aligned with verve's `rust-i18n` locale and gpui-component theme mode.

pub mod app_menu;
pub mod components;
pub mod config;
pub mod editor;
pub mod export;
pub mod i18n;
pub mod net;
pub mod sync;
pub mod theme;
pub mod window_chrome;
