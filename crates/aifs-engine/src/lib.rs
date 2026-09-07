//! Isolated workspace engine used by the `aifs-engine` stdio binary.
//!
//! This slice implements `hello`, `scan`, `propose`, `patch`, `plan`, `apply`,
//! `undo`, `chat`, `cancel`, `get_settings`, `put_settings`, `get_models`,
//! `put_models`, `download_model`, `probe_endpoint`, and `shutdown`.

mod analyze;
mod cancel;
mod chat;
mod checkpoint;
mod download;
mod extract;
mod hosted;
#[cfg(test)]
mod http_stub;

use crate::analyze::AnalyzeNotice;
use crate::cancel::{CancelGate, CancelScope, WorkStatus};
use aifs_ai_tools::{MOCK_ASSISTANT_MODEL, ToolOutput, execute, interpret};
use aifs_apply::{ApplyHook, apply_plan_with_hooks, undo_journal_with_hooks};
use aifs_domain::{
    BundleConstraint, JournalId, PlanId, RevisionAuthor, RevisionId, SessionId, SkipReason,
    WorkspaceSnapshot,
};
use aifs_planner::{propose, validate};
use aifs_protocol::worker::WorkerKind;
use aifs_protocol::{
    AppSettings, Command, Envelope, ErrorCode, Event, LogLevel, ModelBackend, ModelInventory,
    PROTOCOL_VERSION, ProposalPolicy, Request, RequestId, ScanOptions, decode_line, encode_line,
    hosted_model_label, is_hosted_backend,
};
use aifs_relationships::enrich;
use aifs_scanner::{ScanError, scan};
use aifs_store::WorkspaceStore;
use aifs_worker_client::WorkerClient;
use std::cell::RefCell;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
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
        "download_model".to_owned(),
        "cancel".to_owned(),
    ];
    caps.extend(extract::worker_capabilities());
    caps
}

