//! Copies engine/worker binaries into `binaries/{stem}-{target-triple}` for Tauri `externalBin`.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Process stems copied into `binaries/` for Tauri `externalBin`.
const SIDECAR_STEMS: &[&str] = &[
    "aifs-engine",
    "aifs-worker-media",
    "aifs-worker-document",
    "aifs-worker-vision",
    "aifs-worker-llm",
];

fn main() {
    copy_sidecars();
    tauri_build::build();
}

/// Copies `target/{profile}/{stem}` to `binaries/{stem}-{triple}`.
fn copy_sidecars() {
    let triple = env::var("TARGET").unwrap_or_default();
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_owned());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned()));
    let workspace = manifest.join("../../..");
    let src_dir = workspace.join("target").join(&profile);
    let dest_dir = manifest.join("binaries");
    let ext = if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
        ".exe"
    } else {
        ""
    };
    let release = profile == "release";
    let _ = fs::create_dir_all(&dest_dir);
    println!("cargo:rerun-if-changed={}", src_dir.display());
    for stem in SIDECAR_STEMS {
        let src = src_dir.join(format!("{stem}{ext}"));
        println!("cargo:rerun-if-changed={}", src.display());
        if !src.is_file() {
            if release {
                panic!(
                    "missing sidecar {} — run `cargo engine-bins --release` before bundling",
                    src.display()
                );
            }
            continue;
        }
        if triple.is_empty() {
            panic!("TARGET is unset; cannot name sidecar {stem}");
        }
        let dest = dest_dir.join(format!("{stem}-{triple}{ext}"));
        if let Err(error) = fs::copy(&src, &dest) {
            panic!("copy {} → {}: {error}", src.display(), dest.display());
        }
    }
}
