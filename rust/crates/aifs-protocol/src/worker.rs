//! JSONL protocol between `aifs-engine` and isolated worker processes.
//!
//! Workers never open SQLite and never mutate the user's files. They receive an
//! observed entry, return evidence or artifacts, and exit on `shutdown`.

use crate::{
    CodecError, ErrorCode, ModelBackend, PROTOCOL_VERSION, RequestId, decode_line, encode_line,
};
use aifs_domain::{Evidence, ObservedEntry};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// API key on the wire. `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RedactedString(String);

impl RedactedString {
    /// Wraps a secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Consumes the wrapper.
    pub fn into_inner(self) -> String {
        self.0
    }

    /// True when the value is empty or whitespace.
    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

fn skip_blank_api_key(value: &Option<RedactedString>) -> bool {
    value.as_ref().is_none_or(RedactedString::is_blank)
}

/// Worker wire version; currently matches [`PROTOCOL_VERSION`].
pub const WORKER_PROTOCOL_VERSION: u32 = PROTOCOL_VERSION;

/// Kind of disposable worker the engine may supervise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerKind {
    /// Audio/video tag and stream metadata (currently the Rust extractors).
    Media,
    /// PDF / Office text and properties.
    Document,
    /// Image stills, EXIF, and later vision models.
    Vision,
    /// Local or remote language model.
    Llm,
}

impl WorkerKind {
    /// Binary file stem (`aifs-worker-media`, ...).
    pub fn binary_stem(self) -> &'static str {
        match self {
            Self::Media => "aifs-worker-media",
            Self::Document => "aifs-worker-document",
            Self::Vision => "aifs-worker-vision",
            Self::Llm => "aifs-worker-llm",
        }
    }

    /// Environment variable that overrides discovery (`AIFS_WORKER_MEDIA`, ...).
    pub fn env_var(self) -> &'static str {
        match self {
            Self::Media => "AIFS_WORKER_MEDIA",
            Self::Document => "AIFS_WORKER_DOCUMENT",
            Self::Vision => "AIFS_WORKER_VISION",
            Self::Llm => "AIFS_WORKER_LLM",
        }
    }
}

/// Something the engine asks a worker to do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerCommand {
    /// First message; negotiates version and kind.
    Hello {
        /// Kind the engine expects this process to be.
        worker: WorkerKind,
        /// Protocol version the engine speaks.
        protocol_version: u32,
    },
    /// Extract evidence for one observed file. Workers must not write the file.
    Extract {
        /// Session root the entry path is relative to.
        root: PathBuf,
        /// File to inspect.
        entry: ObservedEntry,
    },
    /// Bind a local GGUF or hosted backend for later infer commands. LLM worker only.
    Load {
        /// Slot backend (catalog, local GGUF, or hosted).
        backend: ModelBackend,
        /// Accelerator preference (`auto`, `cpu`, `cuda`, `vulkan`, `metal`).
        #[serde(default)]
        gpu_preference: String,
        /// Optional GPU layer count. `None` lets the worker choose.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        n_gpu_layers: Option<u32>,
        /// Hosted API key; never logged.
        #[serde(default, skip_serializing_if = "skip_blank_api_key")]
        api_key: Option<RedactedString>,
        /// Directory that holds catalog GGUFs.
        #[serde(default)]
        storage_dir: String,
    },
    /// Drop the loaded model.
    Unload,
    /// Text categorization for an entry. LLM worker only.
    Categorize {
        /// Session root.
        root: PathBuf,
        /// File to label.
        entry: ObservedEntry,
        /// Prior evidence the model may read (never execute).
        #[serde(default)]
        evidence: Vec<Evidence>,
    },
    /// Image description for an entry. LLM worker only.
    Describe {
        /// Session root.
        root: PathBuf,
        /// Image to describe.
        entry: ObservedEntry,
        /// Prior evidence (EXIF, etc.).
        #[serde(default)]
        evidence: Vec<Evidence>,
    },
    /// Natural-language assistant turn. LLM worker only; engine still applies tools.
    Chat {
        /// User utterance.
        utterance: String,
        /// Frozen revision/summary context; not a path of SQL.
        #[serde(default)]
        context: String,
    },
    /// Stop after in-flight work.
    Shutdown,
}

/// A worker command with its correlation id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerRequest {
    /// Correlation id.
    pub id: RequestId,
    /// Command.
    #[serde(flatten)]
    pub command: WorkerCommand,
}

