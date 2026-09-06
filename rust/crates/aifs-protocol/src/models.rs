//! Model slot configuration. Keys are stored by the engine and redacted on `get_models`.

use serde::{Deserialize, Serialize};

/// Built-in catalog entries the Setup UI can assign without a filesystem path.
pub const BUILTIN_CATALOG: &[(&str, &str)] = &[
    ("gemma-3-4b-it", "Gemma 3 4B Instruct (text)"),
    ("gemma-3-4b-it-mmproj", "Gemma 3 4B Instruct + mmproj (vision)"),
];

/// Analysis slots the workspace can assign independently.
pub const MODEL_SLOT_IDS: &[&str] = &["categorize", "vision", "document", "chat"];

/// How a slot is backed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelBackend {
    /// Heuristics only.
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

impl Default for ModelBackend {
    fn default() -> Self {
        Self::Off
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
            api_key_set: self
                .api_key
                .as_ref()
                .is_some_and(|key| !key.is_empty())
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
    /// Preferred accelerator (`auto`, `cpu`, `vulkan`, `metal`).
    pub gpu_preference: String,
    /// Four analysis slots.
    pub slots: Vec<ModelSlot>,
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
            let incoming = slot.api_key.as_ref().is_some_and(|key| !key.trim().is_empty());
            if incoming {
                continue;
            }
            if let Some(stored) = previous.slots.iter().find(|candidate| candidate.id == slot.id)
            {
                slot.api_key = stored.api_key.clone();
                slot.api_key_set = stored.api_key_set
                    || stored
                        .api_key
                        .as_ref()
                        .is_some_and(|key| !key.is_empty());
            }
        }
        next
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

/// Checks a backend without contacting model vendors.
pub fn probe_backend(backend: &ModelBackend) -> (bool, String) {
    match backend {
        ModelBackend::Off => (true, "Slot is off. Heuristics still run.".to_owned()),
        ModelBackend::Catalog { catalog_id } => {
            if BUILTIN_CATALOG
                .iter()
                .any(|(catalog, _)| *catalog == catalog_id)
            {
                (
                    true,
                    format!("{catalog_id} will download when the model runtime is connected."),
                )
            } else {
                (false, format!("Unknown catalog id {catalog_id}"))
            }
        }
        ModelBackend::LocalGguf { path, mmproj } => {
            if !std::path::Path::new(path).is_file() {
                return (false, format!("{path} was not found"));
            }
            if let Some(proj) = mmproj {
                if !proj.is_empty() && !std::path::Path::new(proj).is_file() {
                    return (false, format!("{proj} was not found"));
                }
            }
            (
                true,
                "Local GGUF found. Model runtime is not connected yet.".to_owned(),
            )
        }
        ModelBackend::OpenAi { model } if model.trim().is_empty() => {
            (false, "OpenAI model id is required".to_owned())
        }
        ModelBackend::OpenAi { .. } => (
            true,
            "OpenAI assignment recorded. Model runtime is not connected yet.".to_owned(),
        ),
        ModelBackend::Gemini { model } if model.trim().is_empty() => {
            (false, "Gemini model id is required".to_owned())
        }
        ModelBackend::Gemini { .. } => (
            true,
            "Gemini assignment recorded. Model runtime is not connected yet.".to_owned(),
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
                "Custom endpoint recorded. Model runtime is not connected yet.".to_owned(),
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
        assert!(json.contains("api_key_set") || json.contains("true"), "{json}");
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
}
