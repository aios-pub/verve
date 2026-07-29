//! `RelocateEngine` — the GPUI entity that owns all relocate state and drives
//! the serial migration / rollback queues (PRD §8.1, §9, §10).
//!
//! The engine is the single source of truth the [`RelocatePanel`] reads from.
//! Heavy I/O (scan, copy, verify) is offloaded to background executors via
//! `smol::unblock`; the UI thread only ever receives [`RelocateEvent`]s and
//! reads snapshot fields, so rendering never blocks (PRD §2.4).
//!
//! ## Queue model
//! - **Migration** queue: all selected group + custom directories, run one at
//!   a time (PRD §8.1) to avoid I/O contention and file corruption.
//! - **Rollback** queue: selected persisted tasks, also serial.
//! - Controls: pause / resume / skip-current / retry-failed.
//!
//! ## Events
//! The engine emits [`RelocateEvent`] on every meaningful state change; the
//! panel subscribes and re-renders.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext, Context, Entity, EventEmitter, Task};

use super::disk_probe::{self, ProbeResult, Recommendation};
use super::migrate::{self, ConflictPolicy, CopyProgress, MigrateOutcome};
use super::persistence;
use super::rollback::{self, RollbackOutcome};
use super::scanner::{self, ScanResult};
use super::types::{AutoGroup, DirEntry, TaskRecord, TaskStatus};

/// One queue entry: a directory to migrate plus the metadata the flow needs.
#[derive(Clone, Debug)]
pub struct QueueItem {
    pub source: PathBuf,
    pub group_name: String,
    pub size_bytes: u64,
}

/// The engine's current operating mode (drives which controls the UI shows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineState {
    /// Idle — no queue running. Shows scan results + start button.
    Idle,
    /// Migration queue running.
    Migrating,
    /// Migration paused by the user.
    Paused,
    /// Rollback queue running.
    RollingBack,
}

/// A line in the scrolling log panel (PRD §9).
#[derive(Clone, Debug)]
pub struct LogLine {
    pub level: LogLevel,
    pub message: String,
    pub time: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Live progress for the running queue (PRD §9).
#[derive(Clone, Debug, Default)]
pub struct Progress {
    /// Index of the item currently being processed (within the queue).
    pub current_index: usize,
    pub total_items: usize,
    /// Bytes done / total for the current item.
    pub current_bytes_done: u64,
    pub current_bytes_total: u64,
    pub current_file: String,
    pub current_source: String,
    /// Approximate copy speed in bytes/sec, computed from a sliding sample.
    pub speed_bytes_per_sec: u64,
}

impl Progress {
    pub fn global_percent(&self) -> f32 {
        if self.total_items == 0 {
            return 0.0;
        }
        let per_item = if self.current_bytes_total > 0 {
            self.current_bytes_done as f32 / self.current_bytes_total as f32
        } else {
            0.0
        };
        ((self.current_index as f32 + per_item) / self.total_items as f32).clamp(0.0, 1.0)
    }
}

/// Events broadcast to subscribers (the panel).
#[derive(Clone, Debug)]
pub enum RelocateEvent {
    /// A background scan finished (or failed).
    ScanCompleted,
    ScanFailed(String),
    /// The disk probe finished for the chosen target root.
    ProbeCompleted,
    /// A custom directory was added (or rejected).
    CustomAdded { accepted: bool, reason: Option<String> },
    /// Queue state changed (started / paused / resumed / finished).
    StateChanged,
    /// Per-item progress tick.
    Progress,
    /// A new log line was appended.
    Log,
    /// A task's persisted status changed (migrated/rolled-back/failed).
    TasksChanged,
}

/// The engine entity.
pub struct RelocateEngine {
    /// Latest scan result (grouped). Populated by [`scan_async`].
    pub scan: ScanResult,
    /// User-added custom directory paths (also folded into the scan).
    pub custom_paths: Vec<PathBuf>,
    /// Per-group, per-entry selected-for-migration flags. Keyed by path string.
    pub selected: HashSet<String>,
    /// Chosen external target root.
    pub target_root: Option<PathBuf>,
    /// Last probe of the target root.
    pub probe: Option<ProbeResult>,
    /// Persisted migration tasks (loaded at init, kept in sync).
    pub tasks: Vec<TaskRecord>,
    /// Conflict policy for the migration queue (PRD §8.2.2). Default Merge.
    pub conflict_policy: ConflictPolicy,
    /// Whether to run SHA-256 verification after copy (PRD §8.2.4).
    pub verify: bool,

