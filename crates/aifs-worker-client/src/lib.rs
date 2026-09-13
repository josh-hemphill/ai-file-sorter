//! Spawn a worker binary and speak the worker JSONL protocol on its stdio.

use aifs_domain::{Evidence, ObservedEntry};
use aifs_protocol::worker::{
    WORKER_PROTOCOL_VERSION, WorkerCommand, WorkerEnvelope, WorkerEvent, WorkerKind, WorkerRequest,
};
use aifs_protocol::{
    ErrorCode, FolderStyle, ModelBackend, RequestId, decode_line, discover_process_binary,
    encode_line, is_usable_process_binary,
};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
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
/// Last stderr / non-JSON stdout lines kept for spawn/hello failures.
const DIAGNOSTIC_LINE_CAP: usize = 16;
const DIAGNOSTIC_DRAIN: Duration = Duration::from_millis(50);

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
    #[error("{0}")]
    Disconnected(String),
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
    binary: PathBuf,
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Result<WorkerEnvelope, WorkerClientError>>,
    next_id: u64,
    capabilities: Vec<String>,
    diagnostics: Arc<Mutex<Vec<String>>>,
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
        apply_worker_library_path(&mut command, binary);
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("worker stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("worker stdout was not piped"))?;
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let prefix = kind.binary_stem();
        if let Some(stderr) = child.stderr.take() {
            let captured = Arc::clone(&diagnostics);
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[{prefix}] {line}");
                    push_diagnostic(&captured, line);
                }
            });
        }
        let (tx, rx) = mpsc::channel();
        let stdout_diag = Arc::clone(&diagnostics);
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        // llama.cpp / CUDA may print banners on stdout before hello JSONL.
                        if !line.trim_start().starts_with('{') {
                            eprintln!("[{prefix}] {line}");
                            push_diagnostic(&stdout_diag, line);
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
            binary: binary.to_path_buf(),
            child,
            stdin: Some(stdin),
            rx,
            next_id: 1,
            capabilities: Vec::new(),
            diagnostics,
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
        let write_error = {
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| std::io::Error::other("worker stdin closed"))?;
            writeln!(stdin, "{line}").and_then(|()| stdin.flush()).err()
        };
        if let Some(error) = write_error {
            return Err(self.io_or_disconnected(error));
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
                    return Err(self.disconnected_error());
                }
            }
        }
    }

    fn disconnected_error(&mut self) -> WorkerClientError {
        thread::sleep(DIAGNOSTIC_DRAIN);
        let status = match self.child.try_wait() {
            Ok(Some(status)) => Some(status),
            Ok(None) => {
                thread::sleep(DIAGNOSTIC_DRAIN);
                self.child.try_wait().ok().flatten()
            }
            Err(_) => None,
        };
        WorkerClientError::Disconnected(format_disconnected(
            status,
            &diagnostic_snapshot(&self.diagnostics),
            Some(&self.binary),
        ))
    }

    fn io_or_disconnected(&mut self, error: std::io::Error) -> WorkerClientError {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            self.disconnected_error()
        } else {
            WorkerClientError::Spawn(error)
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
        if is_usable_process_binary(&path) {
            return Ok(path);
        }
        return Err(WorkerClientError::NotFound(path.display().to_string()));
    }
    discover_process_binary(
        kind.binary_stem(),
        std::env::current_exe().ok().as_deref(),
        std::env::var_os("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .as_deref(),
    )
    .ok_or_else(|| {
        WorkerClientError::NotFound(format!(
            "set {} or install {name} next to aifs-engine",
            kind.env_var()
        ))
    })
}

fn push_diagnostic(buffer: &Mutex<Vec<String>>, line: String) {
    let Ok(mut lines) = buffer.lock() else {
        return;
    };
    if lines.len() >= DIAGNOSTIC_LINE_CAP {
        lines.remove(0);
    }
    lines.push(line);
}

fn diagnostic_snapshot(buffer: &Mutex<Vec<String>>) -> Vec<String> {
    buffer.lock().map(|lines| lines.clone()).unwrap_or_default()
}

