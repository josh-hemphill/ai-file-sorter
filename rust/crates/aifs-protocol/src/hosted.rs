//! Hosted backend URLs. HTTP happens in the engine (probe) and LLM worker (infer).

use crate::ModelBackend;

/// OpenAI chat-completions endpoint.
pub const OPENAI_CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";

/// OpenAI models list used for probes.
pub const OPENAI_MODELS_URL: &str = "https://api.openai.com/v1/models";

const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// True when the backend talks to a remote HTTP API.
pub fn is_hosted_backend(backend: &ModelBackend) -> bool {
    matches!(
        backend,
        ModelBackend::OpenAi { .. }
            | ModelBackend::Gemini { .. }
            | ModelBackend::CustomEndpoint { .. }
    )
}

/// Label used in evidence and loaded events.
pub fn hosted_model_label(backend: &ModelBackend) -> String {
    match backend {
        ModelBackend::OpenAi { model } => format!("openai:{model}"),
        ModelBackend::Gemini { model } => format!("gemini:{model}"),
        ModelBackend::CustomEndpoint { model, .. } => format!("custom:{model}"),
        ModelBackend::Off | ModelBackend::Catalog { .. } | ModelBackend::LocalGguf { .. } => {
            "hosted".to_owned()
        }
    }
}

/// Gemini generateContent URL (key is sent as a header, never in the URL).
pub fn gemini_generate_url(model: &str) -> String {
    format!("{GEMINI_BASE}/{model}:generateContent")
}

/// Gemini model GET used for probes.
pub fn gemini_model_url(model: &str) -> String {
    format!("{GEMINI_BASE}/{model}")
}

/// Custom OpenAI-compatible chat URL. Accepts a base or a full `/chat/completions` path.
pub fn custom_chat_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_url_appends_chat_completions_once() {
        assert_eq!(
            custom_chat_url("http://127.0.0.1:9/v1"),
            "http://127.0.0.1:9/v1/chat/completions"
        );
        assert_eq!(
            custom_chat_url("http://127.0.0.1:9/v1/chat/completions/"),
            "http://127.0.0.1:9/v1/chat/completions"
        );
    }

    #[test]
    fn gemini_urls_do_not_embed_a_key() {
        let url = gemini_generate_url("gemini-2.0-flash");
        assert!(url.contains("gemini-2.0-flash"));
        assert!(!url.contains("key="));
    }
}
