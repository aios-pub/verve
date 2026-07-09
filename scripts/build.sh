#!/usr/bin/env bash
# Verve cross-platform build script.
#
# Usage:
#   ./scripts/build.sh macos    # Build + bundle macOS .app
#   ./scripts/build.sh linux    # Build .deb + .AppImage
#   ./scripts/build.sh windows  # Build .msi (run on Windows or cross-compile)
#   ./scripts/build.sh all      # Build for current platform
#
# Prerequisites:
#   - Rust toolchain (rustup)
#   - cargo-bundle (for macOS): cargo install cargo-bundle
#   - For Linux .deb: cargo install cargo-deb
#   - For Windows .msi: cargo install cargo-wix
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

PLATFORM="${1:-$(uname -s | tr '[:upper:]' '[:lower:]')}"
VERSION="0.1.0"

echo "🔨 Building Verve v$VERSION for: $PLATFORM"
echo "   Project: $PROJECT_DIR"
echo ""

# ─── Common: compile release binary ─────────────────────────────────────────
build_release() {
    local target="$1"
    echo "📦 Compiling release binary ($target)..."
    if [ -n "$target" ]; then
        cargo build --release --target "$target" --bin verve
    else
        cargo build --release --bin verve
    fi
    echo "✅ Binary compiled"
}

# ─── macOS ──────────────────────────────────────────────────────────────────
build_macos() {
    echo "🍎 Building macOS .app bundle..."

    build_release ""

    echo "📦 Creating .app bundle with cargo-bundle..."
    cargo bundle --release --format app

    APP_DIR="target/release/bundle/osx/Verve.app"

    if [ -d "$APP_DIR" ]; then
        echo "✅ macOS bundle created: $APP_DIR"

        # Optionally create .dmg
        if command -v hdiutil &>/dev/null; then
            echo "💿 Creating .dmg..."
            DMG="target/release/Verve-$VERSION.dmg"
            hdiutil create -volname "Verve" -srcfolder "$APP_DIR" -ov -format UDZO "$DMG" 2>/dev/null || true
            if [ -f "$DMG" ]; then
                echo "✅ DMG created: $DMG"
            fi
        fi

        # Also build verve-server binary
        echo "📦 Building verve-server binary..."
        cargo build --release --bin verve-server
        echo "✅ verve-server compiled: target/release/verve-server"
    else
        echo "❌ Bundle creation failed"
        exit 1
    fi
}

# ─── Linux ──────────────────────────────────────────────────────────────────
build_linux() {
    echo "🐧 Building Linux packages..."

    build_release ""

    # Build .deb
    if cargo install --list | grep -q cargo-deb; then
        echo "📦 Creating .deb package..."
        cargo deb --output target/release/verve-$VERSION-amd64.deb 2>/dev/null || {
            echo "⚠️  cargo-deb not available, skipping .deb"
        }
        [ -f "target/release/verve-$VERSION-amd64.deb" ] && echo "✅ .deb created"
    else
        echo "⚠️  cargo-deb not installed. Install with: cargo install cargo-deb"
    fi

    # Copy binary + icon + desktop entry for manual install
    mkdir -p target/release/dist
    cp target/release/verve target/release/dist/
    cp assets/icons/verve.png target/release/dist/
    cp assets/verve.desktop target/release/dist/

    # Build verve-server
    cargo build --release --bin verve-server

    echo "✅ Linux build complete (target/release/dist/)"
    echo "   Manual install: sudo cp dist/verve /usr/bin/"
    echo "                   sudo cp dist/verve.png /usr/share/icons/hicolor/512x512/apps/"
    echo "                   sudo cp dist/verve.desktop /usr/share/applications/"
}

# ─── Windows ────────────────────────────────────────────────────────────────
build_windows() {
    echo "🪟 Building Windows packages..."

    # Check if we're on Windows or cross-compiling
    if [[ "$(uname -s)" == MINGW* ]] || [[ "$(uname -s)" == MSYS* ]] || [[ "$(uname -s)" == CYGWIN* ]]; then
        build_release ""
    else
        echo "📦 Cross-compiling for Windows (x86_64-pc-windows-msvc)..."
        rustup target add x86_64-pc-windows-msvc 2>/dev/null || true
        build_release "x86_64-pc-windows-msvc"
    fi

    # Build .msi with cargo-wix
    if cargo install --list | grep -q cargo-wix; then
        echo "📦 Creating .msi installer..."
        cargo wix --no-build -o target/release/verve-$VERSION.msi 2>/dev/null || {
            echo "⚠️  cargo-wix not available, skipping .msi"
        }
    else
        echo "⚠️  cargo-wix not installed. Install with: cargo install cargo-wix"
    fi

    # Copy binary + icon
    mkdir -p target/release/dist
    if [ -f "target/x86_64-pc-windows-msvc/release/verve.exe" ]; then
        cp target/x86_64-pc-windows-msvc/release/verve.exe target/release/dist/
    elif [ -f "target/release/verve.exe" ]; then
        cp target/release/verve.exe target/release/dist/
    fi
    cp assets/icons/verve.ico target/release/dist/

    echo "✅ Windows build complete (target/release/dist/)"
}

# ─── Main ───────────────────────────────────────────────────────────────────
case "$PLATFORM" in
    darwin|macos)
        build_macos
        ;;
    linux)
        build_linux
        ;;
    mingw*|msys*|cygwin*|windows)
        build_windows
        ;;
    all|"")
        case "$(uname -s)" in
            Darwin) build_macos ;;
            Linux)  build_linux ;;
            MINGW*|MSYS*|CYGWIN*) build_windows ;;
        esac
        ;;
    *)
        echo "❌ Unknown platform: $PLATFORM"
        echo "   Supported: macos, linux, windows, all"
        exit 1
        ;;
esac

echo ""
echo "🎉 Build complete!"
