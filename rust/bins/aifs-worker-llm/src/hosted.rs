//! Hosted OpenAI / Gemini / custom HTTP infer. Keys are never logged.

use crate::parse::{apply_parsed, parse_infer_json};
use crate::prompt::{
    CATEGORIZE_SYSTEM, CHAT_SYSTEM, DESCRIBE_SYSTEM, categorize_user, describe_user,
};
use aifs_domain::{Confidence, EntryKind, Evidence, EvidenceSource, FileFamily, ObservedEntry};
use aifs_protocol::{
    ModelBackend, OPENAI_CHAT_URL, custom_chat_url, gemini_generate_url, hosted_model_label,
    is_hosted_backend, sanitize_hosted_text,
};
use aifs_worker_runtime::{LoadedModel, WorkerHandler};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

const INFER_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_GEN_TOKENS: u32 = 128;
const CHAT_GEN_TOKENS: u32 = 256;
const HOSTED_CONFIDENCE: f32 = 0.55;
const RESPONSE_CHARS: usize = 32_768;

struct HostedSession {
    backend: ModelBackend,
    api_key: Option<String>,
    info: LoadedModel,
}

/// Session-lived remote HTTP backend.
#[derive(Default)]
pub struct HostedHandler {
    loaded: Option<HostedSession>,
}

impl WorkerHandler for HostedHandler {
    fn extract(
        &mut self,
        _root: &Path,
        _entry: &ObservedEntry,
    ) -> Result<Option<Evidence>, String> {
        Err("llm worker does not extract files".to_owned())
    }

    fn load(
        &mut self,
        backend: ModelBackend,
        _gpu_preference: &str,
        _n_gpu_layers: Option<u32>,
        api_key: Option<String>,
        _storage_dir: &str,
    ) -> Result<LoadedModel, String> {
        if !is_hosted_backend(&backend) {
            return Err("not a hosted backend".to_owned());
        }
        if matches!(backend, ModelBackend::Off) {
            return Err("cannot load an off slot".to_owned());
        }
        if requires_key(&backend) && api_key.as_ref().is_none_or(|key| key.trim().is_empty()) {
            return Err("hosted load requires an API key".to_owned());
        }
        let info = LoadedModel {
            device: "cpu".to_owned(),
            model: hosted_model_label(&backend),
            n_gpu_layers: 0,
            fallback: Some("hosted HTTP; no local accelerator".to_owned()),
        };
        self.loaded = Some(HostedSession {
            backend,
            api_key: api_key.filter(|key| !key.trim().is_empty()),
            info: info.clone(),
        });
        Ok(info)
    }

    fn unload(&mut self) -> Result<(), String> {
        self.loaded = None;
        Ok(())
    }

    fn categorize(
        &mut self,
        _root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        let model_id = self.require_loaded()?.info.model.clone();
        if entry.kind != EntryKind::File {
            return Ok(None);
        }
        let text = self.complete(
            CATEGORIZE_SYSTEM,
            &categorize_user(entry, evidence),
            MAX_GEN_TOKENS,
        )?;
        Ok(evidence_from_text(&model_id, entry, &text, true))
    }

    fn describe(
        &mut self,
        _root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        let model_id = self.require_loaded()?.info.model.clone();
        if entry.kind != EntryKind::File {
            return Ok(None);
        }
        if !matches!(entry.family, FileFamily::Image | FileFamily::RawImage) {
            return Ok(None);
        }
        let text = self.complete(
            DESCRIBE_SYSTEM,
            &describe_user(entry, evidence),
            MAX_GEN_TOKENS,
        )?;
        Ok(evidence_from_text(&model_id, entry, &text, false))
    }

    fn chat(&mut self, utterance: &str, context: &str) -> Result<String, String> {
        let _ = self.require_loaded()?;
        let user = if context.trim().is_empty() {
            utterance.to_owned()
        } else {
            format!("{utterance}\n\nContext:\n{context}")
        };
        self.complete(CHAT_SYSTEM, &user, CHAT_GEN_TOKENS)
    }
}

impl HostedHandler {
    fn require_loaded(&self) -> Result<&HostedSession, String> {
        self.loaded
            .as_ref()
            .ok_or_else(|| "load a model before infer".to_owned())
    }

    fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Result<String, String> {
        let loaded = self.require_loaded()?;
        chat_completion(
            &loaded.backend,
            loaded.api_key.as_deref(),
            system,
            user,
            max_tokens,
        )
    }
}

