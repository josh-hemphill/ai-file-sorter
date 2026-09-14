// Copy sidecar binaries only when contents change so `tauri dev` does not loop.

use aifs_protocol::worker::WorkerKind;
use aifs_protocol::{
    LlmAccel, binary_imports_cuda_runtime, ensure_process_binary_executable, first_process_binary,
    llm_payload_dir, runtime_lib_matches_prefix,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Native library filename prefixes that `aifs-worker-llm` loads at process start.
const WORKER_RUNTIME_LIB_PREFIXES: &[&str] = &[
    "ggml",
    "llama",
    "mtmd",
    "cudart",
    "cublas",
    "nvrtc",
    "nvjitlink",
    "vulkan",
];

/// Copies `src` to `dest` when the destination is missing or differs. Returns true if written.
fn copy_if_changed(src: &Path, dest: &Path) -> std::io::Result<bool> {
    if dest.is_file() && files_have_same_contents(src, dest) {
        return Ok(false);
    }
    std::fs::copy(src, dest)?;
    Ok(true)
}

/// Writes `contents` when the destination is missing or differs. Returns true if written.
fn write_if_changed(dest: &Path, contents: &[u8]) -> std::io::Result<bool> {
    if dest.is_file()
        && let Ok(existing) = std::fs::read(dest)
        && existing == contents
    {
        return Ok(false);
    }
    std::fs::write(dest, contents)?;
    Ok(true)
}

fn files_have_same_contents(src: &Path, dest: &Path) -> bool {
    let Ok(src_meta) = std::fs::metadata(src) else {
        return false;
    };
    let Ok(dest_meta) = std::fs::metadata(dest) else {
        return false;
    };
    if src_meta.len() != dest_meta.len() {
        return false;
    }
    let Ok(mut src_file) = std::fs::File::open(src) else {
        return false;
    };
    let Ok(mut dest_file) = std::fs::File::open(dest) else {
        return false;
    };
    let mut src_buf = [0_u8; 8192];
    let mut dest_buf = [0_u8; 8192];
    loop {
        let Ok(src_n) = std::io::Read::read(&mut src_file, &mut src_buf) else {
            return false;
        };
        let Ok(dest_n) = std::io::Read::read(&mut dest_file, &mut dest_buf) else {
            return false;
        };
        if src_n != dest_n || src_buf[..src_n] != dest_buf[..dest_n] {
            return false;
        }
        if src_n == 0 {
            return true;
        }
    }
}

fn is_native_lib_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".dll")
        || lower.ends_with(".dylib")
        || lower.ends_with(".so")
        || lower.contains(".so.")
}

fn runtime_lib_stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let file = lower.rsplit(['/', '\\']).next().unwrap_or(lower.as_str());
    let file = file.strip_prefix("lib").unwrap_or(file);
    if let Some(stem) = file.strip_suffix(".dll") {
        return stem.to_owned();
    }
    if let Some(stem) = file.strip_suffix(".dylib") {
        return stem.to_owned();
    }
    if let Some(idx) = file.find(".so") {
        return file[..idx].to_owned();
    }
    file.to_owned()
}

/// True when `name` is a ggml/llama/CUDA runtime library the LLM sidecar needs beside itself.
fn is_worker_runtime_lib(name: &str) -> bool {
    if !is_native_lib_name(name) {
        return false;
    }
    let stem = runtime_lib_stem(name);
    WORKER_RUNTIME_LIB_PREFIXES
        .iter()
        .any(|prefix| stem.starts_with(prefix))
}

fn collect_runtime_libs_from(
    dir: &Path,
    files: &mut HashMap<String, PathBuf>,
) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_worker_runtime_lib(name) {
            continue;
        }
        let src = entry.path();
        if src.is_file() {
            files.insert(name.to_owned(), src);
        }
    }
    Ok(())
}

/// MSVC cmake-rs nests `ggml-cuda.dll` under `out/bin/Release` (and similar).
const LLAMA_OUT_MAX_DEPTH: usize = 6;

