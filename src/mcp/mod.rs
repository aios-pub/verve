//! MCP (Model Context Protocol) support.
//!
//! External AI clients (Claude Desktop, Cursor, Claude Code, VS Code Copilot,
//! …) can launch Verve as a stdio MCP server (`verve mcp`) and then read and
//! manage the API workspace — projects, folders, requests and environment
//! variables — through the tools in [`server`].
//!
//! - [`config`] builds the client config snippets copied from the
//!   项目管理 → 对外能力 panel (one-click copy).
//! - [`store`] is the disk-backed workspace store with a backup/rollback guard.
//! - [`dto`] defines the JSON views returned to AI clients.
//! - [`server`] defines the MCP tools and the process entry point.

pub mod config;
pub mod dto;
mod server;
mod store;

pub use config::{ClientKind, claude_code_command, client_config, command_pair};
pub use server::{run_cli, run_stdio};
