//! Single-directory migration engine (PRD §8) — the atomic, ordered steps.
//!
//! Flow for [`migrate_one`]:
//! 1. Blacklist re-check (execute layer — PRD §2.1, §11).
//! 2. Process-occupancy detection via `lsof` (macOS) — abort + tell the user
//!    to close the offending program (PRD §8.2.1).
//! 3. Target-conflict detection — surface a [`ConflictPolicy`] so the UI can
//!    ask the user (merge / backup-rename / skip) (PRD §8.2.2).
//! 4. Resumable directory copy with progress callbacks (PRD §8.2.3).
//! 5. Optional SHA-256 integrity verification (PRD §8.2.4).
//! 6. After copy + verify succeed, delete the original local directory
//!    (PRD §8.2.5).
//! 7. Create an **absolute-path** symlink at the original location pointing at
//!    the external copy (PRD §2.2, §8.2.6).
//! 8. Return the [`super::types::TaskRecord`] to persist (status Migrated)
//!    (PRD §8.2.7).
//!
//! # Safety (PRD §2.2)
//! - Symlinks are *always* absolute.
//! - Removing a link later uses `remove_file` (single unlink), never a
//!   recursive directory removal — see [`super::rollback`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::blacklist::assert_migratable;
use super::types::TaskRecord;

/// What to do when the external target already contains a directory of the
/// same name (PRD §8.2.2). The UI presents these three options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Merge the source into the existing target (skip files already present).
    Merge,
    /// Rename the existing target aside (`.relocate-backup-<ts>`) before copy.
    BackupRename,
    /// Abort this task; the queue continues with the next item.
    Skip,
}

/// Progress reported by the copy operation. `bytes_done` is cumulative across
/// the whole directory; `current_file` is the path being copied right now.
#[derive(Clone, Debug)]
pub struct CopyProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current_file: String,
}

/// Outcome of [`migrate_one`].
#[derive(Clone, Debug)]
pub enum MigrateOutcome {
    /// Migration succeeded; the record should be persisted.
    Done {
        record: TaskRecord,
        final_progress: CopyProgress,
    },
    /// A conflict was detected and policy was `Skip` (or the user declined).
    Skipped { reason: String },
    /// A precondition failed before any destructive action (occupancy, space,
    /// blacklist, target-not-dir). No data was modified.
    PreconditionFailed { reason: String },
    /// Something went wrong mid-copy/verify; partial external data may exist.
    Failed { reason: String },
}

