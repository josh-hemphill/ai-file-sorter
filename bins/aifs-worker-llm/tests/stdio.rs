//! Stdio tests for the LLM worker.

use aifs_domain::{
    AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelativePath,
};
use aifs_protocol::FolderStyle;
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

#[cfg(not(feature = "llama"))]
#[test]
fn llm_stub_loads_and_categorizes_without_gguf_bytes() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-llm");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(dir.path().join("notes.txt"), b"hello")
        .unwrap_or_else(|error| panic!("{error}"));
    let mut client =
        WorkerClient::connect(WorkerKind::Llm, worker).unwrap_or_else(|error| panic!("{error}"));
    assert!(client.capabilities().iter().any(|cap| cap == "stub"));
    assert!(client.capabilities().iter().any(|cap| cap == "hosted"));
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
        .categorize(dir.path(), &entry, vec![], vec![], FolderStyle::Consistent)
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
    let inbox = client
        .categorize(
            dir.path(),
            &entry,
            vec![],
            vec!["Inbox".into()],
            FolderStyle::Consistent,
        )
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("whitelisted categorize"));
    assert_eq!(
        inbox.fact(aifs_domain::evidence::keys::CATEGORY),
        Some("Inbox")
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
    let failed = client.categorize(dir.path(), &entry, vec![], vec![], FolderStyle::Consistent);
    assert!(failed.is_err(), "{failed:?}");
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
}

#[cfg(feature = "llama")]
#[test]
fn llama_worker_refuses_missing_gguf() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-llm");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let mut client =
        WorkerClient::connect(WorkerKind::Llm, worker).unwrap_or_else(|error| panic!("{error}"));
    assert!(client.capabilities().iter().any(|cap| cap == "llama"));
    assert!(!client.capabilities().iter().any(|cap| cap == "stub"));
    let error = client
        .load(
            ModelBackend::Catalog {
                catalog_id: "gemma-3-4b-it".into(),
            },
            "cpu",
            None,
            None,
            dir.path().display().to_string(),
        )
        .err()
        .unwrap_or_else(|| panic!("missing GGUF must fail"));
    assert!(
        error.to_string().contains("not fully downloaded"),
        "{error}"
    );
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn openai_load_requires_a_key() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-llm");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let mut client =
        WorkerClient::connect(WorkerKind::Llm, worker).unwrap_or_else(|error| panic!("{error}"));
    let error = client
        .load(
            ModelBackend::OpenAi {
                model: "gpt-4.1-mini".into(),
            },
            "cpu",
            None,
            None,
            dir.path().display().to_string(),
        )
        .err()
        .unwrap_or_else(|| panic!("OpenAI load without a key must fail"));
    assert!(error.to_string().contains("API key"), "{error}");
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn hosted_custom_endpoint_categorizes_as_remote_model() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-llm");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let body = r#"{"choices":[{"message":{"content":"{\"category\":\"Documents\",\"description\":\"a memo\"}"}}]}"#;
    let (base, server) = serve_json("200 OK", body);
    let mut client =
        WorkerClient::connect(WorkerKind::Llm, worker).unwrap_or_else(|error| panic!("{error}"));
    client
        .load(
            ModelBackend::CustomEndpoint {
                base_url: base,
                model: "local-test".into(),
            },
            "cpu",
            None,
            None,
            dir.path().display().to_string(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let entry = file_entry("notes.txt", FileFamily::Document);
    let evidence = client
        .categorize(dir.path(), &entry, vec![], vec![], FolderStyle::Consistent)
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("categorize evidence"));
    assert!(matches!(
        evidence.source,
        aifs_domain::EvidenceSource::RemoteModel { .. }
    ));
    assert_eq!(
        evidence.fact(aifs_domain::evidence::keys::CATEGORY),
        Some("Documents")
    );
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
    let _ = server.join();
}

