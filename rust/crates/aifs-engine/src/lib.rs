//! Isolated workspace engine used by the `aifs-engine` stdio binary.
//!
//! This slice implements `hello`, `scan`, `propose`, `patch`, `plan`, `apply`,
//! `undo`, `chat`, `cancel`, and `shutdown`.

use aifs_ai_tools::{execute, interpret, MOCK_ASSISTANT_MODEL};
use aifs_apply::{apply_plan_with_hooks, undo_journal_with_hooks, ApplyHook};
use aifs_domain::{JournalId, PlanId, RevisionAuthor, RevisionId, SessionId, WorkspaceSnapshot};
use aifs_extractors::extract_into_with_progress;
use aifs_planner::{propose, validate};
use aifs_protocol::{
    decode_line, encode_line, Command, Envelope, ErrorCode, Event, ProposalPolicy, Request,
    RequestId, ScanOptions, PROTOCOL_VERSION,
};
use aifs_relationships::enrich;
use aifs_scanner::{scan, ScanError};
use aifs_store::WorkspaceStore;
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Engine crate version reported on `hello`.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Capabilities advertised in this slice.
pub fn capabilities() -> Vec<String> {
    vec![
        "scan".to_owned(),
        "media_tags".to_owned(),
        "propose".to_owned(),
        "plan".to_owned(),
        "apply".to_owned(),
        "undo".to_owned(),
        "chat".to_owned(),
    ]
}

/// SQLite-backed session store plus stdio request dispatch.
pub struct Engine {
    store: WorkspaceStore,
    hello_ok: bool,
    shutdown: bool,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Creates an engine with an in-memory store.
    pub fn new() -> Self {
        Self {
            store: WorkspaceStore::open_in_memory()
                .unwrap_or_else(|error| panic!("in-memory sqlite failed: {error}")),
            hello_ok: false,
            shutdown: false,
        }
    }

    /// Creates an engine that persists to `path`.
    pub fn with_store_path(path: &Path) -> Result<Self, aifs_store::StoreError> {
        Ok(Self {
            store: WorkspaceStore::open(path)?,
            hello_ok: false,
            shutdown: false,
        })
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
            Command::Propose { session, policy } => {
                for envelope in self.handle_propose(&request.id, session, policy) {
                    emit(envelope);
                }
            }
            Command::Patch {
                session,
                base_revision,
                author,
                summary,
                patches,
            } => {
                for envelope in self.handle_patch(
                    &request.id,
                    session,
                    base_revision,
                    author,
                    summary,
                    patches,
                ) {
                    emit(envelope);
                }
            }
            Command::Plan { session, revision } => {
                for envelope in self.handle_plan(&request.id, session, revision) {
                    emit(envelope);
                }
            }
            Command::Apply {
                session,
                plan,
                dry_run,
            } => self.handle_apply(&request.id, session, plan, dry_run, emit),
            Command::Undo { session, journal } => {
                self.handle_undo(&request.id, session, journal, emit)
            }
            Command::Chat {
                session,
                revision,
                utterance,
            } => {
                for envelope in self.handle_chat(&request.id, session, revision, utterance) {
                    emit(envelope);
                }
            }
            Command::Shutdown => {
                self.shutdown = true;
                emit(Envelope::reply(&request.id, Event::Shutdown));
            }
            Command::Cancel { .. } => emit(Envelope::reply(&request.id, Event::Cancelled)),
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

        if let Err(error) = self.store.put_snapshot(&snapshot) {
            emit(store_failed(id, error));
            return;
        }
        emit(Envelope::reply(id, Event::ScanCompleted { snapshot }));
    }

    fn handle_propose(
        &mut self,
        id: &RequestId,
        session: SessionId,
        policy: ProposalPolicy,
    ) -> Vec<Envelope> {
        if let Some(failed) = self.require_hello(id) {
            return vec![failed];
        }
        match self.store.get_snapshot(session) {
            Ok(Some(snapshot)) => {
                let revision = propose(&snapshot, &policy);
                if let Err(error) = self.store.put_revision(&revision) {
                    return vec![store_failed(id, error)];
                }
                vec![Envelope::reply(id, Event::Revision { revision })]
            }
            Ok(None) => vec![not_found(id, "session snapshot")],
            Err(error) => vec![store_failed(id, error)],
        }
    }

