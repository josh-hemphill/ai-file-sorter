//! Versioned JSONL protocol between the UI shell, `aifs-engine`, and workers.
//!
//! Every line on the wire is one JSON object. Clients send [`Request`]s on the engine's
//! stdin; the engine replies with [`Envelope`]s on stdout. Events carry the id of the
//! request they belong to so several requests can be in flight (for example a long
//! `scan` plus a `cancel`).
//!
//! The protocol is the stable boundary described in `rust/docs/protocol.md`. Breaking
//! changes bump [`PROTOCOL_VERSION`]; additive changes must keep old fields working.

pub mod catalog;
pub mod codec;
pub mod models;
pub mod options;
pub mod worker;

pub use catalog::{
    all_artifacts, artifact_bytes_on_disk, artifact_is_present, artifact_path,
    catalog_download_url, catalog_entry, catalog_id_is_downloaded, catalog_ids_for_artifact,
    CatalogArtifact, CatalogEntry, ARTIFACT_GEMMA_MMPROJ, ARTIFACT_GEMMA_TEXT, CATALOG_BASE_ENV,
    CATALOG_TEXT, CATALOG_VISION, DEFAULT_CATALOG_BASE, GEMMA_MMPROJ_BYTES, GEMMA_MMPROJ_FILENAME,
    GEMMA_TEXT_BYTES, GEMMA_TEXT_FILENAME,
};
pub use codec::{decode_line, encode_line, CodecError};
pub use models::{
    probe_backend, probe_backend_at, ModelArtifactStatus, ModelBackend, ModelInventory, ModelSlot,
    BUILTIN_CATALOG, GPU_PREFERENCES, MODEL_SLOT_IDS,
};
pub use options::{AppSettings, CategoryWhitelist, FolderStyle, ProposalPolicy, ScanOptions};

use aifs_domain::{
    ApplyJournal, JournalId, OperationPlan, PlanId, PlanIssue, ProposalRevision, RevisionAuthor,
    RevisionId, RevisionPatch, SessionId, WorkspaceSnapshot,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Current wire version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Returns true when a client speaking `client_version` can talk to this engine.
pub const fn is_compatible(client_version: u32) -> bool {
    client_version == PROTOCOL_VERSION
}

/// Correlates a request with its events. Clients choose the value; it must be unique per
/// connection.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub String);

impl From<&str> for RequestId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Something the client asks the engine to do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// First message on a connection; negotiates the protocol version.
    Hello {
        /// Client name, for logs.
        client: String,
        /// Protocol version the client speaks.
        protocol_version: u32,
    },
    /// Scan a root folder and build a snapshot (entries, bundles, evidence).
    Scan {
        /// Absolute folder to scan.
        root: PathBuf,
        /// Scan options.
        #[serde(default)]
        options: ScanOptions,
        /// Reuse an existing session instead of creating one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
    },
    /// Build a heuristic proposal revision from the latest snapshot of a session.
    Propose {
        /// Session.
        session: SessionId,
        /// Placement policy.
        #[serde(default)]
        policy: ProposalPolicy,
    },
    /// Apply patches on top of a revision, producing a child revision.
    Patch {
        /// Session.
        session: SessionId,
        /// Revision the patches were computed against.
        base_revision: RevisionId,
        /// Author of the change.
        author: RevisionAuthor,
        /// Human summary.
        #[serde(default)]
        summary: String,
        /// Edits.
        patches: Vec<RevisionPatch>,
    },
    /// Validate a revision into an operation plan.
    Plan {
        /// Session.
        session: SessionId,
        /// Revision to plan.
        revision: RevisionId,
    },
    /// Execute a plan.
    Apply {
        /// Session.
        session: SessionId,
        /// Plan to apply.
        plan: PlanId,
        /// When true, journal only; touch nothing.
        #[serde(default)]
        dry_run: bool,
    },
    /// Reverse a completed apply.
    Undo {
        /// Session.
        session: SessionId,
        /// Journal to reverse.
        journal: JournalId,
    },
    /// Interpret an utterance into tools and optionally patch the revision.
    Chat {
        /// Session.
        session: SessionId,
        /// Revision the assistant should edit.
        revision: RevisionId,
        /// User utterance. Tools may emit patches; they never return filesystem ops.
        utterance: String,
    },
    /// Cancel an in-flight request.
    Cancel {
        /// Request to cancel.
        target: RequestId,
    },
    /// Stop the engine after in-flight work is cancelled.
    Shutdown,
    /// Load persisted scan/proposal/analysis settings.
    GetSettings,
    /// Replace persisted scan/proposal/analysis settings.
    PutSettings {
        /// Settings blob owned by the engine store.
        settings: AppSettings,
    },
    /// Load redacted model slot assignments.
    GetModels,
    /// Replace model slot assignments. Omitted API keys keep the stored secret.
    PutModels {
        /// Inventory owned by the engine store.
        inventory: ModelInventory,
    },
    /// Validate a backend without scanning.
    ProbeEndpoint {
        /// Backend to probe.
        #[serde(flatten)]
        backend: ModelBackend,
        /// Optional key used only for this probe; never logged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key: Option<String>,
    },
    /// Download every GGUF this catalog id needs. Files already on disk are skipped.
    DownloadModel {
        /// Catalog id such as `gemma-3-4b-it`.
        catalog_id: String,
    },
}

