//! Layout and completeness for a process-isolated llama.cpp payload directory.
//!
//! Each accelerator lives in `llm-runtime/<accel>/` with `aifs-worker-llm` and the
//! native libraries it loads from that folder. Host driver files such as `nvcuda.dll`
//! are not payload files.

use crate::worker::{WorkerKind, first_process_binary, is_usable_process_binary};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Directory name under a Cargo target or Tauri resource root.
pub const LLM_RUNTIME_DIR: &str = "llm-runtime";

/// Auto-select order when `gpu_preference` is `auto` (host probes applied later).
pub const LLM_ACCEL_AUTO_ORDER: &[LlmAccel] = &[
    LlmAccel::Cuda,
    LlmAccel::Vulkan,
    LlmAccel::Metal,
    LlmAccel::Cpu,
];

/// Compiled llama.cpp accelerator packaged as its own worker + runtime libs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmAccel {
    /// CPU / ggml-cpu payload.
    Cpu,
    /// NVIDIA CUDA payload (`ggml-cuda` plus CUDA runtime libs).
    Cuda,
    /// Vulkan payload (`ggml-vulkan`).
    Vulkan,
    /// Apple Metal payload.
    Metal,
}

impl LlmAccel {
    /// Wire / directory name (`cpu`, `cuda`, `vulkan`, `metal`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
            Self::Vulkan => "vulkan",
            Self::Metal => "metal",
        }
    }

    /// Parses `cpu` / `cuda` / `vulkan` / `metal` (`mtl` accepted).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" => Some(Self::Cpu),
            "cuda" => Some(Self::Cuda),
            "vulkan" | "vulcan" => Some(Self::Vulkan),
            "metal" | "mtl" => Some(Self::Metal),
            _ => None,
        }
    }

    /// Native-library stems that must exist beside the worker for this accelerator.
    pub fn required_lib_prefixes(self) -> &'static [&'static str] {
        match self {
            Self::Cpu | Self::Metal => &["llama", "ggml"],
            Self::Cuda => &["llama", "ggml", "ggml-cuda"],
            Self::Vulkan => &["llama", "ggml", "ggml-vulkan"],
        }
    }
}

/// A complete `llm-runtime/<accel>/` tree the engine can spawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmPayload {
    /// Accelerator this directory was checked as.
    pub accel: LlmAccel,
    /// Folder containing the worker and its native libs.
    pub dir: PathBuf,
    /// Usable `aifs-worker-llm` path inside [`Self::dir`].
    pub binary: PathBuf,
}

/// `root/llm-runtime/<accel>`.
pub fn llm_payload_dir(root: impl AsRef<Path>, accel: LlmAccel) -> PathBuf {
    root.as_ref().join(LLM_RUNTIME_DIR).join(accel.as_str())
}

/// True when `dir` has a non-empty LLM worker and every required native lib for `accel`.
///
/// `nvcuda.dll` / `libcuda.so` never satisfy CUDA completeness.
pub fn payload_complete(dir: &Path, accel: LlmAccel) -> bool {
    inspect_payload(dir, accel).is_some()
}

/// Returns a payload when [`payload_complete`] is true.
pub fn inspect_payload(dir: &Path, accel: LlmAccel) -> Option<LlmPayload> {
    let binary = first_process_binary(dir, WorkerKind::Llm.binary_stem())
        .filter(|path| is_usable_process_binary(path))?;
    if !accel
        .required_lib_prefixes()
        .iter()
        .all(|prefix| dir_has_runtime_lib(dir, prefix))
    {
        return None;
    }
    Some(LlmPayload {
        accel,
        dir: dir.to_path_buf(),
        binary,
    })
}

/// Infers accelerator from libraries present (CUDA, then Vulkan, else CPU).
///
/// Metal is not distinguishable from CPU by library prefix alone.
pub fn infer_accel_from_libs(dir: &Path) -> Option<LlmAccel> {
    if payload_complete(dir, LlmAccel::Cuda) {
        return Some(LlmAccel::Cuda);
    }
    if payload_complete(dir, LlmAccel::Vulkan) {
        return Some(LlmAccel::Vulkan);
    }
    if payload_complete(dir, LlmAccel::Cpu) {
        return Some(LlmAccel::Cpu);
    }
    None
}

fn dir_has_runtime_lib(dir: &Path, prefix: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| runtime_lib_matches_prefix(name, prefix))
    })
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

/// Core ggml loader stems; accelerator plugins (`ggml-cuda`, `ggml-vulkan`) are separate.
const GGML_CORE_STEMS: &[&str] = &["ggml", "ggml-base", "ggml-cpu"];

