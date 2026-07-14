//! Document-sharing system (postman-style).
//!
//! Generates self-contained HTML documentation for a project / folder / single
//! request and hosts it on an HTTP server with **strict** access control
//! (expiration + password). Two run modes share the same routing core:
//!
//! - **Desktop** (embedded in the app): binds `127.0.0.1:<port>`, resolves
//!   projects from GPUI state, configs from `shares.json`.
//! - **Cloud** (`verve-server` binary): binds `0.0.0.0:<port>`, resolves
//!   projects from its own file store (`<data>/cloud/projects/`), and serves an
//!   admin UI at `/admin` for uploading projects + managing shares.
//!
//! Modules:
//! - [`models`] — `ShareConfig` and friends.
//! - [`persist`] — `shares.json` load/save.
//! - [`html`] — postman-style HTML document generation.
//! - [`server`] — the HTTP server core (routing + strict enforcement).
//! - [`cloud`] — file-backed project/share store for the cloud binary.
//! - [`qrcode`] — QR code generation for the 二维码 share method.

pub mod cloud;
pub mod html;
pub mod models;
pub mod persist;
pub mod qrcode;
pub mod server;

pub use models::{AccessControl, Expiration, FieldDisplay, ShareConfig, ShareMethod, ShareScope};
