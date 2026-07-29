//! Small helpers, chiefly the [`asset`] function that prefixes every static
//! asset path with the GitHub Pages base path.

use crate::consts::BASE_PATH;

/// Prefix a site-relative asset path with the base path.
///
/// Examples (base path = `/verve`):
/// - `asset("/icons/verve.png")` → `"/verve/icons/verve.png"`
/// - `asset("screenshots/api.png")` → `"/verve/screenshots/api.png"`
#[must_use]
pub fn asset(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    format!("{BASE_PATH}/{trimmed}")
}

/// Prefix an internal *route* path with the base path. Routes keep their
/// leading slash semantics for the router, so we just concatenate.
#[must_use]
pub fn route(path: &str) -> String {
    if path == "/" {
        return format!("{BASE_PATH}/");
    }
    let trimmed = path.trim_start_matches('/');
    format!("{BASE_PATH}/{trimmed}")
}

/// Base-relative path for the Leptos `<Router base=...>` component. The
/// router already knows about the base, so links inside `<A>` must NOT be
/// re-prefixed. This returns the bare route (e.g. `/features`, `/`).
#[must_use]
pub fn rel(path: &str) -> String {
    if path.is_empty() || path == "/" {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// Combine a base URL with a trailing segment.
#[must_use]
pub fn join(base: &str, seg: &str) -> String {
    let seg = seg.trim_start_matches('/');
    format!("{}/{}", base.trim_end_matches('/'), seg)
}
