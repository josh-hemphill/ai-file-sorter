//! Spawn `aifs-engine` and speak the JSONL protocol on its stdio.

use aifs_domain::WorkspaceSnapshot;
use aifs_protocol::worker::WorkerKind;
use aifs_protocol::{
    AppSettings, Command, ENGINE_PROCESS_STEM, Envelope, ErrorCode, Event, ModelBackend,
    ModelInventory, PROTOCOL_VERSION, Request, RequestId, ScanOptions, decode_line,
    discover_process_binary, encode_line, is_usable_process_binary,
};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as ProcessCommand, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Idle wait for the next engine event. Reset on every progress/log line so a long
/// scan can exceed this as long as it keeps emitting. Must be greater than the
/// worker infer timeout (120s).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const GRACEFUL_SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const MUTATING_SHUTDOWN_WAIT: Duration = Duration::from_secs(30 * 60);

/// Assistant reply from a `chat` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatReply {
    /// Tool summary shown to the user.
    pub message: String,
    /// Child revision when tools produced patches.
    pub revision: Option<aifs_domain::ProposalRevision>,
}

/// Failures talking to the engine process.
#[derive(Debug, Error)]
pub enum ClientError {
    /// The engine binary could not be found.
    #[error("engine binary not found: {0}")]
    EngineNotFound(String),
    /// Spawning the process failed.
    #[error("failed to spawn engine: {0}")]
    Spawn(#[from] std::io::Error),
    /// The engine closed stdout before a terminal event.
    #[error("engine closed stdout unexpectedly")]
    Disconnected,
    /// Timed out waiting for a terminal event.
    #[error("timed out waiting for the engine")]
    Timeout,
    /// The in-flight request ended with `cancelled` (no snapshot / no success).
    #[error("request cancelled")]
    Cancelled,
    /// A protocol line could not be parsed.
    #[error("invalid engine output: {0}")]
    Codec(String),
    /// The engine returned a `failed` event.
    #[error("engine error {code:?}: {message}")]
    Engine {
        /// Stable code.
        code: ErrorCode,
        /// Text.
        message: String,
    },
    /// A terminal event was the wrong type.
    #[error("{0}")]
    Unexpected(String),
}

/// Client that owns an engine child process.
pub struct EngineClient {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    rx: Mutex<Receiver<Result<Envelope, ClientError>>>,
    next_id: AtomicU64,
    buffered: Mutex<Vec<Envelope>>,
    mutating: AtomicBool,
    in_flight: Mutex<Option<RequestId>>,
    request_lock: Mutex<()>,
}

impl EngineClient {
    /// Spawns `binary` and starts a stdout reader thread.
    pub fn spawn(binary: impl AsRef<Path>) -> Result<Self, ClientError> {
        Self::spawn_process(binary, None)
    }

    fn spawn_process(binary: impl AsRef<Path>, store: Option<&Path>) -> Result<Self, ClientError> {
        let binary = binary.as_ref();
        if !binary.exists() {
            return Err(ClientError::EngineNotFound(binary.display().to_string()));
        }
        let mut command = ProcessCommand::new(binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(store) = store {
            command.env("AIFS_STORE", store);
        }
        attach_worker_env(&mut command, binary);
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("engine stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("engine stdout was not piped"))?;
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[aifs-engine] {line}");
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
                        let parsed = decode_line::<Envelope>(&line)
                            .map_err(|error| ClientError::Codec(error.to_string()));
                        if tx.send(parsed).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(ClientError::Spawn(error)));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
            rx: Mutex::new(rx),
            next_id: AtomicU64::new(1),
            buffered: Mutex::new(Vec::new()),
            mutating: AtomicBool::new(false),
            in_flight: Mutex::new(None),
            request_lock: Mutex::new(()),
        })
    }

    /// Spawns the engine at `binary` and completes `hello`.
    pub fn connect(binary: impl AsRef<Path>, client_name: &str) -> Result<Self, ClientError> {
        let client = Self::spawn(binary)?;
        client.hello(client_name)?;
        Ok(client)
    }