/// SQLite-backed session store plus stdio request dispatch.
pub struct Engine {
    store: WorkspaceStore,
    hello_ok: bool,
    shutdown: bool,
    cancel: Arc<CancelGate>,
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
            cancel: Arc::new(CancelGate::default()),
        }
    }

    /// Creates an engine that persists to `path`.
    pub fn with_store_path(path: &Path) -> Result<Self, aifs_store::StoreError> {
        Ok(Self {
            store: WorkspaceStore::open(path)?,
            hello_ok: false,
            shutdown: false,
            cancel: Arc::new(CancelGate::default()),
        })
    }

    /// Shared cancel gate (stdin reader + tests).
    pub(crate) fn cancel_gate(&self) -> Arc<CancelGate> {
        Arc::clone(&self.cancel)
    }

    /// Records a cancel for `target` so the next cooperative check stops that request.
    pub(crate) fn request_cancel(&self, target: RequestId) {
        self.cancel.request_cancel(target);
    }

    #[cfg(test)]
    fn stored_snapshot(&self, session: SessionId) -> Option<WorkspaceSnapshot> {
        self.store.get_snapshot(session).ok().flatten()
    }

    fn should_stop(&self, id: &RequestId) -> bool {
        self.cancel.is_cancelled(id)
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
        if let Command::Cancel { target } = &request.command {
            self.request_cancel(target.clone());
            emit(Envelope::reply(&request.id, Event::Cancelled));
            return;
        }
        let _scope = CancelScope::new(Arc::clone(&self.cancel), request.id.clone());
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
            Command::Cancel { .. } => {}
            Command::GetSettings => emit(self.handle_get_settings(&request.id)),
            Command::PutSettings { settings } => {
                emit(self.handle_put_settings(&request.id, settings))
            }
            Command::GetModels => emit(self.handle_get_models(&request.id)),
            Command::PutModels { inventory } => {
                emit(self.handle_put_models(&request.id, inventory))
            }
            Command::ProbeEndpoint { backend, api_key } => {
                emit(self.handle_probe_endpoint(&request.id, backend, api_key))
            }
            Command::DownloadModel { catalog_id } => {
                self.handle_download_model(&request.id, &catalog_id, emit)
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
                    inventory: present_models(inventory),
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
                    inventory: present_models(merged),
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
        let inventory = load_models(&self.store).ok();
        let storage = inventory
            .as_ref()
            .map(|loaded| resolved_models_dir(&loaded.storage_dir));
        let (ok, message) = aifs_protocol::probe_backend_at(&backend, storage.as_deref());
        if !ok || !is_hosted_backend(&backend) {
            return Envelope::reply(id, Event::EndpointProbed { ok, message });
        }
        let key = api_key
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                inventory
                    .as_ref()
                    .and_then(|loaded| stored_api_key_for(loaded, &backend))
            });
        let (ok, message) = hosted::probe_hosted(&backend, key.as_deref());
        Envelope::reply(id, Event::EndpointProbed { ok, message })
    }

    fn handle_download_model(
        &self,
        id: &RequestId,
        catalog_id: &str,
        emit: &mut impl FnMut(Envelope),
    ) {
        if let Some(failed) = self.require_hello(id) {
            emit(failed);
            return;
        }
        let mut inventory = match load_models(&self.store) {
            Ok(inventory) => inventory,
            Err(error) => {
                emit(store_failed(id, error));
                return;
            }
        };
        let dir = resolved_models_dir(&inventory.storage_dir);
        if inventory.storage_dir.trim().is_empty() {
            inventory.storage_dir = dir.display().to_string();
            if let Err(error) = save_models(&self.store, &inventory) {
                emit(store_failed(id, error));
                return;
            }
        }
        match download::download_catalog(&dir, catalog_id, id, emit) {
            Ok(()) => emit(Envelope::reply(
                id,
                Event::Models {
                    inventory: present_models(inventory),
                },
            )),
            Err(download::DownloadError::UnknownCatalog { catalog_id }) => emit(Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::InvalidRequest,
                    message: format!("Unknown catalog id {catalog_id}"),
                    issues: vec![],
                },
            )),
            Err(error) => emit(Envelope::reply(
                id,
                Event::Failed {
                    code: ErrorCode::Io,
                    message: error.to_string(),
                    issues: vec![],
                },
            )),
        }
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
        if self.should_stop(id) {
            emit(Envelope::reply(id, Event::Cancelled));
            return;
        }

        let resume = match session {
            Some(requested) => match self.store.get_snapshot(requested) {
                Ok(Some(prior)) if checkpoint::roots_match(&prior.root, root) => Some(prior),
                Ok(_) => None,
                Err(error) => {
                    emit(store_failed(id, error));
                    return;
                }
            },
            None => None,
        };
        let session = resume
            .as_ref()
            .map(|prior| prior.session)
            .or(session)
            .unwrap_or_default();
        if let Err(error) = emit_model_runtime_notices(&self.store, id, emit) {
            emit_log(
                emit,
                id,
                LogLevel::Warn,
                format!("Could not read settings or models ({error}); scan continues."),
            );
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
            if self.should_stop(id) {
                return false;
            }
            if should_emit_progress(&mut last_progress, current, None) {
                emit_progress(emit, id, "scan", current, None, message);
            }
            true
        }) {
            Ok(snapshot) => snapshot,
            Err(ScanError::Cancelled) => {
                emit(Envelope::reply(id, Event::Cancelled));
                return;
            }
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

        if let Some(prior) = resume {
            checkpoint::carry_evidence(&mut snapshot, &prior);
            let extracted = checkpoint::extract_done_count(&snapshot);
            emit_log(
                emit,
                id,
                LogLevel::Info,
                format!("Resuming session; {extracted} files already have metadata."),
            );
        }

        if self.should_stop(id) {
            self.emit_cancelled_checkpoint(id, &snapshot, emit);
            return;
        }

        let persist_error = RefCell::new(None);
        let store = &self.store;
        let mut persist = |snapshot: &WorkspaceSnapshot| match store.put_snapshot(snapshot) {
            Ok(()) => true,
            Err(error) => {
                persist_error.replace(Some(error));
                false
            }
        };

        if options.extract_metadata {
            last_progress = Instant::now()
                .checked_sub(PROGRESS_INTERVAL)
                .unwrap_or_else(Instant::now);
            let gate = Arc::clone(&self.cancel);
            let cancel_id = id.clone();
            match extract::extract_into_supervised(
                &mut snapshot,
                |current, total, path| {
                    if should_emit_progress(&mut last_progress, current, Some(total)) {
                        emit_progress(emit, id, "extract", current, Some(total), path);
                    }
                },
                &mut persist,
                || !gate.is_cancelled(&cancel_id),
            ) {
                WorkStatus::Completed => {}
                WorkStatus::Cancelled => {
                    self.emit_cancelled_checkpoint(id, &snapshot, emit);
                    return;
                }
                WorkStatus::PersistFailed => {
                    emit(checkpoint_store_failed(
                        id,
                        persist_error.borrow_mut().take(),
                    ));
                    return;
                }
            }
        }

        if self.should_stop(id) {
            self.emit_cancelled_checkpoint(id, &snapshot, emit);
            return;
        }

        match (load_settings(&self.store), load_models(&self.store)) {
            (Ok(settings), Ok(models)) => {
                last_progress = Instant::now()
                    .checked_sub(PROGRESS_INTERVAL)
                    .unwrap_or_else(Instant::now);
                let gate = Arc::clone(&self.cancel);
                let cancel_id = id.clone();
                match analyze::analyze_into_supervised(
                    &mut snapshot,
                    &models,
                    &settings,
                    |notice| match notice {
                        AnalyzeNotice::Log(message) => {
                            emit_log(emit, id, LogLevel::Info, message);
                        }
                        AnalyzeNotice::Progress {
                            stage,
                            current,
                            total,
                            path,
                        } => {
                            if should_emit_progress(&mut last_progress, current, Some(total)) {
                                emit_progress(emit, id, stage, current, Some(total), path);
                            }
                        }
                    },
                    &mut persist,
                    || !gate.is_cancelled(&cancel_id),
                ) {
                    WorkStatus::Completed => {}
                    WorkStatus::Cancelled => {
                        self.emit_cancelled_checkpoint(id, &snapshot, emit);
                        return;
                    }
                    WorkStatus::PersistFailed => {
                        emit(checkpoint_store_failed(
                            id,
                            persist_error.borrow_mut().take(),
                        ));
                        return;
                    }
                }
            }
            (Err(error), _) | (_, Err(error)) => emit_log(
                emit,
                id,
                LogLevel::Warn,
                format!("Could not load models for analysis ({error}); scan continues."),
            ),
        }

        if self.should_stop(id) {
            self.emit_cancelled_checkpoint(id, &snapshot, emit);
            return;
        }

        if let Err(error) = self.store.put_snapshot(&snapshot) {
            emit(store_failed(id, error));
            return;
        }
        emit(Envelope::reply(id, Event::ScanCompleted { snapshot }));
    }

    fn emit_cancelled_checkpoint(
        &self,
        id: &RequestId,
        snapshot: &WorkspaceSnapshot,
        emit: &mut impl FnMut(Envelope),
    ) {
        if let Err(error) = self.store.put_snapshot(snapshot) {
            emit(store_failed(id, error));
            return;
        }
        emit(Envelope::reply(id, Event::Cancelled));
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
        let journal = apply_plan_with_hooks(&snapshot, &plan, dry_run, |hook| {
            if self.should_stop(id) {
                return false;
            }
            match hook {
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
        let journal = undo_journal_with_hooks(&snapshot, &journal, |hook| {
            if self.should_stop(id) {
                return false;
            }
            match hook {
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
        let keyword = execute(&snapshot, &base, &interpret(trimmed));
        let inventory = load_models(&self.store).unwrap_or_default();
        let turn = assistant_turn(&inventory, trimmed, &snapshot, &base, &keyword);
        if turn.patches.is_empty() {
            return vec![Envelope::reply(
                id,
                Event::ChatReply {
                    message: turn.message,
                    revision: None,
                },
            )];
        }
        match base.with_patches(
            RevisionAuthor::Assistant {
                model: turn.model_id,
            },
            trimmed,
            &turn.patches,
        ) {
            Ok(revision) => {
                if let Err(error) = self.store.put_revision(&revision) {
                    return vec![store_failed(id, error)];
                }
                vec![Envelope::reply(
                    id,
                    Event::ChatReply {
                        message: turn.message,
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

fn stored_api_key_for(inventory: &ModelInventory, backend: &ModelBackend) -> Option<String> {
    inventory
        .slots
        .iter()
        .find(|slot| &slot.backend == backend)
        .and_then(|slot| slot.api_key.clone())
        .filter(|key| !key.trim().is_empty())
}

fn assistant_turn(
    inventory: &ModelInventory,
    utterance: &str,
    snapshot: &WorkspaceSnapshot,
    revision: &aifs_domain::ProposalRevision,
    keyword: &ToolOutput,
) -> ChatTurn {
    let Some(slot) = inventory
        .slots
        .iter()
        .find(|slot| slot.id == "chat" && !matches!(slot.backend, ModelBackend::Off))
    else {
        return ChatTurn {
            message: keyword.message.clone(),
            model_id: MOCK_ASSISTANT_MODEL.to_owned(),
            patches: keyword.patches.clone(),
        };
    };
    match chat_via_worker(slot, inventory, utterance, snapshot, revision) {
        Ok(text) if !text.trim().is_empty() => match chat::parse_chat_reply(&text) {
            Some(parsed) => {
                let (patches, skipped) = chat::drop_blocked_moves(snapshot, parsed.patches);
                let mut message = parsed.message.unwrap_or_else(|| text.clone());
                if skipped > 0 {
                    message.push_str(&format!(
                        " Skipped {skipped} protected or layout-preserving item(s)."
                    ));
                }
                ChatTurn {
                    message,
                    model_id: slot_model_id(&slot.backend),
                    patches,
                }
            }
            None => ChatTurn {
                message: text,
                model_id: slot_model_id(&slot.backend),
                patches: keyword.patches.clone(),
            },
        },
        Ok(_) => ChatTurn {
            message: keyword.message.clone(),
            model_id: MOCK_ASSISTANT_MODEL.to_owned(),
            patches: keyword.patches.clone(),
        },
        Err(_) => ChatTurn {
            message: format!("{}\n(Chat model skipped.)", keyword.message),
            model_id: MOCK_ASSISTANT_MODEL.to_owned(),
            patches: keyword.patches.clone(),
        },
    }
}

struct ChatTurn {
    message: String,
    model_id: String,
    patches: Vec<aifs_domain::RevisionPatch>,
}

fn slot_model_id(backend: &ModelBackend) -> String {
    match backend {
        ModelBackend::Catalog { catalog_id } => catalog_id.clone(),
        ModelBackend::LocalGguf { path, .. } => path.clone(),
        _ => hosted_model_label(backend),
    }
}

fn chat_via_worker(
    slot: &aifs_protocol::ModelSlot,
    inventory: &ModelInventory,
    utterance: &str,
    snapshot: &WorkspaceSnapshot,
    revision: &aifs_domain::ProposalRevision,
) -> Result<String, String> {
    let mut llm = WorkerClient::try_connect(WorkerKind::Llm)
        .ok_or_else(|| "LLM worker is not installed".to_owned())?;
    let context = chat::chat_context(snapshot, revision);
    let result = (|| {
        llm.load(
            slot.backend.clone(),
            inventory.gpu_preference.clone(),
            None,
            slot.api_key.clone(),
            inventory.storage_dir.clone(),
        )
        .map_err(|error| error.to_string())?;
        llm.chat(utterance, context)
            .map_err(|error| error.to_string())
    })();
    let _ = llm.unload();
    let _ = llm.shutdown();
    result
}

fn scan_error_event(id: &RequestId, error: ScanError) -> Envelope {
    if matches!(error, ScanError::Cancelled) {
        return Envelope::reply(id, Event::Cancelled);
    }
    let (code, message) = match error {
        ScanError::InvalidRoot { path, message } => (
            ErrorCode::InvalidRoot,
            format!("{}: {message}", path.display()),
        ),
        ScanError::Io { path, source } => (ErrorCode::Io, format!("{}: {source}", path.display())),
        ScanError::Cancelled => unreachable!("cancelled is emitted as Event::Cancelled"),
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

fn checkpoint_store_failed(id: &RequestId, error: Option<aifs_store::StoreError>) -> Envelope {
    match error {
        Some(error) => store_failed(id, error),
        None => Envelope::reply(
            id,
            Event::Failed {
                code: ErrorCode::Storage,
                message: "checkpoint persist failed".to_owned(),
                issues: vec![],
            },
        ),
    }
}

fn load_settings(
    store: &aifs_store::WorkspaceStore,
) -> Result<AppSettings, aifs_store::StoreError> {
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
    let mut stored = inventory.clone();
    stored.artifacts.clear();
    let json = serde_json::to_string(&stored)?;
    store.put_meta(MODELS_META_KEY, &json)
}

fn present_models(inventory: ModelInventory) -> ModelInventory {
    let dir = resolved_models_dir(&inventory.storage_dir);
    let mut next = inventory.redacted();
    if next.storage_dir.trim().is_empty() {
        next.storage_dir = dir.display().to_string();
    }
    next.with_disk_status(&dir)
}

fn resolved_models_dir(storage_dir: &str) -> PathBuf {
    let trimmed = storage_dir.trim();
    if trimmed.is_empty() {
        default_models_dir()
    } else {
        PathBuf::from(trimmed)
    }
}

fn default_models_dir() -> PathBuf {
    durable_store_base(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("LOCALAPPDATA").as_deref(),
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("USERPROFILE").as_deref(),
    )
    .map(|base| base.join("aifs").join("models"))
    .unwrap_or_else(|| PathBuf::from("aifs-models"))
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
                "Vision slot assigned; images will be described after extract.",
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
                "Document slot assigned; documents will be categorized after extract.",
            );
        }
    }
    Ok(())
}

fn default_store_path() -> Option<std::path::PathBuf> {
    durable_store_base(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("LOCALAPPDATA").as_deref(),
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("USERPROFILE").as_deref(),
    )
    .map(|base| base.join("aifs").join("engine.sqlite"))
}

fn durable_store_base(
    xdg_data_home: Option<&std::ffi::OsStr>,
    local_app_data: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
    user_profile: Option<&std::ffi::OsStr>,
) -> Option<std::path::PathBuf> {
    if let Some(xdg) = xdg_data_home {
        return Some(std::path::PathBuf::from(xdg));
    }
    if let Some(local) = local_app_data {
        return Some(std::path::PathBuf::from(local));
    }
    let home = home.or(user_profile)?;
    let mut path = std::path::PathBuf::from(home);
    if cfg!(windows) {
        path.push("AppData");
        path.push("Local");
    } else {
        path.push(".local");
        path.push("share");
    }
    Some(path)
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

fn emit_scan_logs(emit: &mut impl FnMut(Envelope), id: &RequestId, snapshot: &WorkspaceSnapshot) {
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
    for (skip_logged, skipped) in snapshot.skipped.iter().enumerate() {
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
    if let Some(error) = persist_error
        && !mutated
    {
        emit(store_failed(id, error));
        return;
    }
    if let Err(error) = store.put_journal(session, &journal)
        && !mutated
    {
        emit(store_failed(id, error));
        return;
    }
    emit(Envelope::reply(id, Event::Journal { journal }));
}

/// Reads JSONL requests from `stdin` and writes envelopes to `stdout` until shutdown.
pub fn run_stdio() -> io::Result<()> {
    let mut stdout = io::stdout();
    let mut engine = if let Some(explicit) = std::env::var_os("AIFS_STORE") {
        let path = std::path::PathBuf::from(&explicit);
        Engine::with_store_path(&path).map_err(|error| {
            io::Error::other(format!("opening AIFS_STORE {}: {error}", path.display()))
        })?
    } else {
        match default_store_path() {
            Some(path) => Engine::with_store_path(&path).map_err(|error| {
                io::Error::other(format!("opening {}: {error}", path.display()))
            })?,
            None => Engine::new(),
        }
    };
    let gate = engine.cancel_gate();
    let (tx, rx) = mpsc::channel::<io::Result<String>>();
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if let Ok(request) = decode_line::<Request>(&line)
                        && let Command::Cancel { target } = request.command
                    {
                        gate.request_cancel(target);
                    }
                    if tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                    break;
                }
            }
        }
    });
    for line in rx {
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
        assert!(
            completed
                .entries
                .iter()
                .any(|entry| entry.path.as_str() == "note.txt")
        );
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::Progress { .. })),
            "scan should emit progress before completing"
        );
    }

    #[test]
    fn scan_stream_includes_projects_skips_and_bundles() {
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/inbox-mixed");
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
    fn prearmed_cancel_stops_scan_without_snapshot() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        for index in 0..32 {
            fs::write(dir.path().join(format!("file-{index}.txt")), b"x")
                .unwrap_or_else(|e| panic!("{e}"));
        }
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        engine.request_cancel("2".into());
        let session = SessionId::new();
        let events = engine.handle(Request {
            id: "2".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: false,
                    ..ScanOptions::default()
                },
                session: Some(session),
            },
        });
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::Cancelled)),
            "expected cancelled, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::ScanCompleted { .. })),
            "cancelled scan must not complete a snapshot"
        );
        assert!(
            engine.stored_snapshot(session).is_none(),
            "walk cancel must not persist a checkpoint"
        );
    }

    #[test]
    fn cancel_after_relationships_persists_checkpoint() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|e| panic!("{e}"));
        let session = SessionId::new();
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let gate = engine.cancel_gate();
        let mut events = Vec::new();
        engine.handle_with(
            Request {
                id: "2".into(),
                command: Command::Scan {
                    root: dir.path().to_path_buf(),
                    options: ScanOptions {
                        extract_metadata: false,
                        fingerprint_prefix_bytes: 32,
                        ..ScanOptions::default()
                    },
                    session: Some(session),
                },
            },
            &mut |envelope| {
                if let Event::Progress { stage, .. } = &envelope.event
                    && stage == "relationships"
                {
                    gate.request_cancel("2".into());
                }
                events.push(envelope);
            },
        );
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::Cancelled)),
            "expected cancelled, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::ScanCompleted { .. })),
            "must not complete a snapshot"
        );
        let checkpoint = engine
            .stored_snapshot(session)
            .unwrap_or_else(|| panic!("cancel after walk should persist the tree"));
        assert!(
            checkpoint
                .entries
                .iter()
                .any(|entry| entry.path.as_str() == "note.txt"),
            "checkpoint entries={:?}",
            checkpoint.entries
        );
    }

    #[test]
    fn cancel_after_finished_walk_persists_checkpoint() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|e| panic!("{e}"));
        let session = SessionId::new();
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let gate = engine.cancel_gate();
        let mut events = Vec::new();
        engine.handle_with(
            Request {
                id: "2".into(),
                command: Command::Scan {
                    root: dir.path().to_path_buf(),
                    options: ScanOptions {
                        extract_metadata: false,
                        fingerprint_prefix_bytes: 32,
                        ..ScanOptions::default()
                    },
                    session: Some(session),
                },
            },
            &mut |envelope| {
                if let Event::Progress { stage, message, .. } = &envelope.event
                    && stage == "scan"
                    && message == "walk complete"
                {
                    gate.request_cancel("2".into());
                }
                events.push(envelope);
            },
        );
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::Cancelled)),
            "expected cancelled, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::ScanCompleted { .. })),
            "must not complete a snapshot"
        );
        let checkpoint = engine
            .stored_snapshot(session)
            .unwrap_or_else(|| panic!("cancel after a finished walk should persist the tree"));
        assert!(
            checkpoint
                .entries
                .iter()
                .any(|entry| entry.path.as_str() == "note.txt"),
            "checkpoint entries={:?}",
            checkpoint.entries
        );
    }

    #[test]
    fn cancel_command_acks_and_arms_target() {
        let mut engine = Engine::new();
        let events = engine.handle(Request {
            id: "c".into(),
            command: Command::Cancel { target: "2".into() },
        });
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::Cancelled)),
            "expected cancel ack, got {events:?}"
        );
        assert!(engine.cancel_gate().is_cancelled(&"2".into()));
    }

    #[test]
    fn cancel_during_extract_persists_checkpoint_for_resume() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        for index in 0..12 {
            aifs_extractors::write_id3v23_fixture(
                &dir.path().join(format!("track-{index}.mp3")),
                "Night Drive",
                "Ada",
                "After Hours",
                "2019",
            )
            .unwrap_or_else(|e| panic!("{e}"));
        }
        let session = SessionId::new();
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let gate = engine.cancel_gate();
        let mut events = Vec::new();
        engine.handle_with(
            Request {
                id: "2".into(),
                command: Command::Scan {
                    root: dir.path().to_path_buf(),
                    options: ScanOptions {
                        extract_metadata: true,
                        fingerprint_prefix_bytes: 32,
                        ..ScanOptions::default()
                    },
                    session: Some(session),
                },
            },
            &mut |envelope| {
                if let Event::Progress { stage, current, .. } = &envelope.event
                    && stage == "extract"
                    && *current >= 1
                {
                    gate.request_cancel("2".into());
                }
                events.push(envelope);
            },
        );
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::Cancelled)),
            "expected cancelled, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|envelope| matches!(envelope.event, Event::ScanCompleted { .. })),
            "cancelled extract must not complete a snapshot"
        );
        let checkpoint = engine
            .stored_snapshot(session)
            .unwrap_or_else(|| panic!("expected extract checkpoint"));
        assert!(
            !checkpoint.evidence.is_empty(),
            "checkpoint should keep extracted tags, got {:?}",
            checkpoint.evidence
        );

        let resumed = engine.handle(Request {
            id: "3".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: true,
                    fingerprint_prefix_bytes: 32,
                    ..ScanOptions::default()
                },
                session: Some(session),
            },
        });
        let logs: Vec<_> = resumed
            .iter()
            .filter_map(|envelope| match &envelope.event {
                Event::Log { message, .. } => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            logs.iter().any(|line| line.contains("Resuming session")),
            "expected resume log, got {logs:?}"
        );
        let snapshot = match terminal(resumed) {
            Event::ScanCompleted { snapshot } => snapshot,
            other => panic!("unexpected {other:?}"),
        };
        let tagged = snapshot
            .evidence
            .iter()
            .filter(|bag| bag.fact(aifs_domain::evidence::keys::MEDIA_TITLE) == Some("Night Drive"))
            .count();
        assert_eq!(
            tagged, 12,
            "resume should skip files that already have tags, tagged={tagged} evidence={:?}",
            snapshot.evidence
        );
        for entry in snapshot
            .entries
            .iter()
            .filter(|entry| entry.kind == aifs_domain::EntryKind::File)
        {
            let extract_bags = snapshot
                .evidence
                .iter()
                .filter(|bag| {
                    bag.asset == entry.id
                        && matches!(
                            bag.source,
                            aifs_domain::EvidenceSource::MediaTags
                                | aifs_domain::EvidenceSource::Exif
                                | aifs_domain::EvidenceSource::DocumentMetadata
                        )
                })
                .count();
            assert_eq!(
                extract_bags,
                1,
                "resume must not re-extract {} (bags={extract_bags})",
                entry.path.as_str()
            );
        }
    }

    #[test]
    fn mismatched_root_does_not_carry_prior_evidence() {
        let media = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        aifs_extractors::write_id3v23_fixture(
            &media.path().join("show.mp3"),
            "Night Drive",
            "Ada",
            "After Hours",
            "2019",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let other = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::copy(media.path().join("show.mp3"), other.path().join("show.mp3"))
            .unwrap_or_else(|e| panic!("{e}"));
        let session = SessionId::new();
        let mut engine = Engine::new();
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
                root: media.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: true,
                    fingerprint_prefix_bytes: 32,
                    ..ScanOptions::default()
                },
                session: Some(session),
            },
        })) {
            Event::ScanCompleted { snapshot } => {
                assert!(snapshot.evidence.iter().any(|bag| {
                    bag.fact(aifs_domain::evidence::keys::MEDIA_TITLE) == Some("Night Drive")
                }));
            }
            other => panic!("unexpected {other:?}"),
        }
        let events = engine.handle(Request {
            id: "3".into(),
            command: Command::Scan {
                root: other.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: false,
                    fingerprint_prefix_bytes: 32,
                    ..ScanOptions::default()
                },
                session: Some(session),
            },
        });
        assert!(
            events.iter().all(|envelope| match &envelope.event {
                Event::Log { message, .. } => !message.contains("Resuming session"),
                _ => true,
            }),
            "mismatched root must not resume, events={events:?}"
        );
        let snapshot = match terminal(events) {
            Event::ScanCompleted { snapshot } => snapshot,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(snapshot.session, session);
        assert!(
            checkpoint::roots_match(&snapshot.root, other.path()),
            "expected other root {:?}, got {:?}",
            other.path(),
            snapshot.root
        );
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.path.as_str() == "show.mp3")
        );
        assert!(
            snapshot.evidence.iter().all(|bag| {
                bag.fact(aifs_domain::evidence::keys::MEDIA_TITLE) != Some("Night Drive")
            }),
            "must not carry evidence from a different root, evidence={:?}",
            snapshot.evidence
        );
    }

    #[test]
    fn junk_drawer_keeps_library_and_archive_paths() {
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/junk-drawer");
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
            Event::Settings { settings: stored } => assert!(stored.scan.include_hidden),
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

    fn spawn_http(
        status: &'static str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        crate::http_stub::serve_json_once(status, body)
    }

    #[test]
    fn probe_custom_endpoint_hits_http() {
        let (base, server) = spawn_http("200 OK", r#"{"id":"ok"}"#);
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        match terminal(engine.handle(Request {
            id: "2".into(),
            command: Command::ProbeEndpoint {
                backend: ModelBackend::CustomEndpoint {
                    base_url: base,
                    model: "local-test".into(),
                },
                api_key: Some("sk-secret".into()),
            },
        })) {
            Event::EndpointProbed { ok, message } => {
                assert!(ok, "{message}");
                assert!(!message.contains("sk-secret"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
        let request = server.join().unwrap_or_else(|error| panic!("{error:?}"));
        let request_line = request.lines().next().unwrap_or("");
        assert!(
            !request_line.contains("sk-secret"),
            "probe URL leaked the API key: {request_line}"
        );
    }

    #[test]
    fn chat_uses_hosted_slot_and_still_emits_patches() {
        aifs_worker_client::discover_worker_binary(aifs_protocol::worker::WorkerKind::Llm)
            .unwrap_or_else(|error| panic!("build aifs-worker-llm before this test ({error})"));
        let body = r#"{"choices":[{"message":{"content":"Grouping audio into Podcasts."}}]}"#;
        let (base, server) = spawn_http("200 OK", body);
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
        let mut inventory = ModelInventory::default();
        if let Some(slot) = inventory.slots.iter_mut().find(|slot| slot.id == "chat") {
            slot.backend = ModelBackend::CustomEndpoint {
                base_url: base,
                model: "local-test".into(),
            };
        }
        engine.handle(Request {
            id: "2".into(),
            command: Command::PutModels { inventory },
        });
        let snapshot = match terminal(engine.handle(Request {
            id: "3".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: false,
                    ..ScanOptions::default()
                },
                session: None,
            },
        })) {
            Event::ScanCompleted { snapshot } => snapshot,
            other => panic!("unexpected {other:?}"),
        };
        let revision = match terminal(engine.handle(Request {
            id: "4".into(),
            command: Command::Propose {
                session: snapshot.session,
                policy: ProposalPolicy::default(),
            },
        })) {
            Event::Revision { revision } => revision,
            other => panic!("unexpected {other:?}"),
        };
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
        assert!(message.contains("Grouping audio"), "{message}");
        assert!(!message.contains("Chat model skipped"), "{message}");
        let next = next.unwrap_or_else(|| panic!("expected child revision"));
        assert_eq!(next.parent, Some(revision.id));
        let _ = server.join();
    }

    #[test]
    fn chat_model_json_patches_win_over_keywords() {
        aifs_worker_client::discover_worker_binary(aifs_protocol::worker::WorkerKind::Llm)
            .unwrap_or_else(|error| panic!("build aifs-worker-llm before this test ({error})"));
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
        let snapshot = match terminal(engine.handle(Request {
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
        let asset = *revision
            .placements
            .keys()
            .next()
            .unwrap_or_else(|| panic!("expected a placement"));
        let content = serde_json::json!({
            "message": "Filing under Broadcasts.",
            "patches": [{
                "op": "move_to_folder",
                "assets": [asset],
                "folder": "Broadcasts"
            }]
        })
        .to_string();
        let body = serde_json::json!({
            "choices": [{ "message": { "content": content } }]
        })
        .to_string();
        let (base, server) = crate::http_stub::serve_json_once_owned("200 OK", body);
        let mut inventory = ModelInventory::default();
        if let Some(slot) = inventory.slots.iter_mut().find(|slot| slot.id == "chat") {
            slot.backend = ModelBackend::CustomEndpoint {
                base_url: base,
                model: "local-test".into(),
            };
        }
        engine.handle(Request {
            id: "4".into(),
            command: Command::PutModels { inventory },
        });
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
        assert!(message.contains("Broadcasts"), "{message}");
        assert!(!message.contains("Podcasts"), "{message}");
        let next = next.unwrap_or_else(|| panic!("expected child revision"));
        let dest = next
            .placements
            .values()
            .next()
            .map(|placement| placement.destination.as_str().to_owned())
            .unwrap_or_default();
        assert!(
            dest.starts_with("Broadcasts/"),
            "model JSON must win over keyword Podcasts, got {dest}"
        );
        let _ = server.join();
    }

    #[test]
    fn chat_empty_model_patches_do_not_run_keywords() {
        aifs_worker_client::discover_worker_binary(aifs_protocol::worker::WorkerKind::Llm)
            .unwrap_or_else(|error| panic!("build aifs-worker-llm before this test ({error})"));
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
        let snapshot = match terminal(engine.handle(Request {
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
        let before = revision
            .placements
            .values()
            .next()
            .map(|placement| placement.destination.as_str().to_owned())
            .unwrap_or_default();
        let content = serde_json::json!({
            "message": "Just thinking.",
            "patches": []
        })
        .to_string();
        let body = serde_json::json!({
            "choices": [{ "message": { "content": content } }]
        })
        .to_string();
        let (base, server) = crate::http_stub::serve_json_once_owned("200 OK", body);
        let mut inventory = ModelInventory::default();
        if let Some(slot) = inventory.slots.iter_mut().find(|slot| slot.id == "chat") {
            slot.backend = ModelBackend::CustomEndpoint {
                base_url: base,
                model: "local-test".into(),
            };
        }
        engine.handle(Request {
            id: "4".into(),
            command: Command::PutModels { inventory },
        });
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
        assert!(message.contains("Just thinking."), "{message}");
        assert!(
            next.is_none(),
            "empty model patches must not keyword-move, got {next:?}"
        );
        assert!(
            !before.starts_with("Podcasts/"),
            "fixture destination already under Podcasts: {before}"
        );
        let _ = server.join();
    }

    #[test]
    fn chat_skips_unreachable_hosted_slot_without_leaking_errors() {
        aifs_worker_client::discover_worker_binary(aifs_protocol::worker::WorkerKind::Llm)
            .unwrap_or_else(|error| panic!("build aifs-worker-llm before this test ({error})"));
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
        let mut inventory = ModelInventory::default();
        if let Some(slot) = inventory.slots.iter_mut().find(|slot| slot.id == "chat") {
            slot.backend = ModelBackend::CustomEndpoint {
                base_url: "http://127.0.0.1:1/v1".into(),
                model: "local-test".into(),
            };
            slot.api_key = Some("sk-secret".into());
        }
        engine.handle(Request {
            id: "2".into(),
            command: Command::PutModels { inventory },
        });
        let snapshot = match terminal(engine.handle(Request {
            id: "3".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: false,
                    ..ScanOptions::default()
                },
                session: None,
            },
        })) {
            Event::ScanCompleted { snapshot } => snapshot,
            other => panic!("unexpected {other:?}"),
        };
        let revision = match terminal(engine.handle(Request {
            id: "4".into(),
            command: Command::Propose {
                session: snapshot.session,
                policy: ProposalPolicy::default(),
            },
        })) {
            Event::Revision { revision } => revision,
            other => panic!("unexpected {other:?}"),
        };
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
        assert!(message.contains("Chat model skipped."), "{message}");
        assert!(!message.contains("sk-secret"), "{message}");
        assert!(!message.contains("127.0.0.1"), "{message}");
        let next = next.unwrap_or_else(|| panic!("expected child revision"));
        assert_eq!(next.parent, Some(revision.id));
    }

    #[test]
    fn scan_categorizes_when_llm_slot_is_assigned() {
        aifs_worker_client::discover_worker_binary(aifs_protocol::worker::WorkerKind::Llm)
            .unwrap_or_else(|error| panic!("build aifs-worker-llm before this test ({error})"));
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("note.txt"), b"hello").unwrap_or_else(|e| panic!("{e}"));
        let mut engine = Engine::new();
        engine.handle(Request {
            id: "1".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
        let mut inventory = ModelInventory::default();
        inventory.slots[0].backend = ModelBackend::Catalog {
            catalog_id: "gemma-3-4b-it".into(),
        };
        match terminal(engine.handle(Request {
            id: "2".into(),
            command: Command::PutModels {
                inventory: inventory.clone(),
            },
        })) {
            Event::Models { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
        let events = engine.handle(Request {
            id: "3".into(),
            command: Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions::default(),
                session: None,
            },
        });
        let logs: Vec<String> = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                Event::Log { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect();
        let snapshot = match terminal(events) {
            Event::ScanCompleted { snapshot } => snapshot,
            other => panic!("unexpected {other:?}"),
        };
        let categorized = snapshot.evidence.iter().any(|bag| {
            matches!(bag.source, aifs_domain::EvidenceSource::LocalModel { .. })
                && bag.fact(aifs_domain::evidence::keys::CATEGORY) == Some("Documents")
        });
        assert!(
            categorized,
            "expected stub categorize evidence, evidence={:?} logs={logs:?}",
            snapshot.evidence
        );
        assert!(
            logs.iter()
                .any(|message| message.contains("categorized") && message.contains("note.txt")),
            "expected categorize log, logs={logs:?}"
        );
        let revision = match terminal(engine.handle(Request {
            id: "4".into(),
            command: Command::Propose {
                session: snapshot.session,
                policy: ProposalPolicy::default(),
            },
        })) {
            Event::Revision { revision } => revision,
            other => panic!("unexpected {other:?}"),
        };
        let dest = revision
            .placements
            .values()
            .next()
            .map(|placement| placement.destination.as_str().to_owned())
            .unwrap_or_default();
        assert_eq!(dest, "Documents/note.txt");
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
    fn corrupt_settings_json_does_not_abort_scan() {
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
            Event::ScanCompleted { snapshot } => {
                assert!(
                    snapshot
                        .entries
                        .iter()
                        .any(|entry| entry.path.as_str() == "note.txt")
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn corrupt_snapshot_json_is_a_storage_error_on_resume() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|e| panic!("{e}"));
        let db = dir.path().join("engine.sqlite");
        let session = SessionId::new();
        let store = aifs_store::WorkspaceStore::open(&db).unwrap_or_else(|e| panic!("{e}"));
        store
            .put_snapshot_json(session, "{not-json")
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
                session: Some(session),
            },
        })) {
            Event::Failed { code, .. } => assert_eq!(code, ErrorCode::Storage),
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            aifs_store::WorkspaceStore::open(&db)
                .unwrap_or_else(|e| panic!("{e}"))
                .get_snapshot(session)
                .is_err(),
            "corrupt row must still be present so scan does not overwrite it"
        );
    }

    #[test]
    fn durable_store_base_uses_windows_app_data_and_unix_home() {
        use std::ffi::OsStr;
        assert_eq!(
            durable_store_base(
                None,
                Some(OsStr::new("/win/local")),
                Some(OsStr::new("/home")),
                None
            ),
            Some(std::path::PathBuf::from("/win/local"))
        );
        let from_profile = durable_store_base(None, None, None, Some(OsStr::new("/Users/me")));
        assert!(
            from_profile.is_some(),
            "USERPROFILE must yield a store base"
        );
    }

    #[test]
    fn download_model_skips_shared_gguf_already_on_disk() {
        let models = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let files = [
            (
                aifs_protocol::GEMMA_TEXT_FILENAME,
                b"text-weights".as_slice(),
            ),
            (aifs_protocol::GEMMA_MMPROJ_FILENAME, b"mmproj".as_slice()),
        ];
        let (base, hits) = spawn_catalog_http(&files);
        let _guard = CatalogBaseGuard::set(&base);
        let mut engine = Engine::new();
        hello_ok(&mut engine);
        let inventory = ModelInventory {
            storage_dir: models.path().display().to_string(),
            ..ModelInventory::default()
        };
        terminal(engine.handle(Request {
            id: "put".into(),
            command: Command::PutModels { inventory },
        }));

        let first = engine.handle(Request {
            id: "dl1".into(),
            command: Command::DownloadModel {
                catalog_id: "gemma-3-4b-it".into(),
            },
        });
        match terminal(first) {
            Event::Models { inventory: stored } => {
                assert!(
                    stored
                        .artifacts
                        .iter()
                        .any(|artifact| artifact.id == "gemma-text-q4" && artifact.present),
                    "{stored:?}"
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(hit_count(&hits, aifs_protocol::GEMMA_TEXT_FILENAME), 1);
        assert_eq!(hit_count(&hits, aifs_protocol::GEMMA_MMPROJ_FILENAME), 0);

        let second = engine.handle(Request {
            id: "dl2".into(),
            command: Command::DownloadModel {
                catalog_id: "gemma-3-4b-it-mmproj".into(),
            },
        });
        match terminal(second) {
            Event::Models { inventory: stored } => {
                assert!(stored.artifacts.iter().all(|artifact| artifact.present));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(hit_count(&hits, aifs_protocol::GEMMA_TEXT_FILENAME), 1);
        assert_eq!(hit_count(&hits, aifs_protocol::GEMMA_MMPROJ_FILENAME), 1);

        let third = engine.handle(Request {
            id: "dl3".into(),
            command: Command::DownloadModel {
                catalog_id: "gemma-3-4b-it".into(),
            },
        });
        assert!(matches!(terminal(third), Event::Models { .. }));
        assert_eq!(hit_count(&hits, aifs_protocol::GEMMA_TEXT_FILENAME), 1);

        match terminal(engine.handle(Request {
            id: "probe".into(),
            command: Command::ProbeEndpoint {
                backend: ModelBackend::Catalog {
                    catalog_id: "gemma-3-4b-it".into(),
                },
                api_key: None,
            },
        })) {
            Event::EndpointProbed { ok, message } => {
                assert!(ok, "{message}");
                assert!(message.contains("already downloaded"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn download_model_rejects_unknown_catalog() {
        let mut engine = Engine::new();
        hello_ok(&mut engine);
        match terminal(engine.handle(Request {
            id: "bad".into(),
            command: Command::DownloadModel {
                catalog_id: "not-a-model".into(),
            },
        })) {
            Event::Failed { code, .. } => assert_eq!(code, ErrorCode::InvalidRequest),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn hello_ok(engine: &mut Engine) {
        engine.handle(Request {
            id: "hello".into(),
            command: Command::Hello {
                client: "test".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        });
    }

    fn hit_count(
        hits: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>>,
        name: &str,
    ) -> u32 {
        hits.lock()
            .unwrap_or_else(|error| panic!("{error}"))
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    fn spawn_catalog_http(
        files: &[(&str, &[u8])],
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>>,
    ) {
        use std::collections::HashMap;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        use std::thread;

        let bodies: HashMap<String, Vec<u8>> = files
            .iter()
            .map(|(name, body)| ((*name).to_owned(), body.to_vec()))
            .collect();
        let hits = Arc::new(Mutex::new(HashMap::new()));
        let hits_for_thread = hits.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("{error}"));
        thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else {
                    continue;
                };
                let mut buf = [0_u8; 4096];
                let Ok(read) = stream.read(&mut buf) else {
                    continue;
                };
                let request = String::from_utf8_lossy(&buf[..read]);
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/');
                let filename = path.split('?').next().unwrap_or(path);
                if let Some(body) = bodies.get(filename) {
                    {
                        let mut counts = hits_for_thread
                            .lock()
                            .unwrap_or_else(|error| panic!("{error}"));
                        *counts.entry(filename.to_owned()).or_insert(0) += 1;
                    }
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                } else {
                    let header =
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(header.as_bytes());
                }
            }
        });
        (format!("http://{addr}"), hits)
    }

    struct CatalogBaseGuard {
        previous: Option<String>,
    }

    impl CatalogBaseGuard {
        fn set(base: &str) -> Self {
            let previous = aifs_protocol::set_catalog_base_override(Some(base.to_owned()));
            Self { previous }
        }
    }

    impl Drop for CatalogBaseGuard {
        fn drop(&mut self) {
            let _ = aifs_protocol::set_catalog_base_override(self.previous.take());
        }
    }
}