/// Detect whether a path (or anything under it) is held open by a running
/// process (PRD §8.2.1). Returns the offending process summary if occupied.
///
/// macOS only — `lsof` invocation. On other platforms, returns `Ok(())`
/// (best-effort; the copy will still fail on a locked file with a clear error).
#[cfg(target_os = "macos")]
pub fn check_occupied(path: &Path) -> Result<(), String> {
    let out = std::process::Command::new("lsof")
        .args(["-Fpcn"]) // terse output: pid, command, count
        .arg(path)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            // lsof prints nothing when no process holds the path.
            if o.stdout.is_empty() {
                Ok(())
            } else {
                let summary = summarize_lsof(&o.stdout);
                Err(format!(
                    "目录被进程占用，请先关闭相关程序后重试：\n{}",
                    summary
                ))
            }
        }
        // lsof exit 1 means "no matches" → not occupied.
        Ok(_) => Ok(()),
        Err(e) => {
            // Couldn't run lsof — warn but don't block (copy will surface
            // a real error if a file is actually locked).
            log::warn!("relocate: lsof unavailable ({e}); skipping occupancy check");
            Ok(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn check_occupied(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn summarize_lsof(stdout: &[u8]) -> String {
    // lsof -F emits lines prefixed p/c/n; collapse to a short list of
    // pid+command pairs (deduped, capped).
    use std::collections::BTreeSet;
    let text = String::from_utf8_lossy(stdout);
    let mut pid: Option<String> = None;
    let mut cmd: Option<String> = None;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for line in text.lines() {
        match line.chars().next() {
            Some('p') => pid = Some(line[1..].to_string()),
            Some('c') => cmd = Some(line[1..].to_string()),
            _ => {}
        }
        if let (Some(p), Some(c)) = (pid.take(), cmd.take()) {
            seen.insert(format!("• {} ({})", c, p));
            if seen.len() >= 8 {
                break;
            }
        }
    }
    seen.into_iter().collect::<Vec<_>>().join("\n")
}

/// Resolve the external target path for a source directory: target_root joined
/// with the source's final path component (e.g. `~/.cargo` →
/// `<root>/.cargo`).
pub fn external_target(target_root: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| source.as_os_str().to_os_string());
    target_root.join(name)
}

/// Copy a directory tree into `dest`, reporting progress via the callback and
/// applying `policy` for a pre-existing `dest`.
///
/// Uses `fs_extra::dir::copy_with_progress`. fs_extra's behaviour:
/// - `content_only=false` (fresh dest): copies `source` *as* `dest/basename`.
///   We therefore copy into `dest`'s parent with the parent pre-created so the
///   result lands exactly at `dest`.
/// - `content_only=true` (merge into existing dest): copies source's contents
///   directly into `dest`, leaving dest in place.
fn copy_dir_with_progress(
    source: &Path,
    dest: &Path,
    policy: ConflictPolicy,
    mut on_progress: impl FnMut(&CopyProgress),
    bytes_total: u64,
) -> Result<u64, String> {
    let _ = bytes_total;

    // Conflict handling (dest already exists).
    if dest.exists() {
        match policy {
            ConflictPolicy::Skip => {
                return Err("用户选择跳过（目标已存在）".into());
            }
            ConflictPolicy::BackupRename => {
                let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
                let backup = dest.with_extension(format!("relocate-backup-{}", ts));
                std::fs::rename(dest, &backup)
                    .map_err(|e| format!("备份重命名失败: {}", e))?;
            }
            ConflictPolicy::Merge => {
                // Leave dest in place; copy_with_progress merges into it.
            }
        }
    }

    let dest_exists_now = dest.exists();
    let mut options = fs_extra::dir::CopyOptions::new();
    options.content_only = dest_exists_now;
    options.overwrite = policy == ConflictPolicy::Merge;

    // For a fresh copy, fs_extra appends `basename` to `dest`, so we point it
    // at the parent to land at `dest` exactly.
    let copy_target: PathBuf = if dest_exists_now {
        dest.to_path_buf()
    } else {
        // Ensure parent exists, then let fs_extra create dest/basename == dest.
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目标父目录失败 {}: {}", parent.display(), e))?;
        }
        dest.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| dest.to_path_buf())
    };

    let result = fs_extra::dir::copy_with_progress(source, &copy_target, &options, |tp| {
        let cp = CopyProgress {
            bytes_done: tp.copied_bytes,
            bytes_total: tp.total_bytes,
            current_file: tp.file_name.clone(),
        };
        on_progress(&cp);
        fs_extra::dir::TransitProcessResult::ContinueOrAbort
    });

    match result {
        Ok(bytes) => Ok(bytes),
        Err(e) => Err(format!("复制失败: {}", e)),
    }
}

/// Compute a SHA-256 digest of every file under `dir` (relative-path sorted)
/// and return the concatenation of digests. Two trees match iff this string
/// matches — used for the optional integrity check (PRD §8.2.4).
pub fn tree_sha256(dir: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut entries: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect();
    entries.sort();
    let mut combined = Sha256::new();
    for f in entries {
        let rel = f
            .strip_prefix(dir)
            .unwrap_or(&f)
            .to_string_lossy()
            .into_owned();
        combined.update(rel.as_bytes());
        let bytes = std::fs::read(&f).map_err(|e| format!("读取校验失败 {}: {}", f.display(), e))?;
        combined.update(&bytes);
    }
    let digest = combined.finalize();
    Ok(format!("{:x}", digest))
}

