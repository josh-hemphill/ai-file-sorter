//! Stdio tests for the document worker stub.

use aifs_domain::{
    AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelativePath,
};
use aifs_protocol::worker::WorkerKind;
use aifs_worker_client::WorkerClient;

#[test]
fn document_stub_returns_detector_evidence() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-document");
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.path().join("brief.pdf"), b"%PDF-stub").unwrap_or_else(|e| panic!("{e}"));
    let mut client =
        WorkerClient::connect(WorkerKind::Document, worker).unwrap_or_else(|e| panic!("{e}"));
    assert!(client.capabilities().iter().any(|c| c == "stub"));
    let entry = ObservedEntry {
        id: AssetId::new(),
        path: RelativePath::parse("brief.pdf").unwrap_or_else(|e| panic!("{e}")),
        kind: EntryKind::File,
        family: FileFamily::Document,
        identity: FileIdentity::default(),
        is_hidden: false,
        lock: LockState::Readable,
    };
    let evidence = client
        .extract(dir.path(), &entry)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("stub evidence"));
    assert!(
        evidence
            .fact(aifs_domain::evidence::keys::DESCRIPTION)
            .is_some_and(|text| text.contains("stub"))
    );
    client.shutdown().unwrap_or_else(|e| panic!("{e}"));
}
