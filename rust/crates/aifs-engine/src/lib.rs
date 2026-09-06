//! Isolated workspace engine used by the `aifs-engine` stdio binary.
//!
//! This slice implements `hello`, `scan`, `propose`, `patch`, `plan`, `apply`,
//! `undo`, `chat`, `cancel`, `get_settings`, `put_settings`, `get_models`,
//! `put_models`, `probe_endpoint`, and `shutdown`.

mod extract;

use aifs_ai_tools::{execute, interpret, MOCK_ASSISTANT_MODEL};
use aifs_apply::{apply_plan_with_hooks, undo_journal_with_hooks, ApplyHook};
use aifs_domain::{
    BundleConstraint, JournalId, PlanId, RevisionAuthor, RevisionId, SessionId, SkipReason,
    WorkspaceSnapshot,
};
use aifs_planner::{propose, validate};
use aifs_protocol::{
    decode_line, encode_line, AppSettings, Command, Envelope, ErrorCode, Event, LogLevel,
    ModelBackend, ModelInventory, ProposalPolicy, Request, RequestId, ScanOptions, PROTOCOL_VERSION,
};
use aifs_relationships::enrich;
use aifs_scanner::{scan, ScanError};
use aifs_store::WorkspaceStore;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

const SCAN_LOG_CAP: usize = 400;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const SETTINGS_META_KEY: &str = "app_settings";
const MODELS_META_KEY: &str = "model_inventory";

