//! In-process engine used by the `aifs-engine` stdio binary.
//!
//! This slice implements `hello`, `scan`, `cancel`, and `shutdown`. Other commands
//! reply with a clear "not implemented" failure so clients can negotiate capabilities.

use aifs_domain::{SessionId, WorkspaceSnapshot};
use aifs_extractors::extract_into_with_progress;
use aifs_protocol::{
    decode_line, encode_line, Command, Envelope, ErrorCode, Event, Request, RequestId, ScanOptions,
    PROTOCOL_VERSION,
};
use aifs_relationships::enrich;
use aifs_scanner::{scan, ScanError};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Engine crate version reported on `hello`.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Capabilities advertised in this slice.
pub fn capabilities() -> Vec<String> {
    vec!["scan".to_owned(), "media_tags".to_owned()]
}

/// In-memory session store plus stdio request dispatch.
#[derive(Default)]
pub struct Engine {
    sessions: HashMap<SessionId, WorkspaceSnapshot>,
    hello_ok: bool,
    shutdown: bool,
}

impl Engine {
    /// Creates an idle engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// True after a `shutdown` command has been handled.
    pub fn should_exit(&self) -> bool {
        self.shutdown
    }

    /// Handles one already-decoded request, collecting envelopes until the request ends.
    pub fn handle(&mut self, request: Request) -> Vec<Envelope> {
        let mut events = Vec::new();
        self.handle_with(request, &mut |envelope| events.push(envelope));
        events
    }

    /// Handles one request, emitting envelopes as they are produced (including progress).
    pub fn handle_with(&mut self, request: Request, emit: &mut impl FnMut(Envelope)) {
        match request.command {
            Command::Hello {
                client: _,
                protocol_version,
            } => emit(self.handle_hello(&request.id, protocol_version)),
            Command::Scan {
                root,
                options,
                session,
            } => self.handle_scan(&request.id, &root, options, session, emit),
            Command::Shutdown => {
                self.shutdown = true;
                emit(Envelope::reply(&request.id, Event::Shutdown));
            }
            Command::Cancel { .. } => emit(Envelope::reply(&request.id, Event::Cancelled)),
            other => emit(Envelope::reply(
                &request.id,
                Event::Failed {
                    code: ErrorCode::Internal,
                    message: format!(
                        "{} is not implemented in this engine slice",
                        command_name(&other)
                    ),
                    issues: vec![],
                },
            )),
        }
    }

    /// Parses one stdin line and returns the envelopes to write.
    pub fn handle_line(&mut self, line: &str) -> Vec<Envelope> {
        let mut events = Vec::new();
        self.handle_line_with(line, &mut |envelope| events.push(envelope));
        events
    }

    /// Parses one stdin line and emits envelopes as they are produced.
    pub fn handle_line_with(&mut self, line: &str, emit: &mut impl FnMut(Envelope)) {
        match decode_line::<Request>(line) {
            Ok(request) => self.handle_with(request, emit),
            Err(error) => emit(Envelope::broadcast(Event::Failed {
                code: ErrorCode::InvalidRequest,
                message: error.to_string(),
                issues: vec![],
            })),
        }
    }

