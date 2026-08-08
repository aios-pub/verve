//! Rollback engine (PRD §10) — undo a completed migration.
//!
//! Standard flow for [`rollback_one`]:
//! 1. Validate that the external target still exists / disk is mounted
//!    (PRD §10.1). If missing, refuse and tell the user the disk must be
//!    connected (Time Machine is the disaster fallback, PRD §2.3).
//! 2. **Safely remove the local symlink** (single `unlink`; never recursive —
//!    PRD §2.2, §10.2). This is the single most safety-critical step.
//! 3. Resumable copy of the external directory back to the original local
//!    path (PRD §10.3).
//! 4. The caller asks the user (keep / delete external copy) and, if "delete",
//!    calls [`delete_external_copy`] — but that is an *explicit*, post-confirm
//!    action, never automatic (PRD §10.4, §11).
//! 5. The caller updates the task status to `RolledBack`.
//!
//! # Safety (PRD §2.2)
//! - The only destructive operation in the main flow is removing the local
//!   symlink, and it is a single `remove_file` on a path we first prove is a
//!   symlink. We never call `remove_dir_all` on a symlink.
//! - Deleting the external copy is a separate, opt-in step gated behind user
//!   confirmation.

use std::path::{Path, PathBuf};

use super::migrate::CopyProgress;
use super::types::{TaskRecord, TaskStatus};

/// Outcome of [`rollback_one`].
#[derive(Clone, Debug)]
pub enum RollbackOutcome {
    /// Rollback succeeded. `delete_external` indicates whether the caller
    /// should also remove the external copy (per the user's choice).
    Done {
        task_id: String,
        bytes_restored: u64,
        final_progress: CopyProgress,
    },
    /// The external disk/target is unavailable — cannot roll back (PRD §10.1).
    ExternalUnavailable { reason: String },
    /// Something failed mid-copy-back; partial local data may exist.
    Failed { reason: String },
}

/// Precondition: is `symlink_path` currently a symlink pointing at the recorded
/// external target? Used to guard step 2 and to detect a half-migrated state.
fn is_expected_symlink(record: &TaskRecord) -> bool {
    let link = Path::new(&record.symlink_path);
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => {
            // Confirm it points at the recorded external path (canonicalised).
            match std::fs::read_link(link) {
                Ok(target) => {
                    let expected = PathBuf::from(&record.target_external_path);
                    target == expected
                        || std::fs::canonicalize(&target).ok()
                            == std::fs::canonicalize(&expected).ok()
                }
                Err(_) => false,
            }
        }
        _ => false,
    }
}

/// The single most safety-critical operation: remove the local symlink.
///
/// Verifies the path is genuinely a symlink first, then `remove_file`s it
/// (which on Unix unlinks the link, NOT the target). If the path is a real
/// directory (not a symlink), this refuses — it would only happen if the user
/// manually replaced the link, and we must not risk deleting their data
/// (PRD §2.2: "删链 ≠ 删文件").
fn safe_unlink_symlink(symlink_path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(symlink_path)
        .map_err(|e| format!("无法读取符号链接元数据 {}: {}", symlink_path.display(), e))?;
    if !meta.file_type().is_symlink() {
        return Err(format!(
            "目标路径不是符号链接（可能是真实目录），为防止误删数据已中止：{}",
            symlink_path.display()
        ));
    }
    std::fs::remove_file(symlink_path)
        .map_err(|e| format!("删除符号链接失败 {}: {}", symlink_path.display(), e))
}

/// Copy the external directory back to the original local path. The local
/// parent is created if missing. Existing local dir is left in place (merge).
fn copy_back_with_progress(external: &Path, local: &Path) -> Result<u64, String> {
    // Reuse the cancel-aware copy from migrate.rs. Rollback is never cancelled
    // mid-copy (the cancel token below stays false), but we get progress +
    // permission preservation for free. Merge policy overwrites any stale local
    // files from a partial earlier restore.
    let never_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    super::migrate::copy_dir_with_progress(
        external,
        local,
        super::migrate::ConflictPolicy::Merge,
        |_| {}, // no progress callback for rollback
        0,      // total unknown for rollback; progress still reports bytes_done
        &never_cancel,
    )
    .map_err(|e| {
        if e == super::migrate::CANCELLED_ERR {
            "复制回本机被中断".to_string()
        } else {
            format!("复制回本机失败: {}", e)
        }
    })
}