/// Engine crate version reported on `hello`.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Capabilities advertised in this slice.
pub fn capabilities() -> Vec<String> {
    let mut caps = vec![
        "scan".to_owned(),
        "media_tags".to_owned(),
        "propose".to_owned(),
        "plan".to_owned(),
        "apply".to_owned(),
        "undo".to_owned(),
        "chat".to_owned(),
        "settings".to_owned(),
        "models".to_owned(),
    ];
    caps.extend(extract::worker_capabilities());
    caps
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
            Command::GetSettings => emit(self.handle_get_settings(&request.id)),
            Command::PutSettings { settings } => emit(self.handle_put_settings(&request.id, settings)),
            Command::GetModels => emit(self.handle_get_models(&request.id)),
            Command::PutModels { inventory } => emit(self.handle_put_models(&request.id, inventory)),
            Command::ProbeEndpoint { backend, api_key } => {
                emit(self.handle_probe_endpoint(&request.id, backend, api_key))
            }
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

    fn handle_get_settings(&self, id: &RequestId) -> Envelope {
        if let Some(failed) = self.require_hello(id) {
            return failed;
        }
        match load_settings(&self.store) {
            Ok(settings) => Envelope::reply(id, Event::Settings { settings }),
            Err(error) => store_failed(id, error),
        }
    }

    fn handle_put_settings(&self, id: &RequestId, settings: AppSettings) -> Envelope {
        if let Some(failed) = self.require_hello(id) {
            return failed;
        }
        if let Err(message) = settings.policy.whitelist.validate() {
            return Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::InvalidRequest,
                    message,
                    issues: vec![],
                },
            );
        }
        match save_settings(&self.store, &settings) {
            Ok(()) => Envelope::reply(id, Event::Settings { settings }),
            Err(error) => store_failed(id, error),
        }
    }

    fn handle_get_models(&self, id: &RequestId) -> Envelope {
        if let Some(failed) = self.require_hello(id) {
            return failed;
        }
        match load_models(&self.store) {
            Ok(inventory) => Envelope::reply(
                id,
                Event::Models {
                    inventory: inventory.redacted(),
                },
            ),
            Err(error) => store_failed(id, error),
        }
    }

    fn handle_put_models(&self, id: &RequestId, inventory: ModelInventory) -> Envelope {
        if let Some(failed) = self.require_hello(id) {
            return failed;
        }
        let previous = match load_models(&self.store) {
            Ok(inventory) => inventory,
            Err(error) => return store_failed(id, error),
        };
        let merged = inventory.merge_secrets(&previous);
        match save_models(&self.store, &merged) {
            Ok(()) => Envelope::reply(
                id,
                Event::Models {
                    inventory: merged.redacted(),
                },
            ),
            Err(error) => store_failed(id, error),
        }
    }

    fn handle_probe_endpoint(
        &self,
        id: &RequestId,
        backend: ModelBackend,
        api_key: Option<String>,
    ) -> Envelope {
        if let Some(failed) = self.require_hello(id) {
            return failed;
        }
        let _ = api_key;
        let (ok, message) = aifs_protocol::probe_backend(&backend);
        Envelope::reply(id, Event::EndpointProbed { ok, message })
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
        if let Err(error) = emit_model_runtime_notices(&self.store, id, emit) {
            emit(store_failed(id, error));
            return;
        }
        emit(Envelope::reply(
            id,
            Event::Progress {
                stage: "scan".to_owned(),
                current: 0,
                total: None,
                message: root.display().to_string(),
            },
        ));

        let mut last_progress = Instant::now()
            .checked_sub(PROGRESS_INTERVAL)
            .unwrap_or_else(Instant::now);
        let mut snapshot = match scan(root, &options, session, |current, message| {
            if should_emit_progress(&mut last_progress, current, None) {
                emit_progress(emit, id, "scan", current, None, message);
            }
        }) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                emit(scan_error_event(id, error));
                return;
            }
        };

        emit_progress(
            emit,
            id,
            "scan",
            (snapshot.entries.len() + snapshot.skipped.len()) as u64,
            Some((snapshot.entries.len() + snapshot.skipped.len()) as u64),
            "walk complete",
        );
        emit_scan_logs(emit, id, &snapshot);

        emit_progress(
            emit,
            id,
            "relationships",
            0,
            Some(snapshot.entries.len() as u64),
            "detecting bundles",
        );
        enrich(&mut snapshot, options.protect_projects);
        emit_relationship_logs(emit, id, &snapshot);
        emit_progress(
            emit,
            id,
            "relationships",
            snapshot.entries.len() as u64,
            Some(snapshot.entries.len() as u64),
            format!("{} bundles", snapshot.bundles.len()),
        );

        if options.extract_metadata {
            last_progress = Instant::now()
                .checked_sub(PROGRESS_INTERVAL)
                .unwrap_or_else(Instant::now);
            extract::extract_into_supervised(&mut snapshot, |current, total, path| {
                if should_emit_progress(&mut last_progress, current, Some(total)) {
                    emit_progress(emit, id, "extract", current, Some(total), path);
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

fn load_settings(store: &aifs_store::WorkspaceStore) -> Result<AppSettings, aifs_store::StoreError> {
    match store.get_meta(SETTINGS_META_KEY)? {
        Some(json) => Ok(serde_json::from_str(&json)?),
        None => Ok(AppSettings::default()),
    }
}

fn save_settings(
    store: &aifs_store::WorkspaceStore,
    settings: &AppSettings,
) -> Result<(), aifs_store::StoreError> {
    let json = serde_json::to_string(settings)?;
    store.put_meta(SETTINGS_META_KEY, &json)
}

fn load_models(
    store: &aifs_store::WorkspaceStore,
) -> Result<ModelInventory, aifs_store::StoreError> {
    match store.get_meta(MODELS_META_KEY)? {
        Some(json) => Ok(serde_json::from_str(&json)?),
        None => Ok(ModelInventory::default()),
    }
}

fn save_models(
    store: &aifs_store::WorkspaceStore,
    inventory: &ModelInventory,
) -> Result<(), aifs_store::StoreError> {
    let json = serde_json::to_string(inventory)?;
    store.put_meta(MODELS_META_KEY, &json)
}

fn emit_model_runtime_notices(
    store: &aifs_store::WorkspaceStore,
    id: &RequestId,
    emit: &mut impl FnMut(Envelope),
) -> Result<(), aifs_store::StoreError> {
    let settings = load_settings(store)?;
    let models = load_models(store)?;
    let slot_off = |id: &str| {
        models
            .slots
            .iter()
            .find(|slot| slot.id == id)
            .is_none_or(|slot| matches!(slot.backend, ModelBackend::Off))
    };
    if settings.analyze_images {
        if slot_off("vision") {
            emit_log(
                emit,
                id,
                LogLevel::Warn,
                "Image analysis is enabled but the vision slot is off — set up a model.",
            );
        } else {
            emit_log(
                emit,
                id,
                LogLevel::Info,
                "Vision slot assigned; model runtime is not connected yet.",
            );
        }
    }
    if settings.analyze_documents {
        if slot_off("document") {
            emit_log(
                emit,
                id,
                LogLevel::Warn,
                "Document analysis is enabled but the document slot is off — set up a model.",
            );
        } else {
            emit_log(
                emit,
                id,
                LogLevel::Info,
                "Document slot assigned; model runtime is not connected yet.",
            );
        }
    }
    Ok(())
}

fn default_store_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                let mut path = std::path::PathBuf::from(home);
                path.push(".local");
                path.push("share");
                path
            })
        })?;
    Some(base.join("aifs").join("engine.sqlite"))
}

