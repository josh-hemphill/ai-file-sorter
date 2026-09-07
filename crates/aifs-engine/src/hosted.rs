//! HTTP probes for hosted model endpoints. Keys are never logged.

use aifs_protocol::{
    ModelBackend, OPENAI_MODELS_URL, custom_chat_url, gemini_model_url, is_hosted_backend,
    sanitize_hosted_text,
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
        .redirects(0)
        .build()
}

fn probe_get(url: &str, api_key: Option<&str>, auth: AuthStyle) -> (bool, String) {
    let mut request = agent().get(url);
    request = apply_auth(request, api_key, auth);
    match request.call() {
        Ok(response) => finish_probe(url, response.status(), Some(response), api_key),
        Err(ureq::Error::Status(status, response)) => {
            finish_probe(url, status, Some(response), api_key)
        }
        Err(error) => (false, sanitize_hosted_text(&error.to_string(), api_key)),
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
        Ok(response) => finish_probe(url, response.status(), Some(response), api_key),
        Err(ureq::Error::Status(status, response)) => {
            finish_probe(url, status, Some(response), api_key)
        }
        Err(error) => (false, sanitize_hosted_text(&error.to_string(), api_key)),
    }
}

fn finish_probe(
    url: &str,
    status: u16,
    response: Option<ureq::Response>,
    api_key: Option<&str>,
) -> (bool, String) {
    if let Some(response) = response {
        drain(response);
    }
    let raw = if (200..300).contains(&status) {
        format!("Reached {url} ({status}).")
    } else if status == 401 || status == 403 {
        format!("Reached {url} ({status}); check the API key.")
    } else if (300..400).contains(&status) {
        format!("Reached {url} ({status}); redirects are not followed.")
    } else {
        format!("Reached {url} but the server returned {status}.")
    };
    (
        (200..300).contains(&status),
        sanitize_hosted_text(&raw, api_key),
    )
}

fn apply_auth(request: ureq::Request, api_key: Option<&str>, auth: AuthStyle) -> ureq::Request {
    match (auth, api_key.filter(|key| !key.is_empty())) {
        (AuthStyle::Bearer, Some(key)) => request.set("Authorization", &format!("Bearer {key}")),
        (AuthStyle::Gemini, Some(key)) => request.set("x-goog-api-key", key),
        _ => request,
    }
}

fn drain(response: ureq::Response) {
    let mut raw = String::new();
    let _ = response
        .into_reader()
        .take(RESPONSE_CHARS as u64)
        .read_to_string(&mut raw);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_stub::{serve_json_once, serve_redirect_once};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[test]
    fn probe_does_not_follow_redirects_or_leak_keys() {
        let stolen = Arc::new(AtomicBool::new(false));
        let stolen_flag = stolen.clone();
        let sink =
            std::net::TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error}"));
        let sink_addr = sink.local_addr().unwrap_or_else(|error| panic!("{error}"));
        let sink_thread = std::thread::spawn(move || {
            let _ = sink.set_nonblocking(true);
            for _ in 0..40 {
                if sink.accept().is_ok() {
                    stolen_flag.store(true, Ordering::SeqCst);
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        let (url, source) = serve_redirect_once(format!("http://{sink_addr}/stolen"));
        let (ok, message) = probe_get(&url, Some("sk-secret"), AuthStyle::Gemini);
        assert!(!ok, "{message}");
        assert!(message.contains("302"), "{message}");
        assert!(!message.contains("sk-secret"), "{message}");
        assert!(
            !stolen.load(Ordering::SeqCst),
            "probe followed the redirect and forwarded the Gemini key"
        );
        let _ = source.join();
        let _ = sink_thread.join();
    }

    #[test]
    fn json_stub_reads_a_large_post_before_replying() {
        let (base, server) = serve_json_once("200 OK", r#"{"id":"ok"}"#);
        let url = custom_chat_url(&base);
        let payload = "x".repeat(16_384);
        let result = ureq::post(&url)
            .timeout(Duration::from_secs(5))
            .send_json(json!({
                "model": "local-test",
                "messages": [{"role": "user", "content": payload}],
            }));
        assert!(result.is_ok(), "{result:?}");
        let request = server.join().unwrap_or_else(|error| panic!("{error:?}"));
        assert!(request.contains("Content-Length"), "{request}");
        assert!(
            request.len() > 16_000,
            "stub closed before the 16KiB body arrived ({})",
            request.len()
        );
    }
}
