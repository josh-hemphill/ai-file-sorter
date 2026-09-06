//! In-process engine used by the `aifs-engine` stdio binary.
//!
//! This slice implements `hello`, `scan`, `cancel`, and `shutdown`. Other commands
//! reply with a clear "not implemented" failure so clients can negotiate capabilities.

use aifs_domain::{SessionId, WorkspaceSnapshot};
use aifs_extractors::extract_into;
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

    /// Handles one already-decoded request.
    pub fn handle(&mut self, request: Request) -> Vec<Envelope> {
        match request.command {
            Command::Hello {
                client: _,
                protocol_version,
            } => vec![self.handle_hello(&request.id, protocol_version)],
            Command::Scan {
                root,
                options,
                session,
            } => self.handle_scan(&request.id, &root, options, session),
            Command::Shutdown => {
                self.shutdown = true;
                vec![Envelope::reply(&request.id, Event::Shutdown)]
            }
            Command::Cancel { .. } => vec![Envelope::reply(&request.id, Event::Cancelled)],
            other => vec![Envelope::reply(
                &request.id,
                Event::Failed {
                    code: ErrorCode::Internal,
                    message: format!(
                        "{} is not implemented in this engine slice",
                        command_name(&other)
                    ),
                    issues: vec![],
                },
            )],
        }
    }

    /// Parses one stdin line and returns the envelopes to write.
    pub fn handle_line(&mut self, line: &str) -> Vec<Envelope> {
        match decode_line::<Request>(line) {
            Ok(request) => self.handle(request),
            Err(error) => vec![Envelope::broadcast(Event::Failed {
                code: ErrorCode::InvalidRequest,
                message: error.to_string(),
                issues: vec![],
            })],
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
    ) -> Vec<Envelope> {
        if !self.hello_ok {
            return vec![Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::InvalidRequest,
                    message: "send hello before scan".to_owned(),
                    issues: vec![],
                },
            )];
        }

        let session = session.unwrap_or_default();
        let mut events = Vec::new();
        events.push(Envelope::reply(
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
                events.push(Envelope::reply(
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
                return vec![scan_error_event(id, error)];
            }
        };

        events.push(Envelope::reply(
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
            events.push(Envelope::reply(
                id,
                Event::Progress {
                    stage: "extract".to_owned(),
                    current: 0,
                    total: Some(snapshot.entries.len() as u64),
                    message: "reading media tags".to_owned(),
                },
            ));
            extract_into(&mut snapshot);
        }

        self.sessions.insert(session, snapshot.clone());
        events.push(Envelope::reply(id, Event::ScanCompleted { snapshot }));
        events
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
        for envelope in engine.handle_line(&line) {
            let encoded = encode_line(&envelope)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
            writeln!(stdout, "{encoded}")?;
            stdout.flush()?;
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