/// True when `name` is a native lib whose stem is `prefix` or `prefix-*` / `prefix_*`.
///
/// The `ggml` prefix matches only core libs (`ggml`, `ggml-base`, `ggml-cpu`), not
/// `ggml-cuda` / `ggml-vulkan`.
pub fn runtime_lib_matches_prefix(name: &str, prefix: &str) -> bool {
    if !is_native_lib_name(name) {
        return false;
    }
    let stem = runtime_lib_stem(name);
    let prefix = prefix.to_ascii_lowercase();
    if prefix == "ggml" {
        return GGML_CORE_STEMS.iter().any(|core| stem == *core);
    }
    stem == prefix
        || stem.starts_with(&format!("{prefix}-"))
        || stem.starts_with(&format!("{prefix}_"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_worker(dir: &Path) {
        fs::create_dir_all(dir).unwrap_or_else(|error| panic!("{error}"));
        fs::write(dir.join("aifs-worker-llm"), b"worker").unwrap_or_else(|error| panic!("{error}"));
    }

    fn write_lib(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"lib").unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn parse_accel_aliases() {
        assert_eq!(LlmAccel::parse("CUDA"), Some(LlmAccel::Cuda));
        assert_eq!(LlmAccel::parse("vulcan"), Some(LlmAccel::Vulkan));
        assert_eq!(LlmAccel::parse("mtl"), Some(LlmAccel::Metal));
        assert_eq!(LlmAccel::parse("auto"), None);
    }

    #[test]
    fn llm_payload_dir_nests_under_runtime() {
        assert_eq!(
            llm_payload_dir("/app", LlmAccel::Cuda),
            PathBuf::from("/app/llm-runtime/cuda")
        );
    }

    #[test]
    fn runtime_lib_prefix_matches_ggml_cuda_not_nvcuda() {
        assert!(runtime_lib_matches_prefix("ggml-cuda.dll", "ggml-cuda"));
        assert!(runtime_lib_matches_prefix("libggml-cuda.so.0", "ggml-cuda"));
        assert!(runtime_lib_matches_prefix("llama.dll", "llama"));
        assert!(runtime_lib_matches_prefix("libllama.so", "llama"));
        assert!(runtime_lib_matches_prefix("ggml-base.dll", "ggml"));
        assert!(runtime_lib_matches_prefix("ggml-cpu.dll", "ggml"));
        assert!(!runtime_lib_matches_prefix("ggml-cuda.dll", "ggml"));
        assert!(!runtime_lib_matches_prefix("nvcuda.dll", "ggml-cuda"));
        assert!(!runtime_lib_matches_prefix("nvcuda.dll", "ggml"));
        assert!(!runtime_lib_matches_prefix("libcuda.so.1", "ggml-cuda"));
        assert!(!runtime_lib_matches_prefix("aifs-worker-llm.exe", "llama"));
    }

    #[test]
    fn payload_complete_requires_worker_and_libs() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = root.path().join("cuda");
        write_worker(&dir);
        assert!(!payload_complete(&dir, LlmAccel::Cpu));
        write_lib(&dir, "llama.dll");
        write_lib(&dir, "ggml.dll");
        assert!(payload_complete(&dir, LlmAccel::Cpu));
        assert!(!payload_complete(&dir, LlmAccel::Cuda));
        write_lib(&dir, "nvcuda.dll");
        assert!(
            !payload_complete(&dir, LlmAccel::Cuda),
            "driver DLL must not complete a CUDA payload"
        );
        write_lib(&dir, "ggml-cuda.dll");
        assert!(payload_complete(&dir, LlmAccel::Cuda));
        let payload = inspect_payload(&dir, LlmAccel::Cuda).unwrap_or_else(|| panic!("cuda"));
        assert_eq!(payload.accel, LlmAccel::Cuda);
        assert_eq!(payload.dir, dir);
        assert!(payload.binary.ends_with("aifs-worker-llm"));
    }

    #[test]
    fn empty_worker_placeholder_is_incomplete() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = root.path();
        fs::write(dir.join("aifs-worker-llm"), b"").unwrap_or_else(|error| panic!("{error}"));
        write_lib(dir, "llama.dll");
        write_lib(dir, "ggml.dll");
        assert!(!payload_complete(dir, LlmAccel::Cpu));
    }

    #[test]
    fn cuda_payload_needs_core_ggml_not_only_plugin() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = root.path();
        write_worker(dir);
        write_lib(dir, "llama.dll");
        write_lib(dir, "ggml-cuda.dll");
        assert!(!payload_complete(dir, LlmAccel::Cuda));
        write_lib(dir, "ggml.dll");
        assert!(payload_complete(dir, LlmAccel::Cuda));
    }

    #[test]
    fn vulkan_payload_needs_ggml_vulkan() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = root.path();
        write_worker(dir);
        write_lib(dir, "libllama.so");
        write_lib(dir, "libggml.so");
        assert!(!payload_complete(dir, LlmAccel::Vulkan));
        write_lib(dir, "libggml-vulkan.so.0");
        assert!(payload_complete(dir, LlmAccel::Vulkan));
    }

    #[test]
    fn infer_accel_prefers_cuda_then_vulkan_then_cpu() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = root.path();
        write_worker(dir);
        write_lib(dir, "llama.dll");
        write_lib(dir, "ggml.dll");
        assert_eq!(infer_accel_from_libs(dir), Some(LlmAccel::Cpu));
        write_lib(dir, "ggml-vulkan.dll");
        assert_eq!(infer_accel_from_libs(dir), Some(LlmAccel::Vulkan));
        write_lib(dir, "ggml-cuda.dll");
        assert_eq!(infer_accel_from_libs(dir), Some(LlmAccel::Cuda));
    }

    #[test]
    fn auto_order_is_cuda_vulkan_metal_cpu() {
        assert_eq!(
            LLM_ACCEL_AUTO_ORDER,
            &[
                LlmAccel::Cuda,
                LlmAccel::Vulkan,
                LlmAccel::Metal,
                LlmAccel::Cpu
            ]
        );
    }
}
