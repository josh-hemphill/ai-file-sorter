//! Device selection for the LLM worker. The stub never reports an accelerator.

/// Resolves `gpu_preference` to a device id and optional fallback message.
pub fn resolve_device(preference: &str) -> (String, Option<String>) {
    let pref = preference.trim();
    let pref = if pref.is_empty() { "auto" } else { pref };
    match pref {
        "cpu" | "auto" => ("cpu".to_owned(), None),
        "cuda" => cpu_fallback("CUDA"),
        "vulkan" => cpu_fallback("Vulkan"),
        "metal" => cpu_fallback("Metal"),
        other => (
            "cpu".to_owned(),
            Some(format!("unknown gpu_preference {other}; using cpu")),
        ),
    }
}

fn cpu_fallback(label: &str) -> (String, Option<String>) {
    (
        "cpu".to_owned(),
        Some(format!(
            "{label} requested but this worker build cannot use it; using cpu"
        )),
    )
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
    fn cuda_preference_falls_back_until_llama() {
        let (device, fallback) = resolve_device("cuda");
        assert_eq!(device, "cpu");
        assert!(
            fallback
                .as_deref()
                .is_some_and(|text| text.contains("CUDA"))
        );
    }
}
