//! CLI tests against the built `aifs` binary (no extra `--` before the subcommand).

use aifs_engine_client::discover_engine_binary;
use aifs_extractors::write_id3v23_fixture;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn aifs_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_aifs"))
}

fn engine_bin() -> PathBuf {
    if let Ok(path) = discover_engine_binary() {
        return path;
    }
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "aifs-engine-bin"])
        .current_dir(&workspace)
        .status()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(status.success(), "cargo build -p aifs-engine-bin failed");
    discover_engine_binary()
        .unwrap_or_else(|error| panic!("aifs-engine must be built for CLI tests: {error}"))
}

fn run_aifs(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Output {
    let store_dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let store = store_dir.path().join("engine.sqlite");
    let output = Command::new(aifs_bin())
        .env("AIFS_STORE", &store)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    if !output.status.success() {
        panic!(
            "aifs failed ({:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap_or_else(|error| panic!("{error}"));
    for entry in fs::read_dir(src).unwrap_or_else(|error| panic!("{error}")) {
        let entry = entry.unwrap_or_else(|error| panic!("{error}"));
        let to = dst.join(entry.file_name());
        let meta = entry.metadata().unwrap_or_else(|error| panic!("{error}"));
        if meta.is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap_or_else(|error| panic!("{error}"));
            pin_copied_mtime(&to);
        }
    }
}

/// Pins copied fixture files to 2021-07-15 so document date suffixes stay stable.
fn pin_copied_mtime(path: &Path) {
    let file = fs::File::options()
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("{error}"));
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_626_307_200_000))
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn cargo_aifs_alias_does_not_need_a_second_dash_dash() {
    let config = include_str!("../../../.cargo/config.toml");
    assert!(
        config.contains("aifs = \"run -p aifs-cli --\""),
        "cargo aifs scan … must not require an extra --; got:\n{config}"
    );
}

#[test]
fn scan_prints_summary_for_a_folder() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    fs::write(dir.path().join("readme.txt"), b"hello").unwrap_or_else(|error| panic!("{error}"));
    let output = run_aifs([
        OsStr::new("--engine"),
        engine_bin().as_os_str(),
        OsStr::new("scan"),
        dir.path().as_os_str(),
        OsStr::new("--no-extract"),
    ]);
    let text = stdout(&output);
    assert!(text.contains("Scanned"), "{text}");
    assert!(text.contains("entries"), "{text}");
    assert!(text.contains("session"), "{text}");
}

#[test]
fn scan_json_dumps_the_snapshot() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    fs::write(dir.path().join("readme.txt"), b"hello").unwrap_or_else(|error| panic!("{error}"));
    let output = run_aifs([
        OsStr::new("--engine"),
        engine_bin().as_os_str(),
        OsStr::new("scan"),
        dir.path().as_os_str(),
        OsStr::new("--json"),
        OsStr::new("--no-extract"),
    ]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", stdout(&output)));
    let entries = value["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("entries: {value}"));
    assert!(
        entries.iter().any(|entry| entry["path"] == "readme.txt"),
        "{value}"
    );
}

#[test]
fn organize_dry_run_does_not_move_files() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source = dir.path().join("readme.txt");
    fs::write(&source, b"hello").unwrap_or_else(|error| panic!("{error}"));
    let output = run_aifs([
        OsStr::new("--engine"),
        engine_bin().as_os_str(),
        OsStr::new("organize"),
        dir.path().as_os_str(),
    ]);
    let text = stdout(&output);
    assert!(text.contains("Previewed"), "{text}");
    assert!(text.contains("dry_run=true"), "{text}");
    assert!(
        source.is_file(),
        "dry-run must leave {} in place",
        source.display()
    );
}

#[test]
fn chat_prints_a_child_revision() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    fs::write(dir.path().join("show.mp3"), b"id3").unwrap_or_else(|error| panic!("{error}"));
    let output = run_aifs([
        OsStr::new("--engine"),
        engine_bin().as_os_str(),
        OsStr::new("chat"),
        dir.path().as_os_str(),
        OsStr::new("Move podcasts away from music, but keep seasons shallow."),
    ]);
    let text = stdout(&output);
    assert!(text.contains("Podcasts"), "{text}");
    assert!(text.contains("revision"), "{text}");
}

#[test]
fn compare_matches_inbox_mixed_golden_destinations() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    copy_tree(&fixtures.join("inbox-mixed"), dir.path());
    write_id3v23_fixture(
        &dir.path().join("show.mp3"),
        "Night Drive",
        "Ada",
        "After Hours",
        "2019",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let output = run_aifs([
        OsStr::new("--engine"),
        engine_bin().as_os_str(),
        OsStr::new("compare"),
        dir.path().as_os_str(),
        fixtures.join("inbox-mixed.expected.json").as_os_str(),
    ]);
    let text = stdout(&output);
    assert!(text.contains("Matched"), "{text}");
    assert!(text.contains("destinations"), "{text}");
}