fn emit_progress(
    emit: &mut impl FnMut(Envelope),
    id: &RequestId,
    stage: &str,
    current: u64,
    total: Option<u64>,
    message: impl Into<String>,
) {
    emit(Envelope::reply(
        id,
        Event::Progress {
            stage: stage.to_owned(),
            current,
            total,
            message: message.into(),
        },
    ));
}

fn emit_log(
    emit: &mut impl FnMut(Envelope),
    id: &RequestId,
    level: LogLevel,
    message: impl Into<String>,
) {
    emit(Envelope::reply(
        id,
        Event::Log {
            level,
            message: message.into(),
        },
    ));
}

fn should_emit_progress(last: &mut Instant, current: u64, total: Option<u64>) -> bool {
    if current <= 1 || total == Some(current) {
        *last = Instant::now();
        return true;
    }
    if last.elapsed() >= PROGRESS_INTERVAL {
        *last = Instant::now();
        return true;
    }
    false
}

fn skip_log_line(path: &str, reason: &SkipReason) -> String {
    match reason {
        SkipReason::ProtectedProject { rule_id } => {
            format!("{path} · skipped · protected project ({rule_id})")
        }
        SkipReason::Symlink => format!("{path} · skipped · symlink"),
        SkipReason::Hidden => format!("{path} · skipped · hidden"),
        SkipReason::Junk => format!("{path} · skipped · junk"),
        SkipReason::DepthLimit => format!("{path} · skipped · depth limit"),
        SkipReason::Error { message } => format!("{path} · skipped · {message}"),
    }
}

fn emit_scan_logs(
    emit: &mut impl FnMut(Envelope),
    id: &RequestId,
    snapshot: &WorkspaceSnapshot,
) {
    let mut remaining = SCAN_LOG_CAP;
    for project in &snapshot.projects {
        if remaining == 0 {
            break;
        }
        emit_log(
            emit,
            id,
            LogLevel::Info,
            format!(
                "{} · {} · {}",
                project.root.as_str(),
                project.name,
                project.reason
            ),
        );
        remaining -= 1;
    }
    let mut skip_logged = 0usize;
    for skipped in &snapshot.skipped {
        if remaining == 0 {
            let omitted = snapshot.skipped.len().saturating_sub(skip_logged);
            if omitted > 0 {
                emit_log(
                    emit,
                    id,
                    LogLevel::Warn,
                    format!("{omitted} more skipped entries omitted from the stream"),
                );
            }
            break;
        }
        let level = match skipped.reason {
            SkipReason::Error { .. } => LogLevel::Warn,
            _ => LogLevel::Info,
        };
        emit_log(
            emit,
            id,
            level,
            skip_log_line(skipped.path.as_str(), &skipped.reason),
        );
        remaining -= 1;
        skip_logged += 1;
    }
}

