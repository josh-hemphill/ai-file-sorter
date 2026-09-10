//! Spawn a worker binary and speak the worker JSONL protocol on its stdio.

use aifs_domain::{Evidence, ObservedEntry};
use aifs_protocol::worker::{
    WORKER_PROTOCOL_VERSION, WorkerCommand, WorkerEnvelope, WorkerEvent, WorkerKind, WorkerRequest,
};
use aifs_protocol::{
    ErrorCode, FolderStyle, ModelBackend, RequestId, decode_line, encode_line, first_process_binary,
};
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
/// Recv slice while waiting on a worker so the engine can emit idle-resetting logs.
/// Must stay under the engine-client idle timeout (180s).
const WAIT_SLICE: Duration = Duration::from_secs(15);
/// How often infer waits re-check scan cancel. Stays far below [`INFER_TIMEOUT`].
const CANCEL_POLL: Duration = Duration::from_millis(100);

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
    /// The child was killed because the caller cancelled the wait.
    #[error("worker killed because the scan was cancelled")]
    Cancelled,
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

impl WorkerClientError {
    /// True when the child was killed because the caller cancelled.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
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
        Self::connect_command(kind, binary, |_| {})
    }

    /// Like [`Self::connect`], with extra process configuration (test env, etc.).
    pub fn connect_command(
        kind: WorkerKind,
        binary: impl AsRef<Path>,
        configure: impl FnOnce(&mut Command),
    ) -> Result<Self, WorkerClientError> {
        let binary = binary.as_ref();
        if !binary.exists() {
            return Err(WorkerClientError::NotFound(binary.display().to_string()));
        }
        let mut command = Command::new(binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure(&mut command);
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

    /// Kills the child immediately. Used when scan cancel arrives during infer.
    pub fn kill(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
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
        self.load_with(
            backend,
            gpu_preference,
            n_gpu_layers,
            api_key,
            storage_dir,
            || {},
        )
    }

    /// Like [`Self::load`], invoking `on_wait` while the worker is silent so callers
    /// can emit engine progress before the 180s client idle timeout.
    pub fn load_with(
        &mut self,
        backend: ModelBackend,
        gpu_preference: impl Into<String>,
        n_gpu_layers: Option<u32>,
        api_key: Option<String>,
        storage_dir: impl Into<String>,
        mut on_wait: impl FnMut(),
    ) -> Result<WorkerLoad, WorkerClientError> {
        let envelopes = self.request_while(
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
            &mut on_wait,
            &mut || true,
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
        allowed_categories: Vec<String>,
        style: FolderStyle,
    ) -> Result<Option<Evidence>, WorkerClientError> {
        self.infer_while(
            WorkerCommand::Categorize {
                root: root.as_ref().to_path_buf(),
                entry: entry.clone(),
                evidence,
                allowed_categories,
                style,
            },
            || true,
        )
    }

    /// Like [`Self::categorize`], killing the child when `should_continue` returns false.
    pub fn categorize_while(
        &mut self,
        root: impl AsRef<Path>,
        entry: &ObservedEntry,
        evidence: Vec<Evidence>,
        allowed_categories: Vec<String>,
        style: FolderStyle,
        should_continue: impl FnMut() -> bool,
    ) -> Result<Option<Evidence>, WorkerClientError> {
        self.infer_while(
            WorkerCommand::Categorize {
                root: root.as_ref().to_path_buf(),
                entry: entry.clone(),
                evidence,
                allowed_categories,
                style,
            },
            should_continue,
        )
    }

    /// Describes an image with the loaded model.
    pub fn describe(
        &mut self,
        root: impl AsRef<Path>,
        entry: &ObservedEntry,
        evidence: Vec<Evidence>,
    ) -> Result<Option<Evidence>, WorkerClientError> {
        self.infer_while(
            WorkerCommand::Describe {
                root: root.as_ref().to_path_buf(),
                entry: entry.clone(),
                evidence,
            },
            || true,
        )
    }

    /// Like [`Self::describe`], killing the child when `should_continue` returns false.
    pub fn describe_while(
        &mut self,
        root: impl AsRef<Path>,
        entry: &ObservedEntry,
        evidence: Vec<Evidence>,
        should_continue: impl FnMut() -> bool,
    ) -> Result<Option<Evidence>, WorkerClientError> {
        self.infer_while(
            WorkerCommand::Describe {
                root: root.as_ref().to_path_buf(),
                entry: entry.clone(),
                evidence,
            },
            should_continue,
        )
    }

    fn infer_while(
        &mut self,
        command: WorkerCommand,
        mut should_continue: impl FnMut() -> bool,
    ) -> Result<Option<Evidence>, WorkerClientError> {
        let envelopes =
            self.request_while(command, INFER_TIMEOUT, &mut || {}, &mut should_continue)?;
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
        self.request_while(command, REQUEST_TIMEOUT, &mut || {}, &mut || true)
    }

    fn request_with_timeout(
        &mut self,
        command: WorkerCommand,
        timeout: Duration,
    ) -> Result<Vec<WorkerEnvelope>, WorkerClientError> {
        self.request_while(command, timeout, &mut || {}, &mut || true)
    }

    fn request_while(
        &mut self,
        command: WorkerCommand,
        timeout: Duration,
        on_wait: &mut impl FnMut(),
        should_continue: &mut impl FnMut() -> bool,
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
        let deadline = Instant::now() + timeout;
        let mut last_wait = Instant::now();
        let mut collected = Vec::new();
        loop {
            if !should_continue() {
                self.kill();
                return Err(WorkerClientError::Cancelled);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(WorkerClientError::Timeout);
            }
            let slice = remaining.min(WAIT_SLICE).min(CANCEL_POLL);
            match self.rx.recv_timeout(slice) {
                Ok(result) => {
                    let envelope = result?;
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
                Err(RecvTimeoutError::Timeout) => {
                    if !should_continue() {
                        self.kill();
                        return Err(WorkerClientError::Cancelled);
                    }
                    if Instant::now() >= deadline {
                        return Err(WorkerClientError::Timeout);
                    }
                    if last_wait.elapsed() >= WAIT_SLICE {
                        last_wait = Instant::now();
                        on_wait();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(WorkerClientError::Disconnected);
                }
            }
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
/// executable (plain name or Tauri sidecar suffix), then Cargo target directories.
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
        && let Some(sibling) = first_process_binary(dir, kind.binary_stem())
    {
        return Ok(sibling);
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut dir = PathBuf::from(manifest);
        for _ in 0..6 {
            for profile in ["debug", "release"] {
                let candidate_dir = dir.join("target").join(profile);
                if let Some(candidate) = first_process_binary(&candidate_dir, kind.binary_stem()) {
                    return Ok(candidate);
                }
                let nested = dir.join("rust").join("target").join(profile);
                if let Some(candidate) = first_process_binary(&nested, kind.binary_stem()) {
                    return Ok(candidate);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_wait_slice_stays_under_engine_idle_timeout() {
        let engine_idle = Duration::from_secs(180);
        assert!(WAIT_SLICE < engine_idle);
        assert!(LOAD_TIMEOUT > engine_idle);
        assert!(INFER_TIMEOUT < engine_idle);
        assert!(CANCEL_POLL < WAIT_SLICE);
        assert!(CANCEL_POLL < INFER_TIMEOUT);
    }

    #[test]
    fn cancelled_error_is_distinct_from_timeout() {
        assert!(WorkerClientError::Cancelled.is_cancelled());
        assert!(!WorkerClientError::Timeout.is_cancelled());
    }
}
