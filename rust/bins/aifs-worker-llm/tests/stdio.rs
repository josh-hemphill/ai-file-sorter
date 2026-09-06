//! Stdio tests for the LLM worker stub.

use aifs_domain::{
    AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelativePath,
};
use aifs_protocol::ModelBackend;
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
fn llm_stub_loads_and_categorizes_without_gguf_bytes() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-llm");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(dir.path().join("notes.txt"), b"hello")
        .unwrap_or_else(|error| panic!("{error}"));
    let mut client =
        WorkerClient::connect(WorkerKind::Llm, worker).unwrap_or_else(|error| panic!("{error}"));
    assert!(client.capabilities().iter().any(|cap| cap == "stub"));
    assert!(client.capabilities().iter().any(|cap| cap == "load"));
    assert!(client.capabilities().iter().any(|cap| cap == "unload"));
    let loaded = client
        .load(
            ModelBackend::Catalog {
                catalog_id: "gemma-3-4b-it".into(),
            },
            "cuda",
            None,
            None,
            dir.path().display().to_string(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(loaded.model, "gemma-3-4b-it");
    assert_eq!(loaded.device, "cpu");
    assert_eq!(loaded.n_gpu_layers, 0);
    assert!(
        loaded
            .fallback
            .as_deref()
            .is_some_and(|text| text.contains("CUDA")),
        "{loaded:?}"
    );

    let entry = file_entry("notes.txt", FileFamily::Document);
    let evidence = client
        .categorize(dir.path(), &entry, vec![])
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("categorize evidence"));
    assert!(matches!(
        evidence.source,
        aifs_domain::EvidenceSource::LocalModel { .. }
    ));
    assert_eq!(
        evidence.fact(aifs_domain::evidence::keys::CATEGORY),
        Some("Documents")
    );

    let image = file_entry("shot.jpg", FileFamily::Image);
    let described = client
        .describe(dir.path(), &image, vec![])
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("describe evidence"));
    assert!(
        described
            .fact(aifs_domain::evidence::keys::DESCRIPTION)
            .is_some_and(|text| text.contains("shot.jpg"))
    );

    let reply = client
        .chat("group the podcasts", "")
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(reply.contains("stub chat"), "{reply}");
    assert!(reply.contains("group the podcasts"), "{reply}");

    client.unload().unwrap_or_else(|error| panic!("{error}"));
    let failed = client.categorize(dir.path(), &entry, vec![]);
    assert!(failed.is_err(), "{failed:?}");
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn llm_stub_refuses_extract() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-llm");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let mut client =
        WorkerClient::connect(WorkerKind::Llm, worker).unwrap_or_else(|error| panic!("{error}"));
    let entry = file_entry("notes.txt", FileFamily::Document);
    let error = client
        .extract(dir.path(), &entry)
        .err()
        .unwrap_or_else(|| panic!("extract must fail"));
    assert!(error.to_string().contains("extract"), "{error}");
    client.shutdown().unwrap_or_else(|e| panic!("{e}"));
}
