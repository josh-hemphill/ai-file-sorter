//! Stdio tests for the document worker.

use aifs_domain::{
    AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelativePath,
    evidence::keys,
};
use aifs_extractors::{write_docx_fixture, write_pdf_fixture, write_plain_text_fixture};
use aifs_protocol::worker::WorkerKind;
use aifs_worker_client::WorkerClient;

fn file_entry(path: &str, family: FileFamily) -> ObservedEntry {
    ObservedEntry {
        id: AssetId::new(),
        path: RelativePath::parse(path).unwrap_or_else(|error| panic!("{error}")),
        kind: EntryKind::File,
        family,
        identity: FileIdentity::default(),
        is_hidden: false,
        lock: LockState::Readable,
    }
}

#[test]
fn document_worker_extracts_pdf_docx_and_text() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-document");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    write_pdf_fixture(
        dir.path().join("brief.pdf").as_path(),
        "Q2 Brief",
        "Hello PDF",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    write_docx_fixture(
        dir.path().join("memo.docx").as_path(),
        "Staff Memo",
        "Please file this.",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    write_plain_text_fixture(dir.path().join("notes.txt").as_path(), "Inbox notes\nBody")
        .unwrap_or_else(|error| panic!("{error}"));

    let mut client = WorkerClient::connect(WorkerKind::Document, worker)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        client
            .capabilities()
            .iter()
            .any(|cap| cap == "document_text")
    );

    let pdf = client
        .extract(dir.path(), &file_entry("brief.pdf", FileFamily::Document))
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("pdf evidence"));
    assert_eq!(pdf.fact(keys::DOCUMENT_TITLE), Some("Q2 Brief"));
    assert!(
        pdf.fact(keys::DOCUMENT_TEXT)
            .is_some_and(|text| text.contains("Hello PDF"))
    );

    let docx = client
        .extract(dir.path(), &file_entry("memo.docx", FileFamily::Document))
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("docx evidence"));
    assert_eq!(docx.fact(keys::DOCUMENT_TITLE), Some("Staff Memo"));

    let text = client
        .extract(dir.path(), &file_entry("notes.txt", FileFamily::Document))
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("text evidence"));
    assert_eq!(text.fact(keys::DOCUMENT_TITLE), Some("Inbox notes"));
    assert!(
        !text
            .fact(keys::DESCRIPTION)
            .is_some_and(|value| value.contains("stub"))
    );

    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
}