#[test]
fn hosted_categorize_sends_whitelist_style_and_description() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-llm");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let body = r#"{"choices":[{"message":{"content":"{\"category\":\"Screenshots\",\"description\":\"a panel\"}"}}]}"#;
    let (base, server) = serve_json("200 OK", body);
    let mut client =
        WorkerClient::connect(WorkerKind::Llm, worker).unwrap_or_else(|error| panic!("{error}"));
    client
        .load(
            ModelBackend::CustomEndpoint {
                base_url: base,
                model: "local-test".into(),
            },
            "cpu",
            None,
            None,
            dir.path().display().to_string(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let image = file_entry("Screenshot.png", FileFamily::Image);
    let prior = aifs_domain::Evidence::new(
        image.id,
        aifs_domain::EvidenceSource::LocalModel {
            model: "vision".into(),
        },
        aifs_domain::Confidence::new(0.55),
    )
    .with_fact(
        aifs_domain::evidence::keys::DESCRIPTION,
        "a settings panel UI capture",
    );
    let evidence = client
        .categorize(
            dir.path(),
            &image,
            vec![prior],
            vec!["Screenshots".into(), "Pictures".into()],
            FolderStyle::Refined,
        )
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("categorize evidence"));
    assert_eq!(
        evidence.fact(aifs_domain::evidence::keys::CATEGORY),
        Some("Screenshots")
    );
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
    let request = server.join().unwrap_or_else(|error| panic!("{error:?}"));
    assert!(
        request.contains("a settings panel UI capture"),
        "categorize must send the description: {request}"
    );
    assert!(
        request.contains("Screenshots, Pictures") || request.contains("Screenshots"),
        "categorize must send allowed categories: {request}"
    );
    assert!(
        request.contains("refined") || request.contains("Screenshots"),
        "categorize must send refined screenshot guidance: {request}"
    );
}

#[test]
fn hosted_describe_sends_filename_text_and_never_pixels() {
    let worker = env!("CARGO_BIN_EXE_aifs-worker-llm");
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    const PIXEL_MARKER: &[u8] = b"PIXEL-BYTES-UNIQUE-mtmd-describe-never-upload";
    std::fs::write(dir.path().join("shot.jpg"), PIXEL_MARKER)
        .unwrap_or_else(|error| panic!("{error}"));
    let body = r#"{"choices":[{"message":{"content":"{\"description\":\"a desk photo\"}"}}]}"#;
    let (base, server) = serve_json("200 OK", body);
    let mut client =
        WorkerClient::connect(WorkerKind::Llm, worker).unwrap_or_else(|error| panic!("{error}"));
    client
        .load(
            ModelBackend::CustomEndpoint {
                base_url: base,
                model: "local-test".into(),
            },
            "cpu",
            None,
            None,
            dir.path().display().to_string(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let image = file_entry("shot.jpg", FileFamily::Image);
    let evidence = client
        .describe(dir.path(), &image, vec![])
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("describe evidence"));
    assert_eq!(
        evidence.fact(aifs_domain::evidence::keys::DESCRIPTION),
        Some("a desk photo")
    );
    client.shutdown().unwrap_or_else(|error| panic!("{error}"));
    let request = server.join().unwrap_or_else(|error| panic!("{error:?}"));
    assert!(
        request.contains("shot.jpg"),
        "describe must send filename text: {request}"
    );
    assert!(
        request.contains("Relative path: shot.jpg"),
        "describe must send describe_user text: {request}"
    );
    let marker = std::str::from_utf8(PIXEL_MARKER).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        !request.contains(marker),
        "hosted describe must not upload pixels: {request}"
    );
    assert!(
        !request.contains("image_url") && !request.contains("inline_data"),
        "hosted describe must not attach image payloads: {request}"
    );
}

/// One-shot JSON stub. Reads the full request before replying (see aifs-engine http_stub).
fn serve_json(
    status: &'static str,
    body: &'static str,
) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener};
    use std::time::Duration;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap_or_else(|error| panic!("{error}"));
        let request = read_http_request(&mut stream).unwrap_or_default();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        let _ = stream.shutdown(Shutdown::Write);
        let mut sink = [0_u8; 256];
        while stream.read(&mut sink).unwrap_or(0) > 0 {}
        request
    });
    (format!("http://{addr}/v1"), handle)
}

fn read_http_request(stream: &mut std::net::TcpStream) -> std::io::Result<String> {
    use std::io::Read;
    use std::time::Duration;
    const MAX_REQUEST_BYTES: usize = 1024 * 1024;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 2048];
    loop {
        let read = stream.read(&mut tmp)?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..read]);
        if buf.len() > MAX_REQUEST_BYTES {
            break;
        }
        let Some(header_end) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&buf[..header_end]).unwrap_or("");
        let content_len = headers
            .split("\r\n")
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap_or(0))
            })
            .unwrap_or(0);
        let needed = header_end + 4 + content_len;
        while buf.len() < needed && buf.len() <= MAX_REQUEST_BYTES {
            let read = stream.read(&mut tmp)?;
            if read == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..read]);
        }
        break;
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[test]
fn llm_worker_refuses_extract() {
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
