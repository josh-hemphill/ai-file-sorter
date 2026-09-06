//! Spawn `aifs-engine` and speak the JSONL protocol on its stdio.

use aifs_domain::WorkspaceSnapshot;
use aifs_protocol::{
    decode_line, encode_line, Command, Envelope, ErrorCode, Event, Request, RequestId, ScanOptions,
    PROTOCOL_VERSION,
};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const GRACEFUL_SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const MUTATING_SHUTDOWN_WAIT: Duration = Duration::from_secs(30 * 60);
const ENGINE_BINARY: &str = if cfg!(windows) {
    "aifs-engine.exe"
} else {
    "aifs-engine"
};

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
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Result<Envelope, ClientError>>,
    next_id: u64,
    buffered: Vec<Envelope>,
    mutating: bool,
}

impl EngineClient {
    /// Spawns `binary` and starts a stdout reader thread.
    pub fn spawn(binary: impl AsRef<Path>) -> Result<Self, ClientError> {
        let binary = binary.as_ref();
        if !binary.exists() {
            return Err(ClientError::EngineNotFound(binary.display().to_string()));
        }
        let mut command = ProcessCommand::new(binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
            child,
            stdin: Some(stdin),
            rx,
            next_id: 1,
            buffered: Vec::new(),
            mutating: false,
        })
    }

    /// Spawns the engine at `binary` and completes `hello`.
    pub fn connect(binary: impl AsRef<Path>, client_name: &str) -> Result<Self, ClientError> {
        let mut client = Self::spawn(binary)?;
        client.hello(client_name)?;
        Ok(client)
    }

    /// Looks up the engine binary and connects.
    pub fn connect_default(client_name: &str) -> Result<Self, ClientError> {
        Self::connect(discover_engine_binary()?, client_name)
    }

    /// Negotiates the protocol version.
    pub fn hello(&mut self, client: &str) -> Result<Vec<String>, ClientError> {
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
        &mut self,
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
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message })
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected scan event {other:?}"
                    )))
                }
            }
        }
        Err(ClientError::Unexpected(
            "scan ended without scan_completed".to_owned(),
        ))
    }

    /// Asks the engine to exit.
    pub fn shutdown(&mut self) -> Result<(), ClientError> {
        let _ = self.request(Command::Shutdown)?;
        Ok(())
    }

    /// Builds a heuristic proposal for a session.
    pub fn propose(
        &mut self,
        session: aifs_domain::SessionId,
        policy: aifs_protocol::ProposalPolicy,
    ) -> Result<aifs_domain::ProposalRevision, ClientError> {
        self.expect_revision(Command::Propose { session, policy })
    }

    /// Applies patches, producing a child revision.
    pub fn patch(
        &mut self,
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
        &mut self,
        session: aifs_domain::SessionId,
        revision: aifs_domain::RevisionId,
    ) -> Result<(aifs_domain::OperationPlan, Vec<aifs_domain::PlanIssue>), ClientError> {
        let envelopes = self.request(Command::Plan { session, revision })?;
        for envelope in envelopes {
            match envelope.event {
                Event::Planned { plan, issues } => return Ok((plan, issues)),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message })
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected plan event {other:?}"
                    )))
                }
            }
        }
        Err(ClientError::Unexpected(
            "plan ended without planned".to_owned(),
        ))
    }

    /// Applies a plan (or dry-runs it).
    pub fn apply(
        &mut self,
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
        &mut self,
        session: aifs_domain::SessionId,
        journal: aifs_domain::JournalId,
    ) -> Result<aifs_domain::ApplyJournal, ClientError> {
        self.expect_journal(Command::Undo { session, journal })
    }

    fn expect_revision(
        &mut self,
        command: Command,
    ) -> Result<aifs_domain::ProposalRevision, ClientError> {
        let envelopes = self.request(command)?;
        for envelope in envelopes {
            match envelope.event {
                Event::Revision { revision } => return Ok(revision),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message })
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected revision event {other:?}"
                    )))
                }
            }
        }
        Err(ClientError::Unexpected(
            "request ended without revision".to_owned(),
        ))
    }

    fn expect_journal(
        &mut self,
        command: Command,
    ) -> Result<aifs_domain::ApplyJournal, ClientError> {
        let envelopes = self.request(command)?;
        for envelope in envelopes {
            match envelope.event {
                Event::Journal { journal } => return Ok(journal),
                Event::Failed { code, message, .. } => {
                    return Err(ClientError::Engine { code, message })
                }
                Event::Progress { .. } | Event::Log { .. } => {}
                other => {
                    return Err(ClientError::Unexpected(format!(
                        "unexpected journal event {other:?}"
                    )))
                }
            }
        }
        Err(ClientError::Unexpected(
            "request ended without journal".to_owned(),
        ))
    }

    /// Sends a command and collects events until a terminal one for that id.
    pub fn request(&mut self, command: Command) -> Result<Vec<Envelope>, ClientError> {
        self.request_with_events(command, |_| {})
    }

    /// Like [`Self::request`], invoking `on_event` for every matching envelope
    /// (including progress) as it arrives.
    pub fn request_with_events(
        &mut self,
        command: Command,
        mut on_event: impl FnMut(&Envelope),
    ) -> Result<Vec<Envelope>, ClientError> {
        let mutating = command_mutates_disk(&command);
        if mutating {
            self.mutating = true;
        }
        let id = RequestId(self.next_id.to_string());
        self.next_id += 1;
        let request = Request {
            id: id.clone(),
            command,
        };
        let line = encode_line(&request).map_err(|error| ClientError::Codec(error.to_string()))?;
        {
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| std::io::Error::other("engine stdin closed"))?;
            writeln!(stdin, "{line}")?;
            stdin.flush()?;
        }

        let mut collected = Vec::new();
        loop {
            let envelope = self.recv_next()?;
            let matches = envelope.id.as_ref() == Some(&id) || envelope.id.is_none();
            if !matches {
                self.buffered.push(envelope);
                continue;
            }
            on_event(&envelope);
            let terminal = envelope.is_terminal();
            collected.push(envelope);
            if terminal {
                if mutating {
                    self.mutating = false;
                }
                return Ok(collected);
            }
        }
    }

    fn recv_next(&mut self) -> Result<Envelope, ClientError> {
        if !self.buffered.is_empty() {
            return Ok(self.buffered.remove(0));
        }
        match self.rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(ClientError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ClientError::Disconnected),
        }
    }
}

