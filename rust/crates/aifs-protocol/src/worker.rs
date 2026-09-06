//! JSONL protocol between `aifs-engine` and isolated worker processes.
//!
//! Workers never open SQLite and never mutate the user's files. They receive an
//! observed entry, return evidence or artifacts, and exit on `shutdown`.

use crate::{decode_line, encode_line, CodecError, ErrorCode, RequestId, PROTOCOL_VERSION};
use aifs_domain::{Evidence, ObservedEntry};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}
