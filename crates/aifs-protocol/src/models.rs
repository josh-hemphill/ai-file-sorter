//! Model slot configuration. Keys are stored by the engine and redacted on `get_models`.

use crate::catalog::{
    all_artifacts, artifact_bytes_on_disk, artifact_is_verified, artifact_path, catalog_entry,
    catalog_id_is_downloaded, catalog_ids_for_artifact, expected_sha256,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Built-in catalog entries the Setup UI can assign without a filesystem path.
pub const BUILTIN_CATALOG: &[(&str, &str)] = &[
    ("gemma-3-4b-it", "Gemma 3 4B Instruct (text)"),
    (
        "gemma-3-4b-it-mmproj",
        "Gemma 3 4B Instruct + mmproj (vision)",
    ),
];

/// Analysis slots the workspace can assign independently.
pub const MODEL_SLOT_IDS: &[&str] = &["categorize", "vision", "document", "chat"];

/// Accelerator values the Setup UI can persist (`auto` prefers CUDA, then Vulkan/Metal, then CPU).
pub const GPU_PREFERENCES: &[&str] = &["auto", "cpu", "cuda", "vulkan", "metal"];

/// How a slot is backed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelBackend {
    /// Heuristics only.
    #[default]
    Off,
    /// Built-in GGUF to download into the storage directory later.
    Catalog {
        /// Catalog id such as `gemma-3-4b-it`.
        catalog_id: String,
    },
    /// User-supplied GGUF (and optional mmproj for vision).
    LocalGguf {
        /// Absolute path to the GGUF.
        path: String,
        /// Optional projector for vision models.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mmproj: Option<String>,
    },
    /// OpenAI-compatible hosted model.
    OpenAi {
        /// Model id.
        model: String,
    },
    /// Gemini hosted model.
    Gemini {
        /// Model id.
        model: String,
    },
    /// Custom chat-completions (or base URL) endpoint.
    CustomEndpoint {
        /// Base URL or full `/chat/completions` path.
        base_url: String,
        /// Model id.
        model: String,
    },
}

/// How infer will actually run. Computed on `get_models`; never persisted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlotRuntime {
    /// Slot is off; scan and propose stay heuristic.
    Off {
        /// Human explanation.
        detail: String,
    },
    /// Default `aifs-worker-llm` canned infer (no llama.cpp).
    Stub {
        /// Human explanation.
        detail: String,
    },
    /// OpenAI, Gemini, or custom HTTP through the LLM worker.
    Hosted {
        /// Human explanation.
        detail: String,
    },
    /// Local GGUF loaded via llama.cpp.
    Llama {
        /// Human explanation.
        detail: String,
    },
    /// Slot is assigned but `aifs-worker-llm` is not installed.
    MissingWorker {
        /// Human explanation.
        detail: String,
    },
    /// Local GGUF assignment whose files are not on disk.
    MissingFiles {
        /// Human explanation.
        detail: String,
    },
}

impl SlotRuntime {
    /// Wire `kind` for this runtime.
    pub fn kind_id(&self) -> &'static str {
        match self {
            Self::Off { .. } => "off",
            Self::Stub { .. } => "stub",
            Self::Hosted { .. } => "hosted",
            Self::Llama { .. } => "llama",
            Self::MissingWorker { .. } => "missing_worker",
            Self::MissingFiles { .. } => "missing_files",
        }
    }

    /// Human explanation.
    pub fn detail(&self) -> &str {
        match self {
            Self::Off { detail }
            | Self::Stub { detail }
            | Self::Hosted { detail }
            | Self::Llama { detail }
            | Self::MissingWorker { detail }
            | Self::MissingFiles { detail } => detail,
        }
    }

    /// True when scan/chat will call a real model (hosted HTTP or llama.cpp).
    pub fn is_live_infer(&self) -> bool {
        matches!(self, Self::Hosted { .. } | Self::Llama { .. })
    }
}

/// LLM worker hello result used to compute [`SlotRuntime`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LlmWorkerStatus {
    /// No `aifs-worker-llm` binary was found.
    Missing,
    /// Worker answered `hello` with these capability strings.
    Ready {
        /// Values such as `stub`, `llama`, and `hosted`.
        capabilities: Vec<String>,
    },
}

impl LlmWorkerStatus {
    /// True when the worker advertises llama.cpp infer.
    pub fn has_llama(&self) -> bool {
        match self {
            Self::Missing => false,
            Self::Ready { capabilities } => capabilities.iter().any(|cap| cap == "llama"),
        }
    }
}

