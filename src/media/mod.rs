//! Media storage module: local + S3 storage for note images and videos.
//!
//! Phase 1 scope:
//! - Shared, user-configurable media root (`<data_dir>/media` by default) with
//!   `images/` and `videos/` subdirectories.
//! - SparkID-based, time-sortable filenames.
//! - Portable `./media/<subdir>/<sparkid>.<ext>` markdown references that the
//!   renderer maps back to the configured root.
//! - Layered `media_storage.toml` config (`[global]`/`[image]`/`[video]`/`[s3]`)
//!   with serde defaults so older configs missing a section merge cleanly.
//!
//! S3 upload, video pipeline, and delete-on-note-deletion land in later phases.

pub mod config;
pub mod cleanup;
pub mod id;
pub mod local;
pub mod path;
pub mod runtime;
pub mod s3;
pub mod upload;
