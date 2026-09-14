//! Copies engine/extract-worker binaries into `binaries/{stem}-{target-triple}` for Tauri `externalBin`.

use std::env;
use std::fs;

include!("sidecar_copy.rs");

/// Process stems copied into `binaries/` for Tauri `externalBin`.
///
/// The LLM worker is not a sidecar: a flat `aifs-worker-llm` next to the engine
/// would collide with nested `llm-runtime/<accel>/` payloads. It is staged into
/// those payload directories instead.
const SIDECAR_STEMS: &[&str] = &[
    "aifs-engine",
    "aifs-worker-media",
    "aifs-worker-document",
    "aifs-worker-vision",
];

fn main() {
    copy_sidecars();
    tauri_build::build();
}

/// Copies `target/{profile}/{stem}` to `binaries/{stem}-{triple}`.
///
/// After a real `aifs-worker-llm` is present in the profile directory, that
/// worker + ggml/llama/CUDA runtime libraries are snapshotted into
/// `llm-runtime/<accel>/` under the Cargo profile and `resources/` without
/// deleting sibling accelerators and without copying the LLM worker into
/// `binaries/`. Debug builds write empty placeholders for remaining sidecars
/// when the real binaries are missing so `tauri_build` can compile
/// `aifs-desktop` during clippy/test. Release panics.
/// Copies are skipped when the destination already matches so `tauri dev` does
/// not see a sidecar mtime change and rebuild forever.
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
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rerun-if-env-changed=AIFS_LLM_FEATURES");
    let _ = fs::create_dir_all(&dest_dir);
    if triple.is_empty() {
        panic!("TARGET is unset; cannot name sidecar files");
    }
    // Watch each sidecar source, not the whole target profile directory: compiling
    // aifs-desktop writes into target/{profile} and would retrigger this script.
    for stem in SIDECAR_STEMS {
        let src = src_dir.join(format!("{stem}{ext}"));
        let dest = dest_dir.join(format!("{stem}-{triple}{ext}"));
        println!("cargo:rerun-if-changed={}", src.display());
        if src.is_file() {
            if let Err(error) = copy_if_changed(&src, &dest) {
                panic!("copy {} → {}: {error}", src.display(), dest.display());
            }
            continue;
        }
        if release {
            panic!(
                "missing sidecar {} — run `cargo engine-bins --release` before bundling",
                src.display()
            );
        }
        // tauri_build requires externalBin paths to exist even for clippy/test.
        if let Err(error) = write_if_changed(&dest, &[]) {
            panic!("placeholder {} : {error}", dest.display());
        }
    }
    stage_llm_payloads(&src_dir, &manifest, release);
}

fn stage_llm_payloads(src_dir: &Path, manifest: &Path, release: bool) {
    let worker_src = src_dir.join(format!(
        "aifs-worker-llm{}",
        if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
            ".exe"
        } else {
            ""
        }
    ));
    println!("cargo:rerun-if-changed={}", worker_src.display());
    if first_process_binary(src_dir, "aifs-worker-llm").is_none() {
        if release {
            panic!(
                "missing {} — run `cargo engine-llm --release` before bundling so llm-runtime payloads can be staged",
                worker_src.display()
            );
        }
        return;
    }
    for runtime_root in [src_dir, &manifest.join("resources")] {
        if let Err(error) = stage_llm_payload(src_dir, runtime_root) {
            panic!(
                "stage llm-runtime payload under {}: {error}",
                runtime_root.display()
            );
        }
    }
}