/// One analysis slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSlot {
    /// `categorize`, `vision`, `document`, or `chat`.
    pub id: String,
    /// Backend assignment.
    #[serde(flatten)]
    pub backend: ModelBackend,
    /// Write-only API key. Omitted on `get_models`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// True when the engine has a key stored for this slot.
    #[serde(default)]
    pub api_key_set: bool,
    /// Filled by `get_models` / `put_models` / `download_model`. Not stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<SlotRuntime>,
}

impl Default for ModelSlot {
    fn default() -> Self {
        Self {
            id: "categorize".to_owned(),
            backend: ModelBackend::Off,
            api_key: None,
            api_key_set: false,
            runtime: None,
        }
    }
}

impl ModelSlot {
    /// Slot with a given id, off.
    pub fn off(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    /// Drops the secret and records whether one was present.
    pub fn redacted(&self) -> Self {
        Self {
            id: self.id.clone(),
            backend: self.backend.clone(),
            api_key: None,
            api_key_set: self.api_key.as_ref().is_some_and(|key| !key.is_empty())
                || self.api_key_set,
            runtime: self.runtime.clone(),
        }
    }
}

/// Engine-owned model inventory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelInventory {
    /// Directory for downloaded GGUF files.
    pub storage_dir: String,
    /// Preferred accelerator (`auto`, `cpu`, `cuda`, `vulkan`, `metal`).
    pub gpu_preference: String,
    /// Four analysis slots.
    pub slots: Vec<ModelSlot>,
    /// Catalog files under `storage_dir`. Computed on get/download; not a stored secret.
    #[serde(default)]
    pub artifacts: Vec<ModelArtifactStatus>,
}

/// One GGUF on disk that one or more catalog ids share.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelArtifactStatus {
    /// Stable artifact id such as `gemma-text-q4`.
    pub id: String,
    /// Filename under `storage_dir`.
    pub filename: String,
    /// Absolute path.
    pub path: String,
    /// Catalog size hint in bytes.
    pub expected_bytes: u64,
    /// Current file size, or `0` when missing.
    pub bytes_on_disk: u64,
    /// True when a finished file is present (incomplete `.part` files do not count).
    pub present: bool,
    /// Catalog ids that need this file.
    pub used_by: Vec<String>,
}

impl Default for ModelInventory {
    fn default() -> Self {
        Self {
            storage_dir: String::new(),
            gpu_preference: "auto".to_owned(),
            slots: MODEL_SLOT_IDS
                .iter()
                .map(|id| ModelSlot::off(*id))
                .collect(),
            artifacts: Vec::new(),
        }
    }
}

impl ModelInventory {
    /// Ensures the four slots exist and secrets are redacted.
    pub fn redacted(&self) -> Self {
        let mut next = self.clone();
        next.slots = Self::normalized_slots(&next.slots)
            .into_iter()
            .map(|slot| slot.redacted())
            .collect();
        next
    }

    /// Merges an incoming put with stored secrets when the client omitted a key.
    pub fn merge_secrets(self, previous: &ModelInventory) -> Self {
        let mut next = self;
        next.slots = Self::normalized_slots(&next.slots);
        for slot in &mut next.slots {
            if !backend_holds_secrets(&slot.backend) {
                slot.api_key = None;
                slot.api_key_set = false;
                continue;
            }
            match slot.api_key.as_deref().map(str::trim) {
                None => {
                    if let Some(stored) = previous
                        .slots
                        .iter()
                        .find(|candidate| candidate.id == slot.id)
                    {
                        slot.api_key = stored.api_key.clone();
                        slot.api_key_set = stored.api_key_set
                            || stored.api_key.as_ref().is_some_and(|key| !key.is_empty());
                    }
                }
                Some("") => {
                    slot.api_key = None;
                    slot.api_key_set = false;
                }
                Some(_) => {}
            }
        }
        next
    }

    /// Fills [`Self::artifacts`] from files already in `storage_dir`.
    pub fn with_disk_status(mut self, storage_dir: &Path) -> Self {
        self.artifacts = all_artifacts()
            .iter()
            .map(|artifact| {
                let path = artifact_path(storage_dir, artifact.filename);
                let bytes_on_disk = artifact_bytes_on_disk(&path);
                ModelArtifactStatus {
                    id: artifact.id.to_owned(),
                    filename: artifact.filename.to_owned(),
                    path: path.display().to_string(),
                    expected_bytes: artifact.expected_bytes,
                    bytes_on_disk,
                    present: artifact_is_verified(&path, &expected_sha256(artifact)),
                    used_by: catalog_ids_for_artifact(artifact.id)
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                }
            })
            .collect();
        self
    }

