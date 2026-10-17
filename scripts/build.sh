#!/usr/bin/env bash
# Verve cross-platform build script.
#
# Usage:
#   ./scripts/build.sh macos    # Build + bundle macOS .app
#   ./scripts/build.sh linux    # Build .deb + .AppImage
#   ./scripts/build.sh windows  # Build .msi (run on Windows or cross-compile)
#   ./scripts/build.sh all      # Build for current platform
#   ./scripts/build.sh --no-auto-install linux   # skip the dep-install prompt (CI)
#
# On Linux this script checks for the native build dependencies (FreeType /
# fontconfig / pkg-config) and, in an interactive shell, offers to install them
# via ./scripts/install-deps.sh before compiling. Pass --no-auto-install to
# skip the check (the native deps must then already be present).
#
# Prerequisites:
#   - Rust toolchain (rustup)
#   - cargo-bundle (for macOS): cargo install cargo-bundle
#   - For Linux .deb: cargo install cargo-deb
#   - For Windows .msi: cargo install cargo-wix
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

# ─── Argument parsing ──────────────────────────────────────────────────────
# Allow `build.sh [--no-auto-install] <platform>`. The flag skips the native
# dependency auto-install prompt below (useful in CI, where deps are pre-installed
# by the workflow and an interactive sudo prompt would hang the run).
AUTO_INSTALL=1
PLATFORM=""
for arg in "$@"; do
    case "$arg" in
        --no-auto-install) AUTO_INSTALL=0 ;;
        -h|--help)
            sed -n '2,14p' "$0"   # print the usage header comment
            exit 0
            ;;
        *) PLATFORM="$arg" ;;
    esac
done
# Default to the host platform if none given.
PLATFORM="${PLATFORM:-$(uname -S | tr '[:upper:]' '[:lower:]')}"

# ─── Native dependency check (Linux) ───────────────────────────────────────
# verve is a single crate whose dependency graph is shared by every binary, so
# building anything pulls in GPUI's text stack → font-kit → freetype-sys +
# fontconfig-sys. Those resolve the system libs via pkg-config and *panic*
# their build script if the -dev headers are absent (freetype-sys calls
# pkg-config's `.probe("freetype2").unwrap()`). Detect that up front and offer
# to install the headers rather than letting the user hit an opaque C compile
# error. macOS/Windows use native text backends (core-text / dwrote), so this
# check is Linux-only.
ensure_linux_deps() {
    # Build the list of missing items. Each entry is "pkg-config name|label".
    local missing=()
    if ! command -v pkg-config >/dev/null 2>&1; then
        missing+=("pkg-config|pkg-config (the pkg-config tool itself)")
    else
        # pkg-config exists — probe each library it should expose.
        local pc_libs=("freetype2|FreeType (libfreetype6-dev)"
                       "fontconfig|fontconfig (libfontconfig1-dev)")
        local entry name label
        for entry in "${pc_libs[@]}"; do
            name="${entry%%|*}"
            label="${entry##*|}"
            if ! pkg-config --exists "$name" 2>/dev/null; then
                missing+=("$name|$label")
            fi
        done
    fi

    if [ "${#missing[@]}" -eq 0 ]; then
        return 0
    fi

    echo "⚠️  Missing native build dependencies:"
    local m
    for m in "${missing[@]}"; do echo "     - ${m##*|}"; done
    echo "     (GPUI's text stack needs these to compile; without them freetype-sys"
    echo "      panics at the link step.)"

    if [ "$AUTO_INSTALL" -eq 0 ]; then
        echo "❌ --no-auto-install set, not installing. Run ./scripts/install-deps.sh manually."
        exit 1
    fi

    # Non-interactive shell (no TTY, e.g. piped stdin) → don't prompt, just tell.
    if [ ! -t 0 ]; then
        echo "❌ Non-interactive shell detected. Install them, then re-run:"
        echo "     ./scripts/install-deps.sh"
        exit 1
    fi

    printf "Install them now via sudo? [y/N] "
    local reply
    read -r reply
    case "$reply" in
        y|Y|yes|YES)
            echo "📦 Running ./scripts/install-deps.sh (may ask for your sudo password)..."
            bash "$PROJECT_DIR/scripts/install-deps.sh"
            # Re-check; if install-deps.sh couldn't satisfy a lib, fail loudly.
            local still_missing=()
            if ! command -v pkg-config >/dev/null 2>&1; then
                still_missing+=("pkg-config")
            else
                for entry in "${pc_libs[@]}"; do
                    name="${entry%%|*}"
                    pkg-config --exists "$name" 2>/dev/null || still_missing+=("$name")
                done
            fi
            if [ "${#still_missing[@]}" -gt 0 ]; then
                echo "❌ Still missing after install: ${still_missing[*]}"
                echo "   Install them manually, then re-run this script."
                exit 1
            fi
            echo "✅ Dependencies installed"
            ;;
        *)
            echo "❌ Skipping install. Install them manually:"
            echo "     ./scripts/install-deps.sh"
            exit 1
            ;;
    esac
}

