//! Device selection and GPU-failure CPU retry policy for the LLM worker.

use std::path::Path;

/// Process env for offload layers when `load.n_gpu_layers` is omitted.
pub const N_GPU_LAYERS_ENV: &str = "AIFS_N_GPU_LAYERS";

/// Process env for llama.cpp context tokens when omitted.
#[cfg(any(test, feature = "llama"))]
pub const CTX_TOKENS_ENV: &str = "AIFS_CTX_TOKENS";

/// llama.cpp "offload every layer" sentinel used when layer count is unknown.
#[cfg(any(test, feature = "llama"))]
pub const ALL_GPU_LAYERS: u32 = 999;

/// Default llama.cpp context window. Falls back toward 512 on allocation failure.
#[cfg(any(test, feature = "llama"))]
pub const DEFAULT_N_CTX: u32 = 4096;

#[cfg(any(test, feature = "llama"))]
const MIN_CTX: u32 = 512;
#[cfg(any(test, feature = "llama"))]
const MAX_CTX: u32 = 8192;
#[cfg(any(test, feature = "llama"))]
const CONTEXT_FALLBACKS: &[u32] = &[2048, 1024, 512];
#[cfg(any(test, feature = "llama"))]
const GPU_LAYER_RETRY_NUM: u32 = 3;
#[cfg(any(test, feature = "llama"))]
const GPU_LAYER_RETRY_DEN: u32 = 4;
#[cfg(any(test, feature = "llama"))]
const MAX_GPU_LAYER_ATTEMPTS: usize = 3;
#[cfg(any(test, feature = "llama"))]
const MIN_GPU_LAYER_RETRY: u32 = 1;
#[cfg(any(test, feature = "llama"))]
const UNKNOWN_LAYER_FALLBACKS: &[u32] = &[32, 16];

/// Resolves `gpu_preference` to a device id and optional fallback message.
pub fn resolve_device(preference: &str) -> (String, Option<String>) {
    let pref = preference.trim();
    let pref = if pref.is_empty() {
        "auto".to_owned()
    } else {
        pref.to_ascii_lowercase()
    };
    match pref.as_str() {
        "cpu" => ("cpu".to_owned(), None),
        "auto" => pick_auto(),
        "cuda" => prefer("cuda", cuda_available(), "CUDA"),
        "vulkan" => prefer("vulkan", vulkan_available(), "Vulkan"),
        "metal" | "mtl" => prefer("metal", metal_available(), "Metal"),
        other => (
            "cpu".to_owned(),
            Some(format!("unknown gpu_preference {other}; using cpu")),
        ),
    }
}

fn pick_auto() -> (String, Option<String>) {
    if cuda_available() {
        return ("cuda".to_owned(), None);
    }
    if vulkan_available() {
        return ("vulkan".to_owned(), None);
    }
    if metal_available() {
        return ("metal".to_owned(), None);
    }
    ("cpu".to_owned(), None)
}

fn prefer(device: &str, available: bool, label: &str) -> (String, Option<String>) {
    if available {
        (device.to_owned(), None)
    } else {
        cpu_fallback(label)
    }
}

fn cpu_fallback(label: &str) -> (String, Option<String>) {
    let compiled = match label {
        "CUDA" => cfg!(feature = "cuda"),
        "Vulkan" => cfg!(feature = "vulkan"),
        "Metal" => cfg!(feature = "metal"),
        _ => false,
    };
    let message = if compiled {
        format!("{label} requested but no {label} device was found; using cpu")
    } else {
        format!("{label} requested but this worker was not built with {label}; using cpu")
    };
    ("cpu".to_owned(), Some(message))
}

fn cuda_available() -> bool {
    cfg!(feature = "cuda") && nvidia_runtime_present()
}

fn vulkan_available() -> bool {
    cfg!(feature = "vulkan") && vulkan_runtime_present()
}

fn metal_available() -> bool {
    cfg!(all(feature = "metal", target_os = "macos"))
}

