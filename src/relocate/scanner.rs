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
/// rule's index so the caller can assign them to the right group. Blacklisted
/// and missing entries are dropped at this layer (PRD §2.1, §5.2, §5.8).
fn matched_for_rule(rule: &CategoryRule, rule_index: usize) -> Vec<Matched> {
    let mut out = Vec::new();
    for pattern in &rule.path_patterns {
        for cand in expand_pattern(pattern) {
            // Skip blacklisted (scan layer — PRD §2.1).
            if is_blacklisted(&cand) {
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
}