if [ "$(uname -s)" = "Linux" ]; then
    ensure_linux_deps
fi



# Derive the bundle version from Cargo.toml so the packaged app always matches
# the crate version (Info.plist CFBundleShortVersionString, DMG/zip/deb names).
# Reads the first `version = "..."` under `[package]`. Hard-coding it here was
# the cause of the release app always showing 0.1.0 regardless of Cargo.toml.
VERSION="$(awk '
    /^\[package\]/      { in_pkg = 1; next }
    /^\[/               { in_pkg = 0 }
    in_pkg && /^version[[:space:]]*=/ {
        sub(/^version[[:space:]]*=[[:space:]]*"/, "")
        sub(/".*/, "")
        print
        exit
    }
' "$PROJECT_DIR/Cargo.toml")"
if [ -z "$VERSION" ]; then
    echo "❌ Could not read version from Cargo.toml"
    exit 1
fi

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

    # ── Inject the custom Info.plist (registers Verve for .md / .markdown /
    #    .mdown / .mkd / .mkdn / .txt / .mdx file types). cargo-bundle 0.9.0
    #    does not emit CFBundleDocumentTypes from [package.metadata.bundle], so
    #    we overlay our own plist onto the bundle. Done BEFORE the ad-hoc
    #    re-sign below so the signature covers the updated plist. ───────────
    PLIST_SRC="$PROJECT_DIR/assets/macos/Info.plist"
    PLIST_DST="$APP_DIR/Contents/Info.plist"
    if [ -f "$PLIST_SRC" ]; then
        BUILD_TS=$(date +%Y%m%d.%H%M%S)
        sed "s/__VERSION__/$VERSION/g; s/__BUILD__/$BUILD_TS/g" "$PLIST_SRC" > "$PLIST_DST"
        if plutil -lint "$PLIST_DST" >/dev/null 2>&1; then
            echo "✅ Injected custom Info.plist (markdown document types registered)"
        else
            echo "⚠️  Injected Info.plist failed lint — keeping cargo-bundle default"
        fi
    else
        echo "⚠️  assets/macos/Info.plist not found — skipping document-type injection"
    fi

    # Re-sign the whole .app ad-hoc so the updated bundle contents (injected
    # plist) don't invalidate any existing signature.
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

    TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_DIR/target}"

    build_release ""

    # Build .deb
    if cargo install --list 2>/dev/null | grep -q cargo-deb; then
        echo "📦 Creating .deb package..."
        (cd "$PROJECT_DIR" && cargo deb --output "$TARGET_DIR/release/verve-$VERSION-amd64.deb") 2>/dev/null || {
            echo "⚠️  cargo-deb failed, skipping .deb"
        }
        [ -f "$TARGET_DIR/release/verve-$VERSION-amd64.deb" ] && echo "✅ .deb created"
    else
        echo "⚠️  cargo-deb not installed. Install with: cargo install cargo-deb"
    fi

    # Copy binary + icon + desktop entry for a portable tarball / manual install.
    mkdir -p "$TARGET_DIR/release/dist"
    cp "$TARGET_DIR/release/verve" "$TARGET_DIR/release/dist/"
    cp "$PROJECT_DIR/assets/icons/verve.png" "$TARGET_DIR/release/dist/"
    cp "$PROJECT_DIR/assets/verve.desktop" "$TARGET_DIR/release/dist/"
    # Include install script for convenience.
    cat > "$TARGET_DIR/release/dist/install.sh" <<'EOF'
#!/bin/sh
set -e
cd "$(dirname "$0")"
sudo install -m 755 verve /usr/bin/verve
sudo install -m 644 verve.png /usr/share/icons/hicolor/512x512/apps/verve.png
sudo install -m 644 verve.desktop /usr/share/applications/verve.desktop
echo "✅ Verve installed to /usr/bin/verve"
EOF
    chmod +x "$TARGET_DIR/release/dist/install.sh"

    # Portable tar.gz
    tar -C "$TARGET_DIR/release/dist" -czf "$TARGET_DIR/release/Verve-$VERSION-linux-x86_64.tar.gz" \
        verve verve.png verve.desktop install.sh 2>/dev/null || \
        echo "⚠️  tarball creation failed"

    # Mirror dist/ into project-local target/ if using custom target dir.
    if [ "$TARGET_DIR" != "$PROJECT_DIR/target" ]; then
        mkdir -p "$PROJECT_DIR/target/release"
        cp -R "$TARGET_DIR/release/dist" "$PROJECT_DIR/target/release/"
        [ -f "$TARGET_DIR/release/Verve-$VERSION-linux-x86_64.tar.gz" ] && \
            cp "$TARGET_DIR/release/Verve-$VERSION-linux-x86_64.tar.gz" "$PROJECT_DIR/target/release/"
        [ -f "$TARGET_DIR/release/verve-$VERSION-amd64.deb" ] && \
            cp "$TARGET_DIR/release/verve-$VERSION-amd64.deb" "$PROJECT_DIR/target/release/"
    fi

    # Build verve-server
    cargo build --release --bin verve-server
    if [ "$TARGET_DIR" != "$PROJECT_DIR/target" ] && [ -f "$TARGET_DIR/release/verve-server" ]; then
        cp "$TARGET_DIR/release/verve-server" "$PROJECT_DIR/target/release/verve-server"
    fi

    echo "✅ Linux build complete (target/release/dist/ + .deb + .tar.gz)"
    echo "   Install with: sudo bash target/release/dist/install.sh"
}

# ─── Windows ────────────────────────────────────────────────────────────────
build_windows() {
    echo "🪟 Building Windows packages..."

    TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_DIR/target}"

    # Pick the right target triple for native vs. cross-compile.
    # On non-Windows hosts we cross-compile with the GNU toolchain (MSVC
    # requires the Microsoft linker / Visual Studio build tools which are not
    # present on macOS/Linux).
    if [[ "$(uname -s)" == MINGW* ]] || [[ "$(uname -s)" == MSYS* ]] || [[ "$(uname -s)" == CYGWIN* ]]; then
        TRIPLE=""
        BINDIR="$TARGET_DIR/release"
    else
        echo "📦 Cross-compiling for Windows (x86_64-pc-windows-gnu)..."
        TRIPLE="x86_64-pc-windows-gnu"
        rustup target add x86_64-pc-windows-gnu 2>/dev/null || true
        # mingw-w64 is needed for linking; on macOS install with `brew install mingw-w64`.
        if ! command -v x86_64-w64-mingw32-gcc &>/dev/null; then
            echo "⚠️  x86_64-w64-mingw32-gcc not found."
            echo "   On macOS:  brew install mingw-w64"
            echo "   On Linux:  apt install gcc-mingw-w64-x86-64"
        fi
        BINDIR="$TARGET_DIR/x86_64-pc-windows-gnu/release"
    fi

    build_release "$TRIPLE"

    # Build .msi with cargo-wix (only when building natively on Windows with
    # the WiX toolchain installed; skip on cross-compile).
    if [ -z "$TRIPLE" ]; then
        if cargo install --list 2>/dev/null | grep -q cargo-wix; then
            echo "📦 Creating .msi installer..."
            (cd "$PROJECT_DIR" && cargo wix --no-build -o "$TARGET_DIR/release/verve-$VERSION.msi") 2>/dev/null || {
                echo "⚠️  cargo-wix failed, skipping .msi"
            }
        else
            echo "⚠️  cargo-wix not installed. Install with: cargo install cargo-wix"
        fi
    else
        echo "ℹ️  Skipping .msi (requires native Windows + WiX toolchain)."
    fi

    # Collect binary + icon into dist/ for a portable zip.
    mkdir -p "$TARGET_DIR/release/dist"
    if [ -f "$BINDIR/verve.exe" ]; then
        cp "$BINDIR/verve.exe" "$TARGET_DIR/release/dist/"
    else
        echo "❌ verve.exe not found at $BINDIR/verve.exe"
    fi
    cp "$PROJECT_DIR/assets/icons/verve.ico" "$TARGET_DIR/release/dist/"

    # Portable zip.
    (cd "$TARGET_DIR/release/dist" && zip -q "../Verve-$VERSION-windows-x86_64.zip" verve.exe verve.ico 2>/dev/null) || \
        echo "⚠️  zip creation failed (install zip to enable)"

    # Mirror back to project-local target/ if using a custom target dir.
    if [ "$TARGET_DIR" != "$PROJECT_DIR/target" ]; then
        mkdir -p "$PROJECT_DIR/target/release"
        rm -rf "$PROJECT_DIR/target/release/dist"
        cp -R "$TARGET_DIR/release/dist" "$PROJECT_DIR/target/release/"
        [ -f "$TARGET_DIR/release/Verve-$VERSION-windows-x86_64.zip" ] && \
            cp "$TARGET_DIR/release/Verve-$VERSION-windows-x86_64.zip" "$PROJECT_DIR/target/release/"
        [ -f "$TARGET_DIR/release/verve-$VERSION.msi" ] && \
            cp "$TARGET_DIR/release/verve-$VERSION.msi" "$PROJECT_DIR/target/release/"
    fi

    echo "✅ Windows build complete (target/release/dist/ + .zip)"
    echo "   Portable build: target/release/dist/"
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
    # Remove quarantine so Gatekeeper doesn't flag a local copy. Some macOS
    # versions don't support `xattr -r`; fall back to find + xattr -c.
    if xattr -c "$APP_DST" 2>/dev/null; then
        find "$APP_DST" -print0 2>/dev/null | xargs -0 xattr -c 2>/dev/null || true
    else
        find "$APP_DST" -print0 2>/dev/null | xargs -0 xattr -d com.apple.quarantine 2>/dev/null || true
    fi
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
