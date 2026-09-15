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

/// Every packaged accelerator, including ones not in auto-select first.
pub const LLM_ACCELS: &[LlmAccel] = &[
    LlmAccel::Cpu,
    LlmAccel::Cuda,
    LlmAccel::Vulkan,
    LlmAccel::Metal,
];

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

/// Wire view of a discovered payload (no worker hello).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmPayloadStatus {
    /// `cpu` / `cuda` / `vulkan` / `metal`.
    pub accel: LlmAccel,
    /// Payload directory.
    pub dir: String,
    /// Worker binary inside [`Self::dir`].
    pub binary: String,
    /// True when the host driver/OS can run this accelerator.
    pub host_available: bool,
}

impl LlmPayloadStatus {
    /// Copies paths from a complete payload and a host probe.
    pub fn from_payload(payload: &LlmPayload, host_available: bool) -> Self {
        Self {
            accel: payload.accel,
            dir: payload.dir.display().to_string(),
            binary: payload.binary.display().to_string(),
            host_available,
        }
    }
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
    if !accel_libs_present(dir, accel, &binary) {
        return None;
    }
    Some(LlmPayload {
        accel,
        dir: dir.to_path_buf(),
        binary,
    })
}

/// Shared plugin DLLs, or a statically linked CUDA worker with cublas/cudart beside it.
fn accel_libs_present(dir: &Path, accel: LlmAccel, binary: &Path) -> bool {
    if accel
        .required_lib_prefixes()
        .iter()
        .all(|prefix| dir_has_runtime_lib(dir, prefix))
    {
        return true;
    }
    accel == LlmAccel::Cuda && static_cuda_toolkit_payload(dir, binary)
}

fn static_cuda_toolkit_payload(dir: &Path, binary: &Path) -> bool {
    dir_has_cuda_toolkit_lib(dir) && binary_imports_cuda_runtime(binary)
}

fn dir_has_cuda_toolkit_lib(dir: &Path) -> bool {
    dir_has_runtime_lib(dir, "cublas") || dir_has_runtime_lib(dir, "cudart")
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

/// True when `dir` is `…/llm-runtime/<accel>` (folder name is a known accelerator).
pub fn is_staged_payload_dir(dir: &Path) -> bool {
    dir.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some(LLM_RUNTIME_DIR)
        && dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(LlmAccel::parse)
            .is_some()
}

/// Accelerator implied by `…/llm-runtime/<accel>` or complete libraries in `dir`.
///
/// A Cargo `target/debug` folder is not treated as CPU just because inference failed.
pub fn accel_from_payload_dir(dir: &Path) -> Option<LlmAccel> {
    if is_staged_payload_dir(dir)
        && let Some(accel) = dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(LlmAccel::parse)
    {
        return Some(accel);
    }
    infer_accel_from_libs(dir)
}

/// Required native-lib prefixes for `accel` that are not present in `dir`.
pub fn missing_required_lib_prefixes(dir: &Path, accel: LlmAccel) -> Vec<&'static str> {
    if accel == LlmAccel::Cuda
        && let Some(binary) = first_process_binary(dir, WorkerKind::Llm.binary_stem())
            .filter(|path| is_usable_process_binary(path))
        && static_cuda_toolkit_payload(dir, &binary)
    {
        return Vec::new();
    }
    accel
        .required_lib_prefixes()
        .iter()
        .copied()
        .filter(|prefix| !dir_has_runtime_lib(dir, prefix))
        .collect()
}

/// Complete payloads under `root/llm-runtime/<accel>/` (incomplete dirs are skipped).
pub fn list_payloads_under(root: impl AsRef<Path>) -> Vec<LlmPayload> {
    LLM_ACCELS
        .iter()
        .copied()
        .filter_map(|accel| inspect_payload(&llm_payload_dir(root.as_ref(), accel), accel))
        .collect()
}

/// True when the host looks like it can run `accel` (driver / OS, not payload files).
///
/// CPU is always available. CUDA looks for NVIDIA driver files; Vulkan for a
/// loader or DRM node; Metal only on macOS. `CUDA_PATH` is not proof of CUDA.
pub fn host_accel_available(accel: LlmAccel) -> bool {
    match accel {
        LlmAccel::Cpu => true,
        LlmAccel::Cuda => nvidia_driver_present(),
        LlmAccel::Vulkan => vulkan_loader_present(),
        LlmAccel::Metal => cfg!(target_os = "macos"),
    }
}

