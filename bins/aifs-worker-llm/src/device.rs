//! Device selection and GPU-failure CPU retry policy for the LLM worker.

use std::path::Path;

/// Process env for offload layers when `load.n_gpu_layers` is omitted.
pub const N_GPU_LAYERS_ENV: &str = "AIFS_N_GPU_LAYERS";

/// Resolves `gpu_preference` to a device id and optional fallback message.
pub fn resolve_device(preference: &str) -> (String, Option<String>) {
    let pref = preference.trim();
    let pref = if pref.is_empty() { "auto" } else { pref };
    match pref {
        "cpu" => ("cpu".to_owned(), None),
        "auto" => pick_auto(),
        "cuda" => prefer("cuda", cuda_available(), "CUDA"),
        "vulkan" => prefer("vulkan", vulkan_available(), "Vulkan"),
        "metal" => prefer("metal", metal_available(), "Metal"),
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
    cfg!(feature = "vulkan") && cfg!(target_os = "linux") && Path::new("/dev/dri").exists()
}

fn metal_available() -> bool {
    cfg!(all(feature = "metal", target_os = "macos"))
}

fn nvidia_runtime_present() -> bool {
    Path::new("/proc/driver/nvidia/version").is_file() || Path::new("/dev/nvidia0").exists()
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
        "cuda",
        "cublas",
        "nvml",
        "vulkan",
        "ggml",
        "metal",
        "hipblas",
        "failed to allocate",
        "insufficient memory",
        "device lost",
        "no cuda",
        "vram",
    ];
    if NEEDLES.iter().any(|needle| lower.contains(needle)) {
        return true;
    }
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| word == "oom")
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
        if cfg!(all(feature = "vulkan", target_os = "linux")) && Path::new("/dev/dri").exists() {
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
        assert!(!is_gpu_failure("load a model before infer"));
    }

    #[test]
    fn parse_n_gpu_layers_ignores_blank_and_junk() {
        assert_eq!(parse_n_gpu_layers("32"), Some(32));
        assert_eq!(parse_n_gpu_layers(" 0 "), Some(0));
        assert_eq!(parse_n_gpu_layers(""), None);
        assert_eq!(parse_n_gpu_layers("auto"), None);
        assert_eq!(requested_n_gpu_layers(Some(8)), Some(8));
    }
}
