//! Device selection for the LLM worker. CUDA/Vulkan/Metal require a matching feature build.

/// Resolves `gpu_preference` to a device id and optional fallback message.
pub fn resolve_device(preference: &str) -> (String, Option<String>) {
    let pref = preference.trim();
    let pref = if pref.is_empty() { "auto" } else { pref };
    match pref {
        "cpu" => ("cpu".to_owned(), None),
        "cuda" => pick("cuda", cuda_ready(), "CUDA"),
        "vulkan" => pick("vulkan", vulkan_ready(), "Vulkan"),
        "metal" => pick("metal", metal_ready(), "Metal"),
        "auto" => auto_device(),
        other => (
            "cpu".to_owned(),
            Some(format!("unknown gpu_preference {other}; using cpu")),
        ),
    }
}

fn auto_device() -> (String, Option<String>) {
    if cuda_ready() {
        return ("cuda".to_owned(), None);
    }
    if vulkan_ready() {
        return ("vulkan".to_owned(), None);
    }
    if metal_ready() {
        return ("metal".to_owned(), None);
    }
    ("cpu".to_owned(), None)
}

fn pick(name: &str, ready: bool, label: &str) -> (String, Option<String>) {
    if ready {
        (name.to_owned(), None)
    } else {
        (
            "cpu".to_owned(),
            Some(format!(
                "{label} requested but this worker build cannot use it; using cpu"
            )),
        )
    }
}

fn cuda_ready() -> bool {
    cfg!(feature = "cuda") && nvidia_present()
}

fn vulkan_ready() -> bool {
    cfg!(feature = "vulkan")
}

fn metal_ready() -> bool {
    cfg!(feature = "metal") && cfg!(target_os = "macos")
}

fn nvidia_present() -> bool {
    std::path::Path::new("/proc/driver/nvidia/version").is_file()
        || std::process::Command::new("nvidia-smi")
            .arg("-L")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
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
    fn cuda_without_feature_falls_back() {
        if cfg!(feature = "cuda") {
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
}
