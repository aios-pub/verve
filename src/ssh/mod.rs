//! SSH client module — host management, secure credential storage, terminal
//! connection, and rendering.

pub mod client;
pub mod credentials;
pub mod host_store;
pub mod models;
pub mod vault;
pub mod terminal;

/// Global mutex for serializing HOME-dependent tests across all ssh submodules.
#[cfg(test)]
pub static TEST_HOME_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