fn requires_key(backend: &ModelBackend) -> bool {
    matches!(
        backend,
        ModelBackend::OpenAi { .. } | ModelBackend::Gemini { .. }
    )
}

fn evidence_from_text(
    model: &str,
    entry: &ObservedEntry,
    text: &str,
    want_category: bool,
) -> Option<Evidence> {
    let parsed = parse_infer_json(text)?;
    if want_category && parsed.category.is_none() {
        return None;
    }
    if !want_category && parsed.description.is_none() {
        return None;
    }
    let mut bag = Evidence::new(
        entry.id,
        EvidenceSource::RemoteModel {
            model: model.to_owned(),
        },
        Confidence::new(HOSTED_CONFIDENCE),
    );
    apply_parsed(&parsed, |key, value| {
        bag.facts.insert(key.to_owned(), value.to_owned());
    });
    Some(bag)
}

fn chat_completion(
    backend: &ModelBackend,
    api_key: Option<&str>,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    match backend {
        ModelBackend::OpenAi { model } => {
            openai_complete(OPENAI_CHAT_URL, model, api_key, system, user, max_tokens)
        }
        ModelBackend::CustomEndpoint { base_url, model } => openai_complete(
            &custom_chat_url(base_url),
            model,
            api_key,
            system,
            user,
            max_tokens,
        ),
        ModelBackend::Gemini { model } => gemini_complete(model, api_key, system, user, max_tokens),
        _ => Err("not a hosted backend".to_owned()),
    }
}

fn openai_complete(
    url: &str,
    model: &str,
    api_key: Option<&str>,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "temperature": 0,
        "max_tokens": max_tokens,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    });
    let mut request = agent().post(url);
    if let Some(key) = api_key {
        request = request.set("Authorization", &format!("Bearer {key}"));
    }
    let raw = send_json(request, &body, api_key)?;
    let parsed: OpenAiChat = serde_json::from_str(&raw)
        .map_err(|error| sanitize_hosted_text(&error.to_string(), api_key))?;
    parsed
        .choices
        .first()
        .and_then(|choice| choice.message.content.clone())
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "hosted chat returned an empty message".to_owned())
}

fn gemini_complete(
    model: &str,
    api_key: Option<&str>,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let url = gemini_generate_url(model);
    let body = json!({
        "system_instruction": {"parts": [{"text": system}]},
        "contents": [{"role": "user", "parts": [{"text": user}]}],
        "generationConfig": {"maxOutputTokens": max_tokens, "temperature": 0},
    });
    let mut request = agent().post(&url);
    if let Some(key) = api_key {
        request = request.set("x-goog-api-key", key);
    }
    let raw = send_json(request, &body, api_key)?;
    let parsed: GeminiResponse = serde_json::from_str(&raw)
        .map_err(|error| sanitize_hosted_text(&error.to_string(), api_key))?;
    parsed
        .candidates
        .first()
        .and_then(|candidate| candidate.content.parts.first())
        .and_then(|part| part.text.clone())
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "hosted chat returned an empty message".to_owned())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(INFER_TIMEOUT)
        .redirects(0)
        .build()
}

fn send_json(
    request: ureq::Request,
    body: &Value,
    api_key: Option<&str>,
) -> Result<String, String> {
    let response = request
        .set("Content-Type", "application/json")
        .send_json(body.clone())
        .map_err(|error| sanitize_hosted_text(&error.to_string(), api_key))?;
    let status = response.status();
    let mut raw = String::new();
    response
        .into_reader()
        .take(RESPONSE_CHARS as u64)
        .read_to_string(&mut raw)
        .map_err(|error| sanitize_hosted_text(&error.to_string(), api_key))?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "hosted HTTP {status}: {}",
            truncate(&sanitize_hosted_text(&raw, api_key), 300)
        ));
    }
    Ok(raw)
}

fn truncate(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        value.to_owned()
    } else {
        value.chars().take(max_chars).collect()
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChat {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    #[serde(default)]
    content: GeminiContent,
}

#[derive(Debug, Default, Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_does_not_keep_the_key() {
        let leaked = sanitize_hosted_text("401 Authorization Bearer sk-secret", Some("sk-secret"));
        assert!(!leaked.contains("sk-secret"), "{leaked}");
        assert!(leaked.contains("<redacted>"), "{leaked}");
    }
}
