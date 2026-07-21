//! `verve-server` — standalone cloud document-management server.
//!
//! Runs the document-sharing HTTP server independently of the desktop app, with
//! its own file-backed project/share store and an admin UI at `/admin` for
//! uploading projects and managing shares online. Designed for cloud/headless
//! deployment: ship the binary, expose a port, and operators manage everything
//! through the web UI.
//!
//! # Usage
//! ```text
//! verve-server                           # 0.0.0.0:3097, data in ~/.verve/cloud
//! verve-server --port 8080               # custom port
//! verve-server --host 127.0.0.1          # localhost only
//! verve-server --data-dir /var/verve     # custom data root
//! ```
//!
//! # Data layout
//! ```text
//! <data-dir>/                     # $HOME/.verve by default, or --data-dir
//! ├── shares.json                 # all share configs
//! └── cloud/
//!     └── projects/
//!         └── <project-id>.json   # one Verve-native Project per file
//! ```
//!
//! # Endpoints
//! - `GET  /`          — landing page
//! - `GET  /admin`     — admin dashboard (upload projects, create/delete shares)
//! - `POST /admin/api/projects`        — upload a project (raw Project JSON)
//! - `POST /admin/api/projects/<id>`   — delete a project
//! - `POST /admin/api/shares`          — create a share (form-encoded)
//! - `POST /admin/api/shares/<id>`     — delete a share
//! - `GET  /s/<id>`   — view a shared doc (strict expiration + password)
//! - `GET  /health`   — health check

use std::sync::Arc;

use verve::share::cloud::CloudStore;
use verve::share::server;

fn main() {
    // Initialize logging.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let _ = env_logger::Builder::from_default_env()
        .format_timestamp_secs()
        .try_init();

    // Parse CLI args. Lightweight manual parsing — no clap dependency.
    let mut host = "0.0.0.0".to_string();
    let mut port = server::DEFAULT_PORT;
    let mut data_dir: Option<String> = None;
    let mut auth_token: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" | "-h" if arg == "--host" => {
                host = args.next().unwrap_or_else(|| "0.0.0.0".into());
            }
            "--port" | "-p" => {
                if let Some(v) = args.next() {
                    if let Ok(p) = v.parse() {
                        port = p;
                    }
                }
            }
            "--data-dir" | "-d" => {
                data_dir = args.next();
            }
            "--auth-token" | "-t" => {
                auth_token = args.next();
            }
            "--help" => {
                print_help();
                return;
            }
            // Bare numeric arg → port (convenient shorthand: `verve-server 8080`).
            n if n.parse::<u16>().is_ok() => {
                port = n.parse().unwrap_or(port);
            }
            other => {
                eprintln!("unknown argument: {other} (see --help)");
            }
        }
    }

    // Override the data directory: persistence::data_dir() reads $HOME/.verve,
    // so point $HOME at the parent of the requested data dir.
    if let Some(dir) = &data_dir {
        // The cloud store reads <data>/cloud/, and data_dir() = $HOME/.verve.
        // So if the user wants data at /var/verve, set HOME=/var (so
        // $HOME/.verve = /var/verve). For clarity, we also support an absolute
        // .verve path by setting HOME to its parent.
        let p = std::path::Path::new(dir);
        if p.ends_with(".verve") {
            if let Some(parent) = p.parent() {
                std::env::set_var("HOME", parent);
            }
        } else {
            // Treat the arg as the parent that should contain `.verve`.
            std::env::set_var("HOME", dir);
        }
    }

    log::info!("verve-server starting: host={host} port={port}");
    log::info!(
        "data dir: {}",
        data_dir.as_deref().unwrap_or("$HOME/.verve")
    );

    // Load the file-backed cloud store.
    let store = Arc::new(CloudStore::load());
    let n_projects = store.list_projects().len();
    let n_shares = store.shares().len();
    log::info!("loaded {n_projects} project(s), {n_shares} share(s) from cloud store");
    if n_projects == 0 {
        log::warn!(
            "no projects yet — open the admin UI at http://{host}:{port}/admin to upload one"
        );
    }

    // Start the server in cloud mode (binds the host, serves /admin).
    let _server = server::start_cloud(host.clone(), port, store, auth_token);

    log::info!("verve-server listening on http://{host}:{port}");
    log::info!("admin UI:  http://{host}:{port}/admin");
    log::info!("press Ctrl+C to stop");

    // Block forever (the server runs in a smol task; main just parks).
    smol::block_on(async {
        loop {
            smol::Timer::after(std::time::Duration::from_secs(3600)).await;
        }
    });
}

fn print_help() {
    eprintln!("verve-server — cloud document-management server");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    verve-server [OPTIONS] [PORT]");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    --host <HOST>        Bind address (default: 0.0.0.0)");
    eprintln!("    --port <PORT>        Bind port (default: 3097)");
    eprintln!("    --data-dir <DIR>     Data root containing .verve/ (default: $HOME)");
    eprintln!("    --auth-token <TOK>   Bearer token required for /admin routes");
    eprintln!("    --help               Show this help");
    eprintln!();
    eprintln!("EXAMPLES:");
    eprintln!("    verve-server                       # 0.0.0.0:3097");
    eprintln!("    verve-server --port 80             # custom port");
    eprintln!("    verve-server --host 127.0.0.1 9000 # localhost, port 9000");
}