fn nvidia_driver_present() -> bool {
    Path::new("/proc/driver/nvidia/version").is_file()
        || Path::new("/dev/nvidia0").exists()
        || windows_nvidia_driver_dlls()
            .iter()
            .any(|path| path.is_file())
}

/// NVIDIA user-mode driver DLLs Windows searches (`nvcuda` / `nvml`, not `CUDA_PATH`).
pub fn windows_nvidia_driver_dlls() -> Vec<PathBuf> {
    windows_nvidia_driver_dlls_from(&windows_system_root())
}

/// Resolves `%SystemRoot%` so a non-`C:\Windows` install still finds `nvcuda.dll`.
fn windows_system_root() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("SYSTEMROOT"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

/// `System32` and `Sysnative` copies of `nvcuda.dll` / `nvml.dll` under `system_root`.
pub fn windows_nvidia_driver_dlls_from(system_root: &Path) -> Vec<PathBuf> {
    ["nvcuda.dll", "nvml.dll"]
        .into_iter()
        .flat_map(|name| {
            [
                system_root.join("System32").join(name),
                system_root.join("Sysnative").join(name),
            ]
        })
        .collect()
}

fn vulkan_loader_present() -> bool {
    if cfg!(target_os = "linux") {
        return Path::new("/dev/dri").exists();
    }
    if cfg!(target_os = "windows") {
        return Path::new(r"C:\Windows\System32\vulkan-1.dll").is_file()
            || windows_system_root()
                .join("System32")
                .join("vulkan-1.dll")
                .is_file();
    }
    false
}

/// Short copy when CUDA/Vulkan/Metal was skipped because the host probe failed.
pub fn host_accel_unavailable_reason(accel: LlmAccel) -> Option<String> {
    if host_accel_available(accel) {
        return None;
    }
    Some(match accel {
        LlmAccel::Cpu => return None,
        LlmAccel::Cuda => nvidia_driver_lookup_copy(),
        LlmAccel::Vulkan => {
            "Vulkan loader not found (System32\\vulkan-1.dll or /dev/dri)".to_owned()
        }
        LlmAccel::Metal => "Metal is only available on macOS".to_owned(),
    })
}

fn nvidia_driver_lookup_copy() -> String {
    format!(
        "NVIDIA driver probe failed (looked for {primary} and /dev/nvidia0; CUDA_PATH is not a driver probe)",
        primary = windows_nvidia_driver_dlls()
            .into_iter()
            .next()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| r"C:\Windows\System32\nvcuda.dll".to_owned()),
    )
}

/// True when `path` is linked against llama/ggml (PE import names or ELF/dylib sonames).
///
/// A CUDA-linked `aifs-worker-llm.exe` still matches when llama.dll is not on disk
/// (MSVC llama-cpp-sys-2 often static-links llama/ggml and imports `cublas64_*.dll`).
pub fn binary_links_llama_runtime(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    const MARKERS: &[&[u8]] = &[
        b"llama.dll\0",
        b"ggml.dll\0",
        b"ggml-cuda.dll\0",
        b"libllama.so",
        b"libggml.so",
        b"libllama.dylib",
        b"libggml.dylib",
    ];
    MARKERS
        .iter()
        .copied()
        .any(|marker| find_bytes(&data, marker))
        || binary_bytes_import_cuda_runtime(&data)
}

/// True when the worker imports CUDA toolkit runtime (`cublas` / `cudart`), not `nvcuda`.
pub fn binary_imports_cuda_runtime(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    binary_bytes_import_cuda_runtime(&data)
}

fn binary_bytes_import_cuda_runtime(data: &[u8]) -> bool {
    const MARKERS: &[&[u8]] = &[
        b"cublas64_",
        b"cudart64_",
        b"cublas.dll\0",
        b"cudart.dll\0",
        b"libcublas.so",
        b"libcudart.so",
    ];
    MARKERS
        .iter()
        .copied()
        .any(|marker| find_bytes(data, marker))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Picks a complete payload using `preference` (`auto` / `cpu` / `cuda` / …).
///
/// `auto` skips accelerators the host probe rejects. An explicit accelerator
/// (`cuda`, `cpu`, …) is tried first even when that probe fails, so a complete
/// CUDA payload still spawns when the user asked for CUDA. If that leaves
/// nothing and a complete payload still exists (probe false-negative), callers
/// should retry with `host_available = |_| true` rather than a cargo stub.
pub fn select_llm_payload<'a>(
    payloads: &'a [LlmPayload],
    preference: &str,
    host_available: impl Fn(LlmAccel) -> bool,
) -> Option<&'a LlmPayload> {
    let explicit = LlmAccel::parse(preference);
    let mut order = Vec::with_capacity(LLM_ACCEL_AUTO_ORDER.len() + 1);
    if let Some(accel) = explicit {
        order.push(accel);
        for &next in LLM_ACCEL_AUTO_ORDER {
            if next != accel {
                order.push(next);
            }
        }
    } else {
        order.extend(LLM_ACCEL_AUTO_ORDER.iter().copied());
    }
    for accel in order {
        if !host_available(accel) && explicit != Some(accel) {
            continue;
        }
        if let Some(payload) = payloads.iter().find(|payload| payload.accel == accel) {
            return Some(payload);
        }
    }
    None
}