    fn handle_patch(
        &mut self,
        id: &RequestId,
        session: SessionId,
        base_revision: RevisionId,
        author: RevisionAuthor,
        summary: String,
        patches: Vec<aifs_domain::RevisionPatch>,
    ) -> Vec<Envelope> {
        if let Some(failed) = self.require_hello(id) {
            return vec![failed];
        }
        let Some(_) = self.snapshot_or_fail(id, session) else {
            return vec![not_found(id, "session snapshot")];
        };
        match self.store.get_revision(base_revision) {
            Ok(Some(base)) => match base.with_patches(author, summary, &patches) {
                Ok(revision) => {
                    if let Err(error) = self.store.put_revision(&revision) {
                        return vec![store_failed(id, error)];
                    }
                    vec![Envelope::reply(id, Event::Revision { revision })]
                }
                Err(error) => vec![Envelope::reply(
                    id,
                    Event::Failed {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                        issues: vec![],
                    },
                )],
            },
            Ok(None) => vec![not_found(id, "revision")],
            Err(error) => vec![store_failed(id, error)],
        }
    }

    fn handle_plan(
        &mut self,
        id: &RequestId,
        session: SessionId,
        revision: RevisionId,
    ) -> Vec<Envelope> {
        if let Some(failed) = self.require_hello(id) {
            return vec![failed];
        }
        let snapshot = match self.store.get_snapshot(session) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => return vec![not_found(id, "session snapshot")],
            Err(error) => return vec![store_failed(id, error)],
        };
        let revision = match self.store.get_revision(revision) {
            Ok(Some(revision)) => revision,
            Ok(None) => return vec![not_found(id, "revision")],
            Err(error) => return vec![store_failed(id, error)],
        };
        let (plan, issues) = validate(&snapshot, &revision);
        match plan {
            Some(plan) => {
                if let Err(error) = self.store.put_plan(session, &plan) {
                    return vec![store_failed(id, error)];
                }
                vec![Envelope::reply(id, Event::Planned { plan, issues })]
            }
            None => vec![Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::PlanRejected,
                    message: "plan validation failed".to_owned(),
                    issues,
                },
            )],
        }
    }

    fn handle_apply(
        &mut self,
        id: &RequestId,
        session: SessionId,
        plan: PlanId,
        dry_run: bool,
        emit: &mut impl FnMut(Envelope),
    ) {
        if let Some(failed) = self.require_hello(id) {
            emit(failed);
            return;
        }
        let snapshot = match self.store.get_snapshot(session) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                emit(not_found(id, "session snapshot"));
                return;
            }
            Err(error) => {
                emit(store_failed(id, error));
                return;
            }
        };
        let plan = match self.store.get_plan(plan) {
            Ok(Some(plan)) => plan,
            Ok(None) => {
                emit(not_found(id, "plan"));
                return;
            }
            Err(error) => {
                emit(store_failed(id, error));
                return;
            }
        };
        let stage = if dry_run { "apply-dry-run" } else { "apply" };
        let mut persist_error: Option<aifs_store::StoreError> = None;
        let journal = apply_plan_with_hooks(&snapshot, &plan, dry_run, |hook| match hook {
            ApplyHook::Progress(journal) => match self.store.put_journal(session, journal) {
                Ok(()) => {
                    emit_apply_progress(emit, id, stage, journal);
                    true
                }
                Err(error) => {
                    persist_error = Some(error);
                    false
                }
            },
            ApplyHook::Heartbeat => {
                emit(Envelope::reply(
                    id,
                    Event::Progress {
                        stage: stage.to_owned(),
                        current: 0,
                        total: None,
                        message: "working".to_owned(),
                    },
                ));
                true
            }
        });
        emit_journal_outcome(emit, id, session, &self.store, journal, persist_error);
    }

    fn handle_undo(
        &mut self,
        id: &RequestId,
        session: SessionId,
        journal: JournalId,
        emit: &mut impl FnMut(Envelope),
    ) {
        if let Some(failed) = self.require_hello(id) {
            emit(failed);
            return;
        }
        let snapshot = match self.store.get_snapshot(session) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                emit(not_found(id, "session snapshot"));
                return;
            }
            Err(error) => {
                emit(store_failed(id, error));
                return;
            }
        };
        let journal = match self.store.get_journal(journal) {
            Ok(Some(journal)) => journal,
            Ok(None) => {
                emit(not_found(id, "journal"));
                return;
            }
            Err(error) => {
                emit(store_failed(id, error));
                return;
            }
        };
        let mut persist_error: Option<aifs_store::StoreError> = None;
        let journal = undo_journal_with_hooks(&snapshot, &journal, |hook| match hook {
            ApplyHook::Progress(journal) => match self.store.put_journal(session, journal) {
                Ok(()) => {
                    emit(Envelope::reply(
                        id,
                        Event::Progress {
                            stage: "undo".to_owned(),
                            current: journal
                                .entries
                                .iter()
                                .filter(|entry| {
                                    matches!(
                                        entry.state,
                                        aifs_domain::JournalState::RolledBack
                                            | aifs_domain::JournalState::Failed { .. }
                                    )
                                })
                                .count() as u64,
                            total: Some(journal.entries.len() as u64),
                            message: format!("journal {}", journal.id),
                        },
                    ));
                    true
                }
                Err(error) => {
                    persist_error = Some(error);
                    false
                }
            },
            ApplyHook::Heartbeat => {
                emit(Envelope::reply(
                    id,
                    Event::Progress {
                        stage: "undo".to_owned(),
                        current: 0,
                        total: None,
                        message: "working".to_owned(),
                    },
                ));
                true
            }
        });
        emit_journal_outcome(emit, id, session, &self.store, journal, persist_error);
    }

    fn handle_chat(
        &mut self,
        id: &RequestId,
        session: SessionId,
        revision: RevisionId,
        utterance: String,
    ) -> Vec<Envelope> {
        if let Some(failed) = self.require_hello(id) {
            return vec![failed];
        }
        let trimmed = utterance.trim();
        if trimmed.is_empty() {
            return vec![Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::InvalidRequest,
                    message: "chat utterance is empty".to_owned(),
                    issues: vec![],
                },
            )];
        }
        let snapshot = match self.store.get_snapshot(session) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => return vec![not_found(id, "session snapshot")],
            Err(error) => return vec![store_failed(id, error)],
        };
        let base = match self.store.get_revision(revision) {
            Ok(Some(revision)) => revision,
            Ok(None) => return vec![not_found(id, "revision")],
            Err(error) => return vec![store_failed(id, error)],
        };
        if base.session != session {
            return vec![Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::InvalidRequest,
                    message: "revision does not belong to this session".to_owned(),
                    issues: vec![],
                },
            )];
        }
        let output = execute(&snapshot, &base, &interpret(trimmed));
        if output.patches.is_empty() {
            return vec![Envelope::reply(
                id,
                Event::ChatReply {
                    message: output.message,
                    revision: None,
                },
            )];
        }
        match base.with_patches(
            RevisionAuthor::Assistant {
                model: MOCK_ASSISTANT_MODEL.to_owned(),
            },
            trimmed,
            &output.patches,
        ) {
            Ok(revision) => {
                if let Err(error) = self.store.put_revision(&revision) {
                    return vec![store_failed(id, error)];
                }
                vec![Envelope::reply(
                    id,
                    Event::ChatReply {
                        message: output.message,
                        revision: Some(revision),
                    },
                )]
            }
            Err(error) => vec![Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::InvalidRequest,
                    message: error.to_string(),
                    issues: vec![],
                },
            )],
        }
    }

    fn require_hello(&self, id: &RequestId) -> Option<Envelope> {
        if self.hello_ok {
            None
        } else {
            Some(Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::InvalidRequest,
                    message: "send hello first".to_owned(),
                    issues: vec![],
                },
            ))
        }
    }

    fn snapshot_or_fail(&self, _id: &RequestId, session: SessionId) -> Option<WorkspaceSnapshot> {
        self.store.get_snapshot(session).ok().flatten()
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

fn not_found(id: &RequestId, what: &str) -> Envelope {
    Envelope::reply(
        id,
        Event::Failed {
            code: ErrorCode::NotFound,
            message: format!("{what} not found"),
            issues: vec![],
        },
    )
}

fn store_failed(id: &RequestId, error: aifs_store::StoreError) -> Envelope {
    Envelope::reply(
        id,
        Event::Failed {
            code: ErrorCode::Storage,
            message: error.to_string(),
            issues: vec![],
        },
    )
}

fn emit_apply_progress(
    emit: &mut impl FnMut(Envelope),
    id: &RequestId,
    stage: &str,
    journal: &aifs_domain::ApplyJournal,
) {
    emit(Envelope::reply(
        id,
        Event::Progress {
            stage: stage.to_owned(),
            current: (journal.done_count() + journal.skipped_count() + journal.failed_count())
                as u64,
            total: Some(journal.entries.len() as u64),
            message: format!("journal {}", journal.id),
        },
    ));
}

fn journal_has_mutations(journal: &aifs_domain::ApplyJournal) -> bool {
    journal
        .entries
        .iter()
        .any(|entry| !matches!(entry.state, aifs_domain::JournalState::Intended))
}

fn emit_journal_outcome(
    emit: &mut impl FnMut(Envelope),
    id: &RequestId,
    session: SessionId,
    store: &aifs_store::WorkspaceStore,
    journal: aifs_domain::ApplyJournal,
    persist_error: Option<aifs_store::StoreError>,
) {
    let mutated = journal_has_mutations(&journal);
    if let Some(error) = persist_error {
        if !mutated {
            emit(store_failed(id, error));
            return;
        }
    }
    if let Err(error) = store.put_journal(session, &journal) {
        if !mutated {
            emit(store_failed(id, error));
            return;
        }
    }
    emit(Envelope::reply(id, Event::Journal { journal }));
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

    fn terminal(events: Vec<Envelope>) -> Event {
        events
            .into_iter()
            .rev()
            .find(|envelope| envelope.is_terminal())
            .map(|envelope| envelope.event)
            .unwrap_or_else(|| panic!("no terminal event"))
    }

    #[test]
    fn propose_plan_and_dry_run_apply() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|e| panic!("{e}"));
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let scan_events = engine.handle(Request {
            id: "2".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: false,
                    ..ScanOptions::default()
                },
                session: None,
            },
        });
        let session = match terminal(scan_events) {
            Event::ScanCompleted { snapshot } => snapshot.session,
            other => panic!("unexpected {other:?}"),
        };
        let revision = match terminal(engine.handle(Request {
            id: "3".into(),
            command: Command::Propose {
                session,
                policy: ProposalPolicy::default(),
            },
        })) {
            Event::Revision { revision } => revision,
            other => panic!("unexpected {other:?}"),
        };
        let assets: Vec<_> = revision.placements.keys().copied().collect();
        let accepted = match terminal(engine.handle(Request {
            id: "4".into(),
            command: Command::Patch {
                session,
                base_revision: revision.id,
                author: RevisionAuthor::User,
                summary: "accept".into(),
                patches: vec![aifs_domain::RevisionPatch::Accept { assets }],
            },
        })) {
            Event::Revision { revision } => revision,
            other => panic!("unexpected {other:?}"),
        };
        let plan = match terminal(engine.handle(Request {
            id: "5".into(),
            command: Command::Plan {
                session,
                revision: accepted.id,
            },
        })) {
            Event::Planned { plan, .. } => plan,
            other => panic!("unexpected {other:?}"),
        };
        let journal = match terminal(engine.handle(Request {
            id: "6".into(),
            command: Command::Apply {
                session,
                plan: plan.id,
                dry_run: true,
            },
        })) {
            Event::Journal { journal } => journal,
            other => panic!("unexpected {other:?}"),
        };
        assert!(journal.dry_run);
        assert!(dir.path().join("note.txt").exists());
    }

    #[test]
    fn chat_moves_audio_into_podcasts_and_search_is_read_only() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("show.mp3"), b"id3").unwrap_or_else(|e| panic!("{e}"));
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let scan_events = engine.handle(Request {
            id: "2".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: false,
                    ..ScanOptions::default()
                },
                session: None,
            },
        });
        let snapshot = match terminal(scan_events) {
            Event::ScanCompleted { snapshot } => snapshot,
            other => panic!("unexpected {other:?}"),
        };
        let revision = match terminal(engine.handle(Request {
            id: "3".into(),
            command: Command::Propose {
                session: snapshot.session,
                policy: ProposalPolicy::default(),
            },
        })) {
            Event::Revision { revision } => revision,
            other => panic!("unexpected {other:?}"),
        };
        let search = match terminal(engine.handle(Request {
            id: "4".into(),
            command: Command::Chat {
                session: snapshot.session,
                revision: revision.id,
                utterance: "find show.mp3".into(),
            },
        })) {
            Event::ChatReply { message, revision } => {
                assert!(message.contains("show.mp3"), "{message}");
                assert!(revision.is_none());
                message
            }
            other => panic!("unexpected {other:?}"),
        };
        assert!(search.contains("Found"));
        let (message, next) = match terminal(engine.handle(Request {
            id: "5".into(),
            command: Command::Chat {
                session: snapshot.session,
                revision: revision.id,
                utterance: "Move podcasts away from music, but keep seasons shallow.".into(),
            },
        })) {
            Event::ChatReply { message, revision } => (message, revision),
            other => panic!("unexpected {other:?}"),
        };
        assert!(message.contains("Podcasts"), "{message}");
        let next = next.unwrap_or_else(|| panic!("expected child revision"));
        assert_eq!(next.parent, Some(revision.id));
        let dest = next
            .placements
            .values()
            .next()
            .map(|placement| placement.destination.as_str().to_owned())
            .unwrap_or_default();
        assert!(
            dest.starts_with("Podcasts/"),
            "expected Podcasts destination, got {dest}"
        );
    }
}