    /// Fills per-slot [`SlotRuntime`] from worker hello and files on disk.
    pub fn with_slot_runtime(mut self, worker: &LlmWorkerStatus, storage_dir: &Path) -> Self {
        for slot in &mut self.slots {
            slot.runtime = Some(slot_runtime(&slot.backend, worker, Some(storage_dir)));
        }
        self
    }

    fn normalized_slots(slots: &[ModelSlot]) -> Vec<ModelSlot> {
        MODEL_SLOT_IDS
            .iter()
            .map(|id| {
                slots
                    .iter()
                    .find(|slot| slot.id == *id)
                    .cloned()
                    .unwrap_or_else(|| ModelSlot::off(*id))
            })
            .collect()
    }
}

fn backend_holds_secrets(backend: &ModelBackend) -> bool {
    matches!(
        backend,
        ModelBackend::OpenAi { .. }
            | ModelBackend::Gemini { .. }
            | ModelBackend::CustomEndpoint { .. }
    )
}

/// Checks a backend without contacting model vendors.
pub fn probe_backend(backend: &ModelBackend) -> (bool, String) {
    probe_backend_at(backend, None)
}

/// Like [`probe_backend`], using `storage_dir` to report whether catalog files are already on disk.
pub fn probe_backend_at(backend: &ModelBackend, storage_dir: Option<&Path>) -> (bool, String) {
    match backend {
        ModelBackend::Off => (
            true,
            "Slot is off. Scan and propose still use heuristics.".to_owned(),
        ),
        ModelBackend::Catalog { catalog_id } => {
            if catalog_entry(catalog_id).is_none() {
                return (false, format!("Unknown catalog id {catalog_id}"));
            }
            let downloaded =
                storage_dir.is_some_and(|dir| catalog_id_is_downloaded(dir, catalog_id));
            if downloaded {
                (
                    true,
                    format!(
                        "{catalog_id} is already downloaded. Scan loads the LLM worker; infer uses llama.cpp when that worker is built with `--features llama`."
                    ),
                )
            } else {
                (
                    true,
                    format!(
                        "{catalog_id} is assigned. Use Download now to fetch the GGUF. Scan loads the LLM worker after download; infer uses llama.cpp when that worker is built with `--features llama`."
                    ),
                )
            }
        }
        ModelBackend::LocalGguf { path, mmproj } => {
            if !Path::new(path).is_file() {
                return (false, format!("{path} was not found"));
            }
            if let Some(proj) = mmproj
                && !proj.is_empty()
                && !Path::new(proj).is_file()
            {
                return (false, format!("{proj} was not found"));
            }
            (
                true,
                "Local GGUF found. Scan loads the LLM worker; infer uses llama.cpp when that worker is built with `--features llama`."
                    .to_owned(),
            )
        }
        ModelBackend::OpenAi { model } if model.trim().is_empty() => {
            (false, "OpenAI model id is required".to_owned())
        }
        ModelBackend::OpenAi { .. } => (
            true,
            "OpenAI assignment recorded. Probe contacts the API; scan loads the LLM worker for hosted infer."
                .to_owned(),
        ),
        ModelBackend::Gemini { model } if model.trim().is_empty() => {
            (false, "Gemini model id is required".to_owned())
        }
        ModelBackend::Gemini { .. } => (
            true,
            "Gemini assignment recorded. Probe contacts the API; scan loads the LLM worker for hosted infer."
                .to_owned(),
        ),
        ModelBackend::CustomEndpoint { base_url, model } => {
            let url = base_url.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return (false, "Custom endpoint must be an http(s) URL".to_owned());
            }
            if model.trim().is_empty() {
                return (false, "Custom endpoint model id is required".to_owned());
            }
            (
                true,
                "Custom endpoint recorded. Probe contacts the URL; scan loads the LLM worker for hosted infer."
                    .to_owned(),
            )
        }
    }
}

