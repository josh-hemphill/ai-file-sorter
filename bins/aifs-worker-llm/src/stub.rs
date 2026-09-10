//! Canned infer used until llama.cpp or a hosted backend is loaded.

use crate::device::{requested_n_gpu_layers, resolve_device};
use aifs_domain::evidence::keys;
use aifs_domain::{Confidence, EntryKind, Evidence, EvidenceSource, FileFamily, ObservedEntry};
use aifs_protocol::{FolderStyle, ModelBackend};
use aifs_worker_runtime::{LoadedModel, WorkerHandler};
use std::path::Path;
use std::time::Duration;

const STUB_CONFIDENCE: f32 = 0.4;
const CHAT_UTTERANCE_CHARS: usize = 200;
const TEST_INFER_SLEEP_ENV: &str = "AIFS_TEST_INFER_SLEEP_MS";

/// Session-lived stub backend. Does not read GGUF bytes.
#[derive(Default)]
pub struct StubHandler {
    loaded: Option<LoadedModel>,
}

impl WorkerHandler for StubHandler {
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
        gpu_preference: &str,
        n_gpu_layers: Option<u32>,
        api_key: Option<String>,
        _storage_dir: &str,
    ) -> Result<LoadedModel, String> {
        let _ = api_key;
        if matches!(backend, ModelBackend::Off) {
            return Err("cannot load an off slot".to_owned());
        }
        let model = model_label(&backend);
        let (device, fallback) = resolve_device(gpu_preference);
        let n_gpu_layers = if device == "cpu" {
            0
        } else {
            requested_n_gpu_layers(n_gpu_layers).unwrap_or(0)
        };
        let loaded = LoadedModel {
            device,
            model,
            n_gpu_layers,
            fallback,
        };
        self.loaded = Some(loaded.clone());
        Ok(loaded)
    }

    fn unload(&mut self) -> Result<(), String> {
        self.loaded = None;
        Ok(())
    }

    fn categorize(
        &mut self,
        _root: &Path,
        entry: &ObservedEntry,
        _evidence: &[Evidence],
        allowed_categories: &[String],
        _style: FolderStyle,
    ) -> Result<Option<Evidence>, String> {
        maybe_test_infer_sleep();
        let loaded = self.require_loaded()?;
        if entry.kind != EntryKind::File {
            return Ok(None);
        }
        Ok(Some(
            Evidence::new(
                entry.id,
                EvidenceSource::LocalModel {
                    model: loaded.model.clone(),
                },
                Confidence::new(STUB_CONFIDENCE),
            )
            .with_fact(keys::CATEGORY, stub_category(entry, allowed_categories))
            .with_fact(keys::DESCRIPTION, stub_description(entry))
            .with_fact(keys::SUGGESTED_NAME, entry.path.file_name()),
        ))
    }

    fn describe(
        &mut self,
        _root: &Path,
        entry: &ObservedEntry,
        _evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        maybe_test_infer_sleep();
        let loaded = self.require_loaded()?;
        if entry.kind != EntryKind::File {
            return Ok(None);
        }
        if !matches!(entry.family, FileFamily::Image | FileFamily::RawImage) {
            return Ok(None);
        }
        Ok(Some(
            Evidence::new(
                entry.id,
                EvidenceSource::LocalModel {
                    model: loaded.model.clone(),
                },
                Confidence::new(STUB_CONFIDENCE),
            )
            .with_fact(
                keys::DESCRIPTION,
                format!("stub vision description of {}", entry.path.as_str()),
            ),
        ))
    }

    fn chat(&mut self, utterance: &str, _context: &str) -> Result<String, String> {
        let _ = self.require_loaded()?;
        Ok(format!(
            "stub chat; engine should run tools. utterance={}",
            utterance
                .chars()
                .take(CHAT_UTTERANCE_CHARS)
                .collect::<String>()
        ))
    }
}

impl StubHandler {
    fn require_loaded(&self) -> Result<&LoadedModel, String> {
        self.loaded
            .as_ref()
            .ok_or_else(|| "load a model before infer".to_owned())
    }
}

fn maybe_test_infer_sleep() {
    let Ok(raw) = std::env::var(TEST_INFER_SLEEP_ENV) else {
        return;
    };
    let Ok(ms) = raw.parse::<u64>() else {
        return;
    };
    if ms == 0 {
        return;
    }
    std::thread::sleep(Duration::from_millis(ms));
}

fn model_label(backend: &ModelBackend) -> String {
    match backend {
        ModelBackend::Off => "off".to_owned(),
        ModelBackend::Catalog { catalog_id } => catalog_id.clone(),
        ModelBackend::LocalGguf { path, .. } => path.clone(),
        ModelBackend::OpenAi { model } => format!("openai:{model}"),
        ModelBackend::Gemini { model } => format!("gemini:{model}"),
        ModelBackend::CustomEndpoint { model, .. } => format!("custom:{model}"),
    }
}

fn stub_category(entry: &ObservedEntry, allowed: &[String]) -> String {
    let fallback = entry.family.default_folder();
    if allowed.is_empty() {
        return fallback.to_owned();
    }
    if allowed
        .iter()
        .any(|name| name.eq_ignore_ascii_case(fallback))
    {
        return fallback.to_owned();
    }
    allowed[0].clone()
}

fn stub_description(entry: &ObservedEntry) -> String {
    format!(
        "stub category {} for {}",
        entry.family.default_folder(),
        entry.path.as_str()
    )
}
