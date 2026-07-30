//! Directory scanner + auto-clustering (PRD §5).
//!
//! Strategy:
//! 1. Expand every rule's `~/`-prefixed patterns to absolute candidate paths.
//!    A trailing `/*` enumerates the *top-level children* of that directory
//!    (one level deep — never a full recursive walk, for performance).
//! 2. Filter out blacklisted, missing, and non-directory candidates.
//! 3. Assign each surviving directory to the single highest-priority matching
//!    group; deduplicate so a directory never appears in two groups.
//! 4. Compute each directory's recursive size via `walkdir`.
//! 5. Drop empty groups; collect non-rule-matched non-empty top-level home
//!    directories into 【其他目录】.
//!
//! All of this is designed to run on a background executor — the UI receives
//! the finished [`ScanResult`] and never blocks on I/O.

use std::path::{Path, PathBuf};

use super::blacklist::{expand_tilde, is_blacklisted};
use super::categories::{builtin_rules, CUSTOM_GROUP, OTHER_GROUP};
use super::types::{AutoGroup, CategoryRule, DirEntry, RiskLevel};

/// Sentinel: a directory matched by a builtin rule. Grouped together so the
/// scanner can keep rule-matched entries separate from 【其他目录】.
struct Matched {
    rule_index: usize,
    entry: DirEntry,
}

/// A directory size-computation task. Resolves to `(path, size_bytes)`; the
/// scanner collects these so sizes can be computed in parallel on a background
/// executor and then stitched back into the groups.
pub fn dir_size(path: &Path) -> u64 {
    let mut total: u64 = 0;
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

/// Expand a single `~/...` pattern into the list of absolute candidate
/// directory paths it implies:
/// - `~/foo`        → `[~/.foo]`
/// - `~/foo/*`      → every top-level child of `~/foo` (one level)
/// - a missing/unreadable dir or a `/*` with no children → `[]`
fn expand_pattern(pattern: &str) -> Vec<PathBuf> {
    let base = match expand_tilde(pattern) {
        Some(p) => p,
        None => return Vec::new(),
    };
    // Strip a trailing `/*` (or `/`) to get the enumeration parent.
    let parent_str = base.to_string_lossy();
    let parent_str = parent_str.trim_end_matches('/');
    if let Some(parent) = parent_str.strip_suffix("/*") {
        let parent = PathBuf::from(parent);
        match std::fs::read_dir(&parent) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    // Only directories; symlinks-to-dir are skipped here to
                    // avoid following them (could loop or double-count).
                    let ft = e.file_type().ok()?;
                    if ft.is_dir() {
                        Some(e.path())
                    } else {
                        None
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        vec![base]
    }
}

/// Enumerate the surviving candidate directories for one rule, tagged with the
/// rule's index so the caller can assign them to the right group. Blacklisted,
/// dev-runtime, symlinked (already-migrated), and missing entries are dropped
/// at this layer (PRD §2.1, §5.2, §5.8).
fn matched_for_rule(rule: &CategoryRule, rule_index: usize) -> Vec<Matched> {
    let mut out = Vec::new();
    for pattern in &rule.path_patterns {
        for cand in expand_pattern(pattern) {
            // Skip blacklisted (scan layer — PRD §2.1).
            if is_blacklisted(&cand) {
                continue;
            }
            // Skip active dev runtimes (~/.cargo, ~/.rustup, ...). Moving these
            // breaks the running toolchain; they're excluded even if a rule or
            // custom path matches them.
            if is_dev_runtime(&cand) {
                log::info!("relocate: 跳过活跃开发运行时目录 {}", cand.display());
                continue;
            }
            // Skip if the candidate itself is a symlink — it's already been
            // migrated (points at the external copy). Re-listing it would invite
            // a duplicate migration. Use symlink_metadata so we detect the link
            // itself rather than following it.
            if std::fs::symlink_metadata(&cand)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                continue;
            }
            // Must exist and be a directory.
            let md = match std::fs::metadata(&cand) {
                Ok(m) if m.is_dir() => m,
                _ => continue,
            };
            // Skip empty directories at this stage too (PRD §5.4 auto-prune).
            if std::fs::read_dir(&cand).map(|mut r| r.next().is_none()).unwrap_or(true) {
                continue;
            }
            out.push(Matched {
                rule_index,
                entry: DirEntry {
                    path: cand,
                    size_bytes: 0, // filled in after assignment
                },
            });
            let _ = md; // metadata read for the is_dir check only
        }
    }
    out
}

/// True if `path` is an active development-runtime directory that must never be
/// migrated (e.g. ~/.cargo, ~/.rustup). Compares the canonical/expanded path
/// against [`super::categories::DEV_RUNTIME_EXCLUDE`].
fn is_dev_runtime(path: &Path) -> bool {
    use super::categories::DEV_RUNTIME_EXCLUDE;
    let home = match super::blacklist::home() {
        Some(h) => h,
        None => return false,
    };
    for pat in DEV_RUNTIME_EXCLUDE {
        if let Some(expanded) = super::blacklist::expand_tilde(pat) {
            // Match if path equals the expanded pattern or lives directly under it.
            if &expanded == path || path.parent() == Some(expanded.as_path()) {
                // Only exclude the exact dir itself (not its children), to keep
                // this precise. Exact match is the common case.
                if expanded == path {
                    return true;
                }
            }
        }
    }
    false
}

/// Top-level non-rule-matched home directories, for the 【其他目录】 bucket
/// (PRD §5.7). Only *non-empty*, *non-blacklisted* directories that no builtin
/// rule already claimed are considered.
fn other_home_dirs(
    home: &Path,
    claimed: &std::collections::HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(home) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for e in rd.flatten() {
        let path = e.path();
        // Directories only.
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        // Scan-layer blacklist (PRD §2.1).
        if is_blacklisted(&path) {
            continue;
        }
        // ~/Library itself must never appear in 【其他目录】 — it holds app
        // runtimes (Application Support, Preferences, etc.) and migrating the
        // whole dir breaks apps. Specific cache subdirs under ~/Library are
        // still matched by the dev-caches rule's explicit patterns, which are
        // handled separately in matched_for_rule (not here).
        if path.file_name().map(|n| n == "Library").unwrap_or(false)
            && path.parent() == Some(home)
        {
            continue;
        }
        // Skip active dev runtimes (PRD safety — breaks toolchain).
        if is_dev_runtime(&path) {
            continue;
        }
        // Skip if already claimed by a rule (one directory, one group).
        if claimed.contains(&path) {
            continue;
        }
        // Skip empty (PRD §5.4).
        if std::fs::read_dir(&path).map(|mut r| r.next().is_none()).unwrap_or(true) {
            continue;
        }
        out.push(path);
    }
    out
}

/// Final clustered scan result. Groups appear in the order: builtin rules
/// (priority-desc), then 【用户自定义目录】 (populated by the engine, left
/// empty here), then 【其他目录】 last.
#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub groups: Vec<AutoGroup>,
}

