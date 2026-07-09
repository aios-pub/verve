//! Git-backed project versioning via the pure-Rust `gix` (gitoxide) crate.

pub mod ops;
pub mod state;

pub use ops::{GitAuth, GitCommit, GitStatus};
pub use state::{GitEvent, GitState};