/// llama-cpp-sys-2 often leaves ggml-cuda in `build/llama-cpp-*/out`, not `deps/`.
fn collect_runtime_libs_from_llama_build_out(
    target_dir: &Path,
    files: &mut HashMap<String, PathBuf>,
) -> std::io::Result<()> {
    for dir in llama_build_out_dirs(target_dir) {
        collect_runtime_libs_nested(&dir, files, 0)?;
    }
    Ok(())
}

fn skip_llama_out_dir(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "cmakefiles" | "cmaketmp" | ".git" | "compileridcuda" | "compileridcxx" | "compileridc"
    )
}

fn collect_runtime_libs_nested(
    dir: &Path,
    files: &mut HashMap<String, PathBuf>,
    depth: usize,
) -> std::io::Result<()> {
    if depth > LLAMA_OUT_MAX_DEPTH || !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let src = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if skip_llama_out_dir(name) {
                continue;
            }
            collect_runtime_libs_nested(&src, files, depth + 1)?;
            continue;
        }
        if src.is_file() && is_worker_runtime_lib(name) {
            files.insert(name.to_owned(), src);
        }
    }
    Ok(())
}

fn llama_build_out_dirs(target_dir: &Path) -> Vec<PathBuf> {
    let build = target_dir.join("build");
    let Ok(entries) = std::fs::read_dir(&build) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.to_ascii_lowercase().starts_with("llama-cpp"))
                && entry.path().is_dir()
        })
        .map(|entry| entry.path().join("out"))
        .collect()
}

fn cuda_toolkit_lib_dirs(cuda_root: &Path) -> [PathBuf; 2] {
    [cuda_root.join("bin"), cuda_root.join("bin").join("x64")]
}

fn collect_cuda_toolkit_libs(
    cuda_root: &Path,
    files: &mut HashMap<String, PathBuf>,
) -> std::io::Result<()> {
    for dir in cuda_toolkit_lib_dirs(cuda_root) {
        collect_runtime_libs_from(&dir, files)?;
    }
    Ok(())
}

fn llama_out_has_static_plugin(src_dir: &Path, stem: &str) -> bool {
    let names = [format!("{stem}.lib"), format!("lib{stem}.a")];
    llama_build_out_dirs(src_dir)
        .iter()
        .any(|dir| nested_has_named_file(dir, &names, 0))
}

fn nested_has_named_file(dir: &Path, names: &[String], depth: usize) -> bool {
    if depth > LLAMA_OUT_MAX_DEPTH || !dir.is_dir() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if entry.path().is_dir() {
            if skip_llama_out_dir(name) {
                continue;
            }
            if nested_has_named_file(&entry.path(), names, depth + 1) {
                return true;
            }
            continue;
        }
        if names.iter().any(|want| name.eq_ignore_ascii_case(want)) {
            return true;
        }
    }
    false
}

fn collect_src_runtime_libs(
    src_dir: &Path,
    cuda_root: Option<&Path>,
    allow_cuda_toolkit: bool,
) -> std::io::Result<HashMap<String, PathBuf>> {
    let mut files = HashMap::new();
    collect_runtime_libs_from(src_dir, &mut files)?;
    collect_runtime_libs_from(&src_dir.join("deps"), &mut files)?;
    collect_runtime_libs_from_llama_build_out(src_dir, &mut files)?;
    if allow_cuda_toolkit
        && files
            .keys()
            .any(|name| runtime_lib_matches_prefix(name, "ggml-cuda"))
        && let Some(cuda_root) = cuda_root
    {
        collect_cuda_toolkit_libs(cuda_root, &mut files)?;
    }
    Ok(files)
}

