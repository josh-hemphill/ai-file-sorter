//! Supervised evidence extraction. Crash-prone work belongs in worker processes;
//! the engine falls back to in-process Rust readers when a worker binary is missing.

use crate::cancel::WorkStatus;
use aifs_domain::{EntryKind, Evidence, FileFamily, ObservedEntry, WorkspaceSnapshot};
use aifs_protocol::worker::WorkerKind;
use aifs_worker_client::WorkerClient;

/// Spawns whatever worker binaries are installed and fills `snapshot.evidence`.
pub fn extract_into_supervised(
    snapshot: &mut WorkspaceSnapshot,
    mut on_progress: impl FnMut(u64, u64, &str),
    mut should_continue: impl FnMut() -> bool,
) -> WorkStatus {
    let mut media = WorkerClient::try_connect(WorkerKind::Media);
    let mut document = WorkerClient::try_connect(WorkerKind::Document);
    let mut vision = WorkerClient::try_connect(WorkerKind::Vision);
    let root = snapshot.root.clone();
    let total = snapshot.entries.len() as u64;
    let mut bags = Vec::new();
    for (index, entry) in snapshot.entries.iter().enumerate() {
        if !should_continue() {
            snapshot.evidence.extend(bags);
            shutdown_extract_workers(media, document, vision);
            return WorkStatus::Cancelled;
        }
        on_progress(index as u64 + 1, total, entry.path.as_str());
        if let Some(evidence) = extract_one(
            &root,
            entry,
            media.as_mut(),
            document.as_mut(),
            vision.as_mut(),
        ) {
            bags.push(evidence);
        }
    }
    snapshot.evidence.extend(bags);
    shutdown_extract_workers(media, document, vision);
    WorkStatus::Completed
}

fn shutdown_extract_workers(
    media: Option<WorkerClient>,
    document: Option<WorkerClient>,
    vision: Option<WorkerClient>,
) {
    if let Some(mut worker) = media {
        let _ = worker.shutdown();
    }
    if let Some(mut worker) = document {
        let _ = worker.shutdown();
    }
    if let Some(mut worker) = vision {
        let _ = worker.shutdown();
    }
}

fn extract_one(
    root: &std::path::Path,
    entry: &ObservedEntry,
    media: Option<&mut WorkerClient>,
    document: Option<&mut WorkerClient>,
    vision: Option<&mut WorkerClient>,
) -> Option<Evidence> {
    if entry.kind != EntryKind::File {
        return None;
    }
    match entry.family {
        FileFamily::Audio | FileFamily::Video => {
            if let Some(worker) = media
                && let Ok(evidence) = worker.extract(root, entry)
            {
                return evidence;
            }
            aifs_extractors::extract_entry(root, entry)
        }
        FileFamily::Document
        | FileFamily::Spreadsheet
        | FileFamily::Presentation
        | FileFamily::Ebook => {
            if let Some(worker) = document
                && let Ok(evidence) = worker.extract(root, entry)
            {
                return evidence;
            }
            aifs_extractors::extract_document_entry(root, entry)
        }
        FileFamily::Image | FileFamily::RawImage => {
            if let Some(worker) = vision
                && let Ok(evidence) = worker.extract(root, entry)
            {
                return evidence;
            }
            aifs_extractors::extract_image_entry(root, entry)
        }
        _ => None,
    }
}

/// Capability flags for workers whose binaries are discoverable.
pub fn worker_capabilities() -> Vec<String> {
    [
        WorkerKind::Media,
        WorkerKind::Document,
        WorkerKind::Vision,
        WorkerKind::Llm,
    ]
    .into_iter()
    .filter(|kind| aifs_worker_client::discover_worker_binary(*kind).is_ok())
    .map(|kind| kind.binary_stem().replace("aifs-", "").replace('-', "_"))
    .collect()
}
