//! Build script: extract bundled Pdfium native library for the current platform
//! from pre-downloaded archives in assets/lib/ and link against it.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let lib_assets_dir = manifest_dir.join("assets").join("lib");

    // Select the correct archive for current platform
    let archive_name = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "pdfium-mac-arm64.tgz",
        ("macos", "x86_64") => "pdfium-mac-x64.tgz", // not bundled but handled
        ("windows", "x86_64") => "pdfium-win-x64.tgz",
        ("windows", "aarch64") => "pdfium-win-arm64.tgz",
        ("linux", "x86_64") => "pdfium-linux-x64.tgz",
        ("linux", "aarch64") => "pdfium-linux-arm64.tgz",
        (os, arch) => {
            println!("cargo:warning=No bundled Pdfium for {}/{}, PDF rendering will be unavailable", os, arch);
            return;
        }
    };

    let archive_path = lib_assets_dir.join(archive_name);
    if !archive_path.exists() {
        println!("cargo:warning=Pdfium archive not found at {}, PDF rendering will be unavailable", archive_path.display());
        return;
    }

    // Extract to OUT_DIR/pdfium
    let pdfium_out = out_dir.join("pdfium");
    let _ = fs::remove_dir_all(&pdfium_out);
    fs::create_dir_all(&pdfium_out).unwrap();

    let status = Command::new("tar")
        .arg("xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&pdfium_out)
        .status()
        .expect("Failed to extract pdfium archive");

    if !status.success() {
        panic!("Failed to extract pdfium archive");
    }

    // The library is in lib/ subdirectory inside the archive
    let lib_dir = pdfium_out.join("lib");

    // On macOS: fix the dylib's install name BEFORE linking so the binary
    // records `@rpath/libpdfium.dylib` (instead of the archive's default
    // `./libpdfium.dylib`, which resolves relative to CWD and breaks when the
    // binary is launched from a .app bundle or any other working directory).
    // Also strip quarantine and ad-hoc sign here so the dylib that gets linked
    // (and later copied into the bundle) is already prepared.
    #[cfg(target_os = "macos")]
    {
        let lib_file = lib_dir.join("libpdfium.dylib");
        if lib_file.exists() {
            // Rewrite the dylib's LC_ID_DYLIB to @rpath/libpdfium.dylib so the
            // linker embeds an @rpath reference in the binary.
            let _ = Command::new("install_name_tool")
                .arg("-id")
                .arg("@rpath/libpdfium.dylib")
                .arg(&lib_file)
                .status();
            // Remove quarantine attribute if present.
            let _ = Command::new("xattr")
                .arg("-d")
                .arg("com.apple.quarantine")
                .arg(&lib_file)
                .output();
            // Ad-hoc sign the library to satisfy Gatekeeper.
            let _ = Command::new("codesign")
                .arg("--force")
                .arg("--sign")
                .arg("-")
                .arg(&lib_file)
                .output();
        }
    }

    // Tell cargo where to find the library and link against it.
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=dylib=pdfium");
        // LC_RPATH = @loader_path so the dylib is found next to the binary
        // both in a plain `target/release/` layout and inside a .app bundle's
        // Contents/MacOS/.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/");
    }
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rustc-link-lib=dylib=pdfium");
    }
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-lib=dylib=pdfium");
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }

    // Copy library to OUT_DIR for linking.
    copy_libs(&lib_dir, &out_dir);

    // Copy the (already re-id'd and signed) dylib next to the binary in the
    // actual target profile directory so `cargo run` and direct binary
    // execution work. Honor CARGO_TARGET_DIR if set (custom target dir such as
    // /xdfapp/rust-target); fall back to the project-local target/.
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".to_string());
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
            manifest_dir.join("target")
        })
        .join(&profile);
    if target_dir.exists() {
        copy_libs(&lib_dir, &target_dir);

        #[cfg(target_os = "macos")]
        {
            let lib_file = target_dir.join("libpdfium.dylib");
            if lib_file.exists() {
                let _ = Command::new("xattr")
                    .arg("-d")
                    .arg("com.apple.quarantine")
                    .arg(&lib_file)
                    .output();
                let _ = Command::new("codesign")
                    .arg("--force")
                    .arg("--sign")
                    .arg("-")
                    .arg(&lib_file)
                    .output();
            }
        }
    }

    println!("cargo:warning=Using bundled Pdfium from {}", archive_path.display());
}

fn copy_libs(src: &PathBuf, dst: &PathBuf) {
    if let Ok(entries) = fs::read_dir(src) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                // Fix duplicate "dll" | "dll" pattern from original.
                if matches!(ext, "dylib" | "so" | "dll") {
                    let dest = dst.join(path.file_name().unwrap());
                    // Always overwrite so install-name/signature changes propagate.
                    let _ = fs::copy(&path, &dest);
                }
            }
        }
    }
}