fn format_disconnected(
    status: Option<ExitStatus>,
    diagnostics: &[String],
    spawned: Option<&Path>,
) -> String {
    let mut message = String::from("worker closed stdout unexpectedly");
    if let Some(path) = spawned {
        message.push_str(&format!(" [{}]", path.display()));
    }
    if let Some(status) = status {
        if let Some(code) = status.code() {
            message.push_str(&format!(" ({})", format_exit_code(code)));
            if let Some(path) = spawned
                && let Some(hint) = dll_search_hint(code, path)
            {
                message.push(' ');
                message.push_str(&hint);
            }
        } else {
            message.push_str(&format!(" ({status})"));
        }
    }
    if !diagnostics.is_empty() {
        message.push_str(": ");
        message.push_str(&diagnostics.join(" | "));
    }
    message
}

/// Directories Windows/Linux/macOS should search for llama.cpp / CUDA runtime libs.
fn worker_library_dirs(binary: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let Some(start) = binary.parent() else {
        return dirs;
    };
    dirs.push(start.to_path_buf());
    dirs.push(start.join("deps"));
    dirs.push(start.join("resources"));
    dirs.push(start.join("resources").join("binaries"));
    dirs.push(start.join("resources").join("llm-runtime"));
    let mut dir = start.to_path_buf();
    for _ in 0..8 {
        for profile in ["debug", "release"] {
            let target = dir.join("target").join(profile);
            dirs.push(target.clone());
            dirs.push(target.join("deps"));
        }
        dirs.push(dir.join("resources").join("llm-runtime"));
        if !dir.pop() {
            break;
        }
    }
    if let Some(cuda) = std::env::var_os("CUDA_PATH") {
        let cuda = PathBuf::from(cuda);
        dirs.push(cuda.join("bin"));
        dirs.push(cuda.join("lib").join("x64"));
        dirs.push(cuda.join("lib64"));
    }
    dirs
}

fn library_path_key() -> &'static str {
    if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    }
}

fn merge_search_path(existing: Option<OsString>, dirs: &[PathBuf]) -> OsString {
    let mut parts: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();
    for dir in dirs.iter().filter(|dir| dir.is_dir()) {
        if seen.insert(dir.clone()) {
            parts.push(dir.clone());
        }
    }
    if let Some(existing) = &existing {
        for part in std::env::split_paths(existing) {
            if !part.as_os_str().is_empty() && seen.insert(part.clone()) {
                parts.push(part);
            }
        }
    }
    std::env::join_paths(&parts).unwrap_or_else(|_| existing.unwrap_or_default())
}

fn apply_worker_library_path(command: &mut Command, binary: &Path) {
    let key = library_path_key();
    let existing = command
        .get_envs()
        .find(|(name, _)| *name == key)
        .and_then(|(_, value)| value.map(std::ffi::OsStr::to_os_string))
        .or_else(|| std::env::var_os(key));
    command.env(
        key,
        merge_search_path(existing, &worker_library_dirs(binary)),
    );
}

/// Windows `STATUS_DLL_NOT_FOUND` (`NtStatus` 0xC0000135) as a process exit code.
const WINDOWS_STATUS_DLL_NOT_FOUND: u32 = 0xC000_0135;
const WINDOWS_STATUS_DLL_INIT_FAILED: u32 = 0xC000_0142;
const WINDOWS_STATUS_INVALID_IMAGE_FORMAT: u32 = 0xC000_007B;
const WINDOWS_STATUS_ACCESS_VIOLATION: u32 = 0xC000_0005;

fn format_exit_code(code: i32) -> String {
    match explain_worker_exit_code(code) {
        Some(meaning) => format!("exit {code} / 0x{:08X}: {meaning}", code as u32),
        None => format!("exit {code}"),
    }
}

fn dll_search_hint(code: i32, spawned: &Path) -> Option<String> {
    if code as u32 != WINDOWS_STATUS_DLL_NOT_FOUND {
        return None;
    }
    let folder = spawned.parent().unwrap_or(spawned);
    Some(format!(
        "Windows searched {folder} first for PE imports; ggml/llama/CUDA runtime DLLs must sit in that folder (other CUDA apps working does not put those sidecar DLLs beside this worker)",
        folder = folder.display()
    ))
}

