//! Core data structures for the relocate module (PRD §3, fixed).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Risk classification for a migration group (PRD §3.1).
///
/// `Safe` directories can be moved to any SSD without impact. `PerformanceWarn`
/// directories (code projects) should only move to Thunderbolt/USB4 SSDs; an
/// HDD would noticeably slow indexing/compile. `Forbidden` is reserved for
/// anything that must never be migrated (purely defensive — blacklisted paths
/// are filtered out long before this is consulted).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Safe,
    PerformanceWarn,
    Forbidden,
}

impl RiskLevel {
    /// One-line human label for UI badges (zh-CN, matches PRD hint copy).
    pub fn label(self) -> &'static str {
        match self {
            RiskLevel::Safe => "🟢 安全",
            RiskLevel::PerformanceWarn => "⚠️ 性能敏感",
            RiskLevel::Forbidden => "🔴 禁止",
        }
    }
}

/// A single scanned directory with its on-disk size (PRD §3.4).
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path: PathBuf,
    pub size_bytes: u64,
}

impl DirEntry {
    /// Pretty-print the size as a human-friendly string (e.g. "12.3 GB").
    pub fn size_human(&self) -> String {
        human_bytes(self.size_bytes)
    }
}

/// An auto-clustering rule definition (PRD §3.2).
///
/// `path_patterns` are tilde-prefixed globs (e.g. `~/Library/Caches/*`). A
/// directory is matched by expansion + prefix/equality against the rule's
/// patterns. `priority` breaks ties — a directory joins exactly one group,
/// the highest-priority rule that matches it.
#[derive(Clone, Debug)]
pub struct CategoryRule {
    pub group_name: String,
    pub description: String,
    pub path_patterns: Vec<String>,
    pub priority: u16,
    pub default_risk: RiskLevel,
    pub hint_text: String,
}

/// The result of clustering scanned directories into a named group (PRD §3.3).
#[derive(Clone, Debug)]
pub struct AutoGroup {
    pub group_name: String,
    pub description: String,
    pub hint_text: String,
    pub risk: RiskLevel,
    pub entries: Vec<DirEntry>,
    pub total_size_bytes: u64,
}

impl AutoGroup {
    /// Sum of every entry's size (kept in sync when entries are added/removed).
    pub fn recompute_total(&mut self) {
        self.total_size_bytes = self.entries.iter().map(|e| e.size_bytes).sum();
    }

    /// Whether this group exceeds the PRD §5 "recommended migration" threshold
    /// (5 GB) — drives the UI "推荐迁移" badge.
    pub fn is_recommended(&self) -> bool {
        self.total_size_bytes >= 5 * 1024 * 1024 * 1024
    }

    pub fn size_human(&self) -> String {
        human_bytes(self.total_size_bytes)
    }
}

/// Persisted task status (PRD §3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TaskStatus {
    Migrated,
    RolledBack,
    Failed,
}

impl TaskStatus {
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Migrated => "已迁移",
            TaskStatus::RolledBack => "已回滚",
            TaskStatus::Failed => "失败",
        }
    }
}

/// One persisted migration task (PRD §3.5), stored as a JSONL line in
/// `~/.verve/relocate/tasks.jsonl`. Forward-compatible: unknown fields are
/// ignored on load, and all fields use serde defaults so older records decode.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task_id: String,
    pub source_path: String,
    pub target_external_path: String,
    pub symlink_path: String,
    pub group_name: String,
    /// ISO-8601 creation timestamp (created via `chrono`).
    #[serde(default)]
    pub created_at: String,
    #[serde(default = "default_status_failed")]
    pub status: TaskStatus,
    /// Captured human-readable failure reason when status == Failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Total bytes migrated; used to show reclaimed space across the session.
    #[serde(default)]
    pub size_bytes: u64,
}

fn default_status_failed() -> TaskStatus {
    TaskStatus::Failed
}

impl TaskRecord {
    pub fn size_human(&self) -> String {
        human_bytes(self.size_bytes)
    }
}

/// Format a byte count as a compact human string (binary units).
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}