/// Resolves how infer will run for `backend` given worker hello and files on disk.
pub fn slot_runtime(
    backend: &ModelBackend,
    worker: &LlmWorkerStatus,
    storage_dir: Option<&Path>,
) -> SlotRuntime {
    if matches!(backend, ModelBackend::Off) {
        return SlotRuntime::Off {
            detail: "Slot is off. Scan and propose still use heuristics.".to_owned(),
        };
    }
    if matches!(worker, LlmWorkerStatus::Missing) {
        return SlotRuntime::MissingWorker {
            detail: "Slot is assigned but the LLM worker is not installed.".to_owned(),
        };
    }
    if matches!(
        backend,
        ModelBackend::OpenAi { .. }
            | ModelBackend::Gemini { .. }
            | ModelBackend::CustomEndpoint { .. }
    ) {
        return SlotRuntime::Hosted {
            detail: "Hosted infer via the LLM worker (OpenAI, Gemini, or custom HTTP).".to_owned(),
        };
    }
    if worker.has_llama() {
        if local_weights_present(backend, storage_dir) {
            return SlotRuntime::Llama {
                detail: "llama.cpp will load this GGUF when scan or chat runs.".to_owned(),
            };
        }
        return SlotRuntime::MissingFiles {
            detail: "Local slot assigned but the GGUF is not on disk.".to_owned(),
        };
    }
    SlotRuntime::Stub {
        detail: "Default LLM worker stubs infer. Rebuild aifs-worker-llm with `--features llama` to load GGUFs.".to_owned(),
    }
}