fn remove_stale_runtime_libs(
    dest_dir: &Path,
    keep: &HashMap<String, PathBuf>,
) -> std::io::Result<()> {
    if !dest_dir.is_dir() {
        return Ok(());
    }
    let mut stale = Vec::new();
    for entry in std::fs::read_dir(dest_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_worker_runtime_lib(name) && !keep.contains_key(name) && entry.path().is_file() {
            stale.push(entry.path());
        }
    }
    for path in stale {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn write_runtime_libs(dest_dir: &Path, files: &HashMap<String, PathBuf>) -> std::io::Result<()> {
    std::fs::create_dir_all(dest_dir)?;
    for (name, src) in files {
        copy_if_changed(src, &dest_dir.join(name))?;
    }
    remove_stale_runtime_libs(dest_dir, files)?;
    Ok(())
}

/// Copies llama.cpp / CUDA runtime libs from `src_dir` (and `deps`) next to a dest dir.
#[cfg_attr(not(test), allow(dead_code))]
fn copy_worker_runtime_libs_from(
    src_dir: &Path,
    dest_dir: &Path,
    cuda_root: Option<&Path>,
) -> std::io::Result<()> {
    let files = collect_src_runtime_libs(src_dir, cuda_root, true)?;
    write_runtime_libs(dest_dir, &files)
}

/// Snapshots worker + runtime libs into `runtime_root/llm-runtime/<accel>/`.
#[cfg_attr(test, allow(dead_code))]
fn stage_llm_payload(src_dir: &Path, runtime_root: &Path) -> std::io::Result<LlmAccel> {
    let cuda_root = std::env::var_os("CUDA_PATH").map(PathBuf::from);
    stage_llm_payload_from_expected(
        src_dir,
        runtime_root,
        cuda_root.as_deref(),
        expected_accel_from_env(),
    )
}

/// Infers `<accel>` from `src_dir` (and `deps`) and writes that payload only.
#[cfg_attr(not(test), allow(dead_code))]
fn stage_llm_payload_from(
    src_dir: &Path,
    runtime_root: &Path,
    cuda_root: Option<&Path>,
) -> std::io::Result<LlmAccel> {
    stage_llm_payload_from_expected(src_dir, runtime_root, cuda_root, None)
}

fn stage_llm_payload_from_expected(
    src_dir: &Path,
    runtime_root: &Path,
    cuda_root: Option<&Path>,
    expected: Option<LlmAccel>,
) -> std::io::Result<LlmAccel> {
    let mut files = HashMap::new();
    collect_runtime_libs_from(src_dir, &mut files)?;
    collect_runtime_libs_from(&src_dir.join("deps"), &mut files)?;
    collect_runtime_libs_from_llama_build_out(src_dir, &mut files)?;
    let worker = first_process_binary(src_dir, WorkerKind::Llm.binary_stem());
    let mut accel = infer_staging_accel_from_lib_names(files.keys().map(String::as_str));
    if accel == LlmAccel::Cpu
        && (llama_out_has_static_plugin(src_dir, "ggml-cuda")
            || worker
                .as_ref()
                .is_some_and(|path| binary_imports_cuda_runtime(path)))
    {
        accel = LlmAccel::Cuda;
    }
    if accel == LlmAccel::Cpu && llama_out_has_static_plugin(src_dir, "ggml-vulkan") {
        accel = LlmAccel::Vulkan;
    }
    if expected == Some(LlmAccel::Cuda) && accel == LlmAccel::Cpu {
        accel = LlmAccel::Cuda;
    }
    if accel == LlmAccel::Cuda
        && let Some(cuda_root) = cuda_root
    {
        collect_cuda_toolkit_libs(cuda_root, &mut files)?;
    }
    assert_expected_staging_accel(expected, accel, &files, src_dir)?;
    let dest_dir = llm_payload_dir(runtime_root, accel);
    if let Some(src_bin) = first_process_binary(src_dir, WorkerKind::Llm.binary_stem()) {
        std::fs::create_dir_all(&dest_dir)?;
        let dest_bin = dest_dir.join(
            src_bin
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new(WorkerKind::Llm.binary_stem())),
        );
        copy_if_changed(&src_bin, &dest_bin)?;
        ensure_process_binary_executable(&dest_bin)?;
    }
    write_runtime_libs(&dest_dir, &files)?;
    Ok(accel)
}

/// CUDA / Vulkan plugins in collected filenames win over a CPU-only payload dir.
fn infer_staging_accel_from_lib_names<'a>(names: impl IntoIterator<Item = &'a str>) -> LlmAccel {
    let names: Vec<&str> = names.into_iter().collect();
    if names
        .iter()
        .any(|name| runtime_lib_matches_prefix(name, "ggml-cuda"))
    {
        return LlmAccel::Cuda;
    }
    if names
        .iter()
        .any(|name| runtime_lib_matches_prefix(name, "ggml-vulkan"))
    {
        return LlmAccel::Vulkan;
    }
    LlmAccel::Cpu
}

