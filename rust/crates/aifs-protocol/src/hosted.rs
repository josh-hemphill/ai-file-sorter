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

const SECRET_QUERY_KEYS: &[&str] = &["key", "api_key", "token", "access_token"];

/// Redacts API keys, URL userinfo, and secret query params from UI-facing text.
pub fn sanitize_hosted_text(message: &str, api_key: Option<&str>) -> String {
    let mut out = message.to_owned();
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        out = out.replace(key, "<redacted>");
    }
    redact_urls(&out)
}

fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme) = rest.find("://") {
        out.push_str(&rest[..scheme]);
        out.push_str("://");
        let after = &rest[scheme + 3..];
        let url_end = after
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | '<' | '>' | ','))
            .unwrap_or(after.len());
        out.push_str(&redact_url_body(&after[..url_end]));
        rest = &after[url_end..];
    }
    out.push_str(rest);
    out
}

fn redact_url_body(body: &str) -> String {
    let (authority, tail) = match body.find('/') {
        Some(slash) => (&body[..slash], &body[slash..]),
        None => (body, ""),
    };
    let host = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };
    let (path, query) = match tail.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (tail, None),
    };
    let Some(query) = query else {
        return format!("{host}{path}");
    };
    let redacted = query
        .split('&')
        .map(|pair| {
            let key = pair.split('=').next().unwrap_or(pair);
            if SECRET_QUERY_KEYS
                .iter()
                .any(|secret| secret.eq_ignore_ascii_case(key))
            {
                format!("{key}=<redacted>")
            } else {
                pair.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{host}{path}?{redacted}")
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

    #[test]
    fn sanitize_strips_keys_userinfo_and_query() {
        let leaked = sanitize_hosted_text(
            "Reached http://user:pass@127.0.0.1/v1?api_key=secret123&q=ok Bearer sk-live",
            Some("sk-live"),
        );
        assert!(!leaked.contains("sk-live"), "{leaked}");
        assert!(!leaked.contains("secret123"), "{leaked}");
        assert!(!leaked.contains("user:pass"), "{leaked}");
        assert!(leaked.contains("<redacted>"), "{leaked}");
        assert!(leaked.contains("q=ok"), "{leaked}");
    }
}