impl Drop for EngineClient {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            if let Ok(line) = encode_line(&Request {
                id: RequestId("shutdown".to_owned()),
                command: Command::Shutdown,
            }) {
                let _ = writeln!(stdin, "{line}");
                let _ = stdin.flush();
            }
        }
        let wait = if self.mutating {
            MUTATING_SHUTDOWN_WAIT
        } else {
            GRACEFUL_SHUTDOWN_WAIT
        };
        let deadline = Instant::now() + wait;
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

fn command_mutates_disk(command: &Command) -> bool {
    matches!(
        command,
        Command::Apply { dry_run: false, .. } | Command::Undo { .. }
    )
}

/// Resolves the engine binary from `AIFS_ENGINE`, then a sibling of the current
/// executable, then well-known Cargo target directories.
pub fn discover_engine_binary() -> Result<PathBuf, ClientError> {
    if let Ok(explicit) = std::env::var("AIFS_ENGINE") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Ok(path);
        }
        return Err(ClientError::EngineNotFound(path.display().to_string()));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(ENGINE_BINARY);
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut dir = PathBuf::from(manifest);
        for _ in 0..6 {
            for profile in ["debug", "release"] {
                let candidate = dir.join("target").join(profile).join(ENGINE_BINARY);
                if candidate.exists() {
                    return Ok(candidate);
                }
                let nested = dir
                    .join("rust")
                    .join("target")
                    .join(profile)
                    .join(ENGINE_BINARY);
                if nested.exists() {
                    return Ok(nested);
                }
            }
            if !dir.pop() {
                break;
            }
        }
    }

    Err(ClientError::EngineNotFound(
        "set AIFS_ENGINE or install aifs-engine next to this binary".to_owned(),
    ))
}
