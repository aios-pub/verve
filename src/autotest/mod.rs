//! Automated test cases (test suites / cases / steps / reports).
//!
//! Data models live in [`models`]; [`runner`] is the sequential executor that
//! walks a `TestCase` step-by-step, reusing the existing `scripting` and
//! `http` infrastructure.

pub mod models;
pub mod runner;

pub use models::*;
pub use runner::{run_case, RunEvent};
