//! Session-lived LLM analysis after deterministic extract.

use crate::cancel::WorkStatus;
use crate::checkpoint::{
    CHECKPOINT_EVERY, has_category_evidence, has_description_evidence, has_grouping_evidence,
};
use aifs_domain::{
    BundleConstraint, Confidence, EntryKind, Evidence, EvidenceSource, FileFamily, ObservedEntry,
    WorkspaceSnapshot, evidence::keys, looks_like_screenshot,
};
use aifs_protocol::{
    AppSettings, FolderStyle, ModelBackend, ModelInventory, ModelSlot, sanitize_hosted_text,
};
use aifs_relationships::apply_grouping_evidence;
use aifs_worker_client::{WorkerClient, WorkerClientError};

const ANALYSIS_STOPPED_LOG: &str = "analysis stopped";
const DESCRIBE_LOG_CHARS: usize = 96;
const GROUPING_CHILD_LIMIT: usize = 24;
const GROUPING_STEM_LIMIT: usize = 12;

/// Scan line when a layout unit is not described or categorized yet.
pub(crate) fn deferred_unit_log(root: &str, files: usize) -> String {
    format!(
        "{} · deferred · {files} files stay in this folder as a unit",
        aifs_domain::decode_oem_path(root)
    )
}

/// Scan line for a finished image caption.
pub(crate) fn described_log(path: &str, evidence: &Evidence) -> String {
    match evidence.fact(keys::DESCRIPTION) {
        Some(text) => format!(
            "described {} · {}",
            aifs_domain::decode_oem_path(path),
            truncate_log(text, DESCRIBE_LOG_CHARS)
        ),
        None => format!("described {}", aifs_domain::decode_oem_path(path)),
    }
}

fn truncate_log(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let mut truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

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

    let mut llm = match WorkerClient::connect_llm(&models.gpu_preference) {
        Ok(client) => {
            on_notice(AnalyzeNotice::Log(format!(
                "LLM worker started [{}]",
                client.binary().display()
            )));
            client
        }
        Err(WorkerClientError::NotFound(_)) => {
            on_notice(AnalyzeNotice::Log(
                "LLM worker is not installed; scan continues with extract and heuristics."
                    .to_owned(),
            ));
            return WorkStatus::Completed;
        }
        Err(error) => {
            on_notice(AnalyzeNotice::Log(format!(
                "LLM worker failed to start ({error}); scan continues with extract and heuristics."
            )));
            return WorkStatus::Completed;
        }
    };

    let storage_dir = crate::resolved_models_dir(&models.storage_dir)
        .display()
        .to_string();
    let mut loaded: Option<String> = None;
    let allowed_categories = settings.policy.whitelist.main.clone();
    let style = settings.policy.style;

    if run_categorize && let Some(slot) = categorize {
        let targets = grouping_targets(snapshot);
        match group_directories(GroupPass {
            llm: &mut llm,
            loaded: &mut loaded,
            snapshot,
            models,
            storage_dir: &storage_dir,
            slot,
            targets,
            on_notice: &mut on_notice,
            on_checkpoint: &mut on_checkpoint,
            should_continue: &mut should_continue,
        }) {
            WorkStatus::Completed => {}
            other => return other,
        }
        apply_grouping_evidence(snapshot);
    }

    log_deferred_units(snapshot, &mut on_notice);

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
                            && !snapshot.defers_content_analysis(entry)
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
                            on_notice(AnalyzeNotice::Log(described_log(
                                entry.path.as_str(),
                                &evidence,
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
            .filter(|entry| {
                should_include_in_categorize(entry, run_document)
                    && !snapshot.defers_content_analysis(entry)
            })
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
            .filter(|entry| {
                should_include_in_document(entry) && !snapshot.defers_content_analysis(entry)
            })
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

struct GroupPass<'a, Notice, Checkpoint, Continue>
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
    on_notice: &'a mut Notice,
    on_checkpoint: &'a mut Checkpoint,
    should_continue: &'a mut Continue,
}

fn group_directories<Notice, Checkpoint, Continue>(
    pass: GroupPass<'_, Notice, Checkpoint, Continue>,
) -> WorkStatus
where
    Notice: FnMut(AnalyzeNotice<'_>),
    Checkpoint: FnMut(&WorkspaceSnapshot) -> bool,
    Continue: FnMut() -> bool,
{
    let GroupPass {
        llm,
        loaded,
        snapshot,
        models,
        storage_dir,
        slot,
        targets,
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
                if has_grouping_evidence(snapshot, &entry) {
                    continue;
                }
                let prior = grouping_context(snapshot, &entry);
                match llm.categorize_while(
                    &snapshot.root,
                    &entry,
                    prior,
                    Vec::new(),
                    FolderStyle::Consistent,
                    &mut *should_continue,
                ) {
                    Ok(Some(evidence)) => {
                        on_notice(AnalyzeNotice::Log(grouped_log(
                            entry.path.as_str(),
                            &evidence,
                        )));
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
                        "grouping skipped {}: {}",
                        entry.path.as_str(),
                        sanitize_hosted_text(&error.to_string(), slot.api_key.as_deref())
                    ))),
                }
            }
            snapshot.evidence.extend(bags);
        }
        Err(message) => on_notice(AnalyzeNotice::Log(format!(
            "Grouping load failed ({message}); folder roles stay heuristic."
        ))),
    }
    WorkStatus::Completed
}

fn grouping_targets(snapshot: &WorkspaceSnapshot) -> Vec<ObservedEntry> {
    snapshot
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == EntryKind::Directory
                && snapshot
                    .directory_roles
                    .iter()
                    .all(|role| role.root != entry.path)
                && snapshot.covering_layout_root(&entry.path).is_none()
        })
        .cloned()
        .collect()
}