fn emit_relationship_logs(
    emit: &mut impl FnMut(Envelope),
    id: &RequestId,
    snapshot: &WorkspaceSnapshot,
) {
    for bundle in snapshot.bundles.iter().take(SCAN_LOG_CAP) {
        let constraint = match &bundle.constraint {
            BundleConstraint::Protected { reason } => format!("protected · {reason}"),
            BundleConstraint::MoveTogether => "keep together".to_owned(),
            BundleConstraint::PreserveLayout { root } => {
                format!("move as a unit · {}", root.as_str())
            }
            BundleConstraint::Soft => "suggestion".to_owned(),
        };
        emit_log(
            emit,
            id,
            LogLevel::Info,
            format!(
                "{} · {constraint} · {} members",
                bundle.label,
                bundle.members.len()
            ),
        );
    }
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
    let mut engine = if let Some(explicit) = std::env::var_os("AIFS_STORE") {
        let path = std::path::PathBuf::from(&explicit);
        Engine::with_store_path(&path).map_err(|error| {
            io::Error::other(format!("opening AIFS_STORE {}: {error}", path.display()))
        })?
    } else {
        match default_store_path() {
            Some(path) => Engine::with_store_path(&path).unwrap_or_else(|error| {
                eprintln!("aifs-engine: opening {path:?} failed ({error}); using memory store");
                Engine::new()
            }),
            None => Engine::new(),
        }
    };
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
    fn scan_stream_includes_projects_skips_and_bundles() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/inbox-mixed");
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let events = engine.handle(Request {
            id: "2".into(),
            command: Command::Scan {
                root,
                options: ScanOptions {
                    extract_metadata: false,
                    fingerprint_prefix_bytes: 32,
                    ..ScanOptions::default()
                },
                session: None,
            },
        });
        let logs: Vec<_> = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                Event::Log { message, .. } => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            logs.iter().any(|line| line.contains("Rust project")),
            "expected project log, got {logs:?}"
        );
        assert!(
            logs.iter()
                .any(|line| line.contains("protected project") || line.contains("keep together")),
            "expected skip or bundle log, got {logs:?}"
        );
        let stages: Vec<_> = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                Event::Progress { stage, .. } => Some(stage.as_str()),
                _ => None,
            })
            .collect();
        assert!(stages.contains(&"scan"), "stages={stages:?}");
        assert!(stages.contains(&"relationships"), "stages={stages:?}");
    }

    #[test]
    fn junk_drawer_keeps_library_and_archive_paths() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/junk-drawer");
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
                root,
                options: ScanOptions {
                    extract_metadata: false,
                    fingerprint_prefix_bytes: 32,
                    ..ScanOptions::default()
                },
                session: None,
            },
        });
        let snapshot = match terminal(scan_events) {
            Event::ScanCompleted { snapshot } => snapshot,
            other => panic!("unexpected {other:?}"),
        };
        assert!(
            !snapshot.directory_roles.is_empty(),
            "expected directory roles, got none; entries={:?}",
            snapshot
                .entries
                .iter()
                .map(|entry| entry.path.as_str().to_owned())
                .collect::<Vec<_>>()
        );
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
        let map = aifs_planner::destination_map(&snapshot, &revision);
        assert_eq!(
            map.get("Music/Ada/night.mp3").map(String::as_str),
            Some("Music/Ada/night.mp3"),
            "library file flattened: {map:?}"
        );
        assert_eq!(
            map.get("old/2019/client-a/invoice.txt").map(String::as_str),
            Some("old/2019/client-a/invoice.txt"),
            "archive file flattened: {map:?}"
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

    #[test]
    fn scan_extracts_id3_tags_in_process_when_workers_absent() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        aifs_extractors::write_id3v23_fixture(
            &dir.path().join("show.mp3"),
            "Night Drive",
            "Ada",
            "After Hours",
            "2019",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let events = engine.handle(Request {
            id: "2".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: true,
                    fingerprint_prefix_bytes: 32,
                    ..ScanOptions::default()
                },
                session: None,
            },
        });
        let snapshot = match terminal(events) {
            Event::ScanCompleted { snapshot } => snapshot,
            other => panic!("unexpected {other:?}"),
        };
        assert!(
            snapshot.evidence.iter().any(|bag| {
                bag.fact(aifs_domain::evidence::keys::MEDIA_TITLE) == Some("Night Drive")
            }),
            "expected ID3 title in evidence, got {:?}",
            snapshot.evidence
        );
    }

    #[test]
    fn settings_round_trip_and_reject_exclusive_whitelist() {
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let loaded = match terminal(engine.handle(Request {
            id: "2".into(),
            command: Command::GetSettings,
        })) {
            Event::Settings { settings } => settings,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(loaded, AppSettings::default());
        let mut settings = AppSettings::default();
        settings.scan.include_hidden = true;
        settings.analyze_images = true;
        settings.policy.style = aifs_protocol::FolderStyle::Refined;
        settings.policy.whitelist.main = vec!["Documents".into()];
        match terminal(engine.handle(Request {
            id: "3".into(),
            command: Command::PutSettings {
                settings: settings.clone(),
            },
        })) {
            Event::Settings { settings: stored } => assert_eq!(stored, settings),
            other => panic!("unexpected {other:?}"),
        }
        match terminal(engine.handle(Request {
            id: "4".into(),
            command: Command::GetSettings,
        })) {
            Event::Settings { settings: stored } => assert_eq!(stored.scan.include_hidden, true),
            other => panic!("unexpected {other:?}"),
        }
        settings.policy.whitelist.global_subcategories = vec!["Reports".into()];
        settings
            .policy
            .whitelist
            .branching
            .insert("Documents".into(), vec!["Notes".into()]);
        match terminal(engine.handle(Request {
            id: "5".into(),
            command: Command::PutSettings { settings },
        })) {
            Event::Failed { code, .. } => assert_eq!(code, ErrorCode::InvalidRequest),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn models_redact_keys_and_probe_rejects_bad_urls() {
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let mut inventory = ModelInventory::default();
        inventory.slots[0].backend = ModelBackend::OpenAi {
            model: "gpt-4.1-mini".into(),
        };
        inventory.slots[0].api_key = Some("sk-secret".into());
        match terminal(engine.handle(Request {
            id: "2".into(),
            command: Command::PutModels {
                inventory: inventory.clone(),
            },
        })) {
            Event::Models { inventory: stored } => {
                assert!(stored.slots[0].api_key.is_none());
                assert!(stored.slots[0].api_key_set);
            }
            other => panic!("unexpected {other:?}"),
        }
        match terminal(engine.handle(Request {
            id: "3".into(),
            command: Command::GetModels,
        })) {
            Event::Models { inventory: stored } => {
                assert!(stored.slots[0].api_key.is_none());
                assert!(stored.slots[0].api_key_set);
            }
            other => panic!("unexpected {other:?}"),
        }
        match terminal(engine.handle(Request {
            id: "4".into(),
            command: Command::ProbeEndpoint {
                backend: ModelBackend::CustomEndpoint {
                    base_url: "not-a-url".into(),
                    model: "x".into(),
                },
                api_key: Some("sk-never-log".into()),
            },
        })) {
            Event::EndpointProbed { ok, message } => {
                assert!(!ok, "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn corrupt_settings_json_is_a_storage_error() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let db = dir.path().join("engine.sqlite");
        let store = aifs_store::WorkspaceStore::open(&db).unwrap_or_else(|e| panic!("{e}"));
        store
            .put_meta("app_settings", "{not-json")
            .unwrap_or_else(|e| panic!("{e}"));
        drop(store);
        let mut engine = Engine::with_store_path(&db).unwrap_or_else(|e| panic!("{e}"));
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        match terminal(engine.handle(Request {
            id: "2".into(),
            command: Command::GetSettings,
        })) {
            Event::Failed { code, .. } => assert_eq!(code, ErrorCode::Storage),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn corrupt_settings_json_fails_scan() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|e| panic!("{e}"));
        let db = dir.path().join("engine.sqlite");
        let store = aifs_store::WorkspaceStore::open(&db).unwrap_or_else(|e| panic!("{e}"));
        store
            .put_meta("app_settings", "{not-json")
            .unwrap_or_else(|e| panic!("{e}"));
        drop(store);
        let mut engine = Engine::with_store_path(&db).unwrap_or_else(|e| panic!("{e}"));
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        match terminal(engine.handle(Request {
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
        })) {
            Event::Failed { code, .. } => assert_eq!(code, ErrorCode::Storage),
            other => panic!("unexpected {other:?}"),
        }
    }
}
