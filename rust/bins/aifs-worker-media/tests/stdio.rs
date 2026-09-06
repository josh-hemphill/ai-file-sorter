//! Stdio tests for the media worker binary.

use aifs_domain::{
    AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelativePath,
};
use aifs_extractors::write_id3v23_fixture;
use aifs_protocol::worker::WorkerKind;
use aifs_worker_client::WorkerClient;
use std::fs;

#[test]
fn media_worker_extracts_id3_tags() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-media");
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    write_id3v23_fixture(
        &dir.path().join("show.mp3"),
        "Night Drive",
        "Ada",
        "After Hours",
        "2019",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|e| panic!("{e}"));

    let mut client =
        WorkerClient::connect(WorkerKind::Media, worker).unwrap_or_else(|e| panic!("{e}"));
    assert!(client.capabilities().iter().any(|c| c == "media_tags"));
    let audio = ObservedEntry {
        id: AssetId::new(),
        path: RelativePath::parse("show.mp3").unwrap_or_else(|e| panic!("{e}")),
        kind: EntryKind::File,
        family: FileFamily::Audio,
        identity: FileIdentity::default(),
        is_hidden: false,
        lock: LockState::Readable,
    };
    let evidence = client
        .extract(dir.path(), &audio)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("expected tags"));
    assert_eq!(
        evidence.fact(aifs_domain::evidence::keys::MEDIA_TITLE),
        Some("Night Drive")
    );
    let text = ObservedEntry {
        id: AssetId::new(),
        path: RelativePath::parse("note.txt").unwrap_or_else(|e| panic!("{e}")),
        kind: EntryKind::File,
        family: FileFamily::Document,
        identity: FileIdentity::default(),
        is_hidden: false,
        lock: LockState::Readable,
    };
    assert!(client
        .extract(dir.path(), &text)
        .unwrap_or_else(|e| panic!("{e}"))
        .is_none());
    client.shutdown().unwrap_or_else(|e| panic!("{e}"));
}
