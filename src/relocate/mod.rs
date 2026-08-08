//! Verve-Relocate — disk-directory migration module.
//!
//! Scans the local user home for large cache/project/media directories,
//! clusters them into auto-groups (plus user-added custom directories), then
//! relocates them to an external high-speed SSD and replaces the original
//! path with an absolute-path symlink so applications are unaware.
//!
//! # Core constraints (PRD §2, non-negotiable)
//! - Absolute-path symlinks only.
//! - Removing a link is a single `unlink`; never recursive directory removal.
//! - Business migrate/rollback never depends on Time Machine (disaster-only).
//! - All disk I/O is async — never blocks the GPUI render thread.
//! - Global blacklist is enforced at two layers: scan-time and execute-time.
//!
//! See `docs/relocate 磁盘目录迁移模块.md` for the full specification.

pub mod blacklist;
pub mod categories;
pub mod disk_probe;
pub mod engine;
pub mod migrate;
pub mod persistence;
pub mod rollback;
pub mod scanner;
pub mod types;

pub use engine::{EngineState, RelocateEngine, RelocateEvent};
pub use types::{AutoGroup, CategoryRule, DirEntry, RiskLevel, TaskRecord, TaskStatus};