#[cfg_attr(test, allow(dead_code))]
fn expected_accel_from_env() -> Option<LlmAccel> {
    parse_expected_accel(&std::env::var("AIFS_LLM_FEATURES").ok()?)
}

fn parse_expected_accel(raw: &str) -> Option<LlmAccel> {
    let mut cuda = false;
    let mut vulkan = false;
    for token in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        match token.trim().to_ascii_lowercase().as_str() {
            "cuda" => cuda = true,
            "vulkan" | "vulcan" => vulkan = true,
            _ => {}
        }
    }
    match (cuda, vulkan) {
        (true, false) => Some(LlmAccel::Cuda),
        (false, true) => Some(LlmAccel::Vulkan),
        _ => None,
    }
}

fn assert_expected_staging_accel(
    expected: Option<LlmAccel>,
    actual: LlmAccel,
    files: &HashMap<String, PathBuf>,
    src_dir: &Path,
) -> std::io::Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if expected == LlmAccel::Cuda {
        let has_plugin = files
            .keys()
            .any(|name| runtime_lib_matches_prefix(name, "ggml-cuda"));
        let has_toolkit = files.keys().any(|name| {
            runtime_lib_matches_prefix(name, "cublas") || runtime_lib_matches_prefix(name, "cudart")
        });
        if has_plugin || has_toolkit {
            return Ok(());
        }
        let mut names: Vec<&str> = files.keys().map(String::as_str).collect();
        names.sort_unstable();
        let found = if names.is_empty() {
            "(none)".to_owned()
        } else {
            names.join(", ")
        };
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "aifs: AIFS_LLM_FEATURES=cuda but missing ggml-cuda.dll and CUDA toolkit cublas/cudart. Found: {found}. Searched {}, deps/, nested build/llama-cpp-*/out, and CUDA_PATH/bin plus bin/x64 (CUDA 13). llama-cpp-sys-2 on MSVC is often static (.lib only); the worker still needs cublas64_*.dll beside it.",
                src_dir.display()
            ),
        ));
    }
    let plugin = match expected {
        LlmAccel::Cuda => "ggml-cuda",
        LlmAccel::Vulkan => "ggml-vulkan",
        LlmAccel::Cpu | LlmAccel::Metal => return Ok(()),
    };
    if expected == actual {
        return Ok(());
    }
    let mut names: Vec<&str> = files.keys().map(String::as_str).collect();
    names.sort_unstable();
    let found = if names.is_empty() {
        "(none)".to_owned()
    } else {
        names.join(", ")
    };
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "aifs: AIFS_LLM_FEATURES={} but inferred {} (missing {plugin}). Found: {found}. Searched {}, deps/, and nested build/llama-cpp-*/out.",
            expected.as_str(),
            actual.as_str(),
            src_dir.display()
        ),
    ))
}

