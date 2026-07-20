//! Automated test cases (test suites / cases / steps / reports).
//!
//! Data models live in [`models`]; the sequential test runner (added in the
//! next iteration) will consume existing `scripting` + `http` infrastructure.

pub mod models;

pub use models::*;
