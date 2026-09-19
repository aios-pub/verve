//! SSH client module — host management, secure credential storage, terminal
//! connection, and rendering.

pub mod client;
pub mod credentials;
pub mod host_store;
pub mod known_hosts;
#[cfg(target_os = "linux")]
pub mod linux_input;
pub mod local_pty;
#[cfg(target_os = "macos")]
pub mod macos_input;
pub mod models;
pub mod port_forward;
pub mod sftp;
pub mod terminal;
pub mod vault;
#[cfg(target_os = "windows")]
pub mod windows_input;
#[cfg(target_os = "windows")]
mod win_conpty;
pub mod zmodem;

pub use port_forward::LocalForward;
