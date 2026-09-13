// Copy sidecar binaries only when contents change so `tauri dev` does not loop.

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

fn dest_has_cuda_backend(dest_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dest_dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_name().to_str().is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            lower.contains("ggml-cuda") || lower.contains("ggml_cuda")
        })
    })
}

fn copy_matching_libs_from(src_dir: &Path, dest_dir: &Path) -> std::io::Result<usize> {
    if !src_dir.is_dir() {
        return Ok(0);
    }
    let mut copied = 0;
    for entry in std::fs::read_dir(src_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_worker_runtime_lib(name) {
            continue;
        }
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        if copy_if_changed(&src, &dest_dir.join(name))? {
            copied += 1;
        }
    }
    Ok(copied)
}

/// Copies llama.cpp / CUDA runtime libs from `src_dir` (and `deps`) next to the sidecar.
#[cfg_attr(test, allow(dead_code))]
fn copy_worker_runtime_libs(src_dir: &Path, dest_dir: &Path) -> std::io::Result<()> {
    let cuda_root = std::env::var_os("CUDA_PATH").map(PathBuf::from);
    copy_worker_runtime_libs_from(src_dir, dest_dir, cuda_root.as_deref())
}

fn copy_worker_runtime_libs_from(
    src_dir: &Path,
    dest_dir: &Path,
    cuda_root: Option<&Path>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dest_dir)?;
    copy_matching_libs_from(src_dir, dest_dir)?;
    copy_matching_libs_from(&src_dir.join("deps"), dest_dir)?;
    if dest_has_cuda_backend(dest_dir)
        && let Some(cuda_root) = cuda_root
    {
        copy_matching_libs_from(&cuda_root.join("bin"), dest_dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod sidecar_copy_tests {
    use super::{
        copy_if_changed, copy_worker_runtime_libs_from, files_have_same_contents,
        is_worker_runtime_lib, write_if_changed,
    };
    use std::fs;
    use std::path::PathBuf;
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
            "llama.dll",
            "libggml.so",
            "libggml.so.0",
            "libllama.dylib",
            "cudart64_12.dll",
            "cublasLt64_12.dll",
            "nvrtc64_120_0.dll",
            "nvJitLink_120_0.dll",
            "libcudart.so.12",
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
}
