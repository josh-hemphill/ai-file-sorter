//! Session-lived LLM analysis after deterministic extract.

use crate::cancel::WorkStatus;
use crate::checkpoint::{CHECKPOINT_EVERY, has_category_evidence, has_description_evidence};
use aifs_domain::{
    EntryKind, Evidence, FileFamily, ObservedEntry, WorkspaceSnapshot, evidence::keys,
};
use aifs_protocol::worker::WorkerKind;
use aifs_protocol::{AppSettings, ModelBackend, ModelInventory, ModelSlot, sanitize_hosted_text};
use aifs_worker_client::{WorkerClient, WorkerClientError};

/// Log line or categorize/describe progress from supervised analysis.
pub enum AnalyzeNotice<'a> {
    /// Informational line (load, skip, category).
    Log(String),
    /// Per-file categorize/describe progress.
    Progress {
        /// `categorize` or `describe`.
        stage: &'a str,
        /// 1-based index.
        current: u64,
        /// Files in this stage.
        total: u64,
        /// Relative path.
        path: &'a str,
    },
}

/// Runs categorize/describe when slots are assigned. Heuristics still propose later.
pub fn analyze_into_supervised(
    snapshot: &mut WorkspaceSnapshot,
    models: &ModelInventory,
    settings: &AppSettings,
    mut on_notice: impl FnMut(AnalyzeNotice<'_>),
    mut on_checkpoint: impl FnMut(&WorkspaceSnapshot) -> bool,
    mut should_continue: impl FnMut() -> bool,
) -> WorkStatus {
    let categorize = slot(models, "categorize").filter(|slot| !is_off(&slot.backend));
    let vision = slot(models, "vision").filter(|slot| !is_off(&slot.backend));
    let document = slot(models, "document").filter(|slot| !is_off(&slot.backend));
    let categorize_slot = categorize.or(if settings.analyze_documents {
        document
    } else {
        None
    });
    let run_categorize = categorize_slot.is_some();
    let run_describe = settings.analyze_images && vision.is_some();
    if !run_categorize && !run_describe {
        return WorkStatus::Completed;
    }

    let mut llm = match WorkerClient::try_connect(WorkerKind::Llm) {
        Some(client) => client,
        None => {
            on_notice(AnalyzeNotice::Log(
                "LLM worker is not installed; scan continues with extract and heuristics."
                    .to_owned(),
            ));
            return WorkStatus::Completed;
        }
    };

    let storage_dir = models.storage_dir.trim().to_owned();
    let mut loaded: Option<String> = None;
    let document_only = categorize.is_none() && document.is_some();

    if run_categorize && let Some(slot) = categorize_slot {
        match ensure_loaded(
            &mut llm,
            &mut loaded,
            slot,
            models,
            &storage_dir,
            &mut |message| on_notice(AnalyzeNotice::Log(message)),
        ) {
            Ok(()) => {
                let targets: Vec<ObservedEntry> = snapshot
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.kind == EntryKind::File
                            && should_categorize(entry, categorize.is_some(), document_only)
                    })
                    .cloned()
                    .collect();
                let total = targets.len() as u64;
                let mut bags = Vec::new();
                for (index, entry) in targets.into_iter().enumerate() {
                    if !should_continue() {
                        snapshot.evidence.extend(bags);
                        shutdown_llm(&mut llm, loaded.is_some());
                        return WorkStatus::Cancelled;
                    }
                    on_notice(AnalyzeNotice::Progress {
                        stage: "categorize",
                        current: index as u64 + 1,
                        total,
                        path: entry.path.as_str(),
                    });
                    if has_category_evidence(snapshot, &entry) {
                        continue;
                    }
                    let prior = prior_evidence(snapshot, &entry);
                    match llm.categorize(&snapshot.root, &entry, prior) {
                        Ok(Some(evidence)) => {
                            log_category(
                                &mut |message| on_notice(AnalyzeNotice::Log(message)),
                                &entry,
                                &evidence,
                            );
                            bags.push(evidence);
                            if bags.len() >= CHECKPOINT_EVERY {
                                snapshot.evidence.extend(std::mem::take(&mut bags));
                                if !on_checkpoint(snapshot) {
                                    shutdown_llm(&mut llm, loaded.is_some());
                                    return WorkStatus::PersistFailed;
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => on_notice(AnalyzeNotice::Log(format!(
                            "categorize skipped {}: {}",
                            entry.path.as_str(),
                            sanitize_hosted_text(&error.to_string(), slot.api_key.as_deref())
                        ))),
                    }
                }
                snapshot.evidence.extend(bags);
            }
            Err(message) => on_notice(AnalyzeNotice::Log(format!(
                "Categorize load failed ({message}); folder labels stay heuristic."
            ))),
        }
    }

    if run_describe && let Some(slot) = vision {
        match ensure_loaded(
            &mut llm,
            &mut loaded,
            slot,
            models,
            &storage_dir,
            &mut |message| on_notice(AnalyzeNotice::Log(message)),
        ) {
            Ok(()) => {
                let targets: Vec<ObservedEntry> = snapshot
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.kind == EntryKind::File
                            && matches!(entry.family, FileFamily::Image | FileFamily::RawImage)
                    })
                    .cloned()
                    .collect();
                let total = targets.len() as u64;
                let mut bags = Vec::new();
                for (index, entry) in targets.into_iter().enumerate() {
                    if !should_continue() {
                        snapshot.evidence.extend(bags);
                        shutdown_llm(&mut llm, loaded.is_some());
                        return WorkStatus::Cancelled;
                    }
                    on_notice(AnalyzeNotice::Progress {
                        stage: "describe",
                        current: index as u64 + 1,
                        total,
                        path: entry.path.as_str(),
                    });
                    if has_description_evidence(snapshot, &entry) {
                        continue;
                    }
                    let prior = prior_evidence(snapshot, &entry);
                    match llm.describe(&snapshot.root, &entry, prior) {
                        Ok(Some(evidence)) => {
                            on_notice(AnalyzeNotice::Log(format!(
                                "described {}",
                                entry.path.as_str()
                            )));
                            bags.push(evidence);
                            if bags.len() >= CHECKPOINT_EVERY {
                                snapshot.evidence.extend(std::mem::take(&mut bags));
                                if !on_checkpoint(snapshot) {
                                    shutdown_llm(&mut llm, loaded.is_some());
                                    return WorkStatus::PersistFailed;
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => on_notice(AnalyzeNotice::Log(format!(
                            "describe skipped {}: {}",
                            entry.path.as_str(),
                            sanitize_hosted_text(&error.to_string(), slot.api_key.as_deref())
                        ))),
                    }
                }
                snapshot.evidence.extend(bags);
            }
            Err(message) => on_notice(AnalyzeNotice::Log(format!(
                "Vision load failed ({message}); image description skipped."
            ))),
        }
    }

    shutdown_llm(&mut llm, loaded.is_some());
    WorkStatus::Completed
}

fn shutdown_llm(llm: &mut WorkerClient, loaded: bool) {
    if loaded {
        let _ = llm.unload();
    }
    let _ = llm.shutdown();
}

fn should_categorize(entry: &ObservedEntry, categorize_on: bool, document_only: bool) -> bool {
    if categorize_on {
        return true;
    }
    document_only
        && matches!(
            entry.family,
            FileFamily::Document
                | FileFamily::Spreadsheet
                | FileFamily::Presentation
                | FileFamily::Ebook
        )
}

fn slot<'a>(models: &'a ModelInventory, id: &str) -> Option<&'a ModelSlot> {
    models.slots.iter().find(|slot| slot.id == id)
}

fn is_off(backend: &ModelBackend) -> bool {
    matches!(backend, ModelBackend::Off)
}

fn backend_key(backend: &ModelBackend) -> String {
    match backend {
        ModelBackend::Off => "off".to_owned(),
        ModelBackend::Catalog { catalog_id } => format!("catalog:{catalog_id}"),
        ModelBackend::LocalGguf { path, mmproj } => {
            format!("gguf:{path}:{}", mmproj.as_deref().unwrap_or(""))
        }
        ModelBackend::OpenAi { model } => format!("openai:{model}"),
        ModelBackend::Gemini { model } => format!("gemini:{model}"),
        ModelBackend::CustomEndpoint { base_url, model } => {
            format!("custom:{base_url}:{model}")
        }
    }
}

fn ensure_loaded(
    llm: &mut WorkerClient,
    loaded: &mut Option<String>,
    slot: &ModelSlot,
    models: &ModelInventory,
    storage_dir: &str,
    on_log: &mut impl FnMut(String),
) -> Result<(), String> {
    let key = backend_key(&slot.backend);
    if loaded.as_ref() == Some(&key) {
        return Ok(());
    }
    if loaded.is_some() {
        llm.unload()
            .map_err(|error| load_error(error, slot.api_key.as_deref()))?;
        *loaded = None;
    }
    let result = llm
        .load(
            slot.backend.clone(),
            models.gpu_preference.clone(),
            None,
            slot.api_key.clone(),
            storage_dir,
        )
        .map_err(|error| load_error(error, slot.api_key.as_deref()))?;
    if let Some(fallback) = &result.fallback {
        on_log(fallback.clone());
    }
    on_log(format!(
        "loaded {} on {} for {}",
        result.model, result.device, slot.id
    ));
    *loaded = Some(key);
    Ok(())
}

fn load_error(error: WorkerClientError, api_key: Option<&str>) -> String {
    sanitize_hosted_text(&error.to_string(), api_key)
}

fn prior_evidence(snapshot: &WorkspaceSnapshot, entry: &ObservedEntry) -> Vec<Evidence> {
    snapshot.evidence_for(entry.id).cloned().collect()
}

fn log_category(on_log: &mut impl FnMut(String), entry: &ObservedEntry, evidence: &Evidence) {
    let label = evidence.fact(keys::CATEGORY).unwrap_or("unlabeled");
    on_log(format!("categorized {} → {label}", entry.path.as_str()));
}
