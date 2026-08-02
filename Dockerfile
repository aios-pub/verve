# Dockerfile for verve-server — the cloud document-management server.
#
# Builds a minimal static-ish image (Debian slim base for glibc) that runs
# verve-server on port 3097 with a persistent volume for project/share data.
#
# Build:
#   docker build -t verve-server .
#
# Run (expose port 3097, persist data to a named volume):
#   docker run -d --name verve-server \
#     -p 3097:3097 \
#     -v verve-data:/data \
#     verve-server
#
# Then open http://localhost:3097/admin to upload projects + create shares.

# ---- Build stage ----------------------------------------------------------
FROM rust:1-bookworm AS builder

WORKDIR /build

# Pre-copy the manifest for dependency caching.
COPY Cargo.toml Cargo.lock ./

# The verve crate references gpui-component as a path dependency, and gpui via
# git. Copy the whole repo so path deps resolve. For a standalone build you'd
# publish gpui-component to a registry and drop this COPY.
COPY . .

# Install build-time native library headers. verve is a single crate whose
# dependency graph is shared by every binary (the desktop app, verve-server,
# and the lib), so building verve-server still compiles the GPUI text stack —
# `font-kit` → freetype-sys + fontconfig-sys — which resolve the system libs
# via pkg-config. Without these -dev headers the build fails at the freetype
# link step. (Runtime stage needs none of these: verve-server is pure HTTP.)
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config libfreetype6-dev libfontconfig1-dev \
    && rm -rf /var/lib/apt/lists/*

# Build only the server binary (skips the GPUI desktop app's link step, which
# needs GTK/Wayland/X11 GUI libs the slim image doesn't have).
RUN cargo build --release --bin verve-server

# ---- Runtime stage --------------------------------------------------------
FROM debian:bookworm-slim

# The server is pure smol HTTP — no GUI, no extra runtime deps. Install ca-
# certificates so HTTPS fetches (if any) work, and tini for clean signal
# handling.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary.
COPY --from=builder /build/target/release/verve-server /usr/local/bin/verve-server

# Data lives under /data/.verve (HOME=/data). The server reads
#   /data/.verve/cloud/projects/<id>.json  and  /data/.verve/shares.json.
ENV HOME=/data
VOLUME ["/data"]

EXPOSE 3097

# tini handles SIGTERM/SIGINT for clean container shutdowns.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["verve-server", "--host", "0.0.0.0", "--port", "3097", "--data-dir", "/data"]