fn nvidia_runtime_present() -> bool {
    Path::new("/proc/driver/nvidia/version").is_file()
        || Path::new("/dev/nvidia0").exists()
        || Path::new(r"C:\Windows\System32\nvcuda.dll").is_file()
        || Path::new(r"C:\Windows\System32\nvml.dll").is_file()
}

fn vulkan_runtime_present() -> bool {
    if cfg!(target_os = "linux") {
        return Path::new("/dev/dri").exists();
    }
    if cfg!(target_os = "windows") {
        return Path::new(r"C:\Windows\System32\vulkan-1.dll").is_file();
    }
    false
}

/// Parses `AIFS_N_GPU_LAYERS` / load `n_gpu_layers` text. Empty or invalid is `None`.
pub fn parse_n_gpu_layers(raw: &str) -> Option<u32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse().ok()
}

/// Explicit `n_gpu_layers`, else `AIFS_N_GPU_LAYERS` when set and valid.
pub fn requested_n_gpu_layers(explicit: Option<u32>) -> Option<u32> {
    explicit.or_else(|| {
        std::env::var(N_GPU_LAYERS_ENV)
            .ok()
            .as_deref()
            .and_then(parse_n_gpu_layers)
    })
}

/// `AIFS_CTX_TOKENS` when set and in range, else [`DEFAULT_N_CTX`].
#[cfg(any(test, feature = "llama"))]
pub fn resolve_n_ctx() -> u32 {
    std::env::var(CTX_TOKENS_ENV)
        .ok()
        .as_deref()
        .and_then(parse_n_gpu_layers)
        .map(|value| value.clamp(MIN_CTX, MAX_CTX))
        .unwrap_or(DEFAULT_N_CTX)
}

/// Preferred context size, then smaller windows that still fit GPU/CPU alloc.
#[cfg(any(test, feature = "llama"))]
pub fn context_size_attempts(preferred: u32) -> Vec<u32> {
    let preferred = preferred.clamp(MIN_CTX, MAX_CTX);
    let mut attempts = vec![preferred];
    for &size in CONTEXT_FALLBACKS {
        if size < preferred {
            push_unique(&mut attempts, size);
        }
    }
    attempts
}

/// GPU offload attempts before CPU. Explicit `n_gpu_layers` is tried once.
#[cfg(any(test, feature = "llama"))]
pub fn gpu_layer_load_attempts(
    device: &str,
    requested: u32,
    explicit: bool,
    block_count: Option<u32>,
) -> Vec<u32> {
    if device == "cpu" || requested == 0 {
        return vec![0];
    }
    if explicit {
        return vec![requested];
    }
    let mut attempts = Vec::new();
    let first = match block_count {
        Some(count) if requested >= ALL_GPU_LAYERS => count,
        Some(count) => requested.min(count),
        None => requested,
    };
    push_unique(&mut attempts, first);
    if let Some(count) = block_count {
        let mut current = first.min(count);
        while attempts.len() < MAX_GPU_LAYER_ATTEMPTS && current > MIN_GPU_LAYER_RETRY {
            let mut reduced = current.saturating_mul(GPU_LAYER_RETRY_NUM) / GPU_LAYER_RETRY_DEN;
            if reduced >= current {
                reduced = current.saturating_sub(1);
            }
            reduced = reduced.max(MIN_GPU_LAYER_RETRY);
            if !push_unique(&mut attempts, reduced) {
                break;
            }
            current = reduced;
        }
    } else if first >= ALL_GPU_LAYERS {
        for &layers in UNKNOWN_LAYER_FALLBACKS {
            if attempts.len() >= MAX_GPU_LAYER_ATTEMPTS {
                break;
            }
            push_unique(&mut attempts, layers);
        }
    }
    attempts
}

#[cfg(any(test, feature = "llama"))]
fn push_unique(values: &mut Vec<u32>, value: u32) -> bool {
    if values.contains(&value) {
        return false;
    }
    values.push(value);
    true
}

/// True when `device` is an accelerator and `error` looks like GPU init or OOM.
#[cfg(any(test, feature = "llama"))]
pub fn should_retry_cpu(device: &str, error: &str) -> bool {
    device != "cpu" && is_gpu_failure(error)
}