#[cfg(test)]
mod sidecar_copy_tests {
    use super::{
        copy_if_changed, copy_worker_runtime_libs_from, files_have_same_contents,
        is_worker_runtime_lib, parse_expected_accel, stage_llm_payload_from,
        stage_llm_payload_from_expected, write_if_changed,
    };
    use aifs_protocol::LlmAccel;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aifs-sidecar-copy-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
        dir
    }

    #[test]
    fn copy_if_changed_skips_identical_bytes() {
        let dir = temp_dir();
        let src = dir.join("src.bin");
        let dest = dir.join("dest.bin");
        fs::write(&src, b"sidecar").unwrap_or_else(|error| panic!("{error}"));
        fs::write(&dest, b"sidecar").unwrap_or_else(|error| panic!("{error}"));
        let before = fs::metadata(&dest)
            .unwrap_or_else(|error| panic!("{error}"))
            .modified()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!copy_if_changed(&src, &dest).unwrap_or_else(|error| panic!("{error}")));
        let after = fs::metadata(&dest)
            .unwrap_or_else(|error| panic!("{error}"))
            .modified()
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(before, after);
        fs::remove_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn copy_if_changed_replaces_different_bytes() {
        let dir = temp_dir();
        let src = dir.join("src.bin");
        let dest = dir.join("dest.bin");
        fs::write(&src, b"new").unwrap_or_else(|error| panic!("{error}"));
        fs::write(&dest, b"old").unwrap_or_else(|error| panic!("{error}"));
        assert!(copy_if_changed(&src, &dest).unwrap_or_else(|error| panic!("{error}")));
        assert_eq!(
            fs::read(&dest).unwrap_or_else(|error| panic!("{error}")),
            b"new"
        );
        fs::remove_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn write_if_changed_skips_matching_placeholder() {
        let dir = temp_dir();
        let dest = dir.join("empty.bin");
        fs::write(&dest, []).unwrap_or_else(|error| panic!("{error}"));
        assert!(!write_if_changed(&dest, &[]).unwrap_or_else(|error| panic!("{error}")));
        assert!(files_have_same_contents(&dest, &dest));
        fs::remove_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn worker_runtime_lib_names_match_ggml_llama_cuda() {
        for name in [
            "ggml.dll",
            "ggml-cuda.dll",
            "ggml-vulkan.dll",
            "ggml-metal.dylib",
            "llama.dll",
            "mtmd.dll",
            "libmtmd.so",
            "libggml.so",
            "libggml.so.0",
            "libllama.dylib",
            "cudart64_12.dll",
            "cublasLt64_12.dll",
            "nvrtc64_120_0.dll",
            "nvJitLink_120_0.dll",
            "libcudart.so.12",
            "vulkan-1.dll",
            "libvulkan.so.1",
        ] {
            assert!(is_worker_runtime_lib(name), "{name}");
        }
        for name in [
            "nvcuda.dll",
            "aifs-worker-llm.exe",
            "aifs-worker-llm",
            "readme.txt",
            "msvcp140.dll",
            "libcuda.so.1",
        ] {
            assert!(!is_worker_runtime_lib(name), "{name}");
        }
    }

    #[test]
    fn copy_worker_runtime_libs_copies_ggml_from_src_and_deps() {
        let root = temp_dir();
        let src = root.join("src");
        let deps = src.join("deps");
        let dest = root.join("dest");
        fs::create_dir_all(&deps).unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("ggml.dll"), b"ggml").unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("readme.txt"), b"skip").unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("aifs-worker-llm.exe"), b"exe")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(deps.join("ggml-cpu.dll"), b"cpu").unwrap_or_else(|error| panic!("{error}"));
        copy_worker_runtime_libs_from(&src, &dest, None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            fs::read(dest.join("ggml.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"ggml"
        );
        assert_eq!(
            fs::read(dest.join("ggml-cpu.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"cpu"
        );
        assert!(!dest.join("readme.txt").exists());
        assert!(!dest.join("aifs-worker-llm.exe").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn copy_worker_runtime_libs_skips_cuda_toolkit_without_ggml_cuda() {
        let root = temp_dir();
        let src = root.join("src");
        let dest = root.join("dest");
        let cuda = root.join("cuda");
        fs::create_dir_all(&src).unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(cuda.join("bin")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("ggml.dll"), b"cpu").unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda.join("bin").join("cudart64_12.dll"), b"cudart")
            .unwrap_or_else(|error| panic!("{error}"));
        copy_worker_runtime_libs_from(&src, &dest, Some(&cuda))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(dest.join("ggml.dll").is_file());
        assert!(!dest.join("cudart64_12.dll").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn copy_worker_runtime_libs_pulls_cudart_when_ggml_cuda_is_present() {
        let root = temp_dir();
        let src = root.join("src");
        let dest = root.join("dest");
        let cuda = root.join("cuda");
        fs::create_dir_all(&src).unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(cuda.join("bin")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("ggml-cuda.dll"), b"cuda").unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda.join("bin").join("cudart64_12.dll"), b"cudart")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda.join("bin").join("nvcuda.dll"), b"driver")
            .unwrap_or_else(|error| panic!("{error}"));
        copy_worker_runtime_libs_from(&src, &dest, Some(&cuda))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            fs::read(dest.join("cudart64_12.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"cudart"
        );
        assert!(!dest.join("nvcuda.dll").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn copy_worker_runtime_libs_removes_stale_cuda_backend_after_cpu_rebuild() {
        let root = temp_dir();
        let src = root.join("src");
        let dest = root.join("dest");
        let cuda = root.join("cuda");
        fs::create_dir_all(&src).unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(&dest).unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(cuda.join("bin")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("ggml.dll"), b"cpu").unwrap_or_else(|error| panic!("{error}"));
        fs::write(dest.join("ggml-cuda.dll"), b"stale").unwrap_or_else(|error| panic!("{error}"));
        fs::write(dest.join("cudart64_12.dll"), b"stale-rt")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(dest.join("aifs-worker-llm.exe"), b"sidecar")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda.join("bin").join("cudart64_12.dll"), b"cudart")
            .unwrap_or_else(|error| panic!("{error}"));
        copy_worker_runtime_libs_from(&src, &dest, Some(&cuda))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            fs::read(dest.join("ggml.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"cpu"
        );
        assert!(!dest.join("ggml-cuda.dll").exists());
        assert!(!dest.join("cudart64_12.dll").exists());
        assert_eq!(
            fs::read(dest.join("aifs-worker-llm.exe")).unwrap_or_else(|error| panic!("{error}")),
            b"sidecar"
        );
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    fn write_cpu_payload_src(src: &Path) {
        fs::create_dir_all(src).unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("aifs-worker-llm"), b"worker").unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("llama.dll"), b"llama").unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("ggml.dll"), b"ggml").unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn stage_cpu_payload_leaves_sibling_cuda_dir() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        let cuda_dest = runtime.join("llm-runtime").join("cuda");
        write_cpu_payload_src(&src);
        fs::create_dir_all(&cuda_dest).unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda_dest.join("ggml-cuda.dll"), b"keep")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda_dest.join("aifs-worker-llm"), b"cuda-worker")
            .unwrap_or_else(|error| panic!("{error}"));
        let accel =
            stage_llm_payload_from(&src, &runtime, None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(accel, LlmAccel::Cpu);
        let cpu_dest = runtime.join("llm-runtime").join("cpu");
        assert_eq!(
            fs::read(cpu_dest.join("aifs-worker-llm")).unwrap_or_else(|error| panic!("{error}")),
            b"worker"
        );
        assert_eq!(
            fs::read(cpu_dest.join("llama.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"llama"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(cpu_dest.join("aifs-worker-llm"))
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions()
                .mode();
            assert_ne!(
                mode & 0o111,
                0,
                "staged payload worker must be executable: {mode:#o}"
            );
        }
        assert!(!cpu_dest.join("ggml-cuda.dll").exists());
        assert_eq!(
            fs::read(cuda_dest.join("ggml-cuda.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"keep"
        );
        assert_eq!(
            fs::read(cuda_dest.join("aifs-worker-llm")).unwrap_or_else(|error| panic!("{error}")),
            b"cuda-worker"
        );
        assert!(!runtime.join("llm-runtime").join("llama.dll").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn stage_cuda_payload_lands_under_cuda_and_pulls_cudart() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        let cuda = root.join("toolkit");
        write_cpu_payload_src(&src);
        fs::write(src.join("ggml-cuda.dll"), b"cuda").unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(cuda.join("bin")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda.join("bin").join("cudart64_12.dll"), b"cudart")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda.join("bin").join("nvcuda.dll"), b"driver")
            .unwrap_or_else(|error| panic!("{error}"));
        let accel = stage_llm_payload_from(&src, &runtime, Some(&cuda))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(accel, LlmAccel::Cuda);
        let dest = runtime.join("llm-runtime").join("cuda");
        assert_eq!(
            fs::read(dest.join("ggml-cuda.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"cuda"
        );
        assert_eq!(
            fs::read(dest.join("cudart64_12.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"cudart"
        );
        assert_eq!(
            fs::read(dest.join("aifs-worker-llm")).unwrap_or_else(|error| panic!("{error}")),
            b"worker"
        );
        assert!(!dest.join("nvcuda.dll").exists());
        assert!(!runtime.join("llm-runtime").join("cpu").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn stage_cuda_plugin_in_deps_still_lands_under_cuda() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        write_cpu_payload_src(&src);
        fs::create_dir_all(src.join("deps")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("deps").join("ggml-cuda.dll"), b"cuda")
            .unwrap_or_else(|error| panic!("{error}"));
        let accel =
            stage_llm_payload_from(&src, &runtime, None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(accel, LlmAccel::Cuda);
        let dest = runtime.join("llm-runtime").join("cuda");
        assert_eq!(
            fs::read(dest.join("ggml-cuda.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"cuda"
        );
        assert!(!runtime.join("llm-runtime").join("cpu").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn stage_cuda_plugin_in_llama_cpp_out_still_lands_under_cuda() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        write_cpu_payload_src(&src);
        let out = src
            .join("build")
            .join("llama-cpp-sys-2-deadbeef")
            .join("out");
        fs::create_dir_all(&out).unwrap_or_else(|error| panic!("{error}"));
        fs::write(out.join("ggml-cuda.dll"), b"cuda").unwrap_or_else(|error| panic!("{error}"));
        let accel =
            stage_llm_payload_from(&src, &runtime, None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(accel, LlmAccel::Cuda);
        let dest = runtime.join("llm-runtime").join("cuda");
        assert_eq!(
            fs::read(dest.join("ggml-cuda.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"cuda"
        );
        assert!(!runtime.join("llm-runtime").join("cpu").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn stage_cuda_plugin_in_nested_msvc_out_still_lands_under_cuda() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        write_cpu_payload_src(&src);
        let release = src
            .join("build")
            .join("llama-cpp-sys-2-deadbeef")
            .join("out")
            .join("build")
            .join("bin")
            .join("Release");
        fs::create_dir_all(&release).unwrap_or_else(|error| panic!("{error}"));
        fs::write(release.join("llama.dll"), b"nested-llama")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(release.join("ggml.dll"), b"nested-ggml")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(release.join("ggml-cuda.dll"), b"nested-cuda")
            .unwrap_or_else(|error| panic!("{error}"));
        let cmake_files = src
            .join("build")
            .join("llama-cpp-sys-2-deadbeef")
            .join("out")
            .join("CMakeFiles");
        fs::create_dir_all(&cmake_files).unwrap_or_else(|error| panic!("{error}"));
        fs::write(cmake_files.join("ggml-vulkan.dll"), b"skip")
            .unwrap_or_else(|error| panic!("{error}"));
        let accel =
            stage_llm_payload_from(&src, &runtime, None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(accel, LlmAccel::Cuda);
        let dest = runtime.join("llm-runtime").join("cuda");
        assert_eq!(
            fs::read(dest.join("ggml-cuda.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"nested-cuda"
        );
        assert!(!dest.join("ggml-vulkan.dll").exists());
        assert!(!runtime.join("llm-runtime").join("cpu").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn expected_cuda_without_plugin_does_not_stage_cpu() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        write_cpu_payload_src(&src);
        let error = stage_llm_payload_from_expected(&src, &runtime, None, Some(LlmAccel::Cuda))
            .expect_err("cuda was requested");
        assert!(error.to_string().contains("missing ggml-cuda"), "{error}");
        assert!(!runtime.join("llm-runtime").join("cpu").exists());
        assert!(!runtime.join("llm-runtime").join("cuda").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn stage_static_cuda_pulls_cublas_from_cuda13_bin_x64() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        let cuda = root.join("toolkit");
        fs::create_dir_all(&src).unwrap_or_else(|error| panic!("{error}"));
        fs::write(src.join("aifs-worker-llm.exe"), b"MZ\0cublas64_13.dll\0")
            .unwrap_or_else(|error| panic!("{error}"));
        let lib_dir = src
            .join("build")
            .join("llama-cpp-sys-2-deadbeef")
            .join("out")
            .join("lib");
        fs::create_dir_all(&lib_dir).unwrap_or_else(|error| panic!("{error}"));
        fs::write(lib_dir.join("ggml-cuda.lib"), b"static")
            .unwrap_or_else(|error| panic!("{error}"));
        let x64 = cuda.join("bin").join("x64");
        fs::create_dir_all(&x64).unwrap_or_else(|error| panic!("{error}"));
        fs::write(x64.join("cublas64_13.dll"), b"cublas").unwrap_or_else(|error| panic!("{error}"));
        fs::write(x64.join("cublasLt64_13.dll"), b"lt").unwrap_or_else(|error| panic!("{error}"));
        fs::write(x64.join("nvcuda.dll"), b"driver").unwrap_or_else(|error| panic!("{error}"));
        let accel = stage_llm_payload_from(&src, &runtime, Some(&cuda))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(accel, LlmAccel::Cuda);
        let dest = runtime.join("llm-runtime").join("cuda");
        assert_eq!(
            fs::read(dest.join("cublas64_13.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"cublas"
        );
        assert_eq!(
            fs::read(dest.join("cublasLt64_13.dll")).unwrap_or_else(|error| panic!("{error}")),
            b"lt"
        );
        assert!(!dest.join("nvcuda.dll").exists());
        assert!(!dest.join("ggml-cuda.lib").exists());
        assert!(!runtime.join("llm-runtime").join("cpu").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn parse_expected_accel_reads_aifs_llm_features() {
        assert_eq!(parse_expected_accel("cuda"), Some(LlmAccel::Cuda));
        assert_eq!(parse_expected_accel("vulcan"), Some(LlmAccel::Vulkan));
        assert_eq!(parse_expected_accel("cuda,vulkan"), None);
        assert_eq!(parse_expected_accel("llama"), None);
        assert_eq!(parse_expected_accel(""), None);
    }

    #[test]
    fn stage_cpu_does_not_copy_cuda_toolkit_into_cpu_payload() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        let cuda = root.join("toolkit");
        write_cpu_payload_src(&src);
        fs::create_dir_all(cuda.join("bin")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(cuda.join("bin").join("cudart64_12.dll"), b"cudart")
            .unwrap_or_else(|error| panic!("{error}"));
        let accel = stage_llm_payload_from(&src, &runtime, Some(&cuda))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(accel, LlmAccel::Cpu);
        let dest = runtime.join("llm-runtime").join("cpu");
        assert!(dest.join("ggml.dll").is_file());
        assert!(!dest.join("cudart64_12.dll").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn stage_vulkan_payload_lands_under_vulkan() {
        let root = temp_dir();
        let src = root.join("src");
        let runtime = root.join("resources");
        write_cpu_payload_src(&src);
        fs::write(src.join("ggml-vulkan.dll"), b"vk").unwrap_or_else(|error| panic!("{error}"));
        let accel =
            stage_llm_payload_from(&src, &runtime, None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(accel, LlmAccel::Vulkan);
        assert!(
            runtime
                .join("llm-runtime")
                .join("vulkan")
                .join("ggml-vulkan.dll")
                .is_file()
        );
        assert!(!runtime.join("llm-runtime").join("cpu").exists());
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }
}
