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
    /// The user cancelled. Carries the exact on-disk state so the engine can
    /// do the *cheapest correct* recovery:
    /// - `copy_completed=false, source_deleted=false`: copy aborted partway;
    ///   the partial external copy (`dest`) was already removed by `migrate_one`
    ///   and the original (`source`) is intact. The engine need not touch it.
    /// - `copy_completed=true, source_deleted=false`: copy finished but the
    ///   original was not yet deleted. The engine should just `remove_dir_all(dest)`;
    ///   the original is intact (zero extra copy).
    /// - `copy_completed=true, source_deleted=true`: original was deleted but the
    ///   symlink was not created. The engine must restore the original from
    ///   `dest` (`rollback::restore_from_external`), then remove `dest`.
    Cancelled {
        copy_completed: bool,
        source_deleted: bool,
        dest: PathBuf,
        source: PathBuf,
    },
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

/// Resolve the external target path for a source directory.
///
/// Preserves the source's path **relative to the user's home directory** so the
/// external copy mirrors the original on-disk layout (e.g.
/// `~/.cargo` → `<root>/.cargo`, `~/Library/Caches/X` →
/// `<root>/Library/Caches/X`). This avoids basename collisions between unrelated
/// directories and keeps the external tree browsable. Directories outside `~`
/// (rare for custom-added paths) fall back to the basename, same as before.
pub fn external_target(target_root: &Path, source: &Path) -> PathBuf {
    let rel: std::path::PathBuf = match super::blacklist::home() {
        Some(home) => source
            .strip_prefix(&home)
            .map(|r| r.to_path_buf())
            .unwrap_or_else(|_| source.to_path_buf()),
        None => source.to_path_buf(),
    };
    // If strip_prefix failed (path not under home), fall back to basename so we
    // never accidentally recreate a full absolute path under the external root.
    let rel = if rel.starts_with("/") || rel == *source {
        source
            .file_name()
            .map(std::ffi::OsString::from)
            .map(PathBuf::from)
            .unwrap_or_else(|| source.to_path_buf())
    } else {
        rel
    };
    target_root.join(rel)
}

/// Sentinel error string used when the user cancels mid-copy. The caller can
/// match on this prefix to distinguish cancellation from a real failure.
pub(crate) const CANCELLED_ERR: &str = "__relocate_cancelled__";

