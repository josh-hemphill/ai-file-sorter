//! Session-lived LLM analysis after deterministic extract.

use crate::cancel::WorkStatus;
use crate::checkpoint::{CHECKPOINT_EVERY, has_category_evidence, has_description_evidence};
use aifs_domain::{
    EntryKind, Evidence, FileFamily, ObservedEntry, WorkspaceSnapshot, evidence::keys,
    looks_like_screenshot,
};
use aifs_protocol::worker::WorkerKind;
use aifs_protocol::{
    AppSettings, FolderStyle, ModelBackend, ModelInventory, ModelSlot, sanitize_hosted_text,
};
use aifs_worker_client::{WorkerClient, WorkerClientError};

const ANALYSIS_STOPPED_LOG: &str = "analysis stopped";

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
    let run_categorize = categorize.is_some();
    let run_describe = settings.analyze_images && vision.is_some();
    let run_document = settings.analyze_documents && document.is_some();
    if !run_categorize && !run_describe && !run_document {
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

    let storage_dir = crate::resolved_models_dir(&models.storage_dir)
        .display()
        .to_string();
    let mut loaded: Option<String> = None;
    let allowed_categories = settings.policy.whitelist.main.clone();
    let style = settings.policy.style;

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
                    if entry.family == FileFamily::RawImage {
                        on_notice(AnalyzeNotice::Log(raw_describe_log(entry.path.as_str())));
                    }
                    match llm.describe_while(
                        &snapshot.root,
                        &entry,
                        prior.clone(),
                        &mut should_continue,
                    ) {
                        Ok(Some(evidence)) => {
                            on_notice(AnalyzeNotice::Log(format!(
                                "described {}",
                                entry.path.as_str()
                            )));
                            let mut combined = prior;
                            combined.push(evidence.clone());
                            maybe_log_screenshot(&mut on_notice, &entry, &combined);
                            bags.push(evidence);
                            if bags.len() >= CHECKPOINT_EVERY {
                                snapshot.evidence.extend(std::mem::take(&mut bags));
                                if !on_checkpoint(snapshot) {
                                    shutdown_llm(&mut llm, loaded.is_some());
                                    return WorkStatus::PersistFailed;
                                }
                            }
                        }
                        Ok(None) => {
                            maybe_log_screenshot(&mut on_notice, &entry, &prior);
                        }
                        Err(error) if error.is_cancelled() => {
                            return stop_analysis(
                                snapshot,
                                bags,
                                &mut llm,
                                loaded.is_some(),
                                &mut on_notice,
                            );
                        }
                        Err(error) => {
                            maybe_log_screenshot(&mut on_notice, &entry, &prior);
                            on_notice(AnalyzeNotice::Log(format!(
                                "describe skipped {}: {}",
                                entry.path.as_str(),
                                sanitize_hosted_text(&error.to_string(), slot.api_key.as_deref())
                            )));
                        }
                    }
                }
                snapshot.evidence.extend(bags);
            }
            Err(message) => on_notice(AnalyzeNotice::Log(format!(
                "Vision load failed ({message}); image description skipped."
            ))),
        }
    }

    if run_categorize && let Some(slot) = categorize {
        let targets: Vec<ObservedEntry> = snapshot
            .entries
            .iter()
            .filter(|entry| should_include_in_categorize(entry, run_document))
            .cloned()
            .collect();
        match categorize_targets(CategorizePass {
            llm: &mut llm,
            loaded: &mut loaded,
            snapshot,
            models,
            storage_dir: &storage_dir,
            slot,
            targets,
            allowed_categories: &allowed_categories,
            style,
            log_screenshots: !run_describe,
            load_failed: "Categorize load failed",
            on_notice: &mut on_notice,
            on_checkpoint: &mut on_checkpoint,
            should_continue: &mut should_continue,
        }) {
            WorkStatus::Completed => {}
            other => return other,
        }
    }

    if run_document && let Some(slot) = document {
        let targets: Vec<ObservedEntry> = snapshot
            .entries
            .iter()
            .filter(|entry| should_include_in_document(entry))
            .cloned()
            .collect();
        match categorize_targets(CategorizePass {
            llm: &mut llm,
            loaded: &mut loaded,
            snapshot,
            models,
            storage_dir: &storage_dir,
            slot,
            targets,
            allowed_categories: &allowed_categories,
            style,
            log_screenshots: !run_describe,
            load_failed: "Document load failed",
            on_notice: &mut on_notice,
            on_checkpoint: &mut on_checkpoint,
            should_continue: &mut should_continue,
        }) {
            WorkStatus::Completed => {}
            other => return other,
        }
    }

    shutdown_llm(&mut llm, loaded.is_some());
    WorkStatus::Completed
}

