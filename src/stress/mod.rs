//! HTTP stress testing.
//!
//! Two execution paths:
//! - **Single-request stress** via [`oha`](https://github.com/hatoo/oha) (a Rust
//!   HTTP load generator): oha is integrated directly as a library, no external
//!   binary required. Its JSON report is parsed into a [`OhaJsonReport`] and
//!   surfaced to the UI.
//! - **Scenario stress** for multi-step test cases (oha cannot drive multi-URL
//!   flows): workers spawned on Tokio each run the autotest runner in a loop,
//!   tracking latency percentiles locally.

pub mod models;
pub mod oha;
pub mod scenario;

pub use models::*;
pub use oha::{find_or_download, run_oha};
pub use scenario::{run_scenario, StressStopHandle};