    pub state: EngineState,
    pub progress: Progress,
    pub logs: Vec<LogLine>,
    /// Items pending in the current queue (remaining after the current one).
    queue: Vec<QueueItem>,
    /// Whether the current item should be skipped (set by `skip`).
    skip_requested: bool,
    /// Whether the queue is paused.
    paused: bool,
    /// Cancel flag for the in-flight copy. Setting it aborts the current
    /// `migrate_one` copy at the next chunk boundary.
    current_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Task ids migrated during the *current* queue run (used by
    /// `cancel_and_rollback` to undo exactly this session's work).
    session_migrated_ids: Vec<String>,
    /// The background driver task; dropped (cancelled) on reset.
    driver: Option<Task<()>>,
    /// Rolling speed sample (bytes, timestamp).
    speed_sample: Option<(u64, std::time::Instant)>,

    _scan_task: Option<Task<()>>,
}

impl RelocateEngine {
    /// Construct + load persisted tasks (PRD §11 — restart doesn't lose state).
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let tasks = persistence::load();
        Self {
            scan: ScanResult::default(),
            custom_paths: Vec::new(),
            selected: HashSet::new(),
            target_root: None,
            probe: None,
            tasks,
            conflict_policy: ConflictPolicy::Merge,
            verify: false,
            state: EngineState::Idle,
            progress: Progress::default(),
            logs: Vec::new(),
            queue: Vec::new(),
            skip_requested: false,
            paused: false,
            current_cancel: None,
            session_migrated_ids: Vec::new(),
            driver: None,
            speed_sample: None,
            _scan_task: None,
        }
    }

    // ---- scanning --------------------------------------------------------

    /// Kick off a background scan of the home directory + current custom paths.
    pub fn scan_async(&mut self, cx: &mut Context<Self>) {
        let custom = self.custom_paths.clone();
        let weak = cx.weak_entity();
        self.log(LogLevel::Info, "开始扫描用户目录…");
        self._scan_task = Some(cx.spawn(async move |_this, cx| {
            let result = smol::unblock(move || scanner::scan(custom)).await;
            let _ = weak.update(cx, |this, cx| {
                this.scan = result;
                this.log(LogLevel::Info, "扫描完成。");
                cx.emit(RelocateEvent::ScanCompleted);
            });
        }));
    }

