//! Isolated document worker. Stub: records a detector note until PDF/Office extractors land.

use aifs_domain::{Confidence, EntryKind, Evidence, EvidenceSource, FileFamily};
use aifs_protocol::worker::WorkerKind;

fn main() {
    if let Err(error) =
        aifs_worker_runtime::run_stdio(WorkerKind::Document, &["stub"], |_, entry| {
            if entry.kind != EntryKind::File {
                return Ok(None);
            }
            if !matches!(
                entry.family,
                FileFamily::Document
                    | FileFamily::Spreadsheet
                    | FileFamily::Presentation
                    | FileFamily::Ebook
            ) {
                return Ok(None);
            }
            Ok(Some(
                Evidence::new(
                    entry.id,
                    EvidenceSource::Detector {
                        name: "document-stub".into(),
                    },
                    Confidence::new(0.1),
                )
                .with_fact(
                    aifs_domain::evidence::keys::DESCRIPTION,
                    "document worker stub; PDF/Office extraction not enabled",
                ),
            ))
        })
    {
        eprintln!("aifs-worker-document: {error}");
        std::process::exit(1);
    }
}
