//! HTTP stress testing.
//!
//! Two execution paths:
//! - **Single-request stress** via [`oha`](https://github.com/hatoo/oha) (a Rust
//!   HTTP load generator): oha is spawned as a child process with
//!   `--no-tui --output-format json`, its JSON report is parsed into a
//!   [`OhaJsonReport`] and surfaced to the UI.
//! - **Scenario stress** for multi-step test cases (oha cannot drive multi-URL
//!   flows): workers spawned on Tokio each run the autotest runner in a loop,
//!   tracking latency percentiles locally.
//!
//! The oha binary is expected at `<data_dir>/bin/oha[.exe]`. If missing, the
//! caller surfaces an error with a download hint; the UI offers a download
//! button (actual GitHub-Releases fetch is implemented in [`oha::find_or_download`]).

pub mod models;
pub mod oha;
pub mod scenario;

pub use models::*;
pub use oha::{find_or_download, run_oha};
pub use scenario::{run_scenario, StressStopHandle};
