//! Session-lived LLM analysis after deterministic extract.

use aifs_domain::{
    EntryKind, Evidence, FileFamily, ObservedEntry, WorkspaceSnapshot, evidence::keys,
};
use aifs_protocol::worker::WorkerKind;
use aifs_protocol::{AppSettings, ModelBackend, ModelInventory, ModelSlot};
use aifs_worker_client::{WorkerClient, WorkerClientError};

/// Runs categorize/describe when slots are assigned. Heuristics still propose later.
pub fn analyze_into_supervised(
    snapshot: &mut WorkspaceSnapshot,
    models: &ModelInventory,
    settings: &AppSettings,
    mut on_log: impl FnMut(String),
) {
    let categorize = slot(models, "categorize").filter(|slot| !is_off(&slot.backend));
    let vision = slot(models, "vision").filter(|slot| !is_off(&slot.backend));
    let document = slot(models, "document").filter(|slot| !is_off(&slot.backend));
    let run_categorize = categorize.is_some() || document.is_some();
    let run_describe = settings.analyze_images && vision.is_some();
    if !run_categorize && !run_describe {
        return;
    }

    let mut llm = match WorkerClient::try_connect(WorkerKind::Llm) {
        Some(client) => client,
        None => {
            on_log(
                "LLM worker is not installed; scan continues with extract and heuristics."
                    .to_owned(),
            );
            return;
        }
    };

    let storage_dir = models.storage_dir.trim().to_owned();
    let mut loaded: Option<String> = None;
    let mut bags = Vec::new();
    let document_only = categorize.is_none() && document.is_some();

    if let Some(slot) = categorize.or(document)
        && let Err(message) = ensure_loaded(
            &mut llm,
            &mut loaded,
            slot,
            models,
            &storage_dir,
            &mut on_log,
        )
    {
        on_log(format!(
            "Categorize load failed ({message}); folder labels stay heuristic."
        ));
    }

    for entry in snapshot.entries.iter() {
        if entry.kind != EntryKind::File {
            continue;
        }
        let prior = prior_evidence(snapshot, entry);
        if run_categorize
            && loaded.is_some()
            && should_categorize(
                entry,
                categorize.is_some(),
                document_only,
                settings.analyze_documents,
            )
        {
            match llm.categorize(&snapshot.root, entry, prior.clone()) {
                Ok(Some(evidence)) => {
                    log_category(&mut on_log, entry, &evidence);
                    bags.push(evidence);
                }
                Ok(None) => {}
                Err(error) => on_log(format!(
                    "categorize skipped {}: {error}",
                    entry.path.as_str()
                )),
            }
        }
        if run_describe
            && matches!(entry.family, FileFamily::Image | FileFamily::RawImage)
            && let Some(slot) = vision
        {
            if let Err(message) = ensure_loaded(
                &mut llm,
                &mut loaded,
                slot,
                models,
                &storage_dir,
                &mut on_log,
            ) {
                on_log(format!(
                    "Vision load failed ({message}); image description skipped."
                ));
                continue;
            }
            match llm.describe(&snapshot.root, entry, prior) {
                Ok(Some(evidence)) => {
                    on_log(format!("described {}", entry.path.as_str()));
                    bags.push(evidence);
                }
                Ok(None) => {}
                Err(error) => on_log(format!("describe skipped {}: {error}", entry.path.as_str())),
            }
        }
    }

    snapshot.evidence.extend(bags);
    if loaded.is_some() {
        let _ = llm.unload();
    }
    let _ = llm.shutdown();
}

fn should_categorize(
    entry: &ObservedEntry,
    categorize_on: bool,
    document_only: bool,
    analyze_documents: bool,
) -> bool {
    if categorize_on {
        return true;
    }
    document_only
        && analyze_documents
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
        llm.unload().map_err(load_error)?;
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
        .map_err(load_error)?;
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

fn load_error(error: WorkerClientError) -> String {
    error.to_string()
}

fn prior_evidence(snapshot: &WorkspaceSnapshot, entry: &ObservedEntry) -> Vec<Evidence> {
    snapshot.evidence_for(entry.id).cloned().collect()
}

fn log_category(on_log: &mut impl FnMut(String), entry: &ObservedEntry, evidence: &Evidence) {
    let label = evidence.fact(keys::CATEGORY).unwrap_or("unlabeled");
    on_log(format!("categorized {} → {label}", entry.path.as_str()));
}