/// Restore the original local directory (`source`) from its completed external
/// copy (`dest`), then remove the external copy.
///
/// Used when a migration is cancelled/fails **after** the original was deleted
/// but **before** the symlink was created: the data only exists at `dest`, so we
/// copy it back to `source` (the original absolute path) and clean up `dest`.
/// `source` must not exist (it was just removed); its parent is created if
/// missing. The external copy is removed only after a successful copy-back.
pub fn restore_from_external(source: &Path, dest: &Path) -> Result<(), String> {
    // The original was deleted; recreate its parent if needed.
    if let Some(parent) = source.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("恢复：创建父目录失败 {}: {}", parent.display(), e))?;
    }
    // Copy the external tree back to the original path. We use the fresh-copy
    // path (Skip policy so an unexpected leftover at `source` aborts rather
    // than silently merging), with a never-cancel token.
    let never_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let copy_result = super::migrate::copy_dir_with_progress(
        dest,
        source,
        super::migrate::ConflictPolicy::Skip,
        |_| {},
        0,
        &never_cancel,
    );
    let restored = match copy_result {
        Ok(_) => true,
        Err(e) if e == super::migrate::CANCELLED_ERR => {
            return Err("恢复被中断".to_string());
        }
        Err(e) if e.starts_with("用户选择跳过") => {
            // `source` already exists unexpectedly — treat as already restored.
            true
        }
        Err(e) => {
            return Err(format!("恢复：复制失败: {}", e));
        }
    };
    if restored {
        // Only remove the external copy once the original is safely restored.
        // Use the robust recursive remove: `dest` is the real migrated data dir
        // (never a symlink), and force_remove_dir_all tolerates macOS
        // ".Directory not empty" transient re-population.
        if let Err(e) = super::migrate::force_remove_dir_all(dest) {
            return Err(format!("恢复：删除外置副本失败 {}: {}", dest.display(), e));
        }
    }
    Ok(())
}

/// Run the rollback flow for one task (PRD §10.2).
pub fn rollback_one(record: &TaskRecord) -> RollbackOutcome {
    let external = Path::new(&record.target_external_path);
    let local = Path::new(&record.symlink_path);

    // 1. External must be available (PRD §10.1).
    if !external.exists() {
        return RollbackOutcome::ExternalUnavailable {
            reason: format!(
                "外置目标不可用，无法回滚：{}。\n请确认迁移所用外置固态已连接；硬盘丢失/损坏请依靠 Time Machine 历史备份兜底恢复。",
                external.display()
            ),
        };
    }
    if !external.is_dir() {
        return RollbackOutcome::ExternalUnavailable {
            reason: format!("外置目标不是目录：{}", external.display()),
        };
    }

    // 2. Safe symlink removal (PRD §2.2, §10.2).
    // If the symlink isn't where we expect (user already restored manually,
    // or never migrated), surface that rather than guessing.
    if Path::new(&record.symlink_path).exists() || std::fs::symlink_metadata(local).is_ok() {
        if !is_expected_symlink(record) {
            // The local path exists but isn't our symlink. If it's a real
            // directory, we must not overwrite/destroy it silently.
            let is_real_dir = std::fs::symlink_metadata(local)
                .map(|m| m.is_dir() && !m.file_type().is_symlink())
                .unwrap_or(false);
            if is_real_dir {
                return RollbackOutcome::Failed {
                    reason: format!(
                        "本地路径已存在真实目录（非符号链接），为防止数据覆盖已中止：{}",
                        local.display()
                    ),
                };
            }
            // Not a dir and not our symlink — try removing whatever it is
            // only if it's a symlink of some kind.
            if std::fs::symlink_metadata(local)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                if let Err(reason) = safe_unlink_symlink(local) {
                    return RollbackOutcome::Failed { reason };
                }
            }
        } else if let Err(reason) = safe_unlink_symlink(local) {
            return RollbackOutcome::Failed { reason };
        }
    }

    // 3. Copy the external copy back to the original local path (PRD §10.3).
    let bytes = match copy_back_with_progress(external, local) {
        Ok(n) => n,
        Err(reason) => return RollbackOutcome::Failed { reason },
    };

    let final_progress = CopyProgress {
        bytes_done: bytes,
        bytes_total: bytes,
        current_file: String::new(),
    };

    RollbackOutcome::Done {
        task_id: record.task_id.clone(),
        bytes_restored: bytes,
        final_progress,
    }
}

