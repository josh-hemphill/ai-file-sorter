//! End-to-end stdio tests against the `aifs-engine` binary.

use aifs_engine_client::EngineClient;
use aifs_protocol::ScanOptions;
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

    assert!(snapshot
        .entries
        .iter()
        .any(|entry| entry.path.as_str() == "readme.txt"));
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
