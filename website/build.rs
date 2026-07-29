// build.rs — leptos_i18n 0.6 codegen.
//
// Loads YAML locale files from ./i18n and emits a typed i18n module into
// OUT_DIR, consumed by `leptos_i18n::load_locales!()`.

use leptos_i18n_build::{Config, FileFormat, ParseOptions, TranslationsInfos};
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=Cargo.toml");

    let i18n_mod_directory =
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("i18n");

    let options = ParseOptions::default().file_format(FileFormat::Yaml);

    // zh-CN is the default locale (mirrors the desktop app), en is secondary.
    let cfg = Config::new("zh-CN")?
        .add_locale("en")?
        .parse_options(options)
        .locales_path("./i18n");

    let translations_infos = TranslationsInfos::parse(cfg)?;
    translations_infos.emit_diagnostics();
    translations_infos.rerun_if_locales_changed();
    translations_infos.generate_i18n_module(i18n_mod_directory)?;

    Ok(())
}
