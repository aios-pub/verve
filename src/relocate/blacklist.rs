//! Global blacklist (PRD §2.1, §11) — enforced at two layers.
//!
//! Layer 1 (scan-time): [`scanner`] skips any path that is — or lives under —
//! a blacklisted directory, so they never enter the in-memory list and are
//! never shown in the UI.
//!
//! Layer 2 (execute-time): [`migrate`] / [`rollback`] call [`assert_migratable`]
//! immediately before touching the filesystem; a hit aborts with a hard error.
//! This second layer is the safety net that closes any path by which a
//! blacklisted directory could reach execution (e.g. a future "add by path"
//! that bypasses the scan, or a hand-edited `tasks.jsonl`).

use std::path::{Path, PathBuf};

/// Resolve `~/`-prefixed path strings to absolute `PathBuf`s against `$HOME`.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// Expand a leading `~/` to the absolute home path. Non-`~/` paths are
/// returned unchanged (still canonicalised by the caller as needed).
pub fn expand_tilde(p: &str) -> Option<PathBuf> {
    if let Some(rest) = p.strip_prefix("~/") {
        home().map(|h| h.join(rest))
    } else if p == "~" {
        home()
    } else {
        Some(PathBuf::from(p))
    }
}

/// The absolute system-level (non-home) paths that are always off-limits
/// (PRD §2.1). Compared against the *canonical* (resolved) path prefix.
fn system_blocklist() -> Vec<PathBuf> {
    [
        "/System",
        "/Library", // top-level system Library (NOT ~/Library)
        "/private",
        "/dev",
        "/Volumes",
        "/.Trash",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

/// User-level (home-relative) paths that are always off-limits (PRD §2.1):
/// macOS app sandboxes and the user trash.
fn home_blocklist() -> Vec<PathBuf> {
    let home = match home() {
        Some(h) => h,
        None => return Vec::new(),
    };
    [
        ".Trash",
        "Library/Containers",
        "Library/Group Containers",
    ]
    .iter()
    .map(|rel| home.join(rel))
    .collect()
}

/// Lexically normalise a path (collapse `..`/`.`), without touching the disk.
/// Used so that prefix comparisons are stable across `foo/./bar` spellings.
fn normalise(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True if `candidate` is, or is contained within, any blacklisted directory.
///
/// `candidate` is first lexical-normalised; if it exists on disk it is also
/// canonicalised so that symlink-resolved real paths are checked. A path is
/// blacklisted if it equals a blocklist entry *or* starts with
/// `<blocklist entry>/` (ancestor containment). The bare home and the root
/// `/` are also rejected (never migrate the whole user or filesystem).
pub fn is_blacklisted(candidate: &Path) -> bool {
    // Never accept the filesystem root or the home directory wholesale.
    if candidate == Path::new("/") {
        return true;
    }
    if let Some(h) = home() {
        if normalise(candidate) == normalise(&h) {
            return true;
        }
    }

    // Prefer the on-disk canonical path when available (resolves symlinks);
    // fall back to the lexical normalisation otherwise.
    let resolved = std::fs::canonicalize(candidate)
        .map(|p| normalise(&p))
        .unwrap_or_else(|_| normalise(candidate));

    for blocked in system_blocklist().iter().chain(home_blocklist().iter()) {
        let blocked = normalise(blocked);
        if resolved == blocked {
            return true;
        }
        // Containment: resolved == blocked/<...>
        if resolved.starts_with(&blocked) {
            // `starts_with` is component-aware; e.g. /Library matches
            // /Library/Preferences but not /LibraryFoo.
            return true;
        }
    }
    false
}

/// Assert that `candidate` is safe to migrate. Used at execute-time (layer 2).
pub fn assert_migratable(candidate: &Path) -> Result<(), String> {
    if is_blacklisted(candidate) {
        Err(format!(
            "该目录禁止迁移（黑名单）：{}",
            candidate.display()
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_home_are_blocked() {
        assert!(is_blacklisted(Path::new("/")));
        let home = home().expect("HOME set in test env");
        assert!(is_blacklisted(&home));
    }

    #[test]
    fn system_paths_blocked() {
        assert!(is_blacklisted(Path::new("/System")));
        assert!(is_blacklisted(Path::new("/System/Library")));
        assert!(is_blacklisted(Path::new("/dev/null")));
        assert!(is_blacklisted(Path::new("/private/var")));
    }

    #[test]
    fn user_sandbox_and_trash_blocked() {
        let home = home().expect("HOME set in test env");
        assert!(is_blacklisted(&home.join("Library/Containers/com.apple.app")));
        assert!(is_blacklisted(&home.join("Library/Group Containers/group.x")));
        assert!(is_blacklisted(&home.join(".Trash/deleted.txt")));
    }

    #[test]
    fn normal_user_dir_allowed() {
        let home = home().expect("HOME set in test env");
        assert!(!is_blacklisted(&home.join("Documents")));
        assert!(!is_blacklisted(&home.join("CodeProjects/verve")));
        // `~/.cargo` is normally a real dir (not blacklisted). But after a
        // relocate migration it may be a symlink into /Volumes (which IS in the
        // system blocklist) — in that migrated state the path legitimately
        // canonicalizes under /Volumes, so only assert when it's a real dir.
        let cargo = home.join(".cargo");
        let is_real_dir = std::fs::symlink_metadata(&cargo)
            .map(|m| m.is_dir() && !m.file_type().is_symlink())
            .unwrap_or(false);
        if is_real_dir {
            assert!(!is_blacklisted(&cargo));
        }
    }

    #[test]
    fn top_level_library_is_blocked_but_home_library_cache_is_not() {
        // /Library is system-level and blocked; ~/Library/Caches is allowed
        // (it's a dev-cache migration target per PRD §4.1).
        assert!(is_blacklisted(Path::new("/Library/Preferences")));
        let home = home().expect("HOME set in test env");
        assert!(!is_blacklisted(&home.join("Library/Caches/MyApp")));
    }
}
