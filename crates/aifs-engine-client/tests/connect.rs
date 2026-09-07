//! End-to-end tests of [`EngineClient`] against the `aifs-engine` binary.

use aifs_engine_client::{ClientError, EngineClient, discover_engine_binary};
use aifs_protocol::ScanOptions;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

fn engine_bin() -> std::path::PathBuf {
    if let Ok(path) = discover_engine_binary() {
        return path;
    }
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "aifs-engine-bin"])
        .current_dir(&workspace)
        .status()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(status.success(), "cargo build -p aifs-engine-bin failed");
    discover_engine_binary()
        .unwrap_or_else(|error| panic!("aifs-engine must be built for client tests: {error}"))
}

fn scan_options() -> ScanOptions {
    ScanOptions {
        extract_metadata: false,
        ..ScanOptions::default()
    }
}

#[test]
fn connect_hello_then_scan_returns_snapshot() {
    let engine = engine_bin();
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|error| panic!("{error}"));
    let client =
        EngineClient::connect(&engine, "client-hello").unwrap_or_else(|error| panic!("{error}"));
    let snapshot = client
        .scan(dir.path(), scan_options(), None)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.path.as_str() == "note.txt")
    );
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn spawn_scan_without_hello_fails() {
    let engine = engine_bin();
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    fs::write(dir.path().join("note.txt"), b"hi").unwrap_or_else(|error| panic!("{error}"));
    let client = EngineClient::spawn(&engine).unwrap_or_else(|error| panic!("{error}"));
    let error = match client.scan(dir.path(), scan_options(), None) {
        Err(error) => error,
        Ok(_) => panic!("scan before hello must fail"),
    };
    match error {
        ClientError::Engine { message, .. } => {
            assert!(message.to_ascii_lowercase().contains("hello"), "{message}");
        }
        other => panic!("expected engine hello error, got {other}"),
    }
}

#[test]
fn scan_maps_cancelled_event_to_client_error() {
    let engine = engine_bin();
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    for index in 0..400 {
        fs::write(dir.path().join(format!("file-{index}.txt")), b"x")
            .unwrap_or_else(|error| panic!("{error}"));
    }
    let client = Arc::new(
        EngineClient::connect(&engine, "client-cancel").unwrap_or_else(|error| panic!("{error}")),
    );
    let canceller = Arc::clone(&client);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let canceller_thread = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            let _ = canceller.cancel_in_flight();
            thread::sleep(Duration::from_millis(5));
        }
    });
    let error = match client.scan(dir.path(), scan_options(), None) {
        Err(error) => error,
        Ok(_) => panic!("cancelled scan must fail"),
    };
    stop.store(true, Ordering::Relaxed);
    let _ = canceller_thread.join();
    assert!(
        matches!(error, ClientError::Cancelled),
        "expected Cancelled, got {error}"
    );
    let snapshot = client
        .scan(dir.path(), scan_options(), None)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        snapshot.entries.len() >= 400,
        "follow-up scan after cancel should complete, got {}",
        snapshot.entries.len()
    );
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
}
