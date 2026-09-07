//! End-to-end stdio tests against the `aifs-engine` binary.

use aifs_engine_client::EngineClient;
use aifs_protocol::{Command, Event, ProposalPolicy, ScanOptions};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

fn connect_isolated(name: &str) -> (tempfile::TempDir, EngineClient) {
    let store_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let store = store_dir.path().join("engine.sqlite");
    let client = EngineClient::connect_with_store(env!("CARGO_BIN_EXE_aifs-engine"), name, &store)
        .unwrap_or_else(|e| panic!("{e}"));
    (store_dir, client)
}

#[test]
fn hello_and_scan_over_stdio() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("readme.txt"), b"hello").unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("photo.CR2"), b"raw").unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("photo.jpg"), b"jpg").unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("photo.xmp"), b"<xmp/>").unwrap_or_else(|e| panic!("{e}"));
    let (_store, client) = connect_isolated("stdio-test");
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
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("show.mp3"), b"id3").unwrap_or_else(|e| panic!("{e}"));
    let (_store, client) = connect_isolated("stdio-chat");
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

#[test]
fn cancel_in_flight_scan_over_stdio() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    for index in 0..400 {
        fs::write(dir.path().join(format!("file-{index}.txt")), b"x")
            .unwrap_or_else(|e| panic!("{e}"));
    }

    let (_store, client) = connect_isolated("stdio-cancel");
    let client = Arc::new(client);
    let canceller = Arc::clone(&client);
    let started = AtomicBool::new(false);
    let envelopes = client
        .request_with_events(
            Command::Scan {
                root: dir.path().to_path_buf(),
                options: ScanOptions {
                    extract_metadata: false,
                    ..ScanOptions::default()
                },
                session: None,
            },
            |envelope| {
                if !matches!(envelope.event, Event::Progress { .. }) {
                    return;
                }
                if started.swap(true, Ordering::Relaxed) {
                    return;
                }
                canceller
                    .cancel_in_flight()
                    .unwrap_or_else(|e| panic!("{e}"));
                let thread_client = Arc::clone(&canceller);
                thread::spawn(move || {
                    let _ = thread_client.cancel_in_flight();
                });
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));

    assert!(
        envelopes
            .iter()
            .any(|envelope| matches!(envelope.event, Event::Cancelled)),
        "expected cancelled, got {envelopes:?}"
    );
    assert!(
        !envelopes
            .iter()
            .any(|envelope| matches!(envelope.event, Event::ScanCompleted { .. })),
        "cancelled scan must not complete a snapshot"
    );

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
        snapshot.entries.len() >= 400,
        "follow-up scan after cancel should complete, got {} entries",
        snapshot.entries.len()
    );
    client.shutdown().unwrap_or_else(|e| panic!("{e}"));
}
