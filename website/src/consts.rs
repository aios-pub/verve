//! Central constants for the Verve website.
//!
//! All URLs and the GitHub Pages base path live here so they can be changed in
//! exactly one place. The site is hosted at
//! `https://aios-pub.github.io/verve/`, hence `BASE_PATH = "/verve"`.

/// Sub-path under which the site is served on GitHub Pages.
/// Every static asset link is prefixed with this (see [`crate::utils::asset`]).
pub const BASE_PATH: &str = "/verve";

/// Public distribution + source repository.
pub const REPO: &str = "https://github.com/aios-pub/verve";

/// Latest release landing page.
pub const RELEASES_LATEST: &str = "https://github.com/aios-pub/verve/releases/latest";

/// All releases.
pub const RELEASES_ALL: &str = "https://github.com/aios-pub/verve/releases";

/// Build a `releases/tag/<tag>` URL.
pub fn release_tag(tag: &str) -> String {
    format!("https://github.com/aios-pub/verve/releases/tag/{tag}")
}

/// Current application version, read at build time from the root `Cargo.toml`.
/// Falls back to a placeholder if parsing fails.
pub const VERVE_VERSION: &str = match option_env!("VERVE_VERSION") {
    Some(v) => v,
    None => "0.3.0",
};
