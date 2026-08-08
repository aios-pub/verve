//! Docker container management.
//!
//! All Docker Engine API access goes through the native `bollard` client (see
//! [`client`]); no external `docker` CLI binary is required.

pub mod cli;
pub mod client;
pub mod host_store;
pub mod models;
pub mod socket_tunnel;

pub use cli::{
    Container, ContainerState, DockerError, ExecSession, ImageInfo, LogStream, close_ssh_tunnel,
    exec_container, list_containers, list_images, prune_images, remove, restart, start, stop,
    stream_logs,
};
pub use host_store::{
    find_host, get_active_id, load_hosts, remove_host, save_hosts, set_active_id, upsert_host,
};
pub use models::DockerHost;
pub use socket_tunnel::SocketTunnel;
