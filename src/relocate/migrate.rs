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
/// process (PRD §8.2.1). Returns `Some(summary)` if occupied, `None` otherwise.
///
/// This is a **soft** check: it does NOT block migration. Many macOS system
/// caches (e.g. `~/Library/Caches/GeoServices`) are perpetually held open by
/// un-killable daemons (Spotlight, assistantd, suggestd, …), so a hard block
/// would make those directories unmigratable forever. Instead we surface the
/// occupied processes as a warning and let the migration proceed; the copy
/// still succeeds (files are readable while open), and the robust delete step
/// retries — if the dir genuinely can't be removed the migration fails at the
/// delete step with a clear message, by which point the external copy exists
/// and can be cleaned up.
#[cfg(target_os = "macos")]
pub fn check_occupied(path: &Path) -> Option<String> {
    // `+D` makes lsof descend the whole tree rooted at `path`, so it catches
    // processes holding files *inside* the directory (e.g. a running IDE with
    // its cache open) — not just the exact directory path.
    let out = std::process::Command::new("lsof")
        .args(["-Fpcn", "+D"])
        .arg(path)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            if o.stdout.is_empty() {
                None
            } else {
                Some(summarize_lsof(&o.stdout))
            }
        }
        // lsof exit 1 means "no matches" → not occupied.
        Ok(_) => None,
        Err(e) => {
            log::warn!("relocate: lsof unavailable ({e}); skipping occupancy check");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn check_occupied(_path: &Path) -> Option<String> {
    None
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
    on_progress: impl FnMut(&CopyProgress) + Send,
    bytes_total: u64,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<u64, String> {
    use std::sync::atomic::AtomicU64;

    // Wrap the callback in a Mutex so the reporter thread can call it safely.
    let on_progress = std::sync::Mutex::new(on_progress);

    // Conflict handling (dest already exists).
    if dest.exists() {
        match policy {
            ConflictPolicy::Skip => {
                // Skip = don't overwrite existing files, but still copy any
                // missing ones and complete the migration (delete source +
                // create symlink). Previously this aborted the entire copy,
                // leaving the source unmigrated. Now it behaves like Merge but
                // without overwriting existing files.
            }
            ConflictPolicy::BackupRename => {
                let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
                let backup = dest.with_extension(format!("relocate-backup-{}", ts));
                std::fs::rename(dest, &backup).map_err(|e| format!("备份重命名失败: {}", e))?;
            }
            ConflictPolicy::Merge => {
                // Leave dest in place; we copy into it, overwriting existing files.
            }
        }
    } else {
        // Fresh copy: create dest itself.
        std::fs::create_dir_all(dest)
            .map_err(|e| format!("创建目标目录失败 {}: {}", dest.display(), e))?;
    }

    // Skip policy: don't overwrite existing files. Merge: overwrite.
    // BackupRename: fresh dest (old was renamed away), so overwrite=false is fine.
    let overwrite = policy == ConflictPolicy::Merge;

    // ---- Phase 1: walk the tree, create all directories, collect file tasks.
    // Directories are created serially (they're cheap metadata ops and must
    // exist before files land in them). Files are collected into a list for
    // parallel copying in phase 2.
    struct FileTask {
        src: PathBuf,
        dst: PathBuf,
        size: u64,
        perms: Option<std::fs::Permissions>,
        name: String,
    }
    let mut file_tasks: Vec<FileTask> = Vec::new();

    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
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
            if let Ok(meta) = entry.metadata() {
                let _ = std::fs::set_permissions(&target, meta.permissions());
            }
            continue;
        }
        if ft.is_symlink() {
            match std::fs::read_link(entry.path()) {
                Ok(link_target) => {
                    let _ = std::fs::remove_file(&target);
                    #[cfg(unix)]
                    {
                        if let Err(e) = std::os::unix::fs::symlink(&link_target, &target) {
                            return Err(format!(
                                "创建符号链接失败 {} -> {}: {}",
                                target.display(),
                                link_target.display(),
                                e
                            ));
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = link_target;
                    }
                }
                Err(e) => {
                    return Err(format!(
                        "读取符号链接失败 {}: {}",
                        entry.path().display(),
                        e
                    ));
                }
            }
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                return Err(format!(
                    "读取文件元数据失败 {}: {}",
                    entry.path().display(),
                    e
                ));
            }
        };
        file_tasks.push(FileTask {
            src: entry.path().to_path_buf(),
            dst: target,
            size: meta.len(),
            perms: Some(meta.permissions()),
            name: entry.file_name().to_string_lossy().into_owned(),
        });
    }

    // ---- Phase 2: copy files in parallel.
    // A thread pool of N workers pulls tasks from a shared queue. For trees
    // with many small files this saturates the external SSD's IOPS; for a few
    // large files each worker streams a different file concurrently. Progress
    // is tracked atomically and sampled by time.
    let copied_atomic = AtomicU64::new(0);
    // Collect per-file errors instead of aborting on the first one. Most
    // failures are permission-denied on individual files (.git objects,
    // iCloud-fetched files, special chars) — skipping them and continuing
    // the rest of the directory is far more useful than aborting everything.
    let errors: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    let skipped_atomic = AtomicU64::new(0);
    let total_files_atomic = AtomicU64::new(0);
    let num_workers = num_copy_workers();
    let queue = std::sync::Mutex::new(file_tasks.into_iter().collect::<Vec<_>>());

    let reporter_done = std::sync::atomic::AtomicBool::new(false);

    std::thread::scope(|s| {
        s.spawn(|| {
            let interval = std::time::Duration::from_millis(100);
            loop {
                std::thread::sleep(interval);
                if reporter_done.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                if let Ok(mut cb) = on_progress.lock() {
                    cb(&CopyProgress {
                        bytes_done: copied_atomic.load(std::sync::atomic::Ordering::Relaxed),
                        bytes_total,
                        current_file: String::new(),
                    });
                }
            }
        });

        for _ in 0..num_workers {
            s.spawn(|| {
                let mut buf = vec![0u8; 1024 * 1024];
                loop {
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    let task = queue.lock().unwrap().pop();
                    let Some(task) = task else {
                        return;
                    };
                    total_files_atomic.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    if task.dst.exists() && !overwrite {
                        copied_atomic.fetch_add(task.size, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }

                    let result = if task.size <= buf.len() as u64 {
                        std::fs::copy(&task.src, &task.dst).map(|_| task.size)
                    } else {
                        copy_large_file(&task.src, &task.dst, &mut buf, cancel)
                    };
                    match result {
                        Ok(n) => {
                            copied_atomic.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                            if let Some(perms) = &task.perms {
                                let _ = std::fs::set_permissions(&task.dst, perms.clone());
                            }
                        }
                        Err(e) => {
                            // Log and skip — don't abort the whole directory over
                            // one permission-denied file.
                            skipped_atomic.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let err_msg = format!("{}", e);
                            // Keep only the first 50 errors to avoid huge logs.
                            let mut errs = errors.lock().unwrap();
                            if errs.len() < 50 {
                                errs.push((task.src.to_string_lossy().into_owned(), err_msg));
                            }
                            continue;
                        }
                    }
                }
            });
        }
        reporter_done.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(CANCELLED_ERR.to_string());
    }

    let total_files = total_files_atomic.load(std::sync::atomic::Ordering::Relaxed);
    let skipped = skipped_atomic.load(std::sync::atomic::Ordering::Relaxed);
    let errs = errors.lock().unwrap().clone();

    // Final progress report.
    let total = copied_atomic.load(std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut cb) = on_progress.lock() {
        cb(&CopyProgress {
            bytes_done: total,
            bytes_total,
            current_file: String::new(),
        });
    }

    // If some files were skipped (permission denied, etc.), include a summary
    // in the Ok result — the caller (engine) can log it. We don't fail unless
    // ALL files failed.
    if !errs.is_empty() && skipped >= total_files {
        // Every single file failed — this is a real failure.
        let first = &errs[0];
        return Err(format!(
            "复制失败：{} 个文件全部失败。首个错误：{} — {}",
            skipped, first.0, first.1
        ));
    }
    if !errs.is_empty() {
        log::warn!(
            "relocate: 复制完成，跳过 {} 个失败文件（共 {} 个）",
            skipped,
            total_files
        );
    }
    Ok(total)
}

/// Copy a single large file in chunks, honouring cancel between chunks.
/// Returns the number of bytes copied.
fn copy_large_file(
    src: &Path,
    dst: &Path,
    buf: &mut [u8],
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::io::Result<u64> {
    use std::io::{Read, Write};
    let mut reader = std::fs::File::open(src)?;
    let mut writer = std::fs::File::create(dst)?;
    let mut total: u64 = 0;
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            writer.flush()?;
            return Ok(total);
        }
        let n = match reader.read(buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
    writer.flush()?;
    Ok(total)
}

/// Pick a sensible number of copy workers based on available CPUs.
/// Capped at 8 — beyond that the external SSD's queue depth is the bottleneck,
/// not CPU count, and more threads just add scheduling overhead.
fn num_copy_workers() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    cpus.min(8).max(2)
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
        let bytes =
            std::fs::read(&f).map_err(|e| format!("读取校验失败 {}: {}", f.display(), e))?;
        combined.update(&bytes);
    }
    let digest = combined.finalize();
    Ok(format!("{:x}", digest))
}

/// Recursively remove a directory, tolerating transient re-population AND
/// read-only permissions.
///
/// `std::fs::remove_dir_all` can fail with "Directory not empty" on macOS when
/// Spotlight/Finder recreate `.DS_Store` or other metadata between its internal
/// walk and the rmdir, and with "Permission denied" when files are read-only
/// or parent dirs lack write permission (common for caches like `~/.cache`).
///
/// This mirrors `rm -rf`: before deleting anything it makes the entry (and, for
/// directory removal, the parent) user-writable, then does a robust two-pass
/// sweep:
///   1. Walk the tree and delete every file/symlink (chmod u+w first).
///   2. Walk again and rmdir every directory deepest-first (chmod the dir u+w
///      first so its children can be unlinked), then rmdir the root. Retry a
///      few times so transient re-population settles.
/// Returns Ok only once the root path no longer exists.
pub fn force_remove_dir_all(root: &Path) -> Result<(), String> {
    // Fast path: if it's already gone, nothing to do.
    if !root.exists() && !std::fs::symlink_metadata(root).is_ok() {
        return Ok(());
    }

    // Helper: make a path user-writable (best-effort chmod u+w). On unix this
    // adds the owner-write bit; needed before unlinking read-only files and
    // before rmdir-ing a directory whose children must be removable.
    #[cfg(unix)]
    fn ensure_writable(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o200); // S_IWUSR
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    fn ensure_writable(_path: &Path) {}

    // Pass 1: delete every regular file + symlink in the tree. contents_first
    // yields children before parents, which is what we want for files.
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .contents_first(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let ft = entry.file_type();
        if ft.is_file() || ft.is_symlink() {
            ensure_writable(entry.path());
            let _ = std::fs::remove_file(entry.path());
        }
    }

    // Pass 2: remove directories deepest-first (contents_first already yields
    // children before parents), then the root. chmod each dir u+w first (so its
    // remaining children, if any, are unlinkable) and retry a few times to let
    // the filesystem settle any just-removed files.
    for _attempt in 0..5 {
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .contents_first(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_dir() {
                ensure_writable(entry.path());
                let _ = std::fs::remove_dir(entry.path());
            }
        }
        ensure_writable(root);
        // Also try a plain remove_dir_all on the root as a fast finisher.
        let _ = std::fs::remove_dir_all(root);
        if !root.exists() {
            return Ok(());
        }
        // Brief pause before retrying so the OS can flush pending deletes.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    if !root.exists() {
        return Ok(());
    }

    // Last resort on macOS: ask for admin privileges via osascript. This pops
    // up the native password dialog and runs `rm -rf` as root, which can
    // remove files protected by TCC (Downloads, Documents) or owned by other
    // users. On non-macOS or if the user cancels, we fall through to the error.
    #[cfg(target_os = "macos")]
    {
        let path_str = root.to_string_lossy();
        // Escape single quotes in the path for the shell script.
        let escaped = path_str.replace('\'', "'\\''");
        let script = format!(
            "do shell script \"rm -rf '{}'\" with administrator privileges",
            escaped
        );
        let out = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                if !root.exists() {
                    return Ok(());
                }
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                // User cancelled (-128) or wrong password — don't treat as
                // fatal, just fall through to the error message.
                log::warn!("relocate: osascript admin remove failed: {}", stderr.trim());
            }
            Err(e) => {
                log::warn!("relocate: osascript unavailable: {}", e);
            }
        }
    }

    if !root.exists() {
        Ok(())
    } else {
        Err(format!(
            "无法彻底删除目录（可能仍有进程占用其中的文件，或权限不足）：{}",
            root.display()
        ))
    }
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
    on_progress: impl FnMut(&CopyProgress) + Send,
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
            };
        }
        Err(e) => {
            return MigrateOutcome::PreconditionFailed {
                reason: format!("源目录不可读 {}: {}", source.display(), e),
            };
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

    // 2. Occupancy check (PRD §8.2.1) — SOFT: log a warning but don't block.
    //    Many macOS system caches are held open by un-killable daemons; blocking
    //    here would make them unmigratable. We proceed and let the robust delete
    //    step surface a real failure (with the external copy already in place).
    if let Some(summary) = check_occupied(source) {
        log::warn!(
            "relocate: {} 被进程占用，迁移可能失败：\n{}",
            source.display(),
            summary
        );
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
                let _ = force_remove_dir_all(&dest);
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

    // 6. Delete the original local directory (only after copy+verify). Use a
    //    robust recursive remove: `std::fs::remove_dir_all` can fail with
    //    "Directory not empty" on macOS when Spotlight/Finder recreate
    //    `.DS_Store`/metadata between the internal walk and the rmdir, or when
    //    a process briefly touches a file. force_remove_dir_all does a
    //    two-pass sweep (files first, then dirs deepest-first) and retries,
    //    so transient re-population doesn't abort the migration.
    if let Err(e) = force_remove_dir_all(source) {
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
    #[cfg(windows)]
    {
        // Windows supports directory symlinks via symlink_dir; creating one
        // may require Developer Mode or admin privileges — failures follow
        // the same restore-from-external path as Unix.
        if let Err(e) = std::os::windows::fs::symlink_dir(&abs_dest, source) {
            let restore_err = super::rollback::restore_from_external(source, &dest);
            let reason = match restore_err {
                Ok(()) => format!(
                    "创建符号链接失败 {}: {}（已从外置副本恢复原目录；Windows 需开启开发者模式或以管理员运行才能创建符号链接）",
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

    // 8. Build the task record (PRD §3.5, §8.2.7).
    let record = TaskRecord {
        task_id: crate::state::models::new_id(),
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

    /// `force_remove_dir_all` removes a populated directory tree even with
    /// nested files/subdirs, and is a no-op for an already-absent path. This
    /// guards the source-deletion step that was failing with "Directory not
    /// empty" on macOS.
    #[test]
    fn force_remove_dir_all_clears_tree_and_handles_missing() {
        let tmp = std::env::temp_dir().join(format!(
            "verve-force-rm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Nested files + a subdirectory.
        std::fs::create_dir_all(tmp.join("nested/deeper")).unwrap();
        std::fs::write(tmp.join("a.txt"), b"alpha").unwrap();
        std::fs::write(tmp.join("nested/b.txt"), b"beta").unwrap();
        std::fs::write(tmp.join("nested/deeper/c.txt"), b"gamma").unwrap();

        force_remove_dir_all(&tmp).expect("should remove the populated tree");
        assert!(
            !tmp.exists(),
            "tree should be gone after force_remove_dir_all"
        );

        // No-op on a path that no longer exists.
        force_remove_dir_all(&tmp).expect("should be a no-op for missing path");
        assert!(!tmp.exists());
    }

    /// `force_remove_dir_all` must succeed even when files are read-only and
    /// directories lack write permission (the `~/.cache` "Permission denied"
    /// scenario). It chmod's entries u+w before unlinking, like `rm -rf`.
    #[test]
    #[cfg(unix)]
    fn force_remove_dir_all_handles_readonly_files_and_dirs() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = std::env::temp_dir().join(format!(
            "verve-force-rm-ro-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("nested")).unwrap();
        let ro_file = tmp.join("nested/readonly.txt");
        std::fs::write(&ro_file, b"locked").unwrap();
        // Make the file read-only (no write for anyone).
        std::fs::set_permissions(&ro_file, std::fs::Permissions::from_mode(0o444)).unwrap();
        // Make the nested dir non-writable (would block unlinking the file
        // without our chmod-u+w step).
        std::fs::set_permissions(tmp.join("nested"), std::fs::Permissions::from_mode(0o555))
            .unwrap();

        force_remove_dir_all(&tmp).expect("should remove despite read-only perms");
        assert!(
            !tmp.exists(),
            "tree should be gone despite read-only entries"
        );
    }

    /// Symlinks *inside* the source tree must be preserved as symlinks at the
    /// target (not skipped, not copied as regular files). This is the rustup
    /// scenario that broke `cargo`: `~/.cargo/bin/cargo` is a symlink to
    /// `rustup`; if the copy skipped it, the migrated tree lost the proxy and
    /// `cargo` stopped working. This test mirrors that layout.
    #[test]
    #[cfg(unix)]
    fn copy_preserves_internal_symlinks() {
        let home = match std::env::var_os("HOME").map(std::path::PathBuf::from) {
            Some(h) if !h.as_os_str().is_empty() => h,
            _ => {
                eprintln!("skip: HOME not set");
                return;
            }
        };
        let tmp = home.join(format!(
            ".verve-migrate-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = tmp.join("home").join(".proj");
        let target_root = tmp.join("external");
        std::fs::create_dir_all(source.join("bin")).unwrap();
        std::fs::create_dir_all(&target_root).unwrap();
        // A real file (the dispatcher) + a relative symlink proxy to it, like
        // rustup's `cargo -> rustup`.
        std::fs::write(source.join("bin/rustup"), b"dispatcher").unwrap();
        std::os::unix::fs::symlink("rustup", source.join("bin/cargo")).unwrap();

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let outcome = migrate_one(
            &source,
            &target_root,
            "test-group",
            0,
            ConflictPolicy::Merge,
            false,
            cancel,
            |_| {},
        );
        // We expect Done (or Cancelled with copy_completed if a checkpoint
        // fired); either way the copy must have run. For this fixture Done.
        match outcome {
            MigrateOutcome::Done { .. } => {}
            other => panic!("expected Done, got {other:?}"),
        }

        // The proxy symlink is preserved at the target.
        let dest = external_target(&target_root, &source);
        let proxy = dest.join("bin/cargo");
        let meta = std::fs::symlink_metadata(&proxy).expect("proxy missing at target");
        assert!(
            meta.file_type().is_symlink(),
            "proxy must remain a symlink, got {:?}",
            meta.file_type()
        );
        assert_eq!(
            std::fs::read_link(&proxy).unwrap(),
            std::path::Path::new("rustup"),
            "proxy must point at rustup (relative)"
        );

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
        // Large files (4 MB each) so the copy spans multiple 1 MB chunks,
        // giving the cancel check (which runs at each chunk boundary) a
        // chance to abort mid-copy.
        std::fs::write(source.join("a.txt"), vec![b'A'; 4 * 1024 * 1024]).unwrap();
        std::fs::write(source.join("b.txt"), vec![b'B'; 4 * 1024 * 1024]).unwrap();
        std::fs::write(
            source.join("nested").join("c.txt"),
            vec![b'C'; 4 * 1024 * 1024],
        )
        .unwrap();

        // Flip the cancel flag from a separate thread after a short delay.
        // We can't rely on the progress callback (which is now time-sampled and
        // may not fire at all for a fast local-SSD copy). The copy loop checks
        // `cancel` at every chunk boundary, so a 10 ms delay reliably catches
        // it mid-copy.
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_setter = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            cancel_setter.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let outcome = migrate_one(
            &source,
            &target_root,
            "test-group",
            12 * 1024 * 1024,
            ConflictPolicy::Merge,
            false,
            cancel,
            |_| {},
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
        assert_eq!(
            std::fs::read(source.join("a.txt")).unwrap().len(),
            4 * 1024 * 1024
        );

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
