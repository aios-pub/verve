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

    # Resolve the real target dir (honor CARGO_TARGET_DIR so build.rs and this
    # script agree on where artifacts land).
    TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_DIR/target}"
    echo "   Target dir: $TARGET_DIR"

    build_release ""

    echo "📦 Creating .app bundle with cargo-bundle..."
    cargo bundle --release --format osx

    # cargo-bundle may emit the bundle under either the project's target/ or
    # CARGO_TARGET_DIR depending on how it resolves env. Detect whichever exists.
    APP_DIR=""
    for candidate in \
        "$TARGET_DIR/release/bundle/osx/Verve.app" \
        "$PROJECT_DIR/target/release/bundle/osx/Verve.app" ; do
        if [ -d "$candidate" ]; then
            APP_DIR="$candidate"
            break
        fi
    done

    if [ -z "$APP_DIR" ]; then
        echo "❌ Bundle creation failed (Verve.app not found)"
        exit 1
    fi

    echo "✅ macOS bundle created: $APP_DIR"

    # ── Bundle libpdfium.dylib next to the binary inside the .app ───────────
    # cargo-bundle does not automatically collect dylibs that the binary links
    # via @rpath; build.rs only copies libpdfium.dylib to target/release/.
    # Copy it into Contents/MacOS/ (same directory as the verve executable) so
    # the @loader_path/ rpath can resolve it at runtime.
    MACOS_DIR="$APP_DIR/Contents/MacOS"
    PDFIUM_SRC="$TARGET_DIR/release/libpdfium.dylib"
    if [ -f "$PDFIUM_SRC" ]; then
        cp "$PDFIUM_SRC" "$MACOS_DIR/libpdfium.dylib"
        # Remove quarantine + ad-hoc sign the bundled dylib so Gatekeeper is
        # happy when the .app is launched after being downloaded / unsigned.
        xattr -d com.apple.quarantine "$MACOS_DIR/libpdfium.dylib" 2>/dev/null || true
        codesign --force --sign - "$MACOS_DIR/libpdfium.dylib" 2>/dev/null || true
        echo "✅ Bundled libpdfium.dylib into Contents/MacOS/"
    else
        echo "⚠️  libpdfium.dylib not found at $PDFIUM_SRC"
    fi

    # Re-sign the whole .app ad-hoc so the updated bundle contents (added
    # dylib) don't invalidate any existing signature.
    codesign --force --deep --sign - "$APP_DIR" 2>/dev/null || true

    # Mirror the finished .app back into the project-local target/ so users
    # running the script from the repo root find it in the documented path
    # even when CARGO_TARGET_DIR points elsewhere.
    LOCAL_APP="$PROJECT_DIR/target/release/bundle/osx/Verve.app"
    mkdir -p "$(dirname "$LOCAL_APP")"
    rm -rf "$LOCAL_APP"
    cp -R "$APP_DIR" "$LOCAL_APP"
    APP_DIR="$LOCAL_APP"

    # Create an install-style DMG: a finder window with Verve.app and an
    # Applications symlink so the user drags-to-install (standard macOS UX).
    if command -v hdiutil &>/dev/null; then
        echo "💿 Creating installer DMG..."
        mkdir -p "$PROJECT_DIR/target/release"
        DMG="$PROJECT_DIR/target/release/Verve-$VERSION.dmg"
        STAGING="$(mktemp -d)/Verve"
        mkdir -p "$STAGING"

        # Copy the signed .app into the staging folder.
        cp -R "$APP_DIR" "$STAGING/Verve.app"
        # Applications symlink — creates the drag-to-install target.
        ln -s /Applications "$STAGING/Applications"

        # Optional: tweak Finder layout (icon positions, icon size, toolbar
        # hidden) by writing a .DS_Store onto a writable image. If any step
        # fails we fall back to a plain DMG (still works for install).
        (
            # Detach any stale Verve mounts left over from prior runs so that
            # the mount point is a clean "/Volumes/Verve" (no "Verve 2" suffixes).
            for m in /Volumes/Verve*; do
                [ -d "$m" ] && hdiutil detach "$m" -force -quiet 2>/dev/null || true
            done

            RW_DMG="$PROJECT_DIR/target/release/Verve-$VERSION-rw.dmg"
            rm -f "$RW_DMG" "$DMG"
            hdiutil create -volname "Verve" -srcfolder "$STAGING" -ov -format UDRW "$RW_DMG" >/dev/null

            # Attach and capture the mount point (path may contain spaces, so
            # parse with sed instead of awk's $NF).
            ATTACH_OUT=$(hdiutil attach "$RW_DMG" -nobrowse -readwrite 2>&1)
            MOUNT=$(echo "$ATTACH_OUT" | sed -n 's|^.*\(/Volumes/.*\)$|\1|p' | head -1)

            if [ -d "$MOUNT" ]; then
                # Arrange the Finder window via AppleScript.
                osascript <<EOF >/dev/null 2>&1 || true
tell application "Finder"
    tell disk "Verve"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set bounds of container window to {400, 200, 900, 520}
        set viewOptions to the icon view options of container window
        set arrangement of viewOptions to not arranged
        set icon size of viewOptions to 128
        set position of item "Verve.app" of container window to {130, 170}
        set position of item "Applications" of container window to {370, 170}
        close
        open
        update without registering applications
        delay 3
    end tell
end tell
EOF
                sync
                # Force-detach; retry a few times in case Finder is still holding it.
                for _ in 1 2 3 4 5 6 7 8; do
                    hdiutil detach "$MOUNT" -quiet 2>/dev/null && break
                    sleep 1
                done
                # Last-resort force detach if still attached.
                hdiutil info | grep -F "$MOUNT" >/dev/null 2>&1 && \
                    hdiutil detach "$MOUNT" -force -quiet 2>/dev/null || true
            fi

            # Compress the RW image into the final distributable DMG.
            if hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 -o "$DMG" 2>&1; then
                : # success
            else
                # Fallback: if convert failed (busy), just use a plain UDZO
                # srcfolder DMG without the custom Finder layout — still works.
                echo "   (custom layout failed, falling back to plain DMG)"
                hdiutil create -volname "Verve" -srcfolder "$STAGING" -ov -format UDZO "$DMG" >/dev/null 2>&1 || true
            fi
            rm -f "$RW_DMG"
        )

        rm -rf "$STAGING"
        if [ -f "$DMG" ]; then
            SIZE=$(du -h "$DMG" | awk '{print $1}')
            echo "✅ DMG created: $DMG ($SIZE)"
        fi
    fi

    # Also build verve-server binary
    echo "📦 Building verve-server binary..."
    cargo build --release --bin verve-server
    # Copy to project-local target/ for convenience if using a custom target dir.
    if [ "$TARGET_DIR" != "$PROJECT_DIR/target" ] && [ -f "$TARGET_DIR/release/verve-server" ]; then
        mkdir -p "$PROJECT_DIR/target/release"
        cp "$TARGET_DIR/release/verve-server" "$PROJECT_DIR/target/release/verve-server"
    fi
    echo "✅ verve-server compiled: target/release/verve-server"
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