/// Scan the home directory and cluster into groups (PRD §5).
///
/// `custom_paths` are user-added directories to fold into the
/// 【用户自定义目录】 group; they are blacklist-checked here too (defense in
/// depth, even though the UI rejects them at add-time).
pub fn scan(custom_paths: Vec<PathBuf>) -> ScanResult {
    let rules = builtin_rules();

    // 1) Collect rule-matched candidates (dedup across patterns, keep the
    //    highest-priority rule's assignment).
    let mut matched: Vec<Matched> = Vec::new();
    let mut assigned: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for (rule_index, rule) in rules.iter().enumerate() {
        for m in matched_for_rule(rule, rule_index) {
            if assigned.contains(&m.entry.path) {
                continue; // already claimed by a higher-priority rule
            }
            assigned.insert(m.entry.path.clone());
            matched.push(m);
        }
    }

    // 2) Compute sizes for every rule-matched directory.
    for m in matched.iter_mut() {
        m.entry.size_bytes = dir_size(&m.entry.path);
    }

    // 3) Build the builtin AutoGroups in priority order.
    let mut groups: Vec<AutoGroup> = Vec::new();
    for (rule_index, rule) in rules.iter().enumerate() {
        let mut entries: Vec<DirEntry> = matched
            .iter()
            .filter(|m| m.rule_index == rule_index)
            .map(|m| m.entry.clone())
            .collect();
        // Skip empty groups (PRD §5.4).
        if entries.is_empty() {
            continue;
        }
        // Largest first — the UI shows the biggest space-hogs at the top.
        entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
        let mut group = AutoGroup {
            group_name: rule.group_name.clone(),
            description: rule.description.clone(),
            hint_text: rule.hint_text.clone(),
            risk: rule.default_risk,
            entries,
            total_size_bytes: 0,
        };
        group.recompute_total();
        groups.push(group);
    }

    // 4) 【用户自定义目录】 group (PRD §6) — only blacklist-passing, existing,
    //    non-empty directories are kept.
    let mut custom_entries: Vec<DirEntry> = Vec::new();
    for p in custom_paths {
        if is_blacklisted(&p) {
            continue;
        }
        // Reject ~/Library as a custom migration target — it holds app
        // runtimes and migrating it breaks applications (JetBrains, Mail, etc.).
        if let Some(home) = super::blacklist::home() {
            if p == home.join("Library") {
                log::warn!(
                    "relocate: 拒绝迁移 ~/Library（包含应用运行时数据，迁移会破坏应用程序）：{}",
                    p.display()
                );
                continue;
            }
        }
        // Reject active dev runtimes even when user-added (safety).
        if is_dev_runtime(&p) {
            log::warn!(
                "relocate: 拒绝迁移活跃开发运行时目录（迁移会破坏工具链）：{}",
                p.display()
            );
            continue;
        }
        // Skip if it's already a symlink (already migrated).
        if std::fs::symlink_metadata(&p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        let md = match std::fs::metadata(&p) {
            Ok(m) if m.is_dir() => m,
            _ => continue,
        };
        let _ = md;
        let size = dir_size(&p);
        if size == 0 {
            continue;
        }
        custom_entries.push(DirEntry {
            path: p,
            size_bytes: size,
        });
    }
    if !custom_entries.is_empty() {
        custom_entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
        let mut group = AutoGroup {
            group_name: CUSTOM_GROUP.to_string(),
            description: "用户手动添加的迁移目录。".to_string(),
            hint_text: "🟢 自定义目录 | 已通过黑名单校验，可安全迁移。".to_string(),
            risk: RiskLevel::Safe,
            entries: custom_entries,
            total_size_bytes: 0,
        };
        group.recompute_total();
        groups.push(group);
    }

    // 5) 【其他目录】 group (PRD §5.7) — non-empty home subdirs matched by no
    //    rule. Size-computed lazily; capped to keep the scan snappy.
    if let Some(home) = super::blacklist::home() {
        let others = other_home_dirs(&home, &assigned);
        if !others.is_empty() {
            let mut entries: Vec<DirEntry> = Vec::new();
            for p in others {
                let size = dir_size(&p);
                if size == 0 {
                    continue;
                }
                entries.push(DirEntry {
                    path: p,
                    size_bytes: size,
                });
            }
            if !entries.is_empty() {
                entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
                let mut group = AutoGroup {
                    group_name: OTHER_GROUP.to_string(),
                    description: "未匹配自动规则的目录，可手动选择是否迁移。".to_string(),
                    hint_text: "⚪ 其他目录 | 手动选择".to_string(),
                    risk: RiskLevel::Safe,
                    entries,
                    total_size_bytes: 0,
                };
                group.recompute_total();
                groups.push(group);
            }
        }
    }

    ScanResult { groups }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "verve-relocate-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dir_size_counts_nested_files() {
        let home = tmp_home();
        let base = home.join(".cache");
        fs::create_dir_all(base.join("a/b")).unwrap();
        fs::write(base.join("f1.bin"), vec![0u8; 1024]).unwrap();
        fs::write(base.join("a/b/f2.bin"), vec![0u8; 2048]).unwrap();
        let size = dir_size(&base);
        assert_eq!(size, 1024 + 2048);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn empty_group_pruned() {
        // Run a scan with a custom path that doesn't exist → group not added.
        let result = scan(vec![PathBuf::from("/nonexistent/verve-test-path")]);
        for g in &result.groups {
            assert!(!g.entries.is_empty());
        }
    }

    /// A custom path that is itself a symlink (already migrated) must be skipped
    /// — otherwise it would invite a duplicate migration.
    #[test]
    fn scan_skips_symlink_custom_paths() {
        let home = super::super::blacklist::home().expect("HOME set in test env");
        let tmp = home.join(format!(
            ".verve-scan-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let real = tmp.join("real_dir");
        let link = tmp.join("linked_dir");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("f.txt"), b"data").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // `link` is a symlink → scan should skip it.
        let result = scan(vec![link.clone()]);
        let all_paths: Vec<String> = result
            .groups
            .iter()
            .flat_map(|g| g.entries.iter())
            .map(|e| e.path.to_string_lossy().into_owned())
            .collect();
        assert!(
            !all_paths.iter().any(|p| *p == link.to_string_lossy()),
            "symlink custom path should be skipped, but it appeared: {:?}",
            all_paths
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Dev-runtime directories (~/.cargo, ~/.rustup) must be rejected even when
    /// passed as custom paths — migrating them breaks the toolchain.
    #[test]
    fn scan_rejects_dev_runtime_custom_paths() {
        let home = super::super::blacklist::home().expect("HOME set in test env");
        // If ~/.cargo is a real dir (not migrated), it should still be rejected
        // as a dev-runtime path by the scanner.
        let cargo = home.join(".cargo");
        let is_real = std::fs::symlink_metadata(&cargo)
            .map(|m| m.is_dir() && !m.file_type().is_symlink())
            .unwrap_or(false);
        if !is_real {
            eprintln!("skip: ~/.cargo not a real dir in this env");
            return;
        }
        let result = scan(vec![cargo.clone()]);
        let all_paths: Vec<String> = result
            .groups
            .iter()
            .flat_map(|g| g.entries.iter())
            .map(|e| e.path.to_string_lossy().into_owned())
            .collect();
        assert!(
            !all_paths.iter().any(|p| *p == cargo.to_string_lossy()),
            "~/.cargo must be rejected as dev-runtime, but it appeared: {:?}",
            all_paths
        );
    }
}
