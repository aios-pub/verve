#!/usr/bin/env bash
# Install Verve's native build dependencies for the current Linux distro.
#
# Verve's GPUI text stack pulls in `freetype-sys` + `fontconfig-sys`, which
# resolve the system FreeType / fontconfig via pkg-config at build time, so
# the matching -dev headers must be present or the build fails at the freetype
# link step. This script installs the full Linux prerequisite set, picking the
# right package manager for the host distro.
#
# Usage:
#   ./scripts/install-deps.sh
#
# Supported: Debian/Ubuntu (apt), Fedora (dnf), Arch (pacman).
set -euo pipefail

# ─── Debian / Ubuntu (apt) ─────────────────────────────────────────────────
install_apt() {
    echo "📦 Detected Debian/Ubuntu — using apt"
    sudo apt-get update
    sudo apt-get install -y \
        build-essential pkg-config gcc g++ clang \
        libssl-dev libfontconfig1-dev libfreetype6-dev \
        libgtk-3-dev libwebkit2gtk-4.1-dev \
        libxkbcommon-x11-dev libx11-xcb-dev libwayland-dev \
        libzstd-dev libvulkan1 vulkan-validationlayers
}

# ─── Fedora (dnf) ─────────────────────────────────────────────────────────
install_dnf() {
    echo "📦 Detected Fedora — using dnf"
    sudo dnf install -y \
        @development-tools pkgconf-pkg-config gcc gcc-c++ clang \
        openssl-devel fontconfig-devel freetype-devel \
        gtk3-devel webkit2gtk4.1-devel \
        libxkbcommon-x11-devel libxcb-devel wayland-devel \
        libzstd-devel vulkan-loader vulkan-validation-layers
}

# ─── Arch / Manjaro (pacman) ──────────────────────────────────────────────
install_pacman() {
    echo "📦 Detected Arch/Manjaro — using pacman"
    sudo pacman -S --needed \
        base-devel pkgconf gcc clang \
        openssl fontconfig freetype2 \
        gtk3 webkit2gtk-4.1 \
        libxkbcommon-x11 libxcb wayland \
        zstd vulkan-icd-loader vulkan-validation-layers
}

# ─── Dispatch ─────────────────────────────────────────────────────────────
if command -v apt-get >/dev/null 2>&1; then
    install_apt
elif command -v dnf >/dev/null 2>&1; then
    install_dnf
elif command -v pacman >/dev/null 2>&1; then
    install_pacman
else
    echo "❌ Unsupported distro: could not find apt-get, dnf, or pacman."
    echo "   Please install the equivalent FreeType / fontconfig / GTK / Vulkan"
    echo "   development packages manually, then re-run cargo build."
    exit 1
fi

echo ""
echo "✅ Build dependencies installed. You can now: cargo build --release"