fn grouping_context(snapshot: &WorkspaceSnapshot, dir: &ObservedEntry) -> Vec<Evidence> {
    let children: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == EntryKind::Directory && entry.path.parent().as_ref() == Some(&dir.path)
        })
        .map(|entry| entry.path.file_name().to_owned())
        .take(GROUPING_CHILD_LIMIT)
        .collect();
    let stems: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == EntryKind::File && entry.path.parent().as_ref() == Some(&dir.path)
        })
        .map(|entry| entry.stem().to_owned())
        .take(GROUPING_STEM_LIMIT)
        .collect();
    let mut bag = Evidence::new(dir.id, EvidenceSource::Filesystem, Confidence::CERTAIN);
    if !children.is_empty() {
        bag = bag.with_fact(keys::DIRECTORY_CHILDREN, children.join(", "));
    }
    if !stems.is_empty() {
        bag = bag.with_fact(keys::DIRECTORY_SAMPLE_STEMS, stems.join(", "));
    }
    if bag.is_empty() {
        Vec::new()
    } else {
        vec![bag]
    }
}

fn grouped_log(path: &str, evidence: &Evidence) -> String {
    let label = evidence
        .fact(keys::DIRECTORY_GROUPING)
        .unwrap_or("unlabeled");
    format!("grouped {} → {label}", aifs_domain::decode_oem_path(path))
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

fn log_deferred_units(snapshot: &WorkspaceSnapshot, on_notice: &mut impl FnMut(AnalyzeNotice<'_>)) {
    for bundle in &snapshot.bundles {
        let BundleConstraint::PreserveLayout { root } = &bundle.constraint else {
            continue;
        };
        let files = snapshot
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File && entry.path.starts_with(root))
            .count();
        if files == 0 {
            continue;
        }
        on_notice(AnalyzeNotice::Log(deferred_unit_log(root.as_str(), files)));
    }
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

    #[test]
    fn described_log_includes_a_truncated_caption() {
        let evidence = Evidence::new(
            aifs_domain::AssetId::new(),
            aifs_domain::EvidenceSource::LocalModel {
                model: "stub".into(),
            },
            aifs_domain::Confidence::new(0.5),
        )
        .with_fact(
            keys::DESCRIPTION,
            "a red car parked on a cobblestone street",
        );
        assert_eq!(
            described_log("trip/car.jpg", &evidence),
            "described trip/car.jpg · a red car parked on a cobblestone street"
        );
        let long = Evidence::new(
            aifs_domain::AssetId::new(),
            aifs_domain::EvidenceSource::LocalModel {
                model: "stub".into(),
            },
            aifs_domain::Confidence::new(0.5),
        )
        .with_fact(keys::DESCRIPTION, "x".repeat(120));
        let line = described_log("shot.jpg", &long);
        assert!(line.starts_with("described shot.jpg · "));
        assert!(line.ends_with('…'));
        assert_eq!(
            line.chars().count(),
            "described shot.jpg · ".chars().count() + DESCRIBE_LOG_CHARS
        );
        assert_eq!(
            deferred_unit_log("Pictures", 133),
            "Pictures · deferred · 133 files stay in this folder as a unit"
        );
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

    #[test]
    fn grouping_targets_skip_labeled_and_frozen_folders() {
        let export = ObservedEntry {
            id: aifs_domain::AssetId::new(),
            path: aifs_domain::RelativePath::parse("Export")
                .unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::Directory,
            family: FileFamily::Generic,
            identity: aifs_domain::FileIdentity::default(),
            is_hidden: false,
            lock: aifs_domain::LockState::Readable,
        };
        let photos = ObservedEntry {
            id: aifs_domain::AssetId::new(),
            path: aifs_domain::RelativePath::parse("Photos")
                .unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::Directory,
            family: FileFamily::Generic,
            identity: aifs_domain::FileIdentity::default(),
            is_hidden: false,
            lock: aifs_domain::LockState::Readable,
        };
        let italy = ObservedEntry {
            id: aifs_domain::AssetId::new(),
            path: aifs_domain::RelativePath::parse("Photos/Italy")
                .unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::Directory,
            family: FileFamily::Generic,
            identity: aifs_domain::FileIdentity::default(),
            is_hidden: false,
            lock: aifs_domain::LockState::Readable,
        };
        let mut snapshot = WorkspaceSnapshot::new(
            aifs_domain::SessionId::new(),
            std::path::PathBuf::from("/tmp"),
        );
        snapshot.entries = vec![export.clone(), photos.clone(), italy];
        snapshot
            .directory_roles
            .push(aifs_domain::DirectoryRoleMatch {
                root: photos.path.clone(),
                kind: aifs_domain::DirectoryRoleKind::Library,
                reason: "library".into(),
            });
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::Folder,
            label: "Photos".into(),
            members: vec![photos.id],
            anchor: Some(photos.id),
            constraint: BundleConstraint::PreserveLayout {
                root: photos.path.clone(),
            },
            reason: "library".into(),
        });
        let paths: Vec<_> = grouping_targets(&snapshot)
            .iter()
            .map(|entry| entry.path.as_str().to_owned())
            .collect();
        assert_eq!(paths, vec!["Export".to_owned()]);
        let evidence = Evidence::new(
            export.id,
            EvidenceSource::LocalModel {
                model: "stub".into(),
            },
            Confidence::new(0.4),
        )
        .with_fact(keys::DIRECTORY_GROUPING, "camera_dump");
        assert_eq!(
            grouped_log("Export", &evidence),
            "grouped Export → camera_dump"
        );
        let context = grouping_context(&snapshot, &export);
        assert!(context.is_empty());
    }
}