/// Something a worker tells the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerEvent {
    /// Reply to `hello`.
    Ready {
        /// Kind this process actually is.
        worker: WorkerKind,
        /// Protocol version.
        protocol_version: u32,
        /// Capability flags (e.g. `media_tags`, `stub`).
        #[serde(default)]
        capabilities: Vec<String>,
    },
    /// `extract` finished.
    Extracted {
        /// Facts when the worker recognised the file; omitted/None when skipped.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evidence: Option<Evidence>,
    },
    /// `load` finished.
    Loaded {
        /// Device actually used (`cpu`, `cuda`, `vulkan`, `metal`, `stub`).
        device: String,
        /// Model id or GGUF filename.
        model: String,
        /// Layers offloaded to GPU.
        #[serde(default)]
        n_gpu_layers: u32,
        /// Why a requested accelerator was not used.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback: Option<String>,
    },
    /// `unload` finished.
    Unloaded,
    /// `categorize` or `describe` finished.
    Inferred {
        /// Model evidence; omitted when the worker skipped the file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evidence: Option<Evidence>,
    },
    /// `chat` finished. Message is untrusted text.
    ChatCompleted {
        /// Assistant text. Engine maps this into tools; it is never SQL or a path.
        message: String,
    },
    /// The request failed.
    Failed {
        /// Code.
        code: ErrorCode,
        /// Text.
        message: String,
    },
    /// Worker is exiting.
    Shutdown,
}

/// A worker event with the id of the request it answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerEnvelope {
    /// Correlation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// Event.
    #[serde(flatten)]
    pub event: WorkerEvent,
}

impl WorkerEnvelope {
    /// Builds an envelope for a request.
    pub fn reply(id: &RequestId, event: WorkerEvent) -> Self {
        Self {
            id: Some(id.clone()),
            event,
        }
    }

    /// True when this event ends the request.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.event,
            WorkerEvent::Ready { .. }
                | WorkerEvent::Extracted { .. }
                | WorkerEvent::Loaded { .. }
                | WorkerEvent::Unloaded
                | WorkerEvent::Inferred { .. }
                | WorkerEvent::ChatCompleted { .. }
                | WorkerEvent::Failed { .. }
                | WorkerEvent::Shutdown
        )
    }
}

/// Encodes a worker request line.
pub fn encode_request(request: &WorkerRequest) -> Result<String, CodecError> {
    encode_line(request)
}

/// Decodes a worker envelope line.
pub fn decode_envelope(line: &str) -> Result<WorkerEnvelope, CodecError> {
    decode_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_request_flattens() {
        let request = WorkerRequest {
            id: "1".into(),
            command: WorkerCommand::Hello {
                worker: WorkerKind::Media,
                protocol_version: WORKER_PROTOCOL_VERSION,
            },
        };
        let json = serde_json::to_value(&request).unwrap_or_default();
        assert_eq!(json["type"], "hello");
        assert_eq!(json["worker"], "media");
        let parsed: WorkerRequest = serde_json::from_value(json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed, request);
    }

    #[test]
    fn extracted_omits_empty_evidence() {
        let envelope =
            WorkerEnvelope::reply(&"1".into(), WorkerEvent::Extracted { evidence: None });
        let json = serde_json::to_string(&envelope).unwrap_or_default();
        assert!(
            !json.contains("evidence"),
            "empty evidence must be omitted: {json}"
        );
        assert!(envelope.is_terminal());
    }

    #[test]
    fn load_omits_empty_secrets_and_infer_is_terminal() {
        let none_key = WorkerRequest {
            id: "2".into(),
            command: load_command(None),
        };
        let json = serde_json::to_string(&none_key).unwrap_or_default();
        assert!(!json.contains("api_key"), "{json}");
        let blank_key = WorkerRequest {
            id: "2".into(),
            command: load_command(Some(RedactedString::new(""))),
        };
        let blank_json = serde_json::to_string(&blank_key).unwrap_or_default();
        assert!(!blank_json.contains("api_key"), "{blank_json}");
        let secret = RedactedString::new("sk-secret");
        let with_key = WorkerRequest {
            id: "2".into(),
            command: load_command(Some(secret.clone())),
        };
        let secret_json = serde_json::to_string(&with_key).unwrap_or_default();
        assert!(
            secret_json.contains("\"api_key\":\"sk-secret\""),
            "{secret_json}"
        );
        let debug = format!("{with_key:?}");
        assert!(!debug.contains("sk-secret"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
        let loaded = WorkerEnvelope::reply(
            &"2".into(),
            WorkerEvent::Loaded {
                device: "stub".into(),
                model: "gemma-3-4b-it".into(),
                n_gpu_layers: 0,
                fallback: None,
            },
        );
        assert!(loaded.is_terminal());
        let inferred = WorkerEnvelope::reply(&"3".into(), WorkerEvent::Inferred { evidence: None });
        let encoded = serde_json::to_string(&inferred).unwrap_or_default();
        assert!(!encoded.contains("evidence"), "{encoded}");
        assert!(inferred.is_terminal());
    }

    fn load_command(api_key: Option<RedactedString>) -> WorkerCommand {
        WorkerCommand::Load {
            backend: crate::ModelBackend::Catalog {
                catalog_id: "gemma-3-4b-it".into(),
            },
            gpu_preference: "auto".into(),
            n_gpu_layers: None,
            api_key,
            storage_dir: String::new(),
        }
    }
}
