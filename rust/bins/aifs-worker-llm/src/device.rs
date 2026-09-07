//! Device selection for the LLM worker.

use std::path::Path;

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
}
