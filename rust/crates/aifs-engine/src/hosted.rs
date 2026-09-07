//! HTTP probes for hosted model endpoints. Keys are never logged.

use aifs_protocol::{
    ModelBackend, OPENAI_MODELS_URL, custom_chat_url, gemini_model_url, is_hosted_backend,
};
use serde_json::json;
use std::io::Read;
use std::time::Duration;

const PROBE_CONNECT: Duration = Duration::from_secs(10);
const PROBE_READ: Duration = Duration::from_secs(15);
const RESPONSE_CHARS: usize = 4096;

/// Contacts a hosted endpoint. Local GGUF backends are not probed over HTTP.
pub fn probe_hosted(backend: &ModelBackend, api_key: Option<&str>) -> (bool, String) {
    if !is_hosted_backend(backend) {
        return (false, "not a hosted backend".to_owned());
    }
    match backend {
        ModelBackend::OpenAi { .. } => probe_get(OPENAI_MODELS_URL, api_key, AuthStyle::Bearer),
        ModelBackend::Gemini { model } => {
            probe_get(&gemini_model_url(model), api_key, AuthStyle::Gemini)
        }
        ModelBackend::CustomEndpoint { base_url, model } => {
            probe_custom(&custom_chat_url(base_url), model, api_key)
        }
        _ => (false, "not a hosted backend".to_owned()),
    }
}

#[derive(Clone, Copy)]
enum AuthStyle {
    Bearer,
    Gemini,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(PROBE_CONNECT)
        .timeout_read(PROBE_READ)
        .build()
}

fn probe_get(url: &str, api_key: Option<&str>, auth: AuthStyle) -> (bool, String) {
    let mut request = agent().get(url);
    request = apply_auth(request, api_key, auth);
    match request.call() {
        Ok(response) => {
            let status = response.status();
            drain(response, api_key);
            if (200..300).contains(&status) {
                (true, format!("Reached {url} ({status})."))
            } else {
                (
                    false,
                    format!("Reached {url} but the server returned {status}."),
                )
            }
        }
        Err(ureq::Error::Status(status, response)) => {
            drain(response, api_key);
            if status == 401 || status == 403 {
                (
                    false,
                    format!("Reached {url} ({status}); check the API key."),
                )
            } else {
                (
                    false,
                    format!("Reached {url} but the server returned {status}."),
                )
            }
        }
        Err(error) => (false, sanitize_error(&error.to_string(), api_key)),
    }
}

fn probe_custom(url: &str, model: &str, api_key: Option<&str>) -> (bool, String) {
    let body = json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "ping"}],
    });
    let mut request = agent().post(url);
    request = apply_auth(request, api_key, AuthStyle::Bearer);
    match request.send_json(body) {
        Ok(response) => {
            let status = response.status();
            drain(response, api_key);
            if (200..300).contains(&status) {
                (true, format!("Reached {url} ({status})."))
            } else {
                (
                    false,
                    format!("Reached {url} but the server returned {status}."),
                )
            }
        }
        Err(ureq::Error::Status(status, response)) => {
            drain(response, api_key);
            if status == 401 || status == 403 {
                (
                    false,
                    format!("Reached {url} ({status}); check the API key."),
                )
            } else {
                (
                    false,
                    format!("Reached {url} but the server returned {status}."),
                )
            }
        }
        Err(error) => (false, sanitize_error(&error.to_string(), api_key)),
    }
}

fn apply_auth(request: ureq::Request, api_key: Option<&str>, auth: AuthStyle) -> ureq::Request {
    match (auth, api_key.filter(|key| !key.is_empty())) {
        (AuthStyle::Bearer, Some(key)) => request.set("Authorization", &format!("Bearer {key}")),
        (AuthStyle::Gemini, Some(key)) => request.set("x-goog-api-key", key),
        _ => request,
    }
}

fn drain(response: ureq::Response, api_key: Option<&str>) {
    let mut raw = String::new();
    let _ = response
        .into_reader()
        .take(RESPONSE_CHARS as u64)
        .read_to_string(&mut raw);
    let _ = sanitize_error(&raw, api_key);
}

fn sanitize_error(message: &str, api_key: Option<&str>) -> String {
    let mut out = message.to_owned();
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        out = out.replace(key, "<redacted>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_keys() {
        let text = sanitize_error("failed Bearer sk-live", Some("sk-live"));
        assert!(!text.contains("sk-live"), "{text}");
    }
}