    /// Like [`Self::spawn`], pointing the engine at `store` via `AIFS_STORE`.
    pub fn spawn_with_store(
        binary: impl AsRef<Path>,
        store: impl AsRef<Path>,
    ) -> Result<Self, ClientError> {
        Self::spawn_process(binary, Some(store.as_ref()))
    }

    /// Like [`Self::connect`], pointing the engine at `store` via `AIFS_STORE`.
    pub fn connect_with_store(
        binary: impl AsRef<Path>,
        client_name: &str,
        store: impl AsRef<Path>,
    ) -> Result<Self, ClientError> {
        let client = Self::spawn_with_store(binary, store)?;
        client.hello(client_name)?;
        Ok(client)
    }

    /// Looks up the engine binary and connects.
    pub fn connect_default(client_name: &str) -> Result<Self, ClientError> {
        Self::connect(discover_engine_binary()?, client_name)
    }

    /// Asks the engine to stop `target` at the next cooperative check.
    pub fn cancel(&self, target: RequestId) -> Result<(), ClientError> {
        let _ = self.write_command(Command::Cancel { target })?;
        Ok(())
    }

    /// Cancels the request currently waiting in [`Self::request_with_events`].
    pub fn cancel_in_flight(&self) -> Result<(), ClientError> {
        let target = self
            .in_flight
            .lock()
            .map_err(|_| std::io::Error::other("in-flight lock poisoned"))?
            .clone();
        let Some(target) = target else {
            return Ok(());
        };
        self.cancel(target)
    }

    fn allocate_id(&self) -> RequestId {
        RequestId(self.next_id.fetch_add(1, Ordering::Relaxed).to_string())
    }

    fn write_command(&self, command: Command) -> Result<RequestId, ClientError> {
        let id = self.allocate_id();
        self.write_request(&id, command)?;
        Ok(id)
    }