/// A command with its correlation id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Correlation id.
    pub id: RequestId,
    /// Command.
    #[serde(flatten)]
    pub command: Command,
}

/// Stable machine-readable failure codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Client and engine protocol versions differ.
    IncompatibleProtocol,
    /// Malformed request line.
    InvalidRequest,
    /// Session, revision, plan, or journal not found.
    NotFound,
    /// Root path does not exist or is not a directory.
    InvalidRoot,
    /// Plan validation produced errors.
    PlanRejected,
    /// Filesystem failure.
    Io,
    /// Persistence failure.
    Storage,
    /// Request cancelled by the client.
    Cancelled,
    /// Anything else.
    Internal,
}

/// Log level for engine diagnostics forwarded to the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Debug.
    Debug,
    /// Info.
    Info,
    /// Warning.
    Warn,
    /// Error.
    Error,
}

/// Something the engine tells the client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Reply to `hello`.
    Ready {
        /// Engine crate version.
        engine_version: String,
        /// Protocol version the engine speaks.
        protocol_version: u32,
        /// Optional capability flags (e.g. `media_tags`, `exif`).
        #[serde(default)]
        capabilities: Vec<String>,
    },
    /// Progress for a long request.
    Progress {
        /// Stage name (`scan`, `extract`, `plan`, `apply`, ...).
        stage: String,
        /// Items done.
        current: u64,
        /// Items total when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
        /// Short message.
        #[serde(default)]
        message: String,
    },
    /// Diagnostic line.
    Log {
        /// Level.
        level: LogLevel,
        /// Text.
        message: String,
    },
    /// `scan` finished.
    ScanCompleted {
        /// Resulting snapshot.
        snapshot: WorkspaceSnapshot,
    },
    /// `propose` or `patch` produced a revision.
    Revision {
        /// The new revision.
        revision: ProposalRevision,
    },
    /// `plan` succeeded.
    Planned {
        /// The plan.
        plan: OperationPlan,
        /// All findings including warnings.
        #[serde(default)]
        issues: Vec<PlanIssue>,
    },
    /// `apply` or `undo` finished (successfully or not; inspect the journal).
    Journal {
        /// Resulting journal.
        journal: ApplyJournal,
    },
    /// `chat` finished. `revision` is present when tools produced a child revision.
    ChatReply {
        /// Assistant summary of tool results.
        message: String,
        /// Child revision when patches were applied.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<ProposalRevision>,
    },
    /// The request was cancelled.
    Cancelled,
    /// The request failed.
    Failed {
        /// Code.
        code: ErrorCode,
        /// Text.
        message: String,
        /// Validation findings when `code == PlanRejected`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        issues: Vec<PlanIssue>,
    },
    /// Engine is exiting.
    Shutdown,
    /// `get_settings` / `put_settings` result.
    Settings {
        /// Persisted classification settings.
        settings: AppSettings,
    },
    /// `get_models` / `put_models` result. API keys are never present.
    Models {
        /// Redacted inventory.
        inventory: ModelInventory,
    },
    /// `probe_endpoint` result.
    EndpointProbed {
        /// True when the path exists or the URL looks usable.
        ok: bool,
        /// Human message.
        message: String,
    },
}