fn local_weights_present(backend: &ModelBackend, storage_dir: Option<&Path>) -> bool {
    match backend {
        ModelBackend::Catalog { catalog_id } => {
            storage_dir.is_some_and(|dir| catalog_id_is_downloaded(dir, catalog_id))
        }
        ModelBackend::LocalGguf { path, mmproj } => {
            if !Path::new(path).is_file() {
                return false;
            }
            match mmproj.as_deref().filter(|proj| !proj.is_empty()) {
                None => true,
                Some(proj) => Path::new(proj).is_file(),
            }
        }
        ModelBackend::Off
        | ModelBackend::OpenAi { .. }
        | ModelBackend::Gemini { .. }
        | ModelBackend::CustomEndpoint { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_models_omits_keys() {
        let mut inventory = ModelInventory::default();
        inventory.slots[0].backend = ModelBackend::OpenAi {
            model: "gpt-4.1-mini".into(),
        };
        inventory.slots[0].api_key = Some("sk-secret".into());
        let json = serde_json::to_string(&inventory.redacted()).unwrap_or_else(|e| panic!("{e}"));
        assert!(!json.contains("sk-secret"), "{json}");
        assert!(
            json.contains("api_key_set") || json.contains("true"),
            "{json}"
        );
    }

    #[test]
    fn put_keeps_previous_key_when_omitted() {
        let mut previous = ModelInventory::default();
        previous.slots[0].api_key = Some("sk-keep".into());
        let incoming = ModelInventory {
            slots: vec![ModelSlot {
                id: "categorize".into(),
                backend: ModelBackend::OpenAi {
                    model: "gpt-4.1-mini".into(),
                },
                api_key: None,
                api_key_set: true,
                runtime: None,
            }],
            ..ModelInventory::default()
        };
        let merged = incoming.merge_secrets(&previous);
        assert_eq!(merged.slots[0].api_key.as_deref(), Some("sk-keep"));
    }

    #[test]
    fn put_clears_key_when_blank_and_drops_secrets_on_off_slots() {
        let mut previous = ModelInventory::default();
        previous.slots[0].api_key = Some("sk-keep".into());
        previous.slots[0].backend = ModelBackend::OpenAi {
            model: "gpt-4.1-mini".into(),
        };
        let cleared = ModelInventory {
            slots: vec![ModelSlot {
                id: "categorize".into(),
                backend: ModelBackend::OpenAi {
                    model: "gpt-4.1-mini".into(),
                },
                api_key: Some(String::new()),
                api_key_set: true,
                runtime: None,
            }],
            ..ModelInventory::default()
        };
        let merged = cleared.merge_secrets(&previous);
        assert!(merged.slots[0].api_key.is_none());
        assert!(!merged.slots[0].api_key_set);

        let off = ModelInventory {
            slots: vec![ModelSlot {
                id: "categorize".into(),
                backend: ModelBackend::Off,
                api_key: None,
                api_key_set: true,
                runtime: None,
            }],
            ..ModelInventory::default()
        };
        let merged = off.merge_secrets(&previous);
        assert!(merged.slots[0].api_key.is_none());
        assert!(!merged.slots[0].api_key_set);
    }

    #[test]
    fn catalog_probe_distinguishes_downloaded_files() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let backend = ModelBackend::Catalog {
            catalog_id: "gemma-3-4b-it".into(),
        };
        let (_, pending) = probe_backend_at(&backend, Some(dir.path()));
        assert!(pending.contains("Download now"), "{pending}");
        let body = b"gguf";
        std::fs::write(
            crate::artifact_path(dir.path(), crate::GEMMA_TEXT_FILENAME),
            body,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let _pin =
            crate::ArtifactSha256Guard::pin(&[(crate::GEMMA_TEXT_FILENAME, body.as_slice())]);
        let (_, ready) = probe_backend_at(&backend, Some(dir.path()));
        assert!(ready.contains("already downloaded"), "{ready}");
        let status = ModelInventory::default().with_disk_status(dir.path());
        assert!(status.artifacts.iter().any(|artifact| artifact.present));
    }

    fn stub_worker() -> LlmWorkerStatus {
        LlmWorkerStatus::Ready {
            capabilities: vec!["stub".into(), "hosted".into()],
        }
    }

    fn llama_worker() -> LlmWorkerStatus {
        LlmWorkerStatus::Ready {
            capabilities: vec!["llama".into(), "hosted".into()],
        }
    }

    #[test]
    fn slot_runtime_is_off_for_heuristics() {
        let runtime = slot_runtime(&ModelBackend::Off, &LlmWorkerStatus::Missing, None);
        assert_eq!(runtime.kind_id(), "off");
        assert!(!runtime.is_live_infer());
    }

    #[test]
    fn slot_runtime_hosted_needs_the_worker() {
        let backend = ModelBackend::OpenAi {
            model: "gpt-4.1-mini".into(),
        };
        assert_eq!(
            slot_runtime(&backend, &LlmWorkerStatus::Missing, None).kind_id(),
            "missing_worker"
        );
        let hosted = slot_runtime(&backend, &stub_worker(), None);
        assert_eq!(hosted.kind_id(), "hosted");
        assert!(hosted.is_live_infer());
    }

    #[test]
    fn slot_runtime_catalog_stays_stub_even_when_files_exist() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            crate::artifact_path(dir.path(), crate::GEMMA_TEXT_FILENAME),
            b"gguf",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let backend = ModelBackend::Catalog {
            catalog_id: "gemma-3-4b-it".into(),
        };
        let runtime = slot_runtime(&backend, &stub_worker(), Some(dir.path()));
        assert_eq!(runtime.kind_id(), "stub");
        assert!(!runtime.is_live_infer());
        assert!(
            runtime.detail().contains("stubs infer"),
            "{}",
            runtime.detail()
        );
    }

    #[test]
    fn slot_runtime_llama_needs_files_on_disk() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let backend = ModelBackend::Catalog {
            catalog_id: "gemma-3-4b-it".into(),
        };
        assert_eq!(
            slot_runtime(&backend, &llama_worker(), Some(dir.path())).kind_id(),
            "missing_files"
        );
        std::fs::write(
            crate::artifact_path(dir.path(), crate::GEMMA_TEXT_FILENAME),
            b"gguf",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            slot_runtime(&backend, &llama_worker(), Some(dir.path())).kind_id(),
            "missing_files"
        );
        let _pin =
            crate::ArtifactSha256Guard::pin(&[(crate::GEMMA_TEXT_FILENAME, b"gguf".as_slice())]);
        let ready = slot_runtime(&backend, &llama_worker(), Some(dir.path()));
        assert_eq!(ready.kind_id(), "llama");
        assert!(ready.is_live_infer());
    }

    #[test]
    fn slot_runtime_local_gguf_checks_weights_and_mmproj() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let weights = dir.path().join("model.gguf");
        let proj = dir.path().join("mmproj.gguf");
        std::fs::write(&weights, b"gguf").unwrap_or_else(|error| panic!("{error}"));
        let missing_proj = ModelBackend::LocalGguf {
            path: weights.display().to_string(),
            mmproj: Some(proj.display().to_string()),
        };
        assert_eq!(
            slot_runtime(&missing_proj, &llama_worker(), None).kind_id(),
            "missing_files"
        );
        std::fs::write(&proj, b"proj").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            slot_runtime(&missing_proj, &llama_worker(), None).kind_id(),
            "llama"
        );
        assert_eq!(
            slot_runtime(&missing_proj, &LlmWorkerStatus::Missing, None).kind_id(),
            "missing_worker"
        );
    }

    #[test]
    fn runtime_omitted_from_stored_json_when_none() {
        let slot = ModelSlot::off("chat");
        let json = serde_json::to_string(&slot).unwrap_or_else(|error| panic!("{error}"));
        assert!(!json.contains("runtime"), "{json}");
        let mut with_runtime = slot;
        with_runtime.runtime = Some(slot_runtime(
            &ModelBackend::Gemini {
                model: "gemini-2.0-flash".into(),
            },
            &stub_worker(),
            None,
        ));
        let json = serde_json::to_string(&with_runtime).unwrap_or_else(|error| panic!("{error}"));
        assert!(json.contains("\"kind\":\"hosted\""), "{json}");
        assert!(json.contains("runtime"), "{json}");
    }
}