    fn write_request(&self, id: &RequestId, command: Command) -> Result<(), ClientError> {
        let request = Request {
            id: id.clone(),
            command,
        };
        let line = encode_line(&request).map_err(|error| ClientError::Codec(error.to_string()))?;
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| std::io::Error::other("stdin lock poisoned"))?;
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("engine stdin closed"))?;
        writeln!(stdin, "{line}")?;
        stdin.flush()?;
        Ok(())
    }

    /// Negotiates the protocol version.
    pub fn hello(&self, client: &str) -> Result<Vec<String>, ClientError> {
        let envelopes = self.request(Command::Hello {
            client: client.to_owned(),
            protocol_version: PROTOCOL_VERSION,
        })?;
        match &envelopes.last().map(|envelope| &envelope.event) {
            Some(Event::Ready { capabilities, .. }) => Ok(capabilities.clone()),
            Some(Event::Failed { code, message, .. }) => Err(ClientError::Engine {
                code: *code,
                message: message.clone(),
            }),
            other => Err(ClientError::Unexpected(format!(
                "expected ready, got {other:?}"
            ))),
        }
    }

    /// Runs a scan and returns the completed snapshot.
    pub fn scan(
        &self,
        root: impl AsRef<Path>,
        options: ScanOptions,
        session: Option<aifs_domain::SessionId>,
    ) -> Result<WorkspaceSnapshot, ClientError> {
        let envelopes = self.request(Command::Scan {
            root: root.as_ref().to_path_buf(),
            options,
            session,
        })?;
        for envelope in envelopes {
            match envelope.event {
                Event::ScanCompleted { snapshot } => return Ok(snapshot),
                Event::Cancelled => return Err(ClientError::Cancelled),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message });
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected scan event {other:?}"
                    )));
                }
            }
        }
        Err(ClientError::Unexpected(
            "scan ended without scan_completed".to_owned(),
        ))
    }

    /// Asks the engine to exit.
    pub fn shutdown(&self) -> Result<(), ClientError> {
        let _ = self.request(Command::Shutdown)?;
        Ok(())
    }

    /// Builds a heuristic proposal for a session.
    pub fn propose(
        &self,
        session: aifs_domain::SessionId,
        policy: aifs_protocol::ProposalPolicy,
    ) -> Result<aifs_domain::ProposalRevision, ClientError> {
        self.expect_revision(Command::Propose { session, policy })
    }

    /// Applies patches, producing a child revision.
    pub fn patch(
        &self,
        session: aifs_domain::SessionId,
        base_revision: aifs_domain::RevisionId,
        author: aifs_domain::RevisionAuthor,
        summary: impl Into<String>,
        patches: Vec<aifs_domain::RevisionPatch>,
    ) -> Result<aifs_domain::ProposalRevision, ClientError> {
        self.expect_revision(Command::Patch {
            session,
            base_revision,
            author,
            summary: summary.into(),
            patches,
        })
    }

    /// Validates a revision into an operation plan.
    pub fn plan(
        &self,
        session: aifs_domain::SessionId,
        revision: aifs_domain::RevisionId,
    ) -> Result<(aifs_domain::OperationPlan, Vec<aifs_domain::PlanIssue>), ClientError> {
        let envelopes = self.request(Command::Plan { session, revision })?;
        for envelope in envelopes {
            match envelope.event {
                Event::Planned { plan, issues } => return Ok((plan, issues)),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message });
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected plan event {other:?}"
                    )));
                }
            }
        }
        Err(ClientError::Unexpected(
            "plan ended without planned".to_owned(),
        ))
    }

    /// Applies a plan (or dry-runs it).
    pub fn apply(
        &self,
        session: aifs_domain::SessionId,
        plan: aifs_domain::PlanId,
        dry_run: bool,
    ) -> Result<aifs_domain::ApplyJournal, ClientError> {
        self.expect_journal(Command::Apply {
            session,
            plan,
            dry_run,
        })
    }

    /// Undoes a journal.
    pub fn undo(
        &self,
        session: aifs_domain::SessionId,
        journal: aifs_domain::JournalId,
    ) -> Result<aifs_domain::ApplyJournal, ClientError> {
        self.expect_journal(Command::Undo { session, journal })
    }

    /// Runs assistant tools against a revision. Returns a child revision when patches land.
    pub fn chat(
        &self,
        session: aifs_domain::SessionId,
        revision: aifs_domain::RevisionId,
        utterance: impl Into<String>,
    ) -> Result<ChatReply, ClientError> {
        let envelopes = self.request(Command::Chat {
            session,
            revision,
            utterance: utterance.into(),
        })?;
        for envelope in envelopes {
            match envelope.event {
                Event::ChatReply { message, revision } => {
                    return Ok(ChatReply { message, revision });
                }
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message });
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected chat event {other:?}"
                    )));
                }
            }
        }
        Err(ClientError::Unexpected(
            "chat ended without chat_reply".to_owned(),
        ))
    }

    /// Loads persisted classification settings.
    pub fn get_settings(&self) -> Result<AppSettings, ClientError> {
        self.expect_settings(Command::GetSettings)
    }

    /// Replaces persisted classification settings.
    pub fn put_settings(&self, settings: AppSettings) -> Result<AppSettings, ClientError> {
        self.expect_settings(Command::PutSettings { settings })
    }

    fn expect_settings(&self, command: Command) -> Result<AppSettings, ClientError> {
        let envelopes = self.request(command)?;
        for envelope in envelopes {
            match envelope.event {
                Event::Settings { settings } => return Ok(settings),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message });
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected settings event {other:?}"
                    )));
                }
            }
        }
        Err(ClientError::Unexpected(
            "request ended without settings".to_owned(),
        ))
    }

    /// Loads redacted model slot assignments, including per-slot `runtime`.
    pub fn get_models(&self) -> Result<ModelInventory, ClientError> {
        self.expect_models(Command::GetModels)
    }

    /// Replaces model slot assignments.
    pub fn put_models(&self, inventory: ModelInventory) -> Result<ModelInventory, ClientError> {
        self.expect_models(Command::PutModels { inventory })
    }

    /// Validates a backend without scanning.
    pub fn probe_endpoint(
        &self,
        backend: ModelBackend,
        api_key: Option<String>,
    ) -> Result<(bool, String), ClientError> {
        let envelopes = self.request(Command::ProbeEndpoint { backend, api_key })?;
        for envelope in envelopes {
            match envelope.event {
                Event::EndpointProbed { ok, message } => return Ok((ok, message)),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message });
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected probe event {other:?}"
                    )));
                }
            }
        }
        Err(ClientError::Unexpected(
            "request ended without endpoint_probed".to_owned(),
        ))
    }

    /// Downloads catalog GGUFs, skipping files already in the storage directory.
    pub fn download_model(
        &self,
        catalog_id: impl Into<String>,
    ) -> Result<ModelInventory, ClientError> {
        self.expect_models(Command::DownloadModel {
            catalog_id: catalog_id.into(),
        })
    }

    fn expect_models(&self, command: Command) -> Result<ModelInventory, ClientError> {
        let envelopes = self.request(command)?;
        for envelope in envelopes {
            match envelope.event {
                Event::Models { inventory } => return Ok(inventory),
                Event::Cancelled => return Err(ClientError::Cancelled),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message });
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected models event {other:?}"
                    )));
                }
            }
        }
        Err(ClientError::Unexpected(
            "request ended without models".to_owned(),
        ))
    }

    fn expect_revision(
        &self,
        command: Command,
    ) -> Result<aifs_domain::ProposalRevision, ClientError> {
        let envelopes = self.request(command)?;
        for envelope in envelopes {
            match envelope.event {
                Event::Revision { revision } => return Ok(revision),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message });
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected revision event {other:?}"
                    )));
                }
            }
        }
        Err(ClientError::Unexpected(
            "request ended without revision".to_owned(),
        ))
    }

    fn expect_journal(&self, command: Command) -> Result<aifs_domain::ApplyJournal, ClientError> {
        let envelopes = self.request(command)?;
        for envelope in envelopes {
            match envelope.event {
                Event::Journal { journal } => return Ok(journal),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message });
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected journal event {other:?}"
                    )));
                }
            }
        }
        Err(ClientError::Unexpected(
            "request ended without journal".to_owned(),
        ))
    }

    /// Sends a command and collects events until a terminal one for that id.
    pub fn request(&self, command: Command) -> Result<Vec<Envelope>, ClientError> {
        self.request_with_events(command, |_| {})
    }

    /// Like [`Self::request`], invoking `on_event` for every matching envelope
    /// (including progress) as it arrives. Cancel can be sent concurrently via
    /// [`Self::cancel_in_flight`].
    pub fn request_with_events(
        &self,
        command: Command,
        mut on_event: impl FnMut(&Envelope),
    ) -> Result<Vec<Envelope>, ClientError> {
        let _request_guard = self
            .request_lock
            .lock()
            .map_err(|_| std::io::Error::other("request lock poisoned"))?;
        let mutating = command_mutates_disk(&command);
        if mutating {
            self.mutating.store(true, Ordering::Relaxed);
        }
        let id = self.allocate_id();
        if let Ok(mut in_flight) = self.in_flight.lock() {
            *in_flight = Some(id.clone());
        }
        if let Err(error) = self.write_request(&id, command) {
            if let Ok(mut in_flight) = self.in_flight.lock() {
                *in_flight = None;
            }
            if mutating {
                self.mutating.store(false, Ordering::Relaxed);
            }
            return Err(error);
        }

        let mut collected = Vec::new();
        let result = loop {
            let envelope = match self.recv_matching(&id) {
                Ok(envelope) => envelope,
                Err(error) => break Err(error),
            };
            on_event(&envelope);
            let terminal = envelope.is_terminal();
            collected.push(envelope);
            if terminal {
                break Ok(collected);
            }
        };
        if let Ok(mut in_flight) = self.in_flight.lock() {
            *in_flight = None;
        }
        if mutating {
            self.mutating.store(false, Ordering::Relaxed);
        }
        result
    }

    fn recv_matching(&self, id: &RequestId) -> Result<Envelope, ClientError> {
        loop {
            if let Some(envelope) = self.take_buffered_matching(id) {
                return Ok(envelope);
            }
            let envelope = self.recv_from_rx()?;
            let matches = envelope.id.as_ref() == Some(id) || envelope.id.is_none();
            if matches {
                return Ok(envelope);
            }
            // Cancel acks use a different request id and must not be re-buffered
            // in a way that spins `recv` on the same unmatched line.
            if matches!(envelope.event, Event::Cancelled) {
                continue;
            }
            if let Ok(mut buffered) = self.buffered.lock() {
                buffered.push(envelope);
            }
        }
    }

    fn take_buffered_matching(&self, id: &RequestId) -> Option<Envelope> {
        let mut buffered = self.buffered.lock().ok()?;
        let position = buffered
            .iter()
            .position(|envelope| envelope.id.as_ref() == Some(id) || envelope.id.is_none())?;
        Some(buffered.remove(position))
    }

    fn recv_from_rx(&self) -> Result<Envelope, ClientError> {
        let rx = self
            .rx
            .lock()
            .map_err(|_| std::io::Error::other("engine output lock poisoned"))?;
        match rx.recv_timeout(IDLE_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(ClientError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ClientError::Disconnected),
        }
    }
}

