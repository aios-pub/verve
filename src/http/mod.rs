//! HTTP layer: request preparation, variable substitution, and execution.

pub mod client;
pub mod curl;
pub mod grpc;
pub mod sse;
pub mod tcp;
pub mod variable;
pub mod ws;

pub use client::{PreparedRequest, execute, normalize_url, normalize_url_with_default, prepare};
pub use variable::{collect_placeholders, substitute};