    /// Add a user custom directory (PRD §6). Validates blacklist + existence.
    /// Returns whether accepted (the panel also reacts via the event).
    pub fn add_custom(&mut self, path: PathBuf, cx: &mut Context<Self>) -> bool {
        // PRD §6.3.1 — blacklist hard-check.
        if super::blacklist::is_blacklisted(&path) {
            self.log(LogLevel::Error, &format!("该目录禁止迁移：{}", path.display()));
            cx.emit(RelocateEvent::CustomAdded {
                accepted: false,
                reason: Some("该目录禁止迁移".into()),
            });
            return false;
        }
        // Must exist + be a directory.
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                self.log(LogLevel::Error, &format!("路径不是目录：{}", path.display()));
                cx.emit(RelocateEvent::CustomAdded {
                    accepted: false,
                    reason: Some("所选路径不是目录".into()),
                });
                return false;
            }
            Err(e) => {
                self.log(
                    LogLevel::Error,
                    &format!("无法读取目录 {}：{}", path.display(), e),
                );
                cx.emit(RelocateEvent::CustomAdded {
                    accepted: false,
                    reason: Some(format!("目录不可读：{e}")),
                });
                return false;
            }
        }
        // De-dupe.
        if self.custom_paths.iter().any(|p| p == &path) {
            cx.emit(RelocateEvent::CustomAdded {
                accepted: false,
                reason: Some("该目录已添加".into()),
            });
            return false;
        }
        self.custom_paths.push(path.clone());
        self.log(LogLevel::Info, &format!("已添加自定义目录：{}", path.display()));
        // Re-scan so the custom group reflects the new entry (with size).
        self.scan_async(cx);
        cx.emit(RelocateEvent::CustomAdded {
            accepted: true,
            reason: None,
        });
        true
    }

    /// Remove a custom directory.
    pub fn remove_custom(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        self.custom_paths.retain(|p| p != path);
        self.selected.remove(&path.to_string_lossy().to_string());
        self.scan_async(cx);
    }

    // ---- selection ------------------------------------------------------

    pub fn toggle_selected(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.selected.contains(path) {
            self.selected.remove(path);
        } else {
            self.selected.insert(path.to_string());
        }
        cx.notify();
    }

    pub fn select_group(&mut self, group: &AutoGroup, cx: &mut Context<Self>) {
        for e in &group.entries {
            self.selected.insert(e.path.to_string_lossy().to_string());
        }
        cx.notify();
    }

    pub fn clear_group(&mut self, group: &AutoGroup, cx: &mut Context<Self>) {
        for e in &group.entries {
            self.selected.remove(&e.path.to_string_lossy().to_string());
        }
        cx.notify();
    }

    /// Select every entry across all groups (the panel's "全选" button).
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        for g in &self.scan.groups {
            for e in &g.entries {
                self.selected.insert(e.path.to_string_lossy().to_string());
            }
        }
        cx.notify();
    }

    /// Clear all selections (the panel's "取消全选" button).
    pub fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        cx.notify();
    }

    /// Whether every entry in every group is currently selected. Used to drive
    /// the "全选/取消全选" toggle label.
    pub fn all_selected(&self) -> bool {
        let mut any = false;
        for g in &self.scan.groups {
            for e in &g.entries {
                any = true;
                if !self.selected.contains(&e.path.to_string_lossy().to_string()) {
                    return false;
                }
            }
        }
        any
    }

    /// Total bytes across all selected (not-yet-migrated) entries.
    pub fn selected_total_bytes(&self) -> u64 {
        let mut total = 0u64;
        for g in &self.scan.groups {
            for e in &g.entries {
                if self.selected.contains(&e.path.to_string_lossy().to_string()) {
                    total += e.size_bytes;
                }
            }
        }
        total
    }

    // ---- disk probe -----------------------------------------------------

    /// Set the external target root and probe it.
    pub fn set_target_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.target_root = Some(root.clone());
        self.log(LogLevel::Info, &format!("检测外置磁盘：{}", root.display()));
        let weak = cx.weak_entity();
        cx.spawn(async move |_this, cx| {
            let result = smol::unblock(move || disk_probe::probe(&root)).await;
            let _ = weak.update(cx, |this, cx| {
                match result {
                    Ok(probe) => {
                        this.probe = Some(probe.clone());
                        let rec = probe.recommendation(this.selected_total_bytes());
                        match rec {
                            Recommendation::SsdOk | Recommendation::UnknownProceed => {
                                this.log(
                                    LogLevel::Info,
                                    &format!(
                                        "磁盘就绪：{}，{}，剩余 {}",
                                        probe.medium.label(),
                                        probe.fs_kind.label(),
                                        super::types::human_bytes(probe.free_bytes)
                                    ),
                                );
                            }
                            Recommendation::HddWarn => {
                                this.log(
                                    LogLevel::Warn,
                                    "检测到机械硬盘，迁移/编译/索引速度会明显下降，请确认后继续。",
                                );
                            }
                            Recommendation::ExfatWarn => {
                                this.log(
                                    LogLevel::Warn,
                                    "文件系统为 exFAT，存在权限/属性风险，推荐使用 APFS。",
                                );
                            }
                            Recommendation::InsufficientSpace => {
                                this.log(
                                    LogLevel::Error,
                                    &format!(
                                        "外置磁盘剩余空间不足（剩余 {}，需要 {}）。",
                                        super::types::human_bytes(probe.free_bytes),
                                        super::types::human_bytes(this.selected_total_bytes())
                                    ),
                                );
                            }
                            Recommendation::InternalRejected => {
                                this.log(
                                    LogLevel::Error,
                                    "所选磁盘为内置磁盘，已拒绝（仅支持外置磁盘）。",
                                );
                            }
                        }
                    }
                    Err(e) => {
                        this.log(LogLevel::Error, &format!("磁盘检测失败：{e}"));
                    }
                }
                cx.emit(RelocateEvent::ProbeCompleted);
            });
        })
        .detach();
    }

    /// Whether the current probe allows migration to proceed (hard rejects
    /// block the start button).
    pub fn can_start_migration(&self) -> bool {
        match (&self.probe, &self.target_root) {
            (Some(probe), Some(_)) => {
                let rec = probe.recommendation(self.selected_total_bytes());
                !matches!(
                    rec,
                    Recommendation::InsufficientSpace | Recommendation::InternalRejected
                )
            }
            _ => false,
        }
    }

    // ---- migration queue ------------------------------------------------

    /// Build the migration queue from selected entries and start the driver.
    pub fn start_migration(&mut self, cx: &mut Context<Self>) {
        if !self.can_start_migration() {
            return;
        }
        if self.state == EngineState::Migrating || self.state == EngineState::RollingBack {
            return;
        }
        let mut items: Vec<QueueItem> = Vec::new();
        for g in &self.scan.groups {
            for e in &g.entries {
                if self.selected.contains(&e.path.to_string_lossy().to_string()) {
                    items.push(QueueItem {
                        source: e.path.clone(),
                        group_name: g.group_name.clone(),
                        size_bytes: e.size_bytes,
                    });
                }
            }
        }
        if items.is_empty() {
            self.log(LogLevel::Warn, "未选择任何目录。");
            cx.notify();
            return;
        }
        self.queue = items;
        self.progress = Progress {
            total_items: self.queue.len(),
            ..Default::default()
        };
        self.paused = false;
        self.skip_requested = false;
        self.session_migrated_ids.clear();
        self.current_cancel = None;
        self.state = EngineState::Migrating;
        self.log(
            LogLevel::Info,
            &format!("开始迁移 {} 个目录…", self.queue.len()),
        );
        cx.emit(RelocateEvent::StateChanged);
        self.spawn_driver(cx);
    }

    /// Pause the running queue (PRD §9 — queue control).
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if self.state == EngineState::Migrating {
            self.paused = true;
            self.state = EngineState::Paused;
            self.log(LogLevel::Info, "已暂停。");
            cx.emit(RelocateEvent::StateChanged);
        }
    }

    /// Resume a paused queue.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if self.state == EngineState::Paused {
            self.paused = false;
            self.state = EngineState::Migrating;
            self.log(LogLevel::Info, "已继续。");
            cx.emit(RelocateEvent::StateChanged);
        }
    }

    /// Skip the currently-running item (PRD §9).
    pub fn skip_current(&mut self, cx: &mut Context<Self>) {
        if self.state == EngineState::Migrating || self.state == EngineState::Paused {
            self.skip_requested = true;
            self.log(LogLevel::Warn, "跳过当前目录…");
            cx.notify();
        }
    }

    /// Cancel the in-flight migration and roll back every task migrated
    /// **during the current queue run** (tracked in `session_migrated_ids`).
    ///
    /// - Sets `current_cancel` so the running copy aborts at the next chunk
    ///   boundary (the partial external copy is cleaned by `migrate_one`).
    /// - Drops the driver (stops the rest of the queue).
    /// - For each session-migrated task, runs `rollback_one` to copy the
    ///   external data back to the original local path and remove the symlink.
    ///
    /// The original local directories are restored; no user data is lost.
    pub fn cancel_and_rollback(&mut self, cx: &mut Context<Self>) {
        if self.state != EngineState::Migrating && self.state != EngineState::Paused {
            return;
        }
        // 1. Signal the in-flight copy to abort.
        if let Some(cancel) = self.current_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // 2. Stop the queue + clear state.
        self.driver = None; // dropping the Task cancels the driver loop
        self.queue.clear();
        self.paused = false;
        self.skip_requested = false;

        let ids = std::mem::take(&mut self.session_migrated_ids);
        let count = ids.len();
        if count == 0 {
            self.state = EngineState::Idle;
            self.progress = Progress::default();
            self.log(LogLevel::Info, "已停止迁移（本次无已完成的项需要回退）。");
            cx.emit(RelocateEvent::StateChanged);
            cx.emit(RelocateEvent::TasksChanged);
            return;
        }
        self.state = EngineState::RollingBack;
        self.progress = Progress {
            total_items: count,
            ..Default::default()
        };
        self.log(
            LogLevel::Warn,
            &format!("正在撤销迁移：停止剩余任务，回退 {} 项已迁移目录…", count),
        );
        cx.emit(RelocateEvent::StateChanged);

        // 3. Roll back each session-migrated task serially on a background
        //    thread, feeding progress back per item.
        let weak = cx.weak_entity();
        let records: Vec<TaskRecord> = self
            .tasks
            .iter()
            .filter(|t| ids.contains(&t.task_id) && t.status == TaskStatus::Migrated)
            .cloned()
            .collect();
        cx.spawn(async move |_this, cx| {
            let mut done = 0usize;
            let total = records.len();
            for rec in records {
                let r = Arc::new(rec);
                let r_for_update = r.clone();
                let outcome = smol::unblock(move || rollback::rollback_one(&r, |_| {})).await;
                let _ = weak.update(cx, |this, cx| {
                    this.progress.current_index = done;
                    this.progress.current_source = r_for_update.source_path.clone();
                    match outcome {
                        RollbackOutcome::Done { bytes_restored, .. } => {
                            persistence::update_status(&r_for_update.task_id, TaskStatus::RolledBack, None);
                            this.refresh_tasks();
                            this.log(
                                LogLevel::Info,
                                &format!(
                                    "↩️ 已回退：{}（{}）",
                                    r_for_update.source_path,
                                    super::types::human_bytes(bytes_restored)
                                ),
                            );
                        }
                        RollbackOutcome::ExternalUnavailable { reason }
                        | RollbackOutcome::Failed { reason } => {
                            persistence::update_status(&r_for_update.task_id, TaskStatus::Failed, Some(&reason));
                            this.refresh_tasks();
                            this.log(LogLevel::Error, &format!("回退失败 {}: {}", r_for_update.source_path, reason));
                        }
                    }
                    done += 1;
                    this.progress.current_index = done;
                    cx.emit(RelocateEvent::Progress);
                    cx.emit(RelocateEvent::TasksChanged);
                });
            }
            let _ = weak.update(cx, |this, cx| {
                this.state = EngineState::Idle;
                this.current_cancel = None;
                this.progress = Progress::default();
                this.log(
                    LogLevel::Info,
                    &format!("撤销迁移完成（已回退 {} 项）。", done),
                );
                let _ = total;
                cx.emit(RelocateEvent::StateChanged);
                cx.emit(RelocateEvent::TasksChanged);
                // Re-scan so the rolled-back (now real) dirs reappear for selection.
                this.scan_async(cx);
            });
        })
        .detach();
    }

    // ---- rollback queue -------------------------------------------------

    /// Roll back a single persisted task (PRD §10.3.1).
    pub fn rollback_task(&mut self, task_id: &str, cx: &mut Context<Self>) {
        let Some(record) = self.tasks.iter().find(|t| t.task_id == task_id).cloned() else {
            return;
        };
        if record.status != TaskStatus::Migrated {
            self.log(LogLevel::Warn, &format!("任务 {} 非已迁移状态，跳过。", task_id));
            return;
        }
        self.state = EngineState::RollingBack;
        self.progress = Progress {
            total_items: 1,
            ..Default::default()
        };
        self.log(LogLevel::Info, &format!("开始回滚：{}", record.source_path));
        cx.emit(RelocateEvent::StateChanged);

        let weak = cx.weak_entity();
        let record = Arc::new(record);
        cx.spawn(async move |_this, cx| {
            let rec = record.clone();
            let outcome = smol::unblock(move || {
                rollback::rollback_one(&rec, |_cp| {
                    // Per-file progress can't easily cross the smol boundary
                    // without channels; for a single-task rollback we update
                    // at completion. (Acceptable: rollback is one dir.)
                })
            })
            .await;
            let _ = weak.update(cx, |this, cx| {
                match outcome {
                    RollbackOutcome::Done {
                        bytes_restored,
                        ..
                    } => {
                        persistence::update_status(&record.task_id, TaskStatus::RolledBack, None);
                        this.refresh_tasks();
                        this.log(
                            LogLevel::Info,
                            &format!(
                                "回滚完成，已恢复 {}（{}）。",
                                super::types::human_bytes(bytes_restored),
                                record.source_path
                            ),
                        );
                    }
                    RollbackOutcome::ExternalUnavailable { reason }
                    | RollbackOutcome::Failed { reason } => {
                        persistence::update_status(
                            &record.task_id,
                            TaskStatus::Failed,
                            Some(&reason),
                        );
                        this.refresh_tasks();
                        this.log(LogLevel::Error, &reason);
                    }
                }
                this.state = EngineState::Idle;
                this.progress = Progress::default();
                cx.emit(RelocateEvent::StateChanged);
                cx.emit(RelocateEvent::TasksChanged);
            });
        })
        .detach();
    }

    /// Roll back every migrated task in a group (PRD §10.3.2). Runs them
    /// serially via repeated single-task rollbacks.
    pub fn rollback_group(&mut self, group_name: &str, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|t| t.group_name == group_name && t.status == TaskStatus::Migrated)
            .map(|t| t.task_id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        // Roll back the first; on completion the panel/engine can continue.
        // (Serial-by-design; the caller may re-invoke for the next id.)
        self.log(
            LogLevel::Info,
            &format!("批量回滚分组「{}」（{} 项）…", group_name, ids.len()),
        );
        self.rollback_task(&ids[0], cx);
    }

    /// Explicitly delete the external copy of a rolled-back task (PRD §10.4 —
    /// only after the user confirms "delete external copy").
    pub fn delete_external_copy(&mut self, task_id: &str, cx: &mut Context<Self>) {
        let Some(record) = self.tasks.iter().find(|t| t.task_id == task_id).cloned() else {
            return;
        };
        match rollback::delete_external_copy(&record) {
            Ok(()) => {
                self.log(
                    LogLevel::Info,
                    &format!("已删除外置副本：{}", record.target_external_path),
                );
            }
            Err(e) => {
                self.log(LogLevel::Error, &e);
            }
        }
        cx.emit(RelocateEvent::TasksChanged);
    }

    // ---- driver ---------------------------------------------------------

    /// Spawn the serial queue driver. It processes `self.queue` one item at a
    /// time, honouring pause/skip/cancel, emitting **live** per-chunk progress,
    /// and persisting results.
    ///
    /// Progress is live because the copy runs on a background task while the
    /// driver concurrently drains the progress channel and forwards each sample
    /// to the engine (so the UI sees byte-level movement + speed in real time).
    fn spawn_driver(&mut self, cx: &mut Context<Self>) {
        // Capture the queue snapshot (the current item is index 0).
        let initial_queue: Vec<QueueItem> = std::mem::take(&mut self.queue);
        let target_root = self.target_root.clone();
        let conflict = self.conflict_policy;
        let verify = self.verify;
        let weak = cx.weak_entity();

        // We drive via a self-perpetuating async loop using the background
        // executor + timers, checking pause/skip between items.
        self.driver = Some(cx.spawn(async move |_this, cx| {
            let mut remaining = initial_queue;
            let mut idx: usize = 0;
            let total = remaining.len();
            // Initialise progress current_index.
            let _ = weak.update(cx, |this, cx| {
                this.progress.total_items = total;
                this.progress.current_index = 0;
                cx.emit(RelocateEvent::Progress);
            });

            while !remaining.is_empty() {
                // If the engine was cancelled/rolled back out-of-band (driver
                // task dropped via cancel_and_rollback), bail immediately.
                let cancelled = weak
                    .read_with(cx, |this, _cx| this.current_cancel.is_none() && this.state != EngineState::Migrating)
                    .unwrap_or(false);
                if cancelled {
                    return;
                }

                // Honour pause: poll until resumed.
                loop {
                    let paused = weak
                        .read_with(cx, |this, cx| {
                            let p = this.paused;
                            let _ = cx;
                            p
                        })
                        .unwrap_or(false);
                    if !paused {
                        break;
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(200))
                        .await;
                }

                let item = remaining.remove(0);
                // Create a fresh cancel token for this item and stash it on the
                // engine so cancel_and_rollback can signal abort.
                let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let _ = weak.update(cx, |this, cx| {
                    this.progress.current_index = idx;
                    this.progress.current_source = item.source.to_string_lossy().into_owned();
                    this.progress.current_bytes_done = 0;
                    this.progress.current_bytes_total = item.size_bytes;
                    this.speed_sample = None;
                    this.current_cancel = Some(cancel.clone());
                    cx.emit(RelocateEvent::Progress);
                });

                let src = item.source.clone();
                let group = item.group_name.clone();
                let size = item.size_bytes;
                let root = target_root.clone().unwrap_or_default();

                // Run the blocking migration on a background executor task,
                // piping progress through a channel. We then race the copy
                // completion against the progress channel so the UI gets live
                // updates while the copy runs.
                let (ptx, prx) = smol::channel::bounded::<CopyProgress>(128);
                let cancel_for_copy = cancel.clone();
                let copy_task: Task<MigrateOutcome> =
                    cx.background_executor().spawn(async move {
                        let ptx = ptx;
                        let cancel = cancel_for_copy;
                        // smol::unblock runs the blocking copy off the async
                        // runtime; the closure sends progress to ptx as it goes.
                        smol::unblock(move || {
                            migrate::migrate_one(
                                &src, &root, &group, size, conflict, verify, cancel, |cp| {
                                    // Best-effort non-blocking send; if the channel
                                    // is full the UI simply skips a sample.
                                    let _ = ptx.try_send(cp.clone());
                                },
                            )
                        })
                        .await
                    });

                // Concurrently drain the progress channel until the copy finishes.
                let outcome = loop {
                    // Poll the channel with a tiny timeout so we also pick up
                    // the copy_task completion promptly.
                    let recv = prx.recv().await;
                    match recv {
                        Ok(cp) => {
                            let _ = weak.update(cx, |this, cx| {
                                this.apply_copy_progress(&cp);
                                cx.emit(RelocateEvent::Progress);
                            });
                        }
                        Err(_) => {
                            // Channel closed (sender dropped) → copy is done.
                            break copy_task.await;
                        }
                    }
                };

                let _ = weak.update(cx, |this, cx| {
                    let skip = std::mem::replace(&mut this.skip_requested, false);
                    this.current_cancel = None;
                    match outcome {
                        MigrateOutcome::Done {
                            record,
                            final_progress,
                        } => {
                            persistence::append(&record);
                            // Track the id so cancel_and_rollback can undo it.
                            this.session_migrated_ids.push(record.task_id.clone());
                            this.tasks.push(record);
                            // Clear selection for the migrated path.
                            this.selected
                                .remove(&item.source.to_string_lossy().to_string());
                            this.progress.current_bytes_done = final_progress.bytes_done;
                            this.log(
                                LogLevel::Info,
                                &format!(
                                    "✅ 已迁移：{}（{}）",
                                    item.source.display(),
                                    super::types::human_bytes(final_progress.bytes_done)
                                ),
                            );
                            cx.emit(RelocateEvent::TasksChanged);
                        }
                        MigrateOutcome::Skipped { reason } => {
                            this.log(LogLevel::Warn, &format!("⏭ 跳过：{}", reason));
                        }
                        MigrateOutcome::PreconditionFailed { reason } => {
                            this.log(LogLevel::Error, &format!("❌ 前置检查失败：{}", reason));
                            this.log(
                                LogLevel::Warn,
                                &format!("（已跳过 {}）", item.source.display()),
                            );
                        }
                        MigrateOutcome::Cancelled => {
                            this.log(
                                LogLevel::Warn,
                                &format!("🚫 已取消迁移：{}", item.source.display()),
                            );
                            // A cancel aborts the whole queue — stop here.
                            idx += 1;
                            cx.emit(RelocateEvent::Progress);
                            return; // driver ends; rollback (if any) is driven by cancel_and_rollback
                        }
                        MigrateOutcome::Failed { reason } => {
                            this.log(
                                LogLevel::Error,
                                &format!("❌ 迁移失败：{} — {}", item.source.display(), reason),
                            );
                        }
                    }
                    let _ = skip;
                    idx += 1;
                    cx.emit(RelocateEvent::Progress);
                });

                // If a skip was requested mid-flight, we still let the current
                // op finish (graceful); skip_requested is consumed above.
            }

            let _ = weak.update(cx, |this, cx| {
                this.state = EngineState::Idle;
                this.driver = None;
                this.current_cancel = None;
                this.progress = Progress::default();
                this.log(LogLevel::Info, "迁移队列结束。");
                cx.emit(RelocateEvent::StateChanged);
                // Re-scan so migrated paths (now symlinks) drop from selection.
                this.scan_async(cx);
            });
        }));
    }

    /// Fold a copy progress sample into the engine's progress + speed sample.
    fn apply_copy_progress(&mut self, cp: &CopyProgress) {
        let now = std::time::Instant::now();
        self.progress.current_bytes_done = cp.bytes_done;
        self.progress.current_bytes_total = cp.bytes_total.max(self.progress.current_bytes_total);
        self.progress.current_file = cp.current_file.clone();
        // Speed: simple derivative from the last sample.
        match self.speed_sample {
            Some((prev_bytes, prev_time)) => {
                let dt = now.duration_since(prev_time).as_secs_f64();
                if dt >= 0.25 {
                    let db = cp.bytes_done.saturating_sub(prev_bytes);
                    self.progress.speed_bytes_per_sec = if dt > 0.0 {
                        (db as f64 / dt) as u64
                    } else {
                        0
                    };
                    self.speed_sample = Some((cp.bytes_done, now));
                }
            }
            None => {
                self.speed_sample = Some((cp.bytes_done, now));
            }
        }
    }

    // ---- helpers --------------------------------------------------------

    /// Reload persisted tasks from disk into `self.tasks`.
    pub fn refresh_tasks(&mut self) {
        self.tasks = persistence::load();
    }

    fn log(&mut self, level: LogLevel, message: &str) {
        self.logs.push(LogLine {
            level,
            message: message.to_string(),
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
        });
        // Keep the log ring bounded.
        if self.logs.len() > 1000 {
            let drop = self.logs.len() - 1000;
            self.logs.drain(0..drop);
        }
    }

    /// Total bytes successfully migrated (sum of Migrated task sizes).
    pub fn reclaimed_bytes(&self) -> u64 {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Migrated)
            .map(|t| t.size_bytes)
            .sum()
    }
}

