//! HTTP stress testing.
//!
//! Two execution paths:
//! - **Single-request stress** via a built-in `reqwest`-based load generator
//!   (formerly oha): keeps `concurrency` workers looping over the same
//!   prepared request for `duration_secs`, streaming live snapshots to the UI
//!   for real-time chart rendering. Final result is a [`StressResult`].
//! - **Scenario stress** for multi-step test cases: workers spawned on Tokio
//!   each run the autotest runner in a loop, tracking latency percentiles
//!   locally.

pub mod models;
pub mod oha;
pub mod scenario;

pub use models::*;
pub use oha::{find_or_download, run_oha};
pub use scenario::{run_scenario, StressStopHandle};
