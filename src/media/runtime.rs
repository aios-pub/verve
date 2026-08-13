//! Shared Tokio runtime for media uploads, mirroring `docker_runtime` /
//! `k8s_runtime`. rust-s3's async I/O needs a Tokio runtime; GPUI tasks await
//! the result via a oneshot channel.

use std::sync::OnceLock;

use tokio::runtime::Runtime;

/// Lazily-initialized shared multi-thread Tokio runtime for media work.
pub fn media_runtime() -> &'static Runtime {
    static MEDIA_RT: OnceLock<Runtime> = OnceLock::new();
    MEDIA_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build shared media tokio runtime")
    })
}