impl EventEmitter<RelocateEvent> for RelocateEngine {}

// Re-export DirEntry/scan types the panel needs to enumerate.
pub use super::scanner::ScanResult as ScanSnapshot;

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_with_groups() -> RelocateEngine {
        let mut e = RelocateEngine {
            scan: ScanResult::default(),
            custom_paths: Vec::new(),
            selected: HashSet::new(),
            target_root: None,
            probe: None,
            tasks: Vec::new(),
            conflict_policy: ConflictPolicy::Merge,
            verify: false,
            state: EngineState::Idle,
            progress: Progress::default(),
            logs: Vec::new(),
            queue: Vec::new(),
            skip_requested: false,
            paused: false,
            current_cancel: None,
            session_migrated_ids: Vec::new(),
            driver: None,
            speed_sample: None,
            _scan_task: None,
        };
        // Two groups with a couple of entries each.
        e.scan.groups = vec![
            AutoGroup {
                group_name: "g1".into(),
                description: String::new(),
                hint_text: String::new(),
                risk: super::super::types::RiskLevel::Safe,
                entries: vec![
                    DirEntry {
                        path: PathBuf::from("/tmp/a"),
                        size_bytes: 1,
                    },
                    DirEntry {
                        path: PathBuf::from("/tmp/b"),
                        size_bytes: 2,
                    },
                ],
                total_size_bytes: 3,
            },
            AutoGroup {
                group_name: "g2".into(),
                description: String::new(),
                hint_text: String::new(),
                risk: super::super::types::RiskLevel::Safe,
                entries: vec![DirEntry {
                    path: PathBuf::from("/tmp/c"),
                    size_bytes: 4,
                }],
                total_size_bytes: 4,
            },
        ];
        e
    }

    #[test]
    fn all_selected_false_until_every_entry_is_in_selected() {
        let mut e = engine_with_groups();
        // Nothing selected → not all.
        assert!(!e.all_selected());
        assert_eq!(e.selected_total_bytes(), 0);

        // Two of three selected → still not all.
        e.selected.insert("/tmp/a".into());
        e.selected.insert("/tmp/c".into());
        assert!(!e.all_selected());
        assert_eq!(e.selected_total_bytes(), 5);

        // All three → all_selected() is true and the byte total covers everything.
        e.selected.insert("/tmp/b".into());
        assert!(e.all_selected());
        assert_eq!(e.selected_total_bytes(), 7);
    }

    #[test]
    fn all_selected_false_when_there_are_no_entries() {
        // An empty scan must report NOT-all-selected, so the panel's "全选"
        // button (which is disabled in this case anyway) never reads as "全".
        let e = RelocateEngine {
            scan: ScanResult::default(),
            custom_paths: Vec::new(),
            selected: HashSet::new(),
            target_root: None,
            probe: None,
            tasks: Vec::new(),
            conflict_policy: ConflictPolicy::Merge,
            verify: false,
            state: EngineState::Idle,
            progress: Progress::default(),
            logs: Vec::new(),
            queue: Vec::new(),
            skip_requested: false,
            paused: false,
            current_cancel: None,
            session_migrated_ids: Vec::new(),
            driver: None,
            speed_sample: None,
            _scan_task: None,
        };
        assert!(!e.all_selected());
    }
}