struct CategorizePass<'a, Notice, Checkpoint, Continue>
where
    Notice: FnMut(AnalyzeNotice<'_>),
    Checkpoint: FnMut(&WorkspaceSnapshot) -> bool,
    Continue: FnMut() -> bool,
{
    llm: &'a mut WorkerClient,
    loaded: &'a mut Option<String>,
    snapshot: &'a mut WorkspaceSnapshot,
    models: &'a ModelInventory,
    storage_dir: &'a str,
    slot: &'a ModelSlot,
    targets: Vec<ObservedEntry>,
    allowed_categories: &'a [String],
    style: FolderStyle,
    log_screenshots: bool,
    load_failed: &'a str,
    on_notice: &'a mut Notice,
    on_checkpoint: &'a mut Checkpoint,
    should_continue: &'a mut Continue,
}

fn categorize_targets<Notice, Checkpoint, Continue>(
    pass: CategorizePass<'_, Notice, Checkpoint, Continue>,
) -> WorkStatus
where
    Notice: FnMut(AnalyzeNotice<'_>),
    Checkpoint: FnMut(&WorkspaceSnapshot) -> bool,
    Continue: FnMut() -> bool,
{
    let CategorizePass {
        llm,
        loaded,
        snapshot,
        models,
        storage_dir,
        slot,
        targets,
        allowed_categories,
        style,
        log_screenshots,
        load_failed,
        on_notice,
        on_checkpoint,
        should_continue,
    } = pass;
    match ensure_loaded(llm, loaded, slot, models, storage_dir, &mut |message| {
        on_notice(AnalyzeNotice::Log(message));
    }) {
        Ok(()) => {
            let total = targets.len() as u64;
            let mut bags = Vec::new();
            for (index, entry) in targets.into_iter().enumerate() {
                if !should_continue() {
                    snapshot.evidence.extend(bags);
                    shutdown_llm(llm, loaded.is_some());
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
                if log_screenshots {
                    maybe_log_screenshot(on_notice, &entry, &prior);
                }
                match llm.categorize_while(
                    &snapshot.root,
                    &entry,
                    prior,
                    allowed_categories.to_vec(),
                    style,
                    &mut *should_continue,
                ) {
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
                                shutdown_llm(llm, loaded.is_some());
                                return WorkStatus::PersistFailed;
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) if error.is_cancelled() => {
                        return stop_analysis(snapshot, bags, llm, loaded.is_some(), on_notice);
                    }
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
            "{load_failed} ({message}); folder labels stay heuristic."
        ))),
    }
    WorkStatus::Completed
}

fn shutdown_llm(llm: &mut WorkerClient, loaded: bool) {
    if loaded {
        let _ = llm.unload();
    }
    let _ = llm.shutdown();
}

fn stop_analysis(
    snapshot: &mut WorkspaceSnapshot,
    bags: Vec<Evidence>,
    llm: &mut WorkerClient,
    loaded: bool,
    on_notice: &mut impl FnMut(AnalyzeNotice<'_>),
) -> WorkStatus {
    on_notice(AnalyzeNotice::Log(ANALYSIS_STOPPED_LOG.to_owned()));
    snapshot.evidence.extend(bags);
    shutdown_llm(llm, loaded);
    WorkStatus::Cancelled
}

fn should_include_in_categorize(entry: &ObservedEntry, run_document: bool) -> bool {
    entry.kind == EntryKind::File && !(run_document && entry.family.is_document_like())
}

fn should_include_in_document(entry: &ObservedEntry) -> bool {
    entry.kind == EntryKind::File && entry.family.is_document_like()
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
        .load_with(
            slot.backend.clone(),
            models.gpu_preference.clone(),
            None,
            slot.api_key.clone(),
            storage_dir,
            || {
                on_log(format!(
                    "loading {} for {}",
                    backend_key(&slot.backend),
                    slot.id
                ));
            },
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

fn maybe_log_screenshot(
    on_notice: &mut impl FnMut(AnalyzeNotice<'_>),
    entry: &ObservedEntry,
    evidence: &[Evidence],
) {
    if looks_like_screenshot(entry, evidence) {
        on_notice(AnalyzeNotice::Log(screenshot_log(entry.path.as_str())));
    }
}

/// Scan line for screenshot/UI captures.
pub(crate) fn screenshot_log(path: &str) -> String {
    format!("{path} · screenshot · Screenshots/UI")
}

/// Scan line for RAW describe: EXIF and filename, never pixels.
pub(crate) fn raw_describe_log(path: &str) -> String {
    format!("RAW {path}: describe uses EXIF and filename, not pixels")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_describe_log_mentions_exif_and_filename_not_pixels() {
        assert_eq!(
            raw_describe_log("DSC_0001.CR2"),
            "RAW DSC_0001.CR2: describe uses EXIF and filename, not pixels"
        );
    }

    #[test]
    fn screenshot_log_uses_golden_path_copy() {
        assert_eq!(
            screenshot_log("IMG_1042.jpg"),
            "IMG_1042.jpg · screenshot · Screenshots/UI"
        );
        let entry = ObservedEntry {
            id: aifs_domain::AssetId::new(),
            path: aifs_domain::RelativePath::parse("Screenshot.png")
                .unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::File,
            family: FileFamily::Image,
            identity: aifs_domain::FileIdentity::default(),
            is_hidden: false,
            lock: aifs_domain::LockState::Readable,
        };
        assert!(looks_like_screenshot(&entry, &[]));
    }

    fn file(path: &str, family: FileFamily) -> ObservedEntry {
        ObservedEntry {
            id: aifs_domain::AssetId::new(),
            path: aifs_domain::RelativePath::parse(path).unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::File,
            family,
            identity: aifs_domain::FileIdentity::default(),
            is_hidden: false,
            lock: aifs_domain::LockState::Readable,
        }
    }

    #[test]
    fn document_pass_excludes_office_files_from_categorize() {
        let note = file("note.txt", FileFamily::Document);
        let sheet = file("sheet.xlsx", FileFamily::Spreadsheet);
        let shot = file("shot.jpg", FileFamily::Image);
        let song = file("clip.mp3", FileFamily::Audio);
        assert!(!should_include_in_categorize(&note, true));
        assert!(!should_include_in_categorize(&sheet, true));
        assert!(should_include_in_categorize(&shot, true));
        assert!(should_include_in_categorize(&song, true));
        assert!(should_include_in_categorize(&note, false));
        assert!(should_include_in_document(&note));
        assert!(should_include_in_document(&sheet));
        assert!(!should_include_in_document(&shot));
        assert!(!should_include_in_document(&song));
    }
}
