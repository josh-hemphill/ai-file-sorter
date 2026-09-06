//! Isolated vision worker. Stub: records that a still exists until EXIF/OCR land.

use aifs_domain::{Confidence, EntryKind, Evidence, EvidenceSource, FileFamily};
use aifs_protocol::worker::WorkerKind;

fn main() {
    if let Err(error) = aifs_worker_runtime::run_stdio(WorkerKind::Vision, &["stub"], |_, entry| {
        if entry.kind != EntryKind::File {
            return Ok(None);
        }
        if !matches!(entry.family, FileFamily::Image | FileFamily::RawImage) {
            return Ok(None);
        }
        Ok(Some(
            Evidence::new(
                entry.id,
                EvidenceSource::Detector {
                    name: "vision-stub".into(),
                },
                Confidence::new(0.1),
            )
            .with_fact(
                aifs_domain::evidence::keys::DESCRIPTION,
                "vision worker stub; EXIF/OCR not enabled",
            ),
        ))
    }) {
        eprintln!("aifs-worker-vision: {error}");
        std::process::exit(1);
    }
}
