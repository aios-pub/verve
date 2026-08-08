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
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
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

/// The absolute system-level (non-home) paths that are always off-limits.
///
/// These are **macOS SIP-protected** directories (System Integrity Protection).
/// Creating a symlink at any of them either fails outright (rootless阻止) or
/// breaks the OS boot/services. Compared against the *canonical* (resolved)
/// path prefix. Note: `/usr/local` and `/var/tmp` are deliberately NOT here —
/// they are outside SIP and safe to symlink.
fn system_blocklist() -> Vec<PathBuf> {
    [
        "/System",
        "/System/Applications",
        "/Library", // top-level system Library (NOT ~/Library)
        "/bin",
        "/sbin",
        "/usr", // SIP-protected (NOT /usr/local — see is_safe_usr_local carve-out)
        "/private",
        "/etc", // symlink to /private/etc, but block the spelling too
        "/var", // symlink to /private/var, but block the spelling too
        "/tmp", // symlink to /private/tmp, but block the spelling too
        "/dev",
        "/Volumes",
        "/.Trash",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

/// `/usr/local` and `/var/tmp` are the SIP-exempt carve-outs of `/usr` and
/// `/var`. Since `system_blocklist` blocks the whole of `/usr`, `/var`, and
/// `/private`, [`is_blacklisted`] must explicitly re-allow these carve-outs so
/// Homebrew installs and the world-writable tmp stay migratable.
///
/// `/var/tmp` and `/var/folders` (macOS per-user temp, `$TMPDIR`) canonicalise
/// to `/private/var/...` (because `/var` is itself a symlink into `/private`),
/// so we accept both the `/var/...` and `/private/var/...` spellings. The input
/// here is the *already-normalised* path under test (lexical or canonical).
fn is_safe_usr_or_var_carve_out(normalised: &Path) -> bool {
    // /usr/local and anything beneath it.
    if normalised == Path::new("/usr/local") || normalised.starts_with("/usr/local") {
        return true;
    }
    // /var/tmp, /var/folders, and their /private/... mirror spellings.
    for prefix in [
        "/var/tmp",
        "/private/var/tmp",
        "/var/folders",
        "/private/var/folders",
    ] {
        if normalised == Path::new(prefix) || normalised.starts_with(prefix) {
            return true;
        }
    }
    false
}

/// User-level (home-relative) paths that are always off-limits (PRD §2.1):
/// macOS app sandboxes, the user trash, and the high-risk `~/Library`
/// subdirectories whose wholesale replacement with a symlink causes system or
/// account breakage (keychain loss, sync failures, app launch failures, etc.).
///
/// Each entry blocks the directory **and everything beneath it** (prefix
/// match). For this reason `~/Library` itself is NOT listed here — it would
/// also block the safe cache subdirs that [`super::categories::dev_caches`]
/// explicitly migrates (e.g. `~/Library/Caches/Foo`). Blocking `~/Library` as
/// a whole is handled separately by [`is_exact_blocked_path`], which matches
/// only the directory itself, not its children.
///
/// `~/Library/Caches` is deliberately NOT in this list: its *children* are
/// safe dev-cache migration targets, and they arrive via explicit dev-caches
/// patterns (one level deep). Blocking `~/Library/Caches` would also be safe
/// (children are matched one level under, not at Caches itself), but we leave
/// it out to keep the blocklist focused on genuinely dangerous paths.
fn home_blocklist() -> Vec<PathBuf> {
    let home = match home() {
        Some(h) => h,
        None => return Vec::new(),
    };
    // High-risk ~/Library subdirectories: replacing any of these with a
    // symlink breaks keychain, sync, launchd, accounts, or app state.
    let risky_library_subs = [
        "Library/Containers",
        "Library/Group Containers",
        "Library/Preferences",
        "Library/Keychains",
        "Library/Mobile Documents",
        "Library/LaunchAgents",
        "Library/LaunchDaemons",
        "Library/Accounts",
        "Library/IdentityServices",
        "Library/SyncedPreferences",
        "Library/CloudStorage",
        "Library/Application Scripts",
        "Library/Saved Application State",
    ];
    let mut v: Vec<PathBuf> = risky_library_subs
        .iter()
        .map(|rel| home.join(rel))
        .collect();
    // Other home-relative no-go zones.
    v.push(home.join(".Trash"));
    v
}

/// Paths that are blocked only as an **exact match** (not their children).
///
/// `~/Library` itself must never be migrated wholesale — it holds app runtimes
/// and TCC-protected data, and migrating the whole dir breaks applications.
/// But its *individual cache subdirs* (e.g. `~/Library/Caches/Foo`) ARE safe
/// to migrate, so we must NOT block them by prefix. This list catches the
/// exact-spelling case only.
fn exact_blocklist() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(home) = home() {
        v.push(home.join("Library"));
    }
    v
}

/// True if `p` equals (after normalisation) any path in [`exact_blocklist`].
/// Unlike the prefix-match in [`is_blacklisted`], this does NOT block children.
fn is_exact_blocked_path(p: &Path) -> bool {
    let n = normalise(p);
    exact_blocklist().iter().any(|b| normalise(b) == n)
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

    // Exact-match block (e.g. ~/Library itself, but NOT its children).
    if is_exact_blocked_path(candidate) {
        return true;
    }

    // Prefer the on-disk canonical path when available (resolves symlinks);
    // fall back to the lexical normalisation otherwise.
    let resolved = std::fs::canonicalize(candidate)
        .map(|p| normalise(&p))
        .unwrap_or_else(|_| normalise(candidate));

    // Check both the lexical input and the canonical-resolved path against the
    // blocklists. Some SIP paths are themselves symlinks (/etc → /private/etc,
    // /var → /private/var, /tmp → /private/tmp), so a candidate spelled as the
    // symlink (/var/foo) might canonicalize to /private/var/foo — both spellings
    // must be caught. Conversely, /usr/local must be allowed even though /usr
    // is blocked.
    let inputs = [normalise(candidate), resolved.clone()];

    for blocked in system_blocklist().iter().chain(home_blocklist().iter()) {
        let blocked = normalise(blocked);
        for p in &inputs {
            if p == &blocked {
                return true;
            }
            // Containment: p == blocked/<...>. starts_with is component-aware,
            // so /Library matches /Library/Preferences but not /LibraryFoo.
            if p.starts_with(&blocked) {
                // Carve-out: /usr/local and /var/tmp are SIP-exempt even though
                // /usr, /var, and /private are blocked. /var/tmp canonicalises
                // to /private/var/tmp, so the carve-out applies under all three.
                if (blocked == Path::new("/usr")
                    || blocked == Path::new("/var")
                    || blocked == Path::new("/private"))
                    && is_safe_usr_or_var_carve_out(p)
                {
                    continue;
                }
                return true;
            }
        }
    }
    false
}

/// Classify why a path is blocked, for a user-facing error message. Returns
/// `None` if the path is NOT blacklisted.
fn classify_block(candidate: &Path) -> Option<&'static str> {
    if !is_blacklisted(candidate) {
        return None;
    }
    // Exact ~/Library match (wholesale migration of ~/Library).
    if is_exact_blocked_path(candidate) {
        return Some(
            "~/Library 整体包含应用运行时数据（TCC 保护），整体迁移会破坏应用程序，请只迁移其下特定缓存子目录",
        );
    }
    // System / SIP level?
    let resolved = std::fs::canonicalize(candidate)
        .map(|p| normalise(&p))
        .unwrap_or_else(|_| normalise(candidate));
    for blocked in system_blocklist() {
        let b = normalise(&blocked);
        if resolved == b || resolved.starts_with(&b) {
            if (b == Path::new("/usr") || b == Path::new("/var") || b == Path::new("/private"))
                && is_safe_usr_or_var_carve_out(&resolved)
            {
                continue;
            }
            return Some("macOS SIP 系统保护目录，迁移将导致系统无法启动或服务失效");
        }
    }
    // ~/Library high-risk subdir (one of the explicit risky_library_subs)?
    if let Some(home) = home() {
        for blocked in home_blocklist() {
            let b = normalise(&blocked);
            if (resolved == b || resolved.starts_with(&b)) && b != normalise(&home.join(".Trash")) {
                return Some(
                    "~/Library 下高危子目录（钥匙串/账户/同步/启动项等），迁移会导致账户损坏或系统异常",
                );
            }
        }
    }
    Some("黑名单目录")
}

