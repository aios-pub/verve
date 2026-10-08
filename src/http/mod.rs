//! HTTP layer: request preparation, variable substitution, and execution.

pub mod client;
pub mod curl;
pub mod grpc;
pub mod sse;
pub mod tcp;
pub mod variable;
pub mod ws;

pub use client::{PreparedRequest, execute, normalize_url, normalize_url_with_default, prepare};

/// Default end-to-end request timeout (seconds) shared by the request panel's
/// HTTP/GraphQL/SSE/gRPC-Web send paths and the autotest runner.
pub const DEFAULT_TIMEOUT_SECS: u64 = 3000;
pub use variable::{collect_placeholders, substitute};
