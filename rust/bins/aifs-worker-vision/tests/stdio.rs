//! Stdio tests for the vision worker.

use aifs_domain::{
    AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelativePath,
    evidence::keys,
};
use aifs_extractors::write_jpeg_exif_fixture;
use aifs_protocol::worker::WorkerKind;
use aifs_worker_client::WorkerClient;

#[test]
fn vision_worker_extracts_exif() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-vision");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    write_jpeg_exif_fixture(
        dir.path().join("shot.jpg").as_path(),
        "Canon",
        "EOS R5",
        "2021:07:15 09:30:00",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mut client =
        WorkerClient::connect(WorkerKind::Vision, worker).unwrap_or_else(|error| panic!("{error}"));
    assert!(client.capabilities().iter().any(|cap| cap == "exif"));
    let entry = ObservedEntry {
        id: AssetId::new(),
        path: RelativePath::parse("shot.jpg").unwrap_or_else(|error| panic!("{error}")),
        kind: EntryKind::File,
        family: FileFamily::Image,
        identity: FileIdentity::default(),
        is_hidden: false,
        lock: LockState::Readable,
    };
    let evidence = client
        .extract(dir.path(), &entry)
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("exif evidence"));
    assert_eq!(evidence.fact(keys::IMAGE_CAPTURED_ON), Some("2021-07-15"));
    assert_eq!(evidence.fact(keys::IMAGE_CAMERA), Some("Canon EOS R5"));
    assert!(
        !evidence
            .fact(keys::DESCRIPTION)
            .is_some_and(|text| text.contains("stub"))
    );
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
}