/// An event with the id of the request it answers. `id` is `None` for unsolicited
/// engine-wide events such as `shutdown` triggered by a broken pipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Correlation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// Event.
    #[serde(flatten)]
    pub event: Event,
}

impl Envelope {
    /// Builds an envelope for a request.
    pub fn reply(id: &RequestId, event: Event) -> Self {
        Self {
            id: Some(id.clone()),
            event,
        }
    }

    /// Builds an unsolicited envelope.
    pub fn broadcast(event: Event) -> Self {
        Self { id: None, event }
    }

    /// True when this event ends the request (no further events will follow).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.event,
            Event::Ready { .. }
                | Event::ScanCompleted { .. }
                | Event::Revision { .. }
                | Event::Planned { .. }
                | Event::Journal { .. }
                | Event::ChatReply { .. }
                | Event::Cancelled
                | Event::Failed { .. }
                | Event::Shutdown
                | Event::Settings { .. }
                | Event::Models { .. }
                | Event::EndpointProbed { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_flatten_command_tag() {
        let request = Request {
            id: "r1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        };
        let json = serde_json::to_value(&request).unwrap_or_default();
        assert_eq!(json["id"], "r1");
        assert_eq!(json["type"], "hello");
        assert_eq!(json["protocol_version"], PROTOCOL_VERSION);
        let parsed: Request = serde_json::from_value(json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed, request);
    }

    #[test]
    fn scan_options_default_when_omitted() {
        let request: Request = serde_json::from_str(r#"{"id":"s","type":"scan","root":"/tmp/x"}"#)
            .unwrap_or_else(|e| panic!("{e}"));
        match request.command {
            Command::Scan {
                options, session, ..
            } => {
                assert_eq!(options, ScanOptions::default());
                assert!(session.is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn terminal_events_are_identified() {
        let progress = Envelope::reply(
            &"r".into(),
            Event::Progress {
                stage: "scan".into(),
                current: 1,
                total: None,
                message: String::new(),
            },
        );
        let failed = Envelope::reply(
            &"r".into(),
            Event::Failed {
                code: ErrorCode::Io,
                message: "x".into(),
                issues: vec![],
            },
        );
        assert!(!progress.is_terminal());
        assert!(failed.is_terminal());
        let json = serde_json::to_string(&failed).unwrap_or_default();
        assert!(
            !json.contains("issues"),
            "empty issues must be omitted: {json}"
        );
    }

    #[test]
    fn broadcast_omits_id() {
        let json = serde_json::to_string(&Envelope::broadcast(Event::Shutdown)).unwrap_or_default();
        assert_eq!(json, r#"{"type":"shutdown"}"#);
    }

    #[test]
    fn chat_request_flattens_and_omits_empty_revision() {
        let request = Request {
            id: "c1".into(),
            command: Command::Chat {
                session: SessionId::default(),
                revision: RevisionId::default(),
                utterance: "validate this plan".into(),
            },
        };
        let json = serde_json::to_value(&request).unwrap_or_default();
        assert_eq!(json["type"], "chat");
        assert_eq!(json["utterance"], "validate this plan");
        let reply = Envelope::reply(
            &"c1".into(),
            Event::ChatReply {
                message: "ok".into(),
                revision: None,
            },
        );
        let encoded = serde_json::to_string(&reply).unwrap_or_default();
        assert!(encoded.contains("\"type\":\"chat_reply\""));
        assert!(
            !encoded.contains("revision"),
            "empty revision must be omitted: {encoded}"
        );
        assert!(reply.is_terminal());
    }
}