    fn handle_hello(&mut self, id: &RequestId, protocol_version: u32) -> Envelope {
        if !aifs_protocol::is_compatible(protocol_version) {
            return Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::IncompatibleProtocol,
                    message: format!(
                        "client speaks protocol {protocol_version}, engine speaks {PROTOCOL_VERSION}"
                    ),
                    issues: vec![],
                },
            );
        }
        self.hello_ok = true;
        Envelope::reply(
            id,
            Event::Ready {
                engine_version: ENGINE_VERSION.to_owned(),
                protocol_version: PROTOCOL_VERSION,
                capabilities: capabilities(),
            },
        )
    }

    fn handle_scan(
        &mut self,
        id: &RequestId,
        root: &Path,
        options: ScanOptions,
        session: Option<SessionId>,
        emit: &mut impl FnMut(Envelope),
    ) {
        if !self.hello_ok {
            emit(Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::InvalidRequest,
                    message: "send hello before scan".to_owned(),
                    issues: vec![],
                },
            ));
            return;
        }

        let session = session.unwrap_or_default();
        emit(Envelope::reply(
            id,
            Event::Progress {
                stage: "scan".to_owned(),
                current: 0,
                total: None,
                message: root.display().to_string(),
            },
        ));

        let mut snapshot = match scan(root, &options, session, |current, message| {
            if current == 1 || current % 50 == 0 {
                emit(Envelope::reply(
                    id,
                    Event::Progress {
                        stage: "scan".to_owned(),
                        current,
                        total: None,
                        message: message.to_owned(),
                    },
                ));
            }
        }) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                emit(scan_error_event(id, error));
                return;
            }
        };

        emit(Envelope::reply(
            id,
            Event::Progress {
                stage: "relationships".to_owned(),
                current: snapshot.entries.len() as u64,
                total: Some(snapshot.entries.len() as u64),
                message: "detecting bundles".to_owned(),
            },
        ));
        enrich(&mut snapshot, options.protect_projects);

        if options.extract_metadata {
            extract_into_with_progress(&mut snapshot, |current, total| {
                if current == 1 || current % 50 == 0 || current == total {
                    emit(Envelope::reply(
                        id,
                        Event::Progress {
                            stage: "extract".to_owned(),
                            current,
                            total: Some(total),
                            message: "reading media tags".to_owned(),
                        },
                    ));
                }
            });
        }

        self.sessions.insert(session, snapshot.clone());
        emit(Envelope::reply(id, Event::ScanCompleted { snapshot }));
    }
}

fn scan_error_event(id: &RequestId, error: ScanError) -> Envelope {
    let (code, message) = match error {
        ScanError::InvalidRoot { path, message } => (
            ErrorCode::InvalidRoot,
            format!("{}: {message}", path.display()),
        ),
        ScanError::Io { path, source } => (ErrorCode::Io, format!("{}: {source}", path.display())),
    };
    Envelope::reply(
        id,
        Event::Failed {
            code,
            message,
            issues: vec![],
        },
    )
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Hello { .. } => "hello",
        Command::Scan { .. } => "scan",
        Command::Propose { .. } => "propose",
        Command::Patch { .. } => "patch",
        Command::Plan { .. } => "plan",
        Command::Apply { .. } => "apply",
        Command::Undo { .. } => "undo",
        Command::Cancel { .. } => "cancel",
        Command::Shutdown => "shutdown",
    }
}

/// Reads JSONL requests from `stdin` and writes envelopes to `stdout` until shutdown.
pub fn run_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut engine = Engine::new();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let mut write_error = None;
        engine.handle_line_with(&line, &mut |envelope| {
            if write_error.is_some() {
                return;
            }
            match encode_line(&envelope)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
            {
                Ok(encoded) => {
                    if let Err(error) = writeln!(stdout, "{encoded}").and_then(|_| stdout.flush()) {
                        write_error = Some(error);
                    }
                }
                Err(error) => write_error = Some(error),
            }
        });
        if let Some(error) = write_error {
            return Err(error);
        }
        if engine.should_exit() {
            return Ok(());
        }
    }
    let encoded = encode_line(&Envelope::broadcast(Event::Shutdown))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    writeln!(stdout, "{encoded}")?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_protocol::PROTOCOL_VERSION;
    use std::fs;

    #[test]
    fn hello_then_scan_returns_snapshot() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|e| panic!("{e}"));
        let mut engine = Engine::new();
        let hello = engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        assert!(matches!(hello[0].event, Event::Ready { .. }));

        let events = engine.handle(Request {
            id: "2".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: false,
                    fingerprint_prefix_bytes: 32,
                    ..ScanOptions::default()
                },
                session: None,
            },
        });
        let completed = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                Event::ScanCompleted { snapshot } => Some(snapshot),
                _ => None,
            })
            .unwrap_or_else(|| panic!("scan_completed"));
        assert!(completed
            .entries
            .iter()
            .any(|entry| entry.path.as_str() == "note.txt"));
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::Progress { .. })),
            "scan should emit progress before completing"
        );
    }

    #[test]
    fn propose_is_not_implemented() {
        let mut engine = Engine::new();
        let events = engine.handle(Request {
            id: "p".into(),
            command: Command::Propose {
                session: SessionId::new(),
                policy: aifs_protocol::ProposalPolicy::default(),
            },
        });
        match &events[0].event {
            Event::Failed { code, message, .. } => {
                assert_eq!(*code, ErrorCode::Internal);
                assert!(message.contains("propose"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