/// GPU init / OOM / driver failures. Context-window errors must not match.
#[cfg(any(test, feature = "llama"))]
pub fn is_gpu_failure(error: &str) -> bool {
    if error.contains("prompt exceeds the llama.cpp context window") {
        return false;
    }
    let lower = error.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "out of memory",
        "out-of-memory",
        "not enough memory",
        "cuda",
        "cublas",
        "nvml",
        "vulkan",
        "ggml",
        "metal",
        "hipblas",
        "failed to allocate",
        "failed to create llama_context",
        "mtmd_helper_eval_chunks",
        "insufficient memory",
        "device lost",
        "no cuda",
        "vram",
        "vk::device::allocatememory",
        "erroroutofdevicememory",
        "erroroutofhostmemory",
        "vk_error_out_of_device_memory",
        "vk_error_out_of_host_memory",
        "cuda_error_out_of_memory",
        "0xc0000409",
        "gpu preflight",
    ];
    if NEEDLES.iter().any(|needle| lower.contains(needle)) {
        return true;
    }
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| word == "oom")
}

/// Load failures that should retry fewer GPU layers or CPU, not missing files.
#[cfg(any(test, feature = "llama"))]
pub fn is_retryable_load_failure(error: &str) -> bool {
    if error.contains("not fully downloaded")
        || error.contains("was not found")
        || error.contains("not a GGUF")
        || error.contains("cannot load an off")
        || error.contains("unknown catalog id")
        || error.contains("prompt exceeds the llama.cpp context window")
    {
        return false;
    }
    true
}