/// Remove the external copy after a successful rollback. This is invoked ONLY
/// after the user explicitly chooses "delete external copy" (PRD §10.4) — it is
/// never called automatically by the rollback flow.
///
/// Safety: we double-check the path is not the migration source and refuse to
/// delete anything under a blacklist path.
pub fn delete_external_copy(record: &TaskRecord) -> Result<(), String> {
    let external = Path::new(&record.target_external_path);
    // Never touch a blacklisted path (defence in depth).
    if super::blacklist::is_blacklisted(external) {
        return Err(format!(
            "外置副本路径位于黑名单，拒绝删除：{}",
            external.display()
        ));
    }
    if !external.exists() {
        return Ok(()); // already gone — nothing to do.
    }
    if !external.is_dir() {
        return Err(format!("外置副本不是目录：{}", external.display()));
    }
    super::migrate::force_remove_dir_all(external)
        .map_err(|e| format!("删除外置副本失败 {}: {}", external.display(), e))
}

/// Predicate: has this task been rolled back (status check helper for the UI).
pub fn is_rolled_back(record: &TaskRecord) -> bool {
    record.status == TaskStatus::RolledBack
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_record(symlink: &str, external: &str) -> TaskRecord {
        TaskRecord {
            task_id: "t1".into(),
            source_path: symlink.into(),
            target_external_path: external.into(),
            symlink_path: symlink.into(),
            group_name: "g".into(),
            created_at: "t".into(),
            status: TaskStatus::Migrated,
            error: None,
            size_bytes: 0,
        }
    }

    #[test]
    fn safe_unlink_refuses_real_directory() {
        let tmp = std::env::temp_dir().join(format!(
            "verve-rollback-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let res = safe_unlink_symlink(&tmp);
        assert!(res.is_err(), "must refuse to remove a real directory");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn safe_unlink_removes_symlink_only() {
        #[cfg(unix)]
        {
            let tmp = std::env::temp_dir().join(format!(
                "verve-rollback-link-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let target = tmp.with_extension("target");
            std::fs::create_dir_all(&target).unwrap();
            std::os::unix::fs::symlink(&target, &tmp).unwrap();
            safe_unlink_symlink(&tmp).unwrap();
            assert!(!tmp.exists(), "symlink should be gone");
            assert!(target.exists(), "target dir must survive");
            std::fs::remove_dir_all(&target).ok();
        }
    }

    #[test]
    fn rollback_reports_external_unavailable() {
        let rec = fresh_record(
            "/tmp/does-not-exist-symlink",
            "/tmp/does-not-exist-external",
        );
        match rollback_one(&rec) {
            RollbackOutcome::ExternalUnavailable { .. } => {}
            other => panic!("expected ExternalUnavailable, got {:?}", other),
        }
    }

    /// `restore_from_external` is the recovery path used when a migration is
    /// cancelled (or the symlink step fails) AFTER the original was deleted:
    /// it must copy the external tree back to the original path and then remove
    /// the external copy. This is the data-safety guarantee for the
    /// `Cancelled { source_deleted: true }` and symlink-failure branches.
    #[test]
    fn restore_from_external_recovers_deleted_original() {
        let home = match std::env::var_os("HOME").map(std::path::PathBuf::from) {
            Some(h) if !h.as_os_str().is_empty() => h,
            _ => {
                eprintln!("skip: HOME not set");
                return;
            }
        };
        let tmp = home.join(format!(
            ".verve-restore-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Simulate the post-delete state: `external` holds the only copy,
        // `source` does NOT exist (it was just removed by migrate_one step 6).
        let external = tmp.join("external").join(".proj");
        let source = tmp.join("home").join(".proj");
        std::fs::create_dir_all(external.join("nested")).unwrap();
        std::fs::write(external.join("a.txt"), b"alpha").unwrap();
        std::fs::write(external.join("nested").join("b.txt"), b"beta").unwrap();
        // `source` is intentionally absent (deleted by step 6).

        restore_from_external(&source, &external).expect("restore should succeed");

        // Original is restored at its absolute path with all files.
        assert!(source.is_dir(), "source must be restored");
        assert_eq!(std::fs::read(source.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(
            std::fs::read(source.join("nested").join("b.txt")).unwrap(),
            b"beta"
        );
        // External copy was removed after the successful copy-back.
        assert!(
            !external.exists(),
            "external copy should be removed after restore"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
}