/// Copy a directory tree into `dest`, reporting per-chunk progress via the
/// callback and applying `policy` for a pre-existing `dest`.
///
/// Unlike `fs_extra::dir::copy_with_progress`, this is a hand-written recursive
/// copy that **checks `cancel` between every buffer chunk and every file**, so
/// the caller can interrupt an in-flight copy (fs_extra's chunk callback
/// returns `()` and cannot abort). File permissions are preserved to match
/// fs_extra's behaviour.
///
/// `bytes_total` is the pre-scanned source size, used to report total bytes in
/// `CopyProgress` (so progress fractions are stable even before the first
/// chunk).
pub(crate) fn copy_dir_with_progress(
    source: &Path,
    dest: &Path,
    policy: ConflictPolicy,
    mut on_progress: impl FnMut(&CopyProgress),
    bytes_total: u64,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<u64, String> {
    use std::io::{Read, Write};

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
                // Leave dest in place; we copy into it, skipping existing files.
            }
        }
    } else {
        // Fresh copy: create dest itself.
        std::fs::create_dir_all(dest)
            .map_err(|e| format!("创建目标目录失败 {}: {}", dest.display(), e))?;
    }

    let overwrite = policy == ConflictPolicy::Merge;
    let mut copied: u64 = 0;
    // A 1 MB buffer keeps read/write syscall counts low for large files (the
    // old 64 KB buffer issued ~16k syscalls per GB). On SSD→SSD this is close
    // to a plain `cp` in throughput.
    let mut buf = vec![0u8; 1024 * 1024];
    // Progress sampling: only emit a sample when ≥100 ms have elapsed OR ≥4 MB
    // have been copied since the last one, so the per-chunk callback (which
    // crosses a channel + emits a GPUI event) doesn't dominate copy time.
    // For a 1 GB file this is ~250 callbacks instead of ~16,000.
    let mut last_progress_bytes: u64 = 0;
    let mut last_progress_time = std::time::Instant::now();
    let progress_min_interval = std::time::Duration::from_millis(100);
    const PROGRESS_MIN_BYTES: u64 = 4 * 1024 * 1024;

    // Walk the source tree, recreating the directory layout under dest and
    // copying each file chunk-by-chunk.
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        // Honour cancellation between entries.
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(CANCELLED_ERR.to_string());
        }
        let rel = match entry.path().strip_prefix(source) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let target = dest.join(rel);
        let ft = entry.file_type();
        if ft.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|e| format!("创建目录失败 {}: {}", target.display(), e))?;
            // Preserve directory permissions where possible.
            if let Ok(meta) = entry.metadata() {
                let _ = std::fs::set_permissions(&target, meta.permissions());
            }
            continue;
        }
        if !ft.is_file() {
            // Symlinks / special files: skip (mirrors fs_extra's file-only copy).
            continue;
        }

        let file_name = entry
            .file_name()
            .to_string_lossy()
            .into_owned();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => return Err(format!("读取文件元数据失败 {}: {}", entry.path().display(), e)),
        };
        let file_size = meta.len();

        // Merge policy: skip files that already exist at the target.
        if target.exists() && !overwrite {
            // Still count the file as "done" for progress purposes.
            copied = copied.saturating_add(file_size);
            // Sample: skiped files are cheap, but avoid per-file callbacks for
            // trees with many small files — only report if enough has accrued.
            if copied - last_progress_bytes >= PROGRESS_MIN_BYTES
                || last_progress_time.elapsed() >= progress_min_interval
            {
                last_progress_bytes = copied;
                last_progress_time = std::time::Instant::now();
                on_progress(&CopyProgress {
                    bytes_done: copied,
                    bytes_total,
                    current_file: file_name.clone(),
                });
            }
            continue;
        }

        // Open + copy chunk-by-chunk, checking cancel after each chunk.
        let mut reader = match std::fs::File::open(entry.path()) {
            Ok(f) => f,
            Err(e) => return Err(format!("打开源文件失败 {}: {}", entry.path().display(), e)),
        };
        let mut writer = match std::fs::File::create(&target) {
            Ok(f) => f,
            Err(e) => return Err(format!("创建目标文件失败 {}: {}", target.display(), e)),
        };
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(CANCELLED_ERR.to_string());
            }
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    return Err(format!(
                        "读取文件失败 {}: {}",
                        entry.path().display(),
                        e
                    ))
                }
            };
            if let Err(e) = writer.write_all(&buf[..n]) {
                return Err(format!("写入文件失败 {}: {}", target.display(), e));
            }
            copied = copied.saturating_add(n as u64);
            // Sample progress to avoid per-chunk callback overhead.
            if copied - last_progress_bytes >= PROGRESS_MIN_BYTES
                || last_progress_time.elapsed() >= progress_min_interval
            {
                last_progress_bytes = copied;
                last_progress_time = std::time::Instant::now();
                on_progress(&CopyProgress {
                    bytes_done: copied,
                    bytes_total,
                    current_file: file_name.clone(),
                });
            }
        }
        // Flush + drop to ensure data is on disk before we set permissions /
        // move on, then preserve the source file's permission bits.
        if let Err(e) = writer.flush() {
            return Err(format!("写入刷新失败 {}: {}", target.display(), e));
        }
        drop(writer);
        let _ = std::fs::set_permissions(&target, meta.permissions());
        // Always report the final byte count for this file so progress never
        // lags behind the actual copied amount between samples.
        if copied != last_progress_bytes {
            last_progress_bytes = copied;
            last_progress_time = std::time::Instant::now();
            on_progress(&CopyProgress {
                bytes_done: copied,
                bytes_total,
                current_file: file_name.clone(),
            });
        }
        let _ = file_size;
    }

    Ok(copied)
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
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
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

    // 3+4. Copy with conflict policy + progress (PRD §8.2.2/3). Cancel-aware:
    //      the copy checks `cancel` between chunks; on cancel the original
    //      directory is still intact (the destructive steps run only after a
    //      successful copy), so we just remove the partial external copy.
    let copied = match copy_dir_with_progress(
        source,
        &dest,
        conflict,
        |cp| on_progress(cp),
        size_bytes,
        &cancel,
    ) {
        Ok(n) => n,
        Err(reason) => {
            // User cancelled mid-copy: clean up the partial external copy and
            // report cancellation. The original local directory is untouched.
            if reason == CANCELLED_ERR {
                let _ = std::fs::remove_dir_all(&dest);
                return MigrateOutcome::Cancelled {
                    copy_completed: false,
                    source_deleted: false,
                    dest: dest.clone(),
                    source: source.to_path_buf(),
                };
            }
            // Distinguish "user chose skip" from a real failure.
            if reason.starts_with("用户选择跳过") {
                return MigrateOutcome::Skipped { reason };
            }
            return MigrateOutcome::Failed { reason };
        }
    };

    // 5. Optional integrity verification (PRD §8.2.4).
    //    The copy is complete; from here on a cancel means "copy_completed=true".
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

    // Cancel checkpoint: copy finished, original still intact. The engine will
    // just remove the external copy (zero extra copy, original untouched).
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return MigrateOutcome::Cancelled {
            copy_completed: true,
            source_deleted: false,
            dest: dest.clone(),
            source: source.to_path_buf(),
        };
    }

    // 6. Delete the original local directory (only after copy+verify).
    if let Err(e) = std::fs::remove_dir_all(source) {
        return MigrateOutcome::Failed {
            reason: format!("删除本地原目录失败 {}: {}", source.display(), e),
        };
    }

    // Cancel checkpoint: original just deleted, symlink not yet created. The
    // engine must restore the original from the external copy.
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return MigrateOutcome::Cancelled {
            copy_completed: true,
            source_deleted: true,
            dest: dest.clone(),
            source: source.to_path_buf(),
        };
    }

    // 7. Create absolute-path symlink (PRD §2.2, §8.2.6). If this fails the
    //    original is already deleted, so we restore it from the external copy
    //    before reporting failure (never leave the user without their data).
    let abs_dest = std::fs::canonicalize(&dest).unwrap_or_else(|_| dest.clone());
    #[cfg(unix)]
    {
        if let Err(e) = std::os::unix::fs::symlink(&abs_dest, source) {
            let restore_err = super::rollback::restore_from_external(source, &dest);
            let reason = match restore_err {
                Ok(()) => format!(
                    "创建符号链接失败 {}: {}（已从外置副本恢复原目录）",
                    source.display(),
                    e
                ),
                Err(re) => format!(
                    "创建符号链接失败 {} 且恢复原目录失败：{}（外置副本仍在 {}）",
                    source.display(),
                    e,
                    re
                ),
            };
            return MigrateOutcome::Failed { reason };
        }
    }
    #[cfg(not(unix))]
    {
        let _ = super::rollback::restore_from_external(source, &dest);
        return MigrateOutcome::Failed {
            reason: "当前平台不支持符号链接（已从外置副本恢复原目录）".into(),
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
    fn external_target_preserves_home_relative_structure() {
        // Paths under $HOME keep their full relative structure (no flattening,
        // no basename collisions between unrelated dirs).
        let home = super::super::blacklist::home().expect("HOME set in test env");
        let root = Path::new("/Volumes/Ext");

        // ~/.cargo → <root>/.cargo
        let t = external_target(root, &home.join(".cargo"));
        assert_eq!(t, PathBuf::from("/Volumes/Ext/.cargo"));

        // ~/Library/Caches/X → <root>/Library/Caches/X (nested structure preserved)
        let t = external_target(root, &home.join("Library/Caches/X"));
        assert_eq!(t, PathBuf::from("/Volumes/Ext/Library/Caches/X"));
    }

    #[test]
    fn external_target_falls_back_to_basename_outside_home() {
        // A path NOT under $HOME (e.g. a custom dir on another volume) falls
        // back to its basename so we never recreate an absolute path tree.
        let root = Path::new("/Volumes/Ext");
        let t = external_target(root, Path::new("/opt/some/dir/cache"));
        assert_eq!(t, PathBuf::from("/Volumes/Ext/cache"));
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
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        // The external target preserves the source's home-relative path, so
        // compute the expected location via the same helper (not a hardcoded
        // ".proj" — the source lives under <HOME>/.verve-migrate-e2e-.../home).
        let canonical_target_root =
            std::fs::canonicalize(&target_root).unwrap_or_else(|_| target_root.clone());
        let expected_ext = external_target(&canonical_target_root, &source);
        assert_eq!(
            link_target, expected_ext,
            "symlink should point at the external copy (home-relative layout)"
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

    /// Cancelling mid-copy returns `MigrateOutcome::Cancelled`, leaves the
    /// original local directory fully intact, and removes the partial external
    /// copy. The cancellation is triggered from the progress callback (the
    /// realistic path: the engine flips its cancel flag while the copy runs).
    #[test]
    #[cfg(unix)]
    fn migrate_cancelled_mid_copy_leaves_original_intact() {
        let home = match std::env::var_os("HOME").map(std::path::PathBuf::from) {
            Some(h) if !h.as_os_str().is_empty() => h,
            _ => {
                eprintln!("skip: HOME not set");
                return;
            }
        };
        let tmp = home.join(format!(
            ".verve-migrate-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = tmp.join("home").join(".proj");
        let target_root = tmp.join("external");
        // Make the source non-trivial so the copy spans multiple chunks/files
        // before the callback flips the cancel flag.
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::create_dir_all(&target_root).unwrap();
        // Large files (4 MB each) so the copy spans many 1 MB chunks and several
        // progress samples before the callback flips the cancel flag.
        std::fs::write(source.join("a.txt"), vec![b'A'; 4 * 1024 * 1024]).unwrap();
        std::fs::write(source.join("b.txt"), vec![b'B'; 4 * 1024 * 1024]).unwrap();
        std::fs::write(source.join("nested").join("c.txt"), vec![b'C'; 4 * 1024 * 1024]).unwrap();

        // Flip the cancel flag on the first progress sample. With 12 MB of data
        // and 1 MB chunks, the first sample lands well before the copy finishes,
        // so the next chunk-boundary check aborts the copy mid-stream.
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_cb = cancel.clone();
        let outcome = migrate_one(
            &source,
            &target_root,
            "test-group",
            12 * 1024 * 1024,
            ConflictPolicy::Merge,
            false,
            cancel.clone(),
            move |_cp| {
                cancel_cb.store(true, std::sync::atomic::Ordering::Relaxed);
            },
        );

        assert!(
            matches!(
                outcome,
                MigrateOutcome::Cancelled {
                    copy_completed: false,
                    source_deleted: false,
                    ..
                }
            ),
            "expected Cancelled (mid-copy), got {:?}",
            outcome
        );

        // 1. Original local directory is intact (not deleted, not a symlink)
        //    and its files are still readable.
        let meta = std::fs::symlink_metadata(&source).expect("source missing");
        assert!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "source must remain a real directory after cancellation"
        );
        assert_eq!(std::fs::read(source.join("a.txt")).unwrap().len(), 4 * 1024 * 1024);

        // 2. The partial external copy was cleaned up.
        let dest = external_target(&target_root, &source);
        assert!(
            !dest.exists(),
            "partial external copy should be removed after cancellation, but {} exists",
            dest.display()
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    // The post-copy / post-delete cancel checkpoints are simple
    // `if cancel.load() { return Cancelled { ... } }` guards; their state
    // fields are exercised by `rollback::restore_from_external` (the recovery
    // path they trigger) — see that test module for the data-safety coverage.
}
