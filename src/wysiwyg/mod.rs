//! WYSIWYG Markdown block editor, ported from velotype.
//!
//! Migration stages:
//! - **Stage 1** (done): pure-logic markdown syntax models (`components::markdown`)
//!   and the HTTP client for remote image loading (`net`).
//! - **Stage 2** (done): theme/i18n/config globals + block runtime + rendering.
//! - **Stage 3** (current): the `Editor` controller, adapted for embedding.
//! - Stage 4: integration into the notes panel.
//! - Stage 5: theme mapping, font alignment, polish.

pub mod app_menu;
pub mod components;
pub mod config;
pub mod editor;
pub mod export;
pub mod i18n;
pub mod net;
pub mod theme;
pub mod window_chrome;
