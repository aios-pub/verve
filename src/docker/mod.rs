//! Docker container management.
//!
//! Drives the local Docker daemon by shelling out to the `docker` CLI — the
//! same pattern used for kubectl. The daemon is assumed running and `docker`
//! on PATH. Supports:
//!
//! - Container listing / start / stop / restart / rm
//! - `docker logs -f` streaming
//! - `docker exec -it` (interactive, feeds the SSH TerminalView)
//! - Port mapping (read-only — published ports are shown as-is)
//! - Image listing + `image prune -f`
//!
//! Explicitly NOT implemented (per task spec): Dockerfile editor, compose
//! graph editor, push/pull, swarm, monitoring dashboards, image scanning.

pub mod cli;

pub use cli::{
    exec_container, list_containers, list_images, prune_images, remove as docker_remove,
    restart as docker_restart, start as docker_start, stop as docker_stop, stream_logs, Container,
    ContainerState, DockerError, ExecSession, ImageInfo, LogStream,
};
