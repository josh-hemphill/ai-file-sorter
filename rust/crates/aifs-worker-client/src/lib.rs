//! Spawn a worker binary and speak the worker JSONL protocol on its stdio.

use aifs_domain::{Evidence, ObservedEntry};
use aifs_protocol::worker::{
    WORKER_PROTOCOL_VERSION, WorkerCommand, WorkerEnvelope, WorkerEvent, WorkerKind, WorkerRequest,
};
use aifs_protocol::{ErrorCode, ModelBackend, RequestId, decode_line, encode_line};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const LOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const INFER_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// Successful `load` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLoad {
    /// Device actually used.
    pub device: String,
    /// Model id or filename.
    pub model: String,
    /// GPU layers offloaded.
    pub n_gpu_layers: u32,
    /// Why a requested accelerator was not used.
    pub fallback: Option<String>,
}

/// Failures talking to a worker process.
#[derive(Debug, Error)]
pub enum WorkerClientError {
    /// The worker binary could not be found.
    #[error("worker binary not found: {0}")]
    NotFound(String),
    /// Spawning the process failed.
    #[error("failed to spawn worker: {0}")]
    Spawn(#[from] std::io::Error),
    /// The worker closed stdout before a terminal event.
    #[error("worker closed stdout unexpectedly")]
    Disconnected,
    /// Timed out waiting for a terminal event.
    #[error("timed out waiting for the worker")]
    Timeout,
    /// A protocol line could not be parsed.
    #[error("invalid worker output: {0}")]
    Codec(String),
    /// The worker returned a `failed` event.
    #[error("worker error {code:?}: {message}")]
    Worker {
        /// Stable code.
        code: ErrorCode,
        /// Text.
        message: String,
    },
    /// A terminal event was the wrong type.
    #[error("{0}")]
    Unexpected(String),
}

/// Client that owns a worker child process.
pub struct WorkerClient {
    kind: WorkerKind,
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Result<WorkerEnvelope, WorkerClientError>>,
    next_id: u64,
    capabilities: Vec<String>,
}

impl WorkerClient {
    /// Spawns `binary` as `kind` and completes `hello`.
    pub fn connect(kind: WorkerKind, binary: impl AsRef<Path>) -> Result<Self, WorkerClientError> {
        let binary = binary.as_ref();
        if !binary.exists() {
            return Err(WorkerClientError::NotFound(binary.display().to_string()));
        }
        let mut command = Command::new(binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("worker stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("worker stdout was not piped"))?;
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[{}] {line}", kind.binary_stem());
                }
            });
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let parsed = decode_line::<WorkerEnvelope>(&line)
                            .map_err(|error| WorkerClientError::Codec(error.to_string()));
                        if tx.send(parsed).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(WorkerClientError::Spawn(error)));
                        break;
                    }
                }
            }
        });
        let mut client = Self {
            kind,
            child,
            stdin: Some(stdin),
            rx,
            next_id: 1,
            capabilities: Vec::new(),
        };
        client.hello()?;
        Ok(client)
    }

    /// Discovers the binary for `kind` and connects.
    pub fn connect_default(kind: WorkerKind) -> Result<Self, WorkerClientError> {
        Self::connect(kind, discover_worker_binary(kind)?)
    }

    /// Connects when the binary is present; `None` when it is not installed.
    pub fn try_connect(kind: WorkerKind) -> Option<Self> {
        Self::connect_default(kind).ok()
    }

    /// Capabilities advertised at hello.
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Worker kind this client supervises.
    pub fn kind(&self) -> WorkerKind {
        self.kind
    }

    fn hello(&mut self) -> Result<(), WorkerClientError> {
        let envelopes = self.request(WorkerCommand::Hello {
            worker: self.kind,
            protocol_version: WORKER_PROTOCOL_VERSION,
        })?;
        match envelopes.last().map(|envelope| &envelope.event) {
            Some(WorkerEvent::Ready { capabilities, .. }) => {
                self.capabilities = capabilities.clone();
                Ok(())
            }
            Some(WorkerEvent::Failed { code, message }) => Err(WorkerClientError::Worker {
                code: *code,
                message: message.clone(),
            }),
            Some(
                WorkerEvent::Extracted { .. }
                | WorkerEvent::Loaded { .. }
                | WorkerEvent::Unloaded
                | WorkerEvent::Inferred { .. }
                | WorkerEvent::ChatCompleted { .. }
                | WorkerEvent::Shutdown,
            )
            | None => Err(WorkerClientError::Unexpected(format!(
                "expected ready, got {:?}",
                envelopes.last().map(|envelope| &envelope.event)
            ))),
        }
    }

    /// Extracts evidence for one entry. `Ok(None)` means the worker skipped the file.
    pub fn extract(
        &mut self,
        root: impl AsRef<Path>,
        entry: &ObservedEntry,
    ) -> Result<Option<Evidence>, WorkerClientError> {
        let envelopes = self.request(WorkerCommand::Extract {
            root: root.as_ref().to_path_buf(),
            entry: entry.clone(),
        })?;
        for envelope in envelopes {
            match envelope.event {
                WorkerEvent::Extracted { evidence } => return Ok(evidence),
                WorkerEvent::Failed { code, message } => {
                    return Err(WorkerClientError::Worker { code, message });
                }
                WorkerEvent::Ready { .. } | WorkerEvent::Shutdown => {}
                WorkerEvent::Loaded { .. }
                | WorkerEvent::Unloaded
                | WorkerEvent::Inferred { .. }
                | WorkerEvent::ChatCompleted { .. } => {
                    return Err(WorkerClientError::Unexpected(
                        "extract ended with a non-extract event".to_owned(),
                    ));
                }
            }
        }
        Err(WorkerClientError::Unexpected(
            "extract ended without extracted".to_owned(),
        ))
    }

    /// Loads a backend into the LLM worker.
    pub fn load(
        &mut self,
        backend: ModelBackend,
        gpu_preference: impl Into<String>,
        n_gpu_layers: Option<u32>,
        api_key: Option<String>,
        storage_dir: impl Into<String>,
    ) -> Result<WorkerLoad, WorkerClientError> {
        let envelopes = self.request_with_timeout(
            WorkerCommand::Load {
                backend,
                gpu_preference: gpu_preference.into(),
                n_gpu_layers,
                api_key: api_key
                    .filter(|key| !key.trim().is_empty())
                    .map(aifs_protocol::worker::RedactedString::new),
                storage_dir: storage_dir.into(),
            },
            LOAD_TIMEOUT,
        )?;
        for envelope in envelopes {
            match envelope.event {
                WorkerEvent::Loaded {
                    device,
                    model,
                    n_gpu_layers,
                    fallback,
                } => {
                    return Ok(WorkerLoad {
                        device,
                        model,
                        n_gpu_layers,
                        fallback,
                    });
                }
                WorkerEvent::Failed { code, message } => {
                    return Err(WorkerClientError::Worker { code, message });
                }
                WorkerEvent::Ready { .. } | WorkerEvent::Shutdown => {}
                WorkerEvent::Extracted { .. }
                | WorkerEvent::Unloaded
                | WorkerEvent::Inferred { .. }
                | WorkerEvent::ChatCompleted { .. } => {
                    return Err(WorkerClientError::Unexpected(
                        "load ended with a non-load event".to_owned(),
                    ));
                }
            }
        }
        Err(WorkerClientError::Unexpected(
            "load ended without loaded".to_owned(),
        ))
    }

    /// Drops a loaded backend.
    pub fn unload(&mut self) -> Result<(), WorkerClientError> {
        let envelopes = self.request(WorkerCommand::Unload)?;
        for envelope in envelopes {
            match envelope.event {
                WorkerEvent::Unloaded => return Ok(()),
                WorkerEvent::Failed { code, message } => {
                    return Err(WorkerClientError::Worker { code, message });
                }
                WorkerEvent::Ready { .. } | WorkerEvent::Shutdown => {}
                WorkerEvent::Extracted { .. }
                | WorkerEvent::Loaded { .. }
                | WorkerEvent::Inferred { .. }
                | WorkerEvent::ChatCompleted { .. } => {
                    return Err(WorkerClientError::Unexpected(
                        "unload ended with a non-unload event".to_owned(),
                    ));
                }
            }
        }
        Err(WorkerClientError::Unexpected(
            "unload ended without unloaded".to_owned(),
        ))
    }

    /// Categorizes a file with the loaded model.
    pub fn categorize(
        &mut self,
        root: impl AsRef<Path>,
        entry: &ObservedEntry,
        evidence: Vec<Evidence>,
    ) -> Result<Option<Evidence>, WorkerClientError> {
        self.infer(WorkerCommand::Categorize {
            root: root.as_ref().to_path_buf(),
            entry: entry.clone(),
            evidence,
        })
    }

    /// Describes an image with the loaded model.
    pub fn describe(
        &mut self,
        root: impl AsRef<Path>,
        entry: &ObservedEntry,
        evidence: Vec<Evidence>,
    ) -> Result<Option<Evidence>, WorkerClientError> {
        self.infer(WorkerCommand::Describe {
            root: root.as_ref().to_path_buf(),
            entry: entry.clone(),
            evidence,
        })
    }

    fn infer(&mut self, command: WorkerCommand) -> Result<Option<Evidence>, WorkerClientError> {
        let envelopes = self.request_with_timeout(command, INFER_TIMEOUT)?;
        for envelope in envelopes {
            match envelope.event {
                WorkerEvent::Inferred { evidence } => return Ok(evidence),
                WorkerEvent::Failed { code, message } => {
                    return Err(WorkerClientError::Worker { code, message });
                }
                WorkerEvent::Ready { .. } | WorkerEvent::Shutdown => {}
                WorkerEvent::Extracted { .. }
                | WorkerEvent::Loaded { .. }
                | WorkerEvent::Unloaded
                | WorkerEvent::ChatCompleted { .. } => {
                    return Err(WorkerClientError::Unexpected(
                        "infer ended with a non-infer event".to_owned(),
                    ));
                }
            }
        }
        Err(WorkerClientError::Unexpected(
            "infer ended without inferred".to_owned(),
        ))
    }

    /// Runs a chat turn. The message is untrusted text.
    pub fn chat(
        &mut self,
        utterance: impl Into<String>,
        context: impl Into<String>,
    ) -> Result<String, WorkerClientError> {
        let envelopes = self.request_with_timeout(
            WorkerCommand::Chat {
                utterance: utterance.into(),
                context: context.into(),
            },
            INFER_TIMEOUT,
        )?;
        for envelope in envelopes {
            match envelope.event {
                WorkerEvent::ChatCompleted { message } => return Ok(message),
                WorkerEvent::Failed { code, message } => {
                    return Err(WorkerClientError::Worker { code, message });
                }
                WorkerEvent::Ready { .. } | WorkerEvent::Shutdown => {}
                WorkerEvent::Extracted { .. }
                | WorkerEvent::Loaded { .. }
                | WorkerEvent::Unloaded
                | WorkerEvent::Inferred { .. } => {
                    return Err(WorkerClientError::Unexpected(
                        "chat ended with a non-chat event".to_owned(),
                    ));
                }
            }
        }
        Err(WorkerClientError::Unexpected(
            "chat ended without chat_completed".to_owned(),
        ))
    }

    /// Asks the worker to exit.
    pub fn shutdown(&mut self) -> Result<(), WorkerClientError> {
        let _ = self.request(WorkerCommand::Shutdown)?;
        Ok(())
    }

    fn request(
        &mut self,
        command: WorkerCommand,
    ) -> Result<Vec<WorkerEnvelope>, WorkerClientError> {
        self.request_with_timeout(command, REQUEST_TIMEOUT)
    }

    fn request_with_timeout(
        &mut self,
        command: WorkerCommand,
        timeout: Duration,
    ) -> Result<Vec<WorkerEnvelope>, WorkerClientError> {
        let id = RequestId(self.next_id.to_string());
        self.next_id += 1;
        let request = WorkerRequest {
            id: id.clone(),
            command,
        };
        let line =
            encode_line(&request).map_err(|error| WorkerClientError::Codec(error.to_string()))?;
        {
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| std::io::Error::other("worker stdin closed"))?;
            writeln!(stdin, "{line}")?;
            stdin.flush()?;
        }
        let mut collected = Vec::new();
        loop {
            let envelope = self.recv_next(timeout)?;
            let matches = envelope.id.as_ref() == Some(&id) || envelope.id.is_none();
            if !matches {
                continue;
            }
            let terminal = envelope.is_terminal();
            collected.push(envelope);
            if terminal {
                return Ok(collected);
            }
        }
    }

    fn recv_next(&mut self, timeout: Duration) -> Result<WorkerEnvelope, WorkerClientError> {
        match self.rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(WorkerClientError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(WorkerClientError::Disconnected),
        }
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take()
            && let Ok(line) = encode_line(&WorkerRequest {
                id: RequestId("shutdown".to_owned()),
                command: WorkerCommand::Shutdown,
            })
        {
            let _ = writeln!(stdin, "{line}");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                _ => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Resolves a worker binary from its env var, then a sibling of the current
/// executable, then well-known Cargo target directories.
pub fn discover_worker_binary(kind: WorkerKind) -> Result<PathBuf, WorkerClientError> {
    let name = worker_file_name(kind);
    if let Ok(explicit) = std::env::var(kind.env_var()) {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Ok(path);
        }
        return Err(WorkerClientError::NotFound(path.display().to_string()));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(&name);
        if sibling.exists() {
            return Ok(sibling);
        }
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut dir = PathBuf::from(manifest);
        for _ in 0..6 {
            for profile in ["debug", "release"] {
                let candidate = dir.join("target").join(profile).join(&name);
                if candidate.exists() {
                    return Ok(candidate);
                }
                let nested = dir.join("rust").join("target").join(profile).join(&name);
                if nested.exists() {
                    return Ok(nested);
                }
            }
            if !dir.pop() {
                break;
            }
        }
    }
    Err(WorkerClientError::NotFound(format!(
        "set {} or install {name} next to aifs-engine",
        kind.env_var()
    )))
}

fn worker_file_name(kind: WorkerKind) -> String {
    if cfg!(windows) {
        format!("{}.exe", kind.binary_stem())
    } else {
        kind.binary_stem().to_owned()
    }
}