fn explain_worker_exit_code(code: i32) -> Option<&'static str> {
    match code as u32 {
        WINDOWS_STATUS_DLL_NOT_FOUND => Some(
            "required DLL not found (Windows STATUS_DLL_NOT_FOUND). The loader exits before stderr exists. Windows looks in the worker exe folder first — ggml/llama/CUDA runtime DLLs must sit beside that sidecar; a working NVIDIA driver is not enough if those DLLs were left in target/debug",
        ),
        WINDOWS_STATUS_DLL_INIT_FAILED => {
            Some("a DLL failed to initialize (Windows STATUS_DLL_INIT_FAILED)")
        }
        WINDOWS_STATUS_INVALID_IMAGE_FORMAT => Some(
            "invalid image format (Windows STATUS_INVALID_IMAGE_FORMAT; often 32-bit vs 64-bit DLL mismatch)",
        ),
        WINDOWS_STATUS_ACCESS_VIOLATION => {
            Some("access violation (Windows STATUS_ACCESS_VIOLATION)")
        }
        _ => None,
    }
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

    #[test]
    fn format_disconnected_includes_stderr_lines() {
        let message = format_disconnected(None, &["libcuda.so.1: cannot open".to_owned()], None);
        assert!(
            message.contains("worker closed stdout unexpectedly"),
            "{message}"
        );
        assert!(message.contains("libcuda.so.1"), "{message}");
    }

    #[test]
    fn format_disconnected_includes_spawned_path() {
        let path = Path::new("apps/desktop/src-tauri/binaries/aifs-worker-llm.exe");
        let message = format_disconnected(None, &[], Some(path));
        assert!(message.contains("aifs-worker-llm.exe"), "{message}");
    }

    #[test]
    fn windows_dll_not_found_exit_explains_sidecar_runtime_libs() {
        let code = WINDOWS_STATUS_DLL_NOT_FOUND as i32;
        assert_eq!(code, -1_073_741_515);
        let meaning = explain_worker_exit_code(code)
            .unwrap_or_else(|| panic!("expected STATUS_DLL_NOT_FOUND"));
        assert!(meaning.contains("STATUS_DLL_NOT_FOUND"), "{meaning}");
        assert!(meaning.contains("target/debug"), "{meaning}");
        assert!(
            !meaning.contains("nvcuda.dll"),
            "driver DLL is the wrong default cause: {meaning}"
        );
        let message = format_exit_code(code);
        assert!(message.contains("0xC0000135"), "{message}");
        assert!(message.contains("STATUS_DLL_NOT_FOUND"), "{message}");
        let spawned = Path::new("apps/desktop/src-tauri/binaries/aifs-worker-llm.exe");
        let hint = dll_search_hint(code, spawned).unwrap_or_else(|| panic!("hint"));
        assert!(hint.contains("binaries"), "{hint}");
        assert!(hint.contains("other CUDA apps"), "{hint}");
    }

    #[test]
    fn worker_library_dirs_include_exe_dir_target_and_cuda() {
        let binary = Path::new("/workspace/apps/desktop/src-tauri/binaries/aifs-worker-llm");
        let dirs = worker_library_dirs(binary);
        let parent = binary.parent().unwrap_or_else(|| panic!("parent"));
        assert!(
            dirs.iter()
                .any(|dir| dir == parent || dir.ends_with("binaries")),
            "{dirs:?}"
        );
        assert!(
            dirs.iter()
                .any(|dir| dir.ends_with(Path::new("target").join("debug"))),
            "{dirs:?}"
        );
        assert!(
            dirs.iter()
                .any(|dir| dir.ends_with(Path::new("target").join("debug").join("deps"))),
            "{dirs:?}"
        );
    }

    #[test]
    fn merge_search_path_prepends_existing_dirs() {
        let root = std::env::temp_dir().join(format!(
            "aifs-worker-libpath-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
        let merged =
            merge_search_path(Some(OsString::from("keep-me")), std::slice::from_ref(&root));
        let merged = merged.to_string_lossy();
        assert!(merged.contains(&root.display().to_string()), "{merged}");
        assert!(merged.contains("keep-me"), "{merged}");
        let root_pos = merged
            .find(&root.display().to_string())
            .unwrap_or(usize::MAX);
        let keep_pos = merged.find("keep-me").unwrap_or(0);
        assert!(root_pos < keep_pos, "{merged}");
        std::fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[cfg(unix)]
    #[test]
    fn hello_failure_includes_stderr_and_exit_code() {
        let error = WorkerClient::connect_command(WorkerKind::Llm, "/bin/sh", |cmd| {
            cmd.arg("-c")
                .arg("echo 'error while loading shared libraries: libcuda.so.1' >&2; exit 127");
        })
        .err()
        .unwrap_or_else(|| panic!("expected hello to fail"));
        let text = error.to_string();
        assert!(text.contains("exit 127"), "{text}");
        assert!(text.contains("libcuda.so.1"), "{text}");
    }
}