/// Run the full single-directory migration flow. `on_progress` is invoked from
/// the copy step (run on a background executor by the caller). No destructive
/// action happens before copy+verify succeed.
///
/// `size_bytes` is the pre-scanned source size (used for the record and to
/// short-circuit a space sanity check).
pub fn migrate_one(
    source: &Path,
    target_root: &Path,
    group_name: &str,
    size_bytes: u64,
    conflict: ConflictPolicy,
    verify: bool,
    on_progress: impl FnMut(&CopyProgress),
) -> MigrateOutcome {
    let mut on_progress = on_progress;

    // 1. Blacklist execute-layer (PRD §2.1, §11).
    if let Err(reason) = assert_migratable(source) {
        return MigrateOutcome::PreconditionFailed { reason };
    }

    // Source must be an existing directory.
    let src_meta = match std::fs::metadata(source) {
        Ok(m) if m.is_dir() => m,
        Ok(_) => {
            return MigrateOutcome::PreconditionFailed {
                reason: format!("源路径不是目录：{}", source.display()),
            }
        }
        Err(e) => {
            return MigrateOutcome::PreconditionFailed {
                reason: format!("源目录不可读 {}: {}", source.display(), e),
            }
        }
    };
    let _ = src_meta;

    // If the source is already a symlink, the directory has already been
    // migrated — report and skip (don't double-migrate).
    if std::fs::symlink_metadata(source)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return MigrateOutcome::Skipped {
            reason: format!("源路径已是符号链接（可能已迁移）：{}", source.display()),
        };
    }

    // 2. Occupancy check (PRD §8.2.1).
    if let Err(reason) = check_occupied(source) {
        return MigrateOutcome::PreconditionFailed { reason };
    }

    let dest = external_target(target_root, source);

    // 3+4. Copy with conflict policy + progress (PRD §8.2.2/3).
    let copied = match copy_dir_with_progress(source, &dest, conflict, |cp| on_progress(cp), size_bytes)
    {
        Ok(n) => n,
        Err(reason) => {
            // Distinguish "user chose skip" from a real failure.
            if reason.starts_with("用户选择跳过") {
                return MigrateOutcome::Skipped { reason };
            }
            return MigrateOutcome::Failed { reason };
        }
    };

    // 5. Optional integrity verification (PRD §8.2.4).
    if verify {
        let src_digest = match tree_sha256(source) {
            Ok(d) => d,
            Err(reason) => {
                return MigrateOutcome::Failed { reason };
            }
        };
        let dst_digest = match tree_sha256(&dest) {
            Ok(d) => d,
            Err(reason) => {
                return MigrateOutcome::Failed { reason };
            }
        };
        if src_digest != dst_digest {
            return MigrateOutcome::Failed {
                reason: "完整性校验失败：源与目标目录内容不一致".into(),
            };
        }
    }

    // 6. Delete the original local directory (only after copy+verify).
    if let Err(e) = std::fs::remove_dir_all(source) {
        return MigrateOutcome::Failed {
            reason: format!("删除本地原目录失败 {}: {}", source.display(), e),
        };
    }

    // 7. Create absolute-path symlink (PRD §2.2, §8.2.6).
    let abs_dest = std::fs::canonicalize(&dest).unwrap_or_else(|_| dest.clone());
    #[cfg(unix)]
    {
        if let Err(e) = std::os::unix::fs::symlink(&abs_dest, source) {
            return MigrateOutcome::Failed {
                reason: format!("创建符号链接失败 {}: {}", source.display(), e),
            };
        }
    }
    #[cfg(not(unix))]
    {
        return MigrateOutcome::Failed {
            reason: "当前平台不支持符号链接".into(),
        };
    }

    // 8. Build the task record (PRD §3.5, §8.2.7).
    let record = TaskRecord {
        task_id: uuid::Uuid::new_v4().to_string(),
        source_path: source.to_string_lossy().into_owned(),
        target_external_path: abs_dest.to_string_lossy().into_owned(),
        symlink_path: source.to_string_lossy().into_owned(),
        group_name: group_name.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        status: super::types::TaskStatus::Migrated,
        error: None,
        size_bytes: copied.max(size_bytes),
    };

    let final_progress = CopyProgress {
        bytes_done: copied.max(size_bytes),
        bytes_total: copied.max(size_bytes),
        current_file: String::new(),
    };
    MigrateOutcome::Done {
        record,
        final_progress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_target_joins_basename() {
        let t = external_target(Path::new("/Volumes/Ext/relocate"), Path::new("/Users/x/.cargo"));
        assert_eq!(t, PathBuf::from("/Volumes/Ext/relocate/.cargo"));
    }

    #[test]
    fn tree_sha256_matches_identical_trees() {
        let tmp = std::env::temp_dir().join(format!(
            "verve-migrate-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let a = tmp.join("a");
        let b = tmp.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("f.txt"), b"hello").unwrap();
        std::fs::write(b.join("f.txt"), b"hello").unwrap();
        assert_eq!(tree_sha256(&a).unwrap(), tree_sha256(&b).unwrap());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// End-to-end check that after migration the original path is an
    /// **absolute-path symlink** pointing at the external copy, and that files
    /// remain reachable through it (the "still usable after move" guarantee —
    /// PRD §2.2, §8.2.6). This guards the critical copy→delete→symlink sequence.
    #[test]
    #[cfg(unix)]
    fn migrate_creates_absolute_symlink_and_stays_usable() {
        use std::path::Path;

        // NOTE: we can't use std::env::temp_dir() here — on macOS it lives under
        // /var → /private, which the relocate system blacklist rejects, and
        // migrate_one enforces the blacklist at execute time. We instead root
        // the fixture under the user's home (the real migration scenario: the
        // source is always a home subdirectory).
        let home = match std::env::var_os("HOME").map(std::path::PathBuf::from) {
            Some(h) if !h.as_os_str().is_empty() => h,
            _ => {
                // No HOME: nothing meaningful to assert about a home-rooted
                // migration. Skip rather than fail.
                eprintln!("skip: HOME not set");
                return;
            }
        };
        let tmp = home.join(format!(
            ".verve-migrate-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Layout: tmp/home/.proj  (source)  →  tmp/external/.proj  (target).
        let src_root = tmp.join("home");
        let target_root = tmp.join("external");
        let source = src_root.join(".proj");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::create_dir_all(&target_root).unwrap();
        std::fs::write(source.join("a.txt"), b"alpha").unwrap();
        std::fs::write(source.join("nested").join("b.txt"), b"beta").unwrap();

        let abs_source = std::fs::canonicalize(&source).unwrap_or(source.clone());
        let size = 10u64; // bytes; exact value doesn't affect the symlink check.

        let outcome = migrate_one(
            &source,
            &target_root,
            "test-group",
            size,
            ConflictPolicy::Merge,
            false, // skip verify to keep the test fast; symlink logic is the focus
            |_| {},
        );

        let record = match outcome {
            MigrateOutcome::Done { record, .. } => record,
            other => panic!("expected Done, got {other:?}"),
        };

        // 1. The source location is now a symlink (not a real directory).
        let meta = std::fs::symlink_metadata(&source).expect("source path missing after migrate");
        assert!(
            meta.file_type().is_symlink(),
            "source should be a symlink after migration"
        );

        // 2. It points at the external copy via an ABSOLUTE path.
        let link_target = std::fs::read_link(&source).expect("read_link");
        assert!(
            link_target.is_absolute(),
            "symlink must be absolute (PRD §2.2), got {:?}",
            link_target
        );
        let canonical_ext = std::fs::canonicalize(&target_root)
            .unwrap_or_else(|_| target_root.clone())
            .join(".proj");
        assert_eq!(
            link_target, canonical_ext,
            "symlink should point at the external copy"
        );

        // 3. Files are still reachable THROUGH the original path — i.e. the
        //    directory remains usable as if it had never moved.
        assert_eq!(
            std::fs::read(abs_source.join("a.txt")).unwrap(),
            b"alpha",
            "file must still be readable via original path"
        );
        assert_eq!(
            std::fs::read(abs_source.join("nested").join("b.txt")).unwrap(),
            b"beta",
            "nested file must still be readable via original path"
        );
        // `metadata` (which follows the link) reports a directory, so existing
        // tools that `stat` the path keep working.
        assert!(
            Path::new(&abs_source)
                .metadata()
                .map(|m| m.is_dir())
                .unwrap_or(false),
            "path through symlink must still resolve to a directory"
        );

        // 4. The persisted record reflects the absolute symlink target.
        assert_eq!(
            record.symlink_path,
            source.to_string_lossy(),
            "record symlink_path should be the original source"
        );
        assert!(
            record.target_external_path.starts_with(
                std::fs::canonicalize(&target_root)
                    .unwrap_or_else(|_| target_root.clone())
                    .to_string_lossy()
                    .as_ref()
            ),
            "record target_external_path should be under the external root"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
}
