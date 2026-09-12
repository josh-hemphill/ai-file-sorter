//! JSONL protocol between `aifs-engine` and isolated worker processes.
//!
//! Workers never open SQLite and never mutate the user's files. They receive an
//! observed entry, return evidence or artifacts, and exit on `shutdown`.

use crate::{
    CodecError, ErrorCode, FolderStyle, ModelBackend, PROTOCOL_VERSION, RequestId, decode_line,
    encode_line,
};
use aifs_domain::{Evidence, ObservedEntry};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

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

/// Engine stdio binary stem (no target-triple suffix).
pub const ENGINE_PROCESS_STEM: &str = "aifs-engine";

/// Compile-time rustc `TARGET` triple, when the build script set it.
pub fn target_triple() -> Option<&'static str> {
    option_env!("AIFS_TARGET_TRIPLE")
}

/// Candidate file names for `stem` including Windows `.exe` and Tauri sidecar triples.
pub fn process_binary_names(stem: &str, target_triple: Option<&str>) -> Vec<String> {
    let mut names = vec![stem.to_owned(), format!("{stem}.exe")];
    if let Some(triple) = target_triple.filter(|triple| !triple.is_empty()) {
        names.push(format!("{stem}-{triple}"));
        names.push(format!("{stem}-{triple}.exe"));
    }
    names
}

/// True when `path` is a non-empty regular file (skips Tauri debug placeholders).
pub fn is_usable_process_binary(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// First usable process binary for `stem` in `dir` (plain name, then sidecar suffix).
pub fn first_process_binary(dir: &Path, stem: &str) -> Option<PathBuf> {
    process_binary_names(stem, target_triple())
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| is_usable_process_binary(path))
}

/// Resolves `stem` next to `current_exe`, then Cargo `target/{debug,release}` ancestors.
///
/// Tauri `tauri dev` can launch sidecars from `src-tauri/binaries/` where debug
/// placeholders are empty. Walking up finds `target/debug/aifs-worker-llm` from
/// `pnpm llama` / `pnpm desktop:cuda`.
pub fn discover_process_binary(
    stem: &str,
    current_exe: Option<&Path>,
    manifest_dir: Option<&Path>,
) -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(exe) = current_exe
        && let Some(dir) = exe.parent()
    {
        roots.push(dir.to_path_buf());
    }
    if let Some(manifest) = manifest_dir {
        roots.push(manifest.to_path_buf());
    }
    for root in roots {
        if let Some(found) = search_process_binary_ancestors(&root, stem) {
            return Some(found);
        }
    }
    None
}

fn search_process_binary_ancestors(start: &Path, stem: &str) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..8 {
        if let Some(found) = first_process_binary(&dir, stem) {
            return Some(found);
        }
        if let Some(found) = first_process_binary(&dir.join("binaries"), stem) {
            return Some(found);
        }
        for profile in ["debug", "release"] {
            if let Some(found) = first_process_binary(&dir.join("target").join(profile), stem) {
                return Some(found);
            }
            if let Some(found) =
                first_process_binary(&dir.join("rust").join("target").join(profile), stem)
            {
                return Some(found);
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
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
        /// Optional allowed top-level folder names. Empty means unconstrained.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        allowed_categories: Vec<String>,
        /// Folder naming style from settings.
        #[serde(default)]
        style: FolderStyle,
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
        /// Device actually used (`cpu`, `cuda`, `vulkan`, `metal`).
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

    #[test]
    fn process_binary_names_include_plain_and_sidecar_suffix() {
        let names = process_binary_names("aifs-engine", Some("x86_64-unknown-linux-gnu"));
        assert_eq!(
            names,
            [
                "aifs-engine",
                "aifs-engine.exe",
                "aifs-engine-x86_64-unknown-linux-gnu",
                "aifs-engine-x86_64-unknown-linux-gnu.exe",
            ]
        );
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let triple = target_triple().unwrap_or_else(|| panic!("AIFS_TARGET_TRIPLE"));
        let sidecar = dir.path().join(format!("aifs-worker-llm-{triple}"));
        std::fs::write(&sidecar, b"sidecar").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            first_process_binary(dir.path(), "aifs-worker-llm").as_deref(),
            Some(sidecar.as_path())
        );
        let plain = dir.path().join("aifs-worker-llm");
        std::fs::write(&plain, b"plain").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            first_process_binary(dir.path(), "aifs-worker-llm").as_deref(),
            Some(plain.as_path())
        );
    }

    #[test]
    fn first_process_binary_skips_empty_tauri_placeholders() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let triple = target_triple().unwrap_or_else(|| panic!("AIFS_TARGET_TRIPLE"));
        let placeholder = dir.path().join(format!("aifs-worker-llm-{triple}"));
        std::fs::write(&placeholder, b"").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            first_process_binary(dir.path(), "aifs-worker-llm").as_deref(),
            None
        );
        let real = dir.path().join("aifs-worker-llm");
        std::fs::write(&real, b"llama").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            first_process_binary(dir.path(), "aifs-worker-llm").as_deref(),
            Some(real.as_path())
        );
    }

    #[test]
    fn discover_process_binary_walks_up_to_target_debug() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let debug = root.path().join("target").join("debug");
        std::fs::create_dir_all(&debug).unwrap_or_else(|error| panic!("{error}"));
        let worker = debug.join("aifs-worker-llm");
        std::fs::write(&worker, b"cuda-llama").unwrap_or_else(|error| panic!("{error}"));
        let binaries = root
            .path()
            .join("apps")
            .join("desktop")
            .join("src-tauri")
            .join("binaries");
        std::fs::create_dir_all(&binaries).unwrap_or_else(|error| panic!("{error}"));
        let triple = target_triple().unwrap_or_else(|| panic!("AIFS_TARGET_TRIPLE"));
        std::fs::write(binaries.join(format!("aifs-worker-llm-{triple}")), b"")
            .unwrap_or_else(|error| panic!("{error}"));
        let sidecar_engine = binaries.join(format!("aifs-engine-{triple}"));
        std::fs::write(&sidecar_engine, b"engine").unwrap_or_else(|error| panic!("{error}"));
        let found = discover_process_binary("aifs-worker-llm", Some(&sidecar_engine), None)
            .unwrap_or_else(|| panic!("should find workspace target/debug worker"));
        assert_eq!(found, worker);
    }

    #[test]
    fn categorize_without_allowlist_fields_still_decodes() {
        let json = serde_json::json!({
            "type": "categorize",
            "root": "/tmp",
            "entry": {
                "id": "00000000-0000-0000-0000-000000000001",
                "path": "note.txt",
                "kind": "file",
                "family": "document",
                "identity": { "size": 1 },
                "is_hidden": false,
                "lock": { "state": "readable" }
            }
        });
        let command: WorkerCommand =
            serde_json::from_value(json).unwrap_or_else(|error| panic!("{error}"));
        match command {
            WorkerCommand::Categorize {
                allowed_categories,
                style,
                evidence,
                ..
            } => {
                assert!(allowed_categories.is_empty());
                assert_eq!(style, FolderStyle::Consistent);
                assert!(evidence.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
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
