//! HTTP forward proxy for capturing traffic while debugging APIs.
//!
//! A minimal HTTP (non-TLS) forward proxy that runs on `127.0.0.1:<port>`. It
//! accepts absolute-form requests (`GET http://host/path HTTP/1.1`), forwards
//! them to the origin, and records both the request and response to an
//! in-memory ring buffer. The UI reads the buffer for display.
//!
//! Scope: HTTP only (no CONNECT / MITM for HTTPS — that requires a trusted CA
//! cert and is reserved for a later iteration). Sufficient for inspecting
//! traffic to the local Mock server or other plaintext endpoints.

pub mod capture;
pub mod server;

pub use capture::{CaptureEntry, CaptureStore};
pub use server::{DEFAULT_PORT, ProxyHandle, serve};
