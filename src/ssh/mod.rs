//! SSH client module — host management, secure credential storage, terminal
//! connection, and rendering.

pub mod client;
pub mod credentials;
pub mod host_store;
pub mod known_hosts;
pub mod models;
pub mod port_forward;
pub mod sftp;
pub mod terminal;
pub mod vault;
pub mod zmodem;

pub use port_forward::LocalForward;