/// Why autoselect found nothing spawnable (incomplete CUDA snapshot vs host skip).
///
/// `sidecar` is a Cargo `aifs-worker-llm` that was not used because it is not a
/// complete payload. `host_available` is the same probe `select_llm_payload` used.
pub fn explain_llm_payload_selection_failure(
    roots: &[PathBuf],
    preference: &str,
    host_available: impl Fn(LlmAccel) -> bool,
    sidecar: Option<&Path>,
) -> String {
    let preference = preference.trim();
    let preference = if preference.is_empty() {
        "auto"
    } else {
        preference
    };
    let mut parts = vec![format!("No usable LLM payload (preference: {preference}).")];
    let cuda = snapshot_gap(roots, LlmAccel::Cuda);
    let cuda_host = host_available(LlmAccel::Cuda);
    if cuda.complete && !cuda_host {
        parts.push(format!(
            "{note} but was skipped ({reason}). Set gpu_preference to cuda to spawn it anyway.",
            note = format_snapshot_gap(LlmAccel::Cuda, &cuda),
            reason = nvidia_driver_lookup_copy(),
        ));
    } else {
        parts.push(format!("{}.", format_snapshot_gap(LlmAccel::Cuda, &cuda)));
    }
    let cpu = snapshot_gap(roots, LlmAccel::Cpu);
    if !cpu.complete {
        parts.push(format!("{}.", format_snapshot_gap(LlmAccel::Cpu, &cpu)));
    }
    if let Some(sidecar) = sidecar {
        let dir = sidecar.parent().unwrap_or(sidecar);
        let missing = missing_required_lib_prefixes(dir, LlmAccel::Cpu);
        if binary_links_llama_runtime(sidecar) {
            parts.push(format!(
                "{sidecar} links llama/ggml but is not a staged llm-runtime payload (missing {names}).",
                sidecar = sidecar.display(),
                names = if missing.is_empty() {
                    "llama, ggml beside the exe".to_owned()
                } else {
                    missing.join(", ")
                },
            ));
        } else if infer_accel_from_libs(dir).is_none() {
            if missing.is_empty() {
                parts.push(format!(
                    "Did not spawn incomplete cargo sidecar {}.",
                    sidecar.display()
                ));
            } else {
                parts.push(format!(
                    "Did not spawn incomplete cargo sidecar {sidecar} (missing {names}).",
                    sidecar = sidecar.display(),
                    names = missing.join(", "),
                ));
            }
        }
    }
    parts.push(
        "Rebuild with `pnpm llama:cuda` so llm-runtime/cuda contains the worker and either ggml-cuda or CUDA toolkit cublas/cudart (CUDA 13: CUDA_PATH/bin/x64)."
            .to_owned(),
    );
    parts.join(" ")
}

struct SnapshotGap {
    dir: PathBuf,
    complete: bool,
    has_worker: bool,
    missing: Vec<&'static str>,
}

fn snapshot_gap(roots: &[PathBuf], accel: LlmAccel) -> SnapshotGap {
    let dir = expected_payload_dir(roots, accel);
    let has_worker = first_process_binary(&dir, WorkerKind::Llm.binary_stem())
        .is_some_and(|path| is_usable_process_binary(&path));
    let missing = missing_required_lib_prefixes(&dir, accel);
    SnapshotGap {
        complete: inspect_payload(&dir, accel).is_some(),
        dir,
        has_worker,
        missing,
    }
}

