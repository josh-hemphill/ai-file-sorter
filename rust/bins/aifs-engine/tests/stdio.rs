//! End-to-end stdio tests against the `aifs-engine` binary.

use aifs_engine_client::EngineClient;
use aifs_protocol::{ProposalPolicy, ScanOptions};
use std::fs;

#[test]
fn hello_and_scan_over_stdio() {
    let engine = env!("CARGO_BIN_EXE_aifs-engine");
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("readme.txt"), b"hello").unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("photo.CR2"), b"raw").unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("photo.jpg"), b"jpg").unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("photo.xmp"), b"<xmp/>").unwrap_or_else(|e| panic!("{e}"));

    let mut client = EngineClient::connect(engine, "stdio-test").unwrap_or_else(|e| panic!("{e}"));
    let snapshot = client
        .scan(
            dir.path(),
            ScanOptions {
                extract_metadata: false,
                ..ScanOptions::default()
            },
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));

    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.path.as_str() == "readme.txt")
    );
    assert!(
        snapshot
            .bundles
            .iter()
            .any(|bundle| bundle.members.len() >= 2),
        "expected a sidecar bundle, got {:?}",
        snapshot.bundles
    );
    client.shutdown().unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn chat_over_stdio_patches_a_child_revision() {
    let engine = env!("CARGO_BIN_EXE_aifs-engine");
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("show.mp3"), b"id3").unwrap_or_else(|e| panic!("{e}"));

    let mut client = EngineClient::connect(engine, "stdio-chat").unwrap_or_else(|e| panic!("{e}"));
    let snapshot = client
        .scan(
            dir.path(),
            ScanOptions {
                extract_metadata: false,
                ..ScanOptions::default()
            },
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let revision = client
        .propose(snapshot.session, ProposalPolicy::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let reply = client
        .chat(
            snapshot.session,
            revision.id,
            "Move podcasts away from music, but keep seasons shallow.",
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(reply.message.contains("Podcasts"), "{}", reply.message);
    let next = reply.revision.unwrap_or_else(|| panic!("child revision"));
    assert_eq!(next.parent, Some(revision.id));
    assert!(
        next.placements
            .values()
            .any(|placement| { placement.destination.as_str().starts_with("Podcasts/") })
    );
    client.shutdown().unwrap_or_else(|e| panic!("{e}"));
}
