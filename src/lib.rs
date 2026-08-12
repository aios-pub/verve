//! Verve library crate — exposes modules for integration tests.

// Load locale files from `locales/` at compile time. Default locale is zh-CN
// (simplified Chinese); English is available as "en". Switch at runtime via
// `rust_i18n::set_locale("en")`.
rust_i18n::i18n!("locales", fallback = "zh-CN");

/// Shared list of .md file paths pending open in the Markdown editor.
/// Populated from argv at launch (main.rs) and from macOS "open file" events
/// (`cx.on_open_urls`). Consumed by `VerveApp::open_pending_markdown_files`.
#[derive(Clone)]
pub struct PendingMarkdownFiles(pub std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>);

impl gpui::Global for PendingMarkdownFiles {}

pub mod assets;
pub mod autotest;
pub mod docker;
pub mod export;
pub mod git;
pub mod hosts;
pub mod hosts_priv;
pub mod hosts_profiles;
pub mod http;
pub mod import;
pub mod k8s;
pub mod mock;
pub mod notes;
pub mod notes_index;
pub mod notes_pdf;
pub mod proxy;
pub mod relocate;
pub mod scripting;
pub mod share;
pub mod ssh;
pub mod state;
pub mod stress;
pub mod ui;
pub mod updater;
pub mod wysiwyg;