impl Drop for EngineClient {
    fn drop(&mut self) {
        if let Ok(mut stdin) = self.stdin.lock()
            && let Some(mut stdin) = stdin.take()
            && let Ok(line) = encode_line(&Request {
                id: RequestId("shutdown".to_owned()),
                command: Command::Shutdown,
            })
        {
            let _ = writeln!(stdin, "{line}");
            let _ = stdin.flush();
        }
        let wait = if self.mutating.load(Ordering::Relaxed) {
            MUTATING_SHUTDOWN_WAIT
        } else {
            GRACEFUL_SHUTDOWN_WAIT
        };
        let deadline = Instant::now() + wait;
        let Ok(mut child) = self.child.lock() else {
            return;
        };
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                _ => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn command_mutates_disk(command: &Command) -> bool {
    matches!(
        command,
        Command::Apply { dry_run: false, .. }
            | Command::Undo { .. }
            | Command::DownloadModel { .. }
    )
}

/// Points the engine at worker binaries next to itself (or Cargo `target/`) so
/// `tauri dev` empty sidecar placeholders do not hide `pnpm desktop:cuda` builds.
fn attach_worker_env(command: &mut ProcessCommand, engine: &Path) {
    for kind in [
        WorkerKind::Media,
        WorkerKind::Document,
        WorkerKind::Vision,
        WorkerKind::Llm,
    ] {
        if std::env::var_os(kind.env_var()).is_some() {
            continue;
        }
        if let Some(path) = discover_process_binary(kind.binary_stem(), Some(engine), None) {
            command.env(kind.env_var(), path);
        }
    }
}

/// Resolves the engine binary from `AIFS_ENGINE`, then a sibling of the current
/// executable (plain name or Tauri `{stem}-{target-triple}` sidecar), then
/// well-known Cargo target directories.
pub fn discover_engine_binary() -> Result<PathBuf, ClientError> {
    discover_engine_binary_from(
        std::env::var("AIFS_ENGINE").ok(),
        std::env::current_exe().ok(),
        std::env::var("CARGO_MANIFEST_DIR").ok(),
    )
}

fn discover_engine_binary_from(
    explicit: Option<String>,
    current_exe: Option<PathBuf>,
    manifest_dir: Option<String>,
) -> Result<PathBuf, ClientError> {
    if let Some(explicit) = explicit {
        let path = PathBuf::from(explicit);
        if is_usable_process_binary(&path) {
            return Ok(path);
        }
        return Err(ClientError::EngineNotFound(path.display().to_string()));
    }
    discover_process_binary(
        ENGINE_PROCESS_STEM,
        current_exe.as_deref(),
        manifest_dir.as_deref().map(Path::new),
    )
    .ok_or_else(|| {
        ClientError::EngineNotFound(
            "set AIFS_ENGINE or install aifs-engine next to this binary".to_owned(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_protocol::target_triple;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn idle_timeout_outlasts_worker_infer() {
        assert_eq!(IDLE_TIMEOUT, Duration::from_secs(180));
        assert!(IDLE_TIMEOUT > Duration::from_secs(120));
    }

    #[test]
    fn discover_uses_explicit_path_when_the_file_exists() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let engine = dir.path().join("custom-engine");
        fs::write(&engine, b"engine").unwrap_or_else(|error| panic!("{error}"));
        let found = discover_engine_binary_from(Some(engine.display().to_string()), None, None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(found, engine);
    }

    #[test]
    fn discover_explicit_missing_path_is_not_found() {
        let error = match discover_engine_binary_from(
            Some("/no/such/aifs-engine".to_owned()),
            None,
            None,
        ) {
            Err(error) => error,
            Ok(path) => panic!("missing explicit path must fail, got {}", path.display()),
        };
        assert!(
            matches!(error, ClientError::EngineNotFound(ref path) if path.contains("aifs-engine")),
            "{error}"
        );
    }

    #[test]
    fn discover_sibling_prefers_plain_name_over_sidecar() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let triple = target_triple().unwrap_or_else(|| panic!("AIFS_TARGET_TRIPLE"));
        let sidecar = dir.path().join(format!("{ENGINE_PROCESS_STEM}-{triple}"));
        let plain = dir.path().join(ENGINE_PROCESS_STEM);
        fs::write(&sidecar, b"sidecar").unwrap_or_else(|error| panic!("{error}"));
        fs::write(&plain, b"plain").unwrap_or_else(|error| panic!("{error}"));
        let exe = dir.path().join("aifs");
        fs::write(&exe, b"ui").unwrap_or_else(|error| panic!("{error}"));
        let found = discover_engine_binary_from(None, Some(exe), None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(found, plain);
    }

    fn engine_bin() -> PathBuf {
        if let Ok(path) = discover_engine_binary() {
            return path;
        }
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "aifs-engine-bin"])
            .current_dir(&workspace)
            .status()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(status.success(), "cargo build -p aifs-engine-bin failed");
        discover_engine_binary()
            .unwrap_or_else(|error| panic!("aifs-engine must be built for client tests: {error}"))
    }

    fn scan_options() -> aifs_protocol::ScanOptions {
        aifs_protocol::ScanOptions {
            extract_metadata: false,
            ..aifs_protocol::ScanOptions::default()
        }
    }

    fn isolated_client(name: &str) -> (tempfile::TempDir, EngineClient) {
        let store_dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store_dir.path().join("engine.sqlite");
        let client = EngineClient::connect_with_store(engine_bin(), name, &store)
            .unwrap_or_else(|error| panic!("{error}"));
        (store_dir, client)
    }

    #[test]
    fn connect_hello_then_scan_returns_snapshot() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.path().join("note.txt"), b"hi").unwrap_or_else(|error| panic!("{error}"));
        let (_store, client) = isolated_client("client-hello");
        let snapshot = client
            .scan(root.path(), scan_options(), None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.path.as_str() == "note.txt")
        );
        client.shutdown().unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn spawn_scan_without_hello_fails() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.path().join("note.txt"), b"hi").unwrap_or_else(|error| panic!("{error}"));
        let store_dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store_dir.path().join("engine.sqlite");
        let client = EngineClient::spawn_with_store(engine_bin(), &store)
            .unwrap_or_else(|error| panic!("{error}"));
        let error = match client.scan(root.path(), scan_options(), None) {
            Err(error) => error,
            Ok(_) => panic!("scan before hello must fail"),
        };
        match error {
            ClientError::Engine { message, .. } => {
                assert!(message.to_ascii_lowercase().contains("hello"), "{message}");
            }
            other => panic!("expected engine hello error, got {other}"),
        }
    }

    #[test]
    fn scan_maps_cancelled_event_to_client_error() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        for index in 0..400 {
            fs::write(root.path().join(format!("file-{index}.txt")), b"x")
                .unwrap_or_else(|error| panic!("{error}"));
        }
        let (_store, client) = isolated_client("client-cancel");
        let client = std::sync::Arc::new(client);
        let canceller = std::sync::Arc::clone(&client);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag = std::sync::Arc::clone(&stop);
        let canceller_thread = thread::spawn(move || {
            while !stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = canceller.cancel_in_flight();
                thread::sleep(Duration::from_millis(5));
            }
        });
        let result = client.scan(root.path(), scan_options(), None);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = canceller_thread.join();
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("cancelled scan must fail"),
        };
        assert!(
            matches!(error, ClientError::Cancelled),
            "expected Cancelled, got {error}"
        );
        let snapshot = client
            .scan(root.path(), scan_options(), None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            snapshot.entries.len() >= 400,
            "follow-up scan after cancel should complete, got {}",
            snapshot.entries.len()
        );
        client.shutdown().unwrap_or_else(|error| panic!("{error}"));
    }
}