/// One CPU retry plan after a GPU load failure, or `None` to surface `error`.
#[cfg(any(test, feature = "llama"))]
pub fn cpu_retry_plan(
    device: &str,
    fallback: Option<String>,
    error: &str,
) -> Option<(String, u32, Option<String>)> {
    if !should_retry_cpu(device, error) {
        return None;
    }
    let note = format!("GPU load failed ({error}); using cpu");
    let fallback = Some(match fallback {
        Some(existing) if !existing.is_empty() => format!("{existing} {note}"),
        _ => note,
    });
    Some(("cpu".to_owned(), 0, fallback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_preference_stays_on_cpu() {
        let (device, fallback) = resolve_device("cpu");
        assert_eq!(device, "cpu");
        assert!(fallback.is_none());
    }

    #[test]
    fn auto_stays_on_cpu_without_an_accelerator_feature() {
        if cfg!(any(feature = "cuda", feature = "vulkan", feature = "metal")) {
            return;
        }
        let (device, fallback) = resolve_device("auto");
        assert_eq!(device, "cpu");
        assert!(fallback.is_none());
    }

    #[test]
    fn cuda_preference_falls_back_without_a_device() {
        if cfg!(feature = "cuda") && nvidia_runtime_present() {
            let (device, fallback) = resolve_device("cuda");
            assert_eq!(device, "cuda");
            assert!(fallback.is_none());
            return;
        }
        let (device, fallback) = resolve_device("cuda");
        assert_eq!(device, "cpu");
        assert!(
            fallback
                .as_deref()
                .is_some_and(|text| text.contains("CUDA"))
        );
    }

    #[test]
    fn vulkan_preference_falls_back_without_a_probed_device() {
        if cfg!(feature = "vulkan") && vulkan_runtime_present() {
            return;
        }
        let (device, fallback) = resolve_device("vulkan");
        assert_eq!(device, "cpu");
        assert!(
            fallback
                .as_deref()
                .is_some_and(|text| text.contains("Vulkan"))
        );
    }

    #[test]
    fn metal_alias_mtl_is_recognized() {
        let (device, fallback) = resolve_device("MTL");
        if cfg!(all(feature = "metal", target_os = "macos")) {
            assert_eq!(device, "metal");
            assert!(fallback.is_none());
            return;
        }
        assert_eq!(device, "cpu");
        assert!(
            fallback
                .as_deref()
                .is_some_and(|text| text.contains("Metal"))
        );
    }

    #[test]
    fn gpu_oom_retries_once_on_cpu() {
        let retry = cpu_retry_plan("cuda", None, "CUDA error: out of memory")
            .unwrap_or_else(|| panic!("expected CPU retry"));
        assert_eq!(retry.0, "cpu");
        assert_eq!(retry.1, 0);
        assert!(retry.2.as_deref().is_some_and(|text| {
            text.contains("GPU load failed")
                && text.contains("out of memory")
                && text.contains("using cpu")
        }));
        assert!(cpu_retry_plan("cpu", retry.2, "CUDA error: out of memory").is_none());
    }

    #[test]
    fn context_window_is_not_a_gpu_failure() {
        let error = "prompt exceeds the llama.cpp context window";
        assert!(!is_gpu_failure(error));
        assert!(cpu_retry_plan("cuda", None, error).is_none());
        assert!(cpu_retry_plan("vulkan", None, error).is_none());
    }

    #[test]
    fn missing_gguf_is_not_retried_as_gpu_failure() {
        let error = format!(
            "gemma-3-4b-it is not fully downloaded under {}",
            std::path::Path::new("/models").display()
        );
        assert!(error.contains("not fully downloaded"));
        assert!(!is_gpu_failure(&error));
        assert!(cpu_retry_plan("cuda", None, &error).is_none());
    }

    #[test]
    fn oom_token_is_a_gpu_failure() {
        assert!(is_gpu_failure("llama OOM while allocating KV cache"));
        assert!(is_gpu_failure("failed to create llama_context"));
        assert!(is_gpu_failure("vk_error_out_of_device_memory"));
        assert!(is_gpu_failure("mtmd_helper_eval_chunks failed"));
        assert!(!is_gpu_failure("load a model before infer"));
    }

    #[test]
    fn load_retries_skip_missing_files() {
        assert!(!is_retryable_load_failure(
            "gemma-3-4b-it is not fully downloaded under /models"
        ));
        assert!(!is_retryable_load_failure("/tmp/model.gguf was not found"));
        assert!(!is_retryable_load_failure(
            "/tmp/page.html is not a GGUF file"
        ));
        assert!(is_retryable_load_failure("CUDA error: out of memory"));
        assert!(is_retryable_load_failure("failed to load model from file"));
    }

    #[test]
    fn gpu_layer_attempts_reduce_before_cpu() {
        assert_eq!(gpu_layer_load_attempts("cpu", 99, false, Some(34)), vec![0]);
        assert_eq!(
            gpu_layer_load_attempts("cuda", 32, true, Some(34)),
            vec![32]
        );
        assert_eq!(
            gpu_layer_load_attempts("cuda", ALL_GPU_LAYERS, false, Some(34)),
            vec![34, 25, 18]
        );
        assert_eq!(
            gpu_layer_load_attempts("cuda", ALL_GPU_LAYERS, false, None),
            vec![ALL_GPU_LAYERS, 32, 16]
        );
    }

    #[test]
    fn context_attempts_step_down_from_preferred() {
        assert_eq!(context_size_attempts(4096), vec![4096, 2048, 1024, 512]);
        assert_eq!(context_size_attempts(1024), vec![1024, 512]);
        assert_eq!(context_size_attempts(512), vec![512]);
    }

    #[test]
    fn parse_n_gpu_layers_ignores_blank_and_junk() {
        assert_eq!(parse_n_gpu_layers("32"), Some(32));
        assert_eq!(parse_n_gpu_layers(" 0 "), Some(0));
        assert_eq!(parse_n_gpu_layers(""), None);
        assert_eq!(parse_n_gpu_layers("auto"), None);
        assert_eq!(requested_n_gpu_layers(Some(8)), Some(8));
        assert_eq!(CTX_TOKENS_ENV, "AIFS_CTX_TOKENS");
        let ctx = resolve_n_ctx();
        assert!((MIN_CTX..=MAX_CTX).contains(&ctx), "{ctx}");
        assert_eq!(DEFAULT_N_CTX, 4096);
    }
}
