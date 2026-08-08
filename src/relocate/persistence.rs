//! JSONL persistence for migration tasks (PRD §3.5, §11 — forward-compatible).
//!
//! One [`TaskRecord`] per line in `~/.verve/relocate/tasks.jsonl`. The store is
//! append-mostly; status updates rewrite the whole file (small, infrequent).
//! Unknown fields are ignored on load so older records / future fields coexist.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use super::types::{TaskRecord, TaskStatus};

/// `~/.verve/relocate/tasks.jsonl` (reuses the app's data dir for consistency
/// with the rest of Verve — see `state::persistence::data_dir`).
pub fn tasks_path() -> Option<PathBuf> {
    match crate::state::persistence::data_dir() {
        Ok(dir) => {
            let relocate_dir = dir.join("relocate");
            if let Err(e) = fs::create_dir_all(&relocate_dir) {
                log::warn!("relocate: cannot create dir {:?}: {e}", relocate_dir);
                return None;
            }
            Some(relocate_dir.join("tasks.jsonl"))
        }
        Err(e) => {
            log::warn!("relocate: data_dir unavailable: {e}");
            None
        }
    }
}

/// Load every persisted task record. Corrupt lines are skipped (logged) so a
/// single bad line never loses the rest.
pub fn load() -> Vec<TaskRecord> {
    let path = match tasks_path() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let contents = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for (i, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<TaskRecord>(line) {
            Ok(rec) => out.push(rec),
            Err(e) => {
                log::warn!("relocate: skipping corrupt task line {}: {e}", i + 1);
            }
        }
    }
    out
}

/// Append a single task record (used when a migration completes). Creates the
/// file on first write.
pub fn append(record: &TaskRecord) {
    let path = match tasks_path() {
        Some(p) => p,
        None => return,
    };
    let mut json = match serde_json::to_string(record) {
        Ok(s) => s,
        Err(e) => {
            log::error!("relocate: serialize task failed: {e}");
            return;
        }
    };
    json.push('\n');
    // Open for append (create if missing).
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            log::error!("relocate: open tasks file failed: {e}");
            return;
        }
    };
    if let Err(e) = file.write_all(json.as_bytes()) {
        log::error!("relocate: write tasks file failed: {e}");
    }
}

/// Update the status (and optional error message) of a task identified by id.
/// Rewrites the whole file. If the id isn't found, this is a no-op.
pub fn update_status(task_id: &str, status: TaskStatus, error: Option<&str>) {
    let mut records = load();
    let mut changed = false;
    for rec in records.iter_mut() {
        if rec.task_id == task_id {
            rec.status = status;
            rec.error = error.map(|s| s.to_string());
            changed = true;
        }
    }
    if changed {
        rewrite_all(&records);
    }
}

/// Rewrite the entire store from `records`. Used after bulk updates.
pub fn rewrite_all(records: &[TaskRecord]) {
    let path = match tasks_path() {
        Some(p) => p,
        None => return,
    };
    let mut file = match fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => {
            log::error!("relocate: open tasks file for rewrite failed: {e}");
            return;
        }
    };
    for rec in records {
        if let Ok(mut line) = serde_json::to_string(rec) {
            line.push('\n');
            let _ = file.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::persistence;

    fn isolated_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "verve-relocate-persist-{}-{}",
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
    fn append_load_roundtrip() {
        let _guard = persistence::set_thread_data_dir(isolated_dir());
        let rec = TaskRecord {
            task_id: "abc-123".into(),
            source_path: "/Users/x/.cargo".into(),
            target_external_path: "/Volumes/Ext/cargo".into(),
            symlink_path: "/Users/x/.cargo".into(),
            group_name: "开发工具缓存".into(),
            created_at: "2026-07-28T00:00:00Z".into(),
            status: TaskStatus::Migrated,
            error: None,
            size_bytes: 12345,
        };
        append(&rec);
        let loaded = load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].task_id, "abc-123");
        assert_eq!(loaded[0].status, TaskStatus::Migrated);
    }

    #[test]
    fn update_status_persists() {
        let _guard = persistence::set_thread_data_dir(isolated_dir());
        let rec = TaskRecord {
            task_id: "def".into(),
            source_path: "/x".into(),
            target_external_path: "/y".into(),
            symlink_path: "/x".into(),
            group_name: "g".into(),
            created_at: "t".into(),
            status: TaskStatus::Migrated,
            error: None,
            size_bytes: 0,
        };
        append(&rec);
        update_status("def", TaskStatus::RolledBack, None);
        let loaded = load();
        assert_eq!(loaded[0].status, TaskStatus::RolledBack);
    }

    #[test]
    fn corrupt_lines_skipped() {
        let _guard = persistence::set_thread_data_dir(isolated_dir());
        let path = tasks_path().unwrap();
        fs::write(&path, "not json\n{\"task_id\":\"ok\",\"source_path\":\"\",\"target_external_path\":\"\",\"symlink_path\":\"\",\"group_name\":\"\",\"created_at\":\"\",\"status\":\"Migrated\"}\n").unwrap();
        let loaded = load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].task_id, "ok");
    }
}