fn expected_payload_dir(roots: &[PathBuf], accel: LlmAccel) -> PathBuf {
    if let Some(dir) = roots
        .iter()
        .map(|root| llm_payload_dir(root, accel))
        .find(|dir| dir.is_dir())
    {
        return dir;
    }
    let root = roots.iter().find(|root| {
        root.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "debug" || name == "release")
            || root.join("aifs-worker-llm").is_file()
            || root.join("aifs-worker-llm.exe").is_file()
    });
    match root.or_else(|| roots.first()) {
        Some(root) => llm_payload_dir(root, accel),
        None => PathBuf::from(LLM_RUNTIME_DIR).join(accel.as_str()),
    }
}

fn format_snapshot_gap(accel: LlmAccel, gap: &SnapshotGap) -> String {
    if !gap.dir.is_dir() {
        return format!(
            "no {accel} snapshot at {dir}",
            accel = accel.as_str(),
            dir = gap.dir.display()
        );
    }
    if gap.complete {
        return format!(
            "{accel} snapshot at {dir} is complete",
            accel = accel.as_str(),
            dir = gap.dir.display()
        );
    }
    let mut gaps = Vec::new();
    if !gap.has_worker {
        gaps.push("worker");
    }
    gaps.extend(gap.missing.iter().copied());
    if gaps.is_empty() {
        return format!(
            "{accel} snapshot at {dir} is incomplete",
            accel = accel.as_str(),
            dir = gap.dir.display()
        );
    }
    format!(
        "{accel} snapshot at {dir} is missing {names}",
        accel = accel.as_str(),
        dir = gap.dir.display(),
        names = gaps.join(", ")
    )
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

/// True when `dir` contains a native lib matching `prefix` (`llama`, `ggml-cuda`, …).
pub fn payload_dir_has_lib_prefix(dir: &Path, prefix: &str) -> bool {
    dir_has_runtime_lib(dir, prefix)
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
    let mut stem = if let Some(stem) = file.strip_suffix(".dll") {
        stem.to_owned()
    } else if let Some(stem) = file.strip_suffix(".dylib") {
        stem.to_owned()
    } else if let Some(idx) = file.find(".so") {
        file[..idx].to_owned()
    } else {
        file.to_owned()
    };
    while let Some((head, tail)) = stem.rsplit_once('.')
        && !tail.is_empty()
        && tail.bytes().all(|b| b.is_ascii_digit())
    {
        stem = head.to_owned();
    }
    stem
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
    if prefix == "cublas" || prefix == "cudart" {
        return stem.starts_with(&prefix);
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
        assert!(runtime_lib_matches_prefix("libllama.0.dylib", "llama"));
        assert!(runtime_lib_matches_prefix("ggml-cpu.dll", "ggml"));
        assert!(!runtime_lib_matches_prefix("ggml-cuda.dll", "ggml"));
        assert!(!runtime_lib_matches_prefix("nvcuda.dll", "ggml-cuda"));
        assert!(!runtime_lib_matches_prefix("nvcuda.dll", "ggml"));
        assert!(!runtime_lib_matches_prefix("libcuda.so.1", "ggml-cuda"));
        assert!(!runtime_lib_matches_prefix("aifs-worker-llm.exe", "llama"));
        assert!(runtime_lib_matches_prefix("cublas64_13.dll", "cublas"));
        assert!(runtime_lib_matches_prefix("cublasLt64_13.dll", "cublas"));
        assert!(runtime_lib_matches_prefix("cudart64_12.dll", "cudart"));
        assert!(!runtime_lib_matches_prefix("nvcuda.dll", "cublas"));
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
    fn missing_required_libs_use_folder_name_not_driver_dlls() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = llm_payload_dir(root.path(), LlmAccel::Cuda);
        write_worker(&dir);
        write_lib(&dir, "llama.dll");
        write_lib(&dir, "ggml.dll");
        write_lib(&dir, "nvcuda.dll");
        assert_eq!(accel_from_payload_dir(&dir), Some(LlmAccel::Cuda));
        assert_eq!(
            missing_required_lib_prefixes(&dir, LlmAccel::Cuda),
            vec!["ggml-cuda"]
        );
        write_lib(&dir, "ggml-cuda.dll");
        assert!(missing_required_lib_prefixes(&dir, LlmAccel::Cuda).is_empty());
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

    fn write_complete_cpu(dir: &Path) {
        write_worker(dir);
        write_lib(dir, "llama.dll");
        write_lib(dir, "ggml.dll");
    }

    #[test]
    fn list_payloads_under_skips_incomplete_and_keeps_siblings() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        write_complete_cpu(&llm_payload_dir(root.path(), LlmAccel::Cpu));
        write_worker(&llm_payload_dir(root.path(), LlmAccel::Cuda));
        write_lib(&llm_payload_dir(root.path(), LlmAccel::Cuda), "llama.dll");
        write_lib(&llm_payload_dir(root.path(), LlmAccel::Cuda), "ggml.dll");
        let listed = list_payloads_under(root.path());
        let accels: Vec<_> = listed.iter().map(|payload| payload.accel).collect();
        assert!(accels.contains(&LlmAccel::Cpu), "{accels:?}");
        assert!(
            !accels.contains(&LlmAccel::Cuda),
            "CUDA without ggml-cuda must be skipped: {accels:?}"
        );
    }

    #[test]
    fn select_auto_prefers_cuda_when_host_and_payload_exist() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        write_complete_cpu(&llm_payload_dir(root.path(), LlmAccel::Cpu));
        write_complete_cpu(&llm_payload_dir(root.path(), LlmAccel::Cuda));
        write_lib(
            &llm_payload_dir(root.path(), LlmAccel::Cuda),
            "ggml-cuda.dll",
        );
        let payloads = list_payloads_under(root.path());
        let selected = select_llm_payload(&payloads, "auto", |_| true)
            .unwrap_or_else(|| panic!("expected a payload"));
        assert_eq!(selected.accel, LlmAccel::Cuda);
    }

    #[test]
    fn select_skips_cuda_when_host_has_no_driver() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        write_complete_cpu(&llm_payload_dir(root.path(), LlmAccel::Cpu));
        write_complete_cpu(&llm_payload_dir(root.path(), LlmAccel::Cuda));
        write_lib(
            &llm_payload_dir(root.path(), LlmAccel::Cuda),
            "ggml-cuda.dll",
        );
        let payloads = list_payloads_under(root.path());
        let host_ok = |accel: LlmAccel| accel != LlmAccel::Cuda;
        let selected = select_llm_payload(&payloads, "auto", host_ok)
            .unwrap_or_else(|| panic!("expected fallback"));
        assert_eq!(selected.accel, LlmAccel::Cpu);
        let explicit = select_llm_payload(&payloads, "cuda", host_ok)
            .unwrap_or_else(|| panic!("explicit cuda should spawn the CUDA payload"));
        assert_eq!(explicit.accel, LlmAccel::Cuda);
    }

    #[test]
    fn windows_nvidia_probe_uses_system_root_and_sysnative() {
        let paths = windows_nvidia_driver_dlls_from(Path::new("/win"));
        assert!(
            paths
                .iter()
                .any(|path| path == Path::new("/win/System32/nvcuda.dll")),
            "{paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|path| path == Path::new("/win/Sysnative/nvcuda.dll")),
            "{paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|path| path == Path::new("/win/System32/nvml.dll")),
            "{paths:?}"
        );
    }

    #[test]
    fn cargo_target_dir_is_not_inferred_as_cpu_payload() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = root.path().join("debug");
        write_worker(&dir);
        assert!(!is_staged_payload_dir(&dir));
        assert_eq!(accel_from_payload_dir(&dir), None);
    }

    #[test]
    fn explain_incomplete_cuda_names_missing_plugin_not_cargo_cpu() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let cuda_dir = llm_payload_dir(root.path(), LlmAccel::Cuda);
        write_worker(&cuda_dir);
        write_lib(&cuda_dir, "llama.dll");
        write_lib(&cuda_dir, "ggml.dll");
        let sidecar = root.path().join("debug").join("aifs-worker-llm.exe");
        fs::create_dir_all(sidecar.parent().unwrap_or_else(|| panic!("parent")))
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(&sidecar, b"worker").unwrap_or_else(|error| panic!("{error}"));
        let message = explain_llm_payload_selection_failure(
            &[root.path().to_path_buf()],
            "auto",
            |_| true,
            Some(&sidecar),
        );
        assert!(message.contains("preference: auto"), "{message}");
        assert!(message.contains("ggml-cuda"), "{message}");
        assert!(
            message.contains("Did not spawn incomplete cargo sidecar"),
            "{message}"
        );
        assert!(message.contains("missing llama, ggml"), "{message}");
        assert!(message.contains("pnpm llama:cuda"), "{message}");
        assert!(!message.contains("cpu payload is missing"), "{message}");
    }

    #[test]
    fn binary_links_llama_runtime_detects_pe_import_names() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let stub = root.path().join("stub");
        let linked = root.path().join("linked.exe");
        fs::write(&stub, b"rust stub worker").unwrap_or_else(|error| panic!("{error}"));
        fs::write(&linked, b"MZ\0llama.dll\0ggml.dll\0").unwrap_or_else(|error| panic!("{error}"));
        assert!(!binary_links_llama_runtime(&stub));
        assert!(binary_links_llama_runtime(&linked));
    }

    #[test]
    fn binary_links_llama_runtime_detects_static_cublas_imports() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let linked = root.path().join("cuda-static.exe");
        fs::write(&linked, b"MZ\0cublas64_13.dll\0").unwrap_or_else(|error| panic!("{error}"));
        assert!(binary_imports_cuda_runtime(&linked));
        assert!(binary_links_llama_runtime(&linked));
    }

    #[test]
    fn static_cuda_worker_with_cublas_is_complete() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = llm_payload_dir(root.path(), LlmAccel::Cuda);
        fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
        fs::write(dir.join("aifs-worker-llm.exe"), b"MZ\0cublas64_13.dll\0")
            .unwrap_or_else(|error| panic!("{error}"));
        write_lib(&dir, "cublas64_13.dll");
        write_lib(&dir, "cublasLt64_13.dll");
        assert!(payload_complete(&dir, LlmAccel::Cuda));
        assert!(missing_required_lib_prefixes(&dir, LlmAccel::Cuda).is_empty());
        assert!(!payload_complete(&dir, LlmAccel::Cpu));
    }

    #[test]
    fn cublas_without_cuda_import_does_not_complete_cuda() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = llm_payload_dir(root.path(), LlmAccel::Cuda);
        write_worker(&dir);
        write_lib(&dir, "cublas64_13.dll");
        assert!(!payload_complete(&dir, LlmAccel::Cuda));
    }

    #[test]
    fn explain_llama_linked_sidecar_is_not_a_stub() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let sidecar = root.path().join("debug").join("aifs-worker-llm.exe");
        fs::create_dir_all(sidecar.parent().unwrap_or_else(|| panic!("parent")))
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(&sidecar, b"MZ\0llama.dll\0").unwrap_or_else(|error| panic!("{error}"));
        let message = explain_llm_payload_selection_failure(
            &[root.path().to_path_buf()],
            "auto",
            |_| true,
            Some(&sidecar),
        );
        assert!(message.contains("links llama/ggml"), "{message}");
        assert!(
            message.contains("not a staged llm-runtime payload"),
            "{message}"
        );
        assert!(!message.contains("cpu payload is missing"), "{message}");
    }

    #[test]
    fn explain_complete_cuda_skipped_by_host_probe() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let cuda_dir = llm_payload_dir(root.path(), LlmAccel::Cuda);
        write_complete_cpu(&cuda_dir);
        write_lib(&cuda_dir, "ggml-cuda.dll");
        let host_ok = |accel: LlmAccel| accel != LlmAccel::Cuda;
        let message = explain_llm_payload_selection_failure(
            &[root.path().to_path_buf()],
            "auto",
            host_ok,
            None,
        );
        assert!(message.contains("is complete but was skipped"), "{message}");
        assert!(message.contains("NVIDIA driver probe failed"), "{message}");
        assert!(message.contains("gpu_preference to cuda"), "{message}");
        assert!(
            message.contains("nvcuda.dll") || message.contains("nvidia0"),
            "{message}"
        );
    }

    #[test]
    fn host_cpu_is_always_available_and_metal_follows_os() {
        assert!(host_accel_available(LlmAccel::Cpu));
        assert_eq!(
            host_accel_available(LlmAccel::Metal),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn payload_status_serializes_accel_and_host_flag() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let dir = llm_payload_dir(root.path(), LlmAccel::Cpu);
        write_complete_cpu(&dir);
        let payload = inspect_payload(&dir, LlmAccel::Cpu).unwrap_or_else(|| panic!("cpu"));
        let status = LlmPayloadStatus::from_payload(&payload, true);
        let json = serde_json::to_value(&status).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(json["accel"], "cpu");
        assert_eq!(json["host_available"], true);
        assert_eq!(status.host_available, host_accel_available(LlmAccel::Cpu));
        assert!(json["dir"].as_str().unwrap_or("").contains("cpu"));
        assert!(
            json["binary"]
                .as_str()
                .unwrap_or("")
                .contains("aifs-worker-llm"),
            "{json}"
        );
    }
}