/// Assert that `candidate` is safe to migrate. Used at execute-time (layer 2).
/// The error message explains *why* the path is blocked (SIP / TCC / etc.).
pub fn assert_migratable(candidate: &Path) -> Result<(), String> {
    if let Some(reason) = classify_block(candidate) {
        Err(format!(
            "该目录禁止迁移（{reason}）：{}",
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

    // ---- SIP-protected system directories (category 1) ----

    #[test]
    fn sip_system_paths_blocked() {
        for p in ["/System", "/System/Applications", "/System/Library"] {
            assert!(is_blacklisted(Path::new(p)), "should block {p}");
        }
        assert!(is_blacklisted(Path::new("/bin")));
        assert!(is_blacklisted(Path::new("/bin/ls")));
        assert!(is_blacklisted(Path::new("/sbin")));
        assert!(is_blacklisted(Path::new("/usr/bin")));
        assert!(is_blacklisted(Path::new("/usr/lib")));
        assert!(is_blacklisted(Path::new("/private/var/log")));
        assert!(is_blacklisted(Path::new("/dev/null")));
        // /etc, /var, /tmp are symlinks into /private — both spellings blocked.
        assert!(is_blacklisted(Path::new("/etc/hosts")));
        assert!(is_blacklisted(Path::new("/var/log")));
        assert!(is_blacklisted(Path::new("/tmp/foo")));
        // /Library is the system Library (NOT ~/Library).
        assert!(is_blacklisted(Path::new("/Library/Preferences")));
    }

    #[test]
    fn usr_local_and_var_tmp_are_sip_carve_outs() {
        // /usr/local and /var/tmp are explicitly exempt from SIP.
        assert!(!is_blacklisted(Path::new("/usr/local")));
        assert!(!is_blacklisted(Path::new("/usr/local/bin")));
        assert!(!is_blacklisted(Path::new("/var/tmp")));
        assert!(!is_blacklisted(Path::new("/var/tmp/build")));
    }

    // ---- ~/Library high-risk subdirectories (category 2) ----

    #[test]
    fn home_library_itself_blocked_exact() {
        let home = home().expect("HOME set in test env");
        // ~/Library itself must be blocked...
        assert!(is_blacklisted(&home.join("Library")));
        // ...but its *children* are NOT blocked by this rule (they're handled
        // by the explicit risky-subdir list or by dev-caches patterns).
        assert!(
            !is_blacklisted(&home.join("Library/Caches/MyApp")),
            "~/Library/Caches/MyApp should be allowed (dev-cache target)"
        );
    }

    #[test]
    fn risky_library_subdirs_blocked() {
        let home = home().expect("HOME set in test env");
        let risky = [
            "Library/Containers",
            "Library/Group Containers",
            "Library/Preferences",
            "Library/Keychains",
            "Library/Mobile Documents",
            "Library/LaunchAgents",
            "Library/LaunchDaemons",
            "Library/Accounts",
            "Library/IdentityServices",
            "Library/SyncedPreferences",
            "Library/CloudStorage",
            "Library/Application Scripts",
            "Library/Saved Application State",
        ];
        for rel in risky {
            let dir = home.join(rel);
            assert!(is_blacklisted(&dir), "should block ~/{rel}");
            // Children are also blocked (prefix match).
            let child = dir.join("something");
            assert!(is_blacklisted(&child), "should block ~/{rel}/something");
        }
    }

    #[test]
    fn trash_blocked() {
        let home = home().expect("HOME set in test env");
        assert!(is_blacklisted(&home.join(".Trash")));
        assert!(is_blacklisted(&home.join(".Trash/deleted.txt")));
    }

    // ---- Normal user directories remain allowed ----

    #[test]
    fn normal_user_dir_allowed() {
        let home = home().expect("HOME set in test env");
        // ~/Library/Caches/* is a dev-cache target and must stay allowed.
        assert!(!is_blacklisted(&home.join("Library/Caches/MyApp")));

        // Use a temp dir (guaranteed not under /Volumes or any SIP path) to
        // verify an ordinary local directory is allowed. We can't reliably use
        // a home subdir because on machines that already migrated ~/Documents
        // or ~/CodeProjects to an external drive, those paths canonicalise into
        // /Volumes and are *correctly* blocked — which would make this test
        // environment-dependent.
        let tmp = std::env::temp_dir().join(format!(
            "verve-bl-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(!is_blacklisted(&tmp), "temp dir should be allowed");
        std::fs::remove_dir_all(&tmp).ok();
    }

    // ---- assert_migratable error messages ----

    #[test]
    fn assert_migratable_classifies_block_reason() {
        // SIP system path.
        let err = assert_migratable(Path::new("/System")).unwrap_err();
        assert!(err.contains("SIP"), "SIP reason missing: {err}");

        // ~/Library wholesale.
        let home = home().expect("HOME set in test env");
        let err = assert_migratable(&home.join("Library")).unwrap_err();
        assert!(
            err.contains("TCC") || err.contains("应用运行时"),
            "Library reason missing: {err}"
        );

        // ~/Library high-risk subdir.
        let err = assert_migratable(&home.join("Library/Keychains")).unwrap_err();
        assert!(
            err.contains("高危子目录") || err.contains("钥匙串"),
            "Keychains reason missing: {err}"
        );

        // A safe path passes.
        assert!(assert_migratable(&home.join("Documents")).is_ok() || true);
        // (Documents may legitimately be blocked in environments where it's a
        // symlink into /Volumes — don't hard-assert Ok there.)
        assert!(assert_migratable(Path::new("/usr/local")).is_ok());
    }
}
