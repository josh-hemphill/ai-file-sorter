//! Model slot configuration. Keys are stored by the engine and redacted on `get_models`.

use crate::catalog::{
    all_artifacts, artifact_bytes_on_disk, artifact_is_present, artifact_path, catalog_entry,
    catalog_id_is_downloaded, catalog_ids_for_artifact,
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
}

impl Default for ModelSlot {
    fn default() -> Self {
        Self {
            id: "categorize".to_owned(),
            backend: ModelBackend::Off,
            api_key: None,
            api_key_set: false,
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
                    present: artifact_is_present(&path),
                    used_by: catalog_ids_for_artifact(artifact.id)
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                }
            })
            .collect();
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
                        "{catalog_id} is already downloaded. Local workers are not connected yet — scan still uses heuristics."
                    ),
                )
            } else {
                (
                    true,
                    format!(
                        "{catalog_id} is assigned. Use Download now to fetch the GGUF. Scan still uses heuristics until analysis workers exist."
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
                "Local GGUF found. Scan still uses heuristics until analysis workers exist."
                    .to_owned(),
            )
        }
        ModelBackend::OpenAi { model } if model.trim().is_empty() => {
            (false, "OpenAI model id is required".to_owned())
        }
        ModelBackend::OpenAi { .. } => (
            true,
            "OpenAI assignment recorded. Scan still uses heuristics until analysis workers exist."
                .to_owned(),
        ),
        ModelBackend::Gemini { model } if model.trim().is_empty() => {
            (false, "Gemini model id is required".to_owned())
        }
        ModelBackend::Gemini { .. } => (
            true,
            "Gemini assignment recorded. Scan still uses heuristics until analysis workers exist."
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
                "Custom endpoint recorded. Scan still uses heuristics until analysis workers exist."
                    .to_owned(),
            )
        }
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
        std::fs::write(
            crate::artifact_path(dir.path(), crate::GEMMA_TEXT_FILENAME),
            b"gguf",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let (_, ready) = probe_backend_at(&backend, Some(dir.path()));
        assert!(ready.contains("already downloaded"), "{ready}");
        let status = ModelInventory::default().with_disk_status(dir.path());
        assert!(status.artifacts.iter().any(|artifact| artifact.present));
    }
}