# ─── Install: copy built .app to /Applications (macOS) ─────────────────────
install_macos() {
    APP_SRC="$PROJECT_DIR/target/release/bundle/osx/Verve.app"
    APP_DST="/Applications/Verve.app"
    if [ ! -d "$APP_SRC" ]; then
        echo "❌ $APP_SRC not found — run './scripts/build.sh macos' first."
        exit 1
    fi
    echo "📥 Installing Verve.app to /Applications..."
    # If already running, offer to kill it.
    if pgrep -x "verve" >/dev/null 2>&1; then
        echo "   Verve is currently running — quitting..."
        pkill -x verve 2>/dev/null || true
        sleep 1
    fi
    # Remove old install.
    if [ -d "$APP_DST" ]; then
        echo "   Removing previous installation..."
        rm -rf "$APP_DST"
    fi
    cp -R "$APP_SRC" "$APP_DST"
    # Remove quarantine so Gatekeeper doesn't flag a local copy.
    # `xattr -r` is not supported on older macOS; -c clears all xattrs safely.
    xattr -rc "$APP_DST" 2>/dev/null || \
        find "$APP_DST" -print0 2>/dev/null | xargs -0 xattr -d com.apple.quarantine 2>/dev/null || true
    # Re-sign ad-hoc after copying to a new path.
    codesign --force --deep --sign - "$APP_DST" 2>/dev/null || true
    echo "✅ Installed: $APP_DST"
    echo ""
    echo "🚀 Launch with: open -a Verve"
    open -a Verve 2>/dev/null && echo "   (launched)" || true
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
    install)
        case "$(uname -s)" in
            Darwin) install_macos ;;
            *) echo "❌ 'install' is currently supported on macOS only."; exit 1 ;;
        esac
        ;;
    all|"")
        case "$(uname -s)" in
            Darwin) build_macos ;;
            Linux)  build_linux ;;
            MINGW*|MSYS*|CYGWIN*) build_windows ;;
        esac
        ;;
    *)
        echo "❌ Unknown platform/command: $PLATFORM"
        echo "   Supported: macos, linux, windows, all, install"
        exit 1
        ;;
esac

echo ""
echo "🎉 Build complete!"
