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

    // Tell cargo where to find the library and link against it
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=dylib=pdfium");
        // Also set rpath so it finds the dylib at runtime
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

    // Copy library to output directory for linking
    copy_libs(&lib_dir, &out_dir);

    // On macOS: ad-hoc sign the extracted dylib and remove quarantine attribute
    // so Gatekeeper doesn't block loading it at runtime.
    #[cfg(target_os = "macos")]
    {
        let lib_file = lib_dir.join("libpdfium.dylib");
        if lib_file.exists() {
            // Remove quarantine attribute if present
            let _ = std::process::Command::new("xattr")
                .arg("-d")
                .arg("com.apple.quarantine")
                .arg(&lib_file)
                .output();
            // Ad-hoc sign the library to satisfy Gatekeeper
            let _ = std::process::Command::new("codesign")
                .arg("--force")
                .arg("--sign")
                .arg("-")
                .arg(&lib_file)
                .output();
        }
    }

    // For development: copy the lib to target/debug/ and target/release/
    // so it's adjacent to the binary. Also apply signing there.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    for profile in ["debug", "release"] {
        let target_dir = manifest_dir.join("target").join(profile);
        if target_dir.exists() {
            copy_libs(&lib_dir, &target_dir);

            #[cfg(target_os = "macos")]
            {
                let lib_file = target_dir.join("libpdfium.dylib");
                if lib_file.exists() {
                    let _ = std::process::Command::new("xattr")
                        .arg("-d")
                        .arg("com.apple.quarantine")
                        .arg(&lib_file)
                        .output();
                    let _ = std::process::Command::new("codesign")
                        .arg("--force")
                        .arg("--sign")
                        .arg("-")
                        .arg(&lib_file)
                        .output();
                }
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
                if matches!(ext, "dylib" | "so" | "dll" | "dll") {
                    let dest = dst.join(path.file_name().unwrap());
                    if !dest.exists() {
                        let _ = fs::copy(&path, &dest);
                    }
                }
            }
        }
    }
}
