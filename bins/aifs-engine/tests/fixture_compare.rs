//! Compare the engine's heuristic proposal against the inbox-mixed golden fixture.
//!
//! The Qt UI is gone on this fork, so regression is rust-engine vs committed
//! destinations rather than old-engine vs new-engine.

use aifs_engine_client::EngineClient;
use aifs_extractors::write_id3v23_fixture;
use aifs_planner::{ExpectedPlan, diff_plan};
use aifs_protocol::{ProposalPolicy, ScanOptions};
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap_or_else(|e| panic!("{e}"));
    for entry in fs::read_dir(src).unwrap_or_else(|e| panic!("{e}")) {
        let entry = entry.unwrap_or_else(|e| panic!("{e}"));
        let to = dst.join(entry.file_name());
        let meta = entry.metadata().unwrap_or_else(|e| panic!("{e}"));
        if meta.is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), to).unwrap_or_else(|e| panic!("{e}"));
        }
    }
}

#[test]
fn inbox_mixed_proposal_matches_golden_destinations() {
    let engine = env!("CARGO_BIN_EXE_aifs-engine");
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    copy_tree(&fixtures_dir().join("inbox-mixed"), dir.path());
    write_id3v23_fixture(
        &dir.path().join("show.mp3"),
        "Night Drive",
        "Ada",
        "After Hours",
        "2019",
    )
    .unwrap_or_else(|e| panic!("{e}"));

    let expected: ExpectedPlan = serde_json::from_str(
        &fs::read_to_string(fixtures_dir().join("inbox-mixed.expected.json"))
            .unwrap_or_else(|e| panic!("{e}")),
    )
    .unwrap_or_else(|e| panic!("{e}"));

    let mut client =
        EngineClient::connect(engine, "fixture-compare").unwrap_or_else(|e| panic!("{e}"));
    let snapshot = client
        .scan(dir.path(), ScanOptions::default(), None)
        .unwrap_or_else(|e| panic!("{e}"));
    let revision = client
        .propose(snapshot.session, ProposalPolicy::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let diffs = diff_plan(&snapshot, &revision, &expected);
    assert!(
        diffs.is_empty(),
        "inbox-mixed proposal drifted:\n{}",
        diffs.join("\n")
    );
    client.shutdown().unwrap_or_else(|e| panic!("{e}"));
}
