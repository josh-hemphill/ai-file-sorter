//! Catalog GGUF downloads with progress. Shared files are fetched once.
//! Partial `.part` files are kept across cancel so HTTP Range can resume.

use aifs_protocol::{
    CatalogArtifact, Envelope, Event, RequestId, artifact_is_verified, artifact_path,
    catalog_download_url, catalog_entry, expected_sha256, remember_file_digest,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const BUFFER_SIZE: usize = 64 * 1024;
const HTTP_PARTIAL_CONTENT: u16 = 206;
const HTTP_OK: u16 = 200;

/// Download every artifact for `catalog_id`, skipping verified files already on disk.
pub fn download_catalog(
    storage_dir: &Path,
    catalog_id: &str,
    id: &RequestId,
    emit: &mut impl FnMut(Envelope),
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), DownloadError> {
    if is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let entry = catalog_entry(catalog_id).ok_or_else(|| DownloadError::UnknownCatalog {
        catalog_id: catalog_id.to_owned(),
    })?;
    fs::create_dir_all(storage_dir).map_err(|error| DownloadError::Io {
        path: storage_dir.to_path_buf(),
        error,
    })?;
    for artifact in entry.artifacts {
        if is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        download_artifact(storage_dir, artifact, id, emit, is_cancelled)?;
    }
    Ok(())
}

fn download_artifact(
    storage_dir: &Path,
    artifact: &CatalogArtifact,
    id: &RequestId,
    emit: &mut impl FnMut(Envelope),
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), DownloadError> {
    let dest = artifact_path(storage_dir, artifact.filename);
    let sha256 = expected_sha256(artifact);
    if artifact_is_verified(&dest, &sha256) {
        let size = fs::metadata(&dest).map(|meta| meta.len()).unwrap_or(0);
        emit_progress(
            emit,
            id,
            size,
            Some(size),
            format!("Already downloaded: {}", artifact.filename),
        );
        return Ok(());
    }
    if dest.exists() {
        let _ = fs::remove_file(&dest);
    }
    let url = catalog_download_url(artifact.filename);
    let part = part_path(&dest);
    emit_progress(
        emit,
        id,
        part_len(&part),
        Some(artifact.expected_bytes),
        format!("Downloading {}", artifact.filename),
    );
    let digest = fetch_to_part(&mut FetchJob {
        url: &url,
        part: &part,
        size_hint: artifact.expected_bytes,
        expected_sha256: &sha256,
        id,
        emit,
        is_cancelled,
    })?;
    fs::rename(&part, &dest).map_err(|error| DownloadError::Io {
        path: dest.clone(),
        error,
    })?;
    remember_file_digest(&dest, &digest);
    let size = fs::metadata(&dest).map(|meta| meta.len()).unwrap_or(0);
    emit_progress(
        emit,
        id,
        size,
        Some(size),
        format!("Saved {}", artifact.filename),
    );
    Ok(())
}

/// `{dest}.part` sibling used for resumable catalog fetches.
fn part_path(dest: &Path) -> PathBuf {
    dest.with_extension("gguf.part")
}

fn part_len(part: &Path) -> u64 {
    fs::metadata(part)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .unwrap_or(0)
}

struct FetchJob<'a, E: FnMut(Envelope)> {
    url: &'a str,
    part: &'a Path,
    size_hint: u64,
    expected_sha256: &'a str,
    id: &'a RequestId,
    emit: &'a mut E,
    is_cancelled: &'a dyn Fn() -> bool,
}

fn fetch_to_part<E: FnMut(Envelope)>(job: &mut FetchJob<'_, E>) -> Result<String, DownloadError> {
    if (job.is_cancelled)() {
        return Err(DownloadError::Cancelled);
    }
    let mut resume_from = part_len(job.part);
    if part_is_unusable(resume_from, job.size_hint) {
        let _ = fs::remove_file(job.part);
        resume_from = 0;
    }
    match fetch_once(job, resume_from) {
        Ok(digest) => Ok(digest),
        Err(DownloadError::Cancelled) => Err(DownloadError::Cancelled),
        Err(error) if resume_from > 0 && matches!(error, DownloadError::Http { .. }) => {
            let _ = fs::remove_file(job.part);
            fetch_once(job, 0)
        }
        Err(error) => Err(error),
    }
}

/// True when a leftover `.part` is already as large as the catalog size hint.
fn part_is_unusable(len: u64, size_hint: u64) -> bool {
    size_hint > 0 && len >= size_hint
}

fn fetch_once<E: FnMut(Envelope)>(
    job: &mut FetchJob<'_, E>,
    resume_from: u64,
) -> Result<String, DownloadError> {
    if (job.is_cancelled)() {
        return Err(DownloadError::Cancelled);
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();
    let mut request = agent.get(job.url);
    if resume_from > 0 {
        request = request.set("Range", &format!("bytes={resume_from}-"));
    }
    let response = request.call().map_err(|error| DownloadError::Http {
        url: job.url.to_owned(),
        message: error.to_string(),
    })?;
    let status = response.status();
    let append = resume_from > 0 && status == HTTP_PARTIAL_CONTENT;
    if resume_from > 0 && status != HTTP_PARTIAL_CONTENT && status != HTTP_OK {
        return Err(DownloadError::Http {
            url: job.url.to_owned(),
            message: format!("unexpected status {status} for ranged GET"),
        });
    }
    let remaining = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok());
    let total = if append {
        remaining
            .map(|len| resume_from.saturating_add(len))
            .or(Some(job.size_hint))
    } else {
        remaining.or(Some(job.size_hint))
    };
    let (mut file, mut hasher, mut written) = if append {
        open_resumed_part(job.part, resume_from)?
    } else {
        if resume_from > 0 {
            let _ = fs::remove_file(job.part);
        }
        let file = File::create(job.part).map_err(|error| DownloadError::Io {
            path: job.part.to_path_buf(),
            error,
        })?;
        (file, Sha256::new(), 0_u64)
    };
    let mut reader = response.into_reader();
    let mut buffer = [0_u8; BUFFER_SIZE];
    let mut last = Instant::now()
        .checked_sub(PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);
    loop {
        if (job.is_cancelled)() {
            let _ = file.flush();
            return Err(DownloadError::Cancelled);
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|error| DownloadError::Io {
                path: job.part.to_path_buf(),
                error,
            })?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| DownloadError::Io {
                path: job.part.to_path_buf(),
                error,
            })?;
        hasher.update(&buffer[..read]);
        written = written.saturating_add(read as u64);
        if last.elapsed() >= PROGRESS_INTERVAL {
            last = Instant::now();
            emit_progress(
                job.emit,
                job.id,
                written,
                total,
                format!("Downloading {}", file_name(job.part)),
            );
        }
    }
    file.flush().map_err(|error| DownloadError::Io {
        path: job.part.to_path_buf(),
        error,
    })?;
    drop(file);
    if written == 0 {
        let _ = fs::remove_file(job.part);
        return Err(DownloadError::Empty {
            url: job.url.to_owned(),
        });
    }
    let digest = hex_lower(hasher.finalize());
    if !digest.eq_ignore_ascii_case(job.expected_sha256) {
        let _ = fs::remove_file(job.part);
        return Err(DownloadError::ChecksumMismatch {
            url: job.url.to_owned(),
            expected: job.expected_sha256.to_owned(),
            actual: digest,
        });
    }
    Ok(digest)
}

fn open_resumed_part(part: &Path, expected_len: u64) -> Result<(File, Sha256, u64), DownloadError> {
    let mut hasher = Sha256::new();
    let hashed = hash_existing(part, &mut hasher)?;
    if hashed != expected_len {
        return Err(DownloadError::Io {
            path: part.to_path_buf(),
            error: io::Error::other("partial download changed while hashing"),
        });
    }
    let file = OpenOptions::new()
        .append(true)
        .open(part)
        .map_err(|error| DownloadError::Io {
            path: part.to_path_buf(),
            error,
        })?;
    Ok((file, hasher, hashed))
}

fn hash_existing(path: &Path, hasher: &mut Sha256) -> Result<u64, DownloadError> {
    let mut file = File::open(path).map_err(|error| DownloadError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let mut buffer = [0_u8; BUFFER_SIZE];
    let mut hashed = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| DownloadError::Io {
            path: path.to_path_buf(),
            error,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        hashed = hashed.saturating_add(read as u64);
    }
    Ok(hashed)
}

fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
        out.push(char::from(b"0123456789abcdef"[(byte & 0x0f) as usize]));
    }
    out
}

fn emit_progress(
    emit: &mut impl FnMut(Envelope),
    id: &RequestId,
    current: u64,
    total: Option<u64>,
    message: String,
) {
    emit(Envelope::reply(
        id,
        Event::Progress {
            stage: "download".to_owned(),
            current,
            total,
            message,
        },
    ));
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Failures fetching or writing a catalog GGUF.
#[derive(Debug)]
pub enum DownloadError {
    /// Unknown catalog id.
    UnknownCatalog {
        /// Requested id.
        catalog_id: String,
    },
    /// The in-flight download noticed a cancel request.
    Cancelled,
    /// HTTP or TLS failure.
    Http {
        /// Request URL.
        url: String,
        /// ureq message.
        message: String,
    },
    /// Empty body.
    Empty {
        /// Request URL.
        url: String,
    },
    /// Downloaded bytes did not match the catalog SHA-256.
    ChecksumMismatch {
        /// Request URL.
        url: String,
        /// Expected lowercase hex digest.
        expected: String,
        /// Actual lowercase hex digest.
        actual: String,
    },
    /// Filesystem failure.
    Io {
        /// Path involved.
        path: PathBuf,
        /// IO error.
        error: io::Error,
    },
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCatalog { catalog_id } => {
                write!(formatter, "Unknown catalog id {catalog_id}")
            }
            Self::Cancelled => write!(formatter, "download cancelled"),
            Self::Http { url, message } => write!(formatter, "download {url} failed: {message}"),
            Self::Empty { url } => write!(formatter, "download {url} returned an empty file"),
            Self::ChecksumMismatch {
                url,
                expected,
                actual,
            } => write!(
                formatter,
                "download {url} checksum mismatch (expected {expected}, got {actual})"
            ),
            Self::Io { path, error } => write!(formatter, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for DownloadError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_stub::read_http_request;
    use aifs_protocol::{GEMMA_MMPROJ_FILENAME, GEMMA_TEXT_FILENAME};
    use std::collections::HashMap;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    enum RangeMode {
        Honor,
        Ignore,
    }

    struct CatalogHttp {
        base: String,
        hits: Arc<Mutex<HashMap<String, u32>>>,
        ranges: Arc<Mutex<Vec<String>>>,
    }

    struct Fixture {
        _guard: aifs_protocol::CatalogTestGuard,
        hits: Arc<Mutex<HashMap<String, u32>>>,
        ranges: Arc<Mutex<Vec<String>>>,
        storage: tempfile::TempDir,
    }

    fn start_server(
        files: &[(&str, &[u8])],
        range: RangeMode,
        stall_after: Option<usize>,
    ) -> CatalogHttp {
        let bodies: HashMap<String, Vec<u8>> = files
            .iter()
            .map(|(name, body)| ((*name).to_owned(), body.to_vec()))
            .collect();
        let hits = Arc::new(Mutex::new(HashMap::new()));
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let hits_thread = hits.clone();
        let ranges_thread = ranges.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("{error}"));
        thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else {
                    continue;
                };
                let Ok(request) = read_http_request(&mut stream) else {
                    continue;
                };
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/');
                let filename = path.split('?').next().unwrap_or(path);
                let range_header = request.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("range") {
                        Some(value.trim().to_owned())
                    } else {
                        None
                    }
                });
                if let Some(header) = range_header.clone() {
                    ranges_thread
                        .lock()
                        .unwrap_or_else(|error| panic!("{error}"))
                        .push(header);
                }
                let Some(body) = bodies.get(filename) else {
                    let header =
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(header.as_bytes());
                    continue;
                };
                {
                    let mut counts = hits_thread.lock().unwrap_or_else(|error| panic!("{error}"));
                    *counts.entry(filename.to_owned()).or_insert(0) += 1;
                }
                let start = match (&range, range_header.as_deref()) {
                    (RangeMode::Honor, Some(header)) => parse_range_start(header).unwrap_or(0),
                    _ => 0,
                };
                let slice = if start >= body.len() {
                    let header = "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(header.as_bytes());
                    continue;
                } else {
                    &body[start..]
                };
                let status = if start > 0 && matches!(range, RangeMode::Honor) {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len().saturating_sub(1),
                        body.len(),
                        slice.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        slice.len()
                    )
                };
                let _ = stream.write_all(status.as_bytes());
                if let Some(stall) = stall_after {
                    let first = stall.min(slice.len());
                    let _ = stream.write_all(&slice[..first]);
                    let _ = stream.flush();
                    thread::sleep(Duration::from_millis(200));
                    let _ = stream.write_all(&slice[first..]);
                } else {
                    let _ = stream.write_all(slice);
                }
            }
        });
        CatalogHttp {
            base: format!("http://{addr}"),
            hits,
            ranges,
        }
    }

    fn parse_range_start(header: &str) -> Option<usize> {
        let rest = header.strip_prefix("bytes=")?;
        let start = rest.split('-').next()?;
        start.parse().ok()
    }

    fn pin_catalog(
        files: &[(&str, &[u8])],
        range: RangeMode,
        stall_after: Option<usize>,
    ) -> Fixture {
        let server = start_server(files, range, stall_after);
        let guard = aifs_protocol::CatalogTestGuard::pin(&server.base, files);
        let storage = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        Fixture {
            _guard: guard,
            hits: server.hits,
            ranges: server.ranges,
            storage,
        }
    }

    fn download(storage: &Path, cancel_after_progress: Option<u64>) -> Result<(), DownloadError> {
        let id: RequestId = "dl".into();
        let cancelled = AtomicU32::new(0);
        download_catalog(
            storage,
            "gemma-3-4b-it",
            &id,
            &mut |envelope| {
                if let Event::Progress { current, .. } = envelope.event
                    && let Some(threshold) = cancel_after_progress
                    && current > threshold
                {
                    cancelled.store(1, Ordering::SeqCst);
                }
            },
            &|| cancelled.load(Ordering::SeqCst) == 1,
        )
    }

    fn hit_count(hits: &Arc<Mutex<HashMap<String, u32>>>, name: &str) -> u32 {
        hits.lock()
            .unwrap_or_else(|error| panic!("{error}"))
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    #[test]
    fn cancel_keeps_partial_file() {
        let body = vec![7_u8; BUFFER_SIZE + 8];
        let files = [
            (GEMMA_TEXT_FILENAME, body.as_slice()),
            (GEMMA_MMPROJ_FILENAME, b"mmproj".as_slice()),
        ];
        let fixture = pin_catalog(&files, RangeMode::Honor, Some(BUFFER_SIZE));
        let error = download(fixture.storage.path(), Some(0))
            .err()
            .unwrap_or_else(|| panic!("expected cancel"));
        assert!(matches!(error, DownloadError::Cancelled), "{error}");
        let dest = artifact_path(fixture.storage.path(), GEMMA_TEXT_FILENAME);
        let part = part_path(&dest);
        assert!(!dest.exists(), "{}", dest.display());
        assert!(part.exists(), "cancel must keep {}", part.display());
        let kept = fs::metadata(&part)
            .unwrap_or_else(|error| panic!("{error}"))
            .len();
        assert!(kept > 0, "part should contain prefix bytes");
        assert!(kept < body.len() as u64, "part should be incomplete");
    }

    #[test]
    fn resume_sends_range_and_finishes() {
        let body = b"prefix-then-suffix-weights";
        let files = [
            (GEMMA_TEXT_FILENAME, body.as_slice()),
            (GEMMA_MMPROJ_FILENAME, b"mmproj".as_slice()),
        ];
        let fixture = pin_catalog(&files, RangeMode::Honor, None);
        let dest = artifact_path(fixture.storage.path(), GEMMA_TEXT_FILENAME);
        let part = part_path(&dest);
        fs::write(&part, &body[..6]).unwrap_or_else(|error| panic!("{error}"));
        download(fixture.storage.path(), None).unwrap_or_else(|error| panic!("{error}"));
        let saved = fs::read(&dest).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved, body);
        assert!(!part.exists(), "finished download must rename part away");
        let ranges = fixture
            .ranges
            .lock()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            ranges.iter().any(|header| header == "bytes=6-"),
            "expected Range resume, got {ranges:?}"
        );
        assert_eq!(hit_count(&fixture.hits, GEMMA_TEXT_FILENAME), 1);
    }

    #[test]
    fn ignored_range_still_downloads_full_body() {
        let body = b"replacement-weights";
        let files = [
            (GEMMA_TEXT_FILENAME, body.as_slice()),
            (GEMMA_MMPROJ_FILENAME, b"mmproj".as_slice()),
        ];
        let fixture = pin_catalog(&files, RangeMode::Ignore, None);
        let dest = artifact_path(fixture.storage.path(), GEMMA_TEXT_FILENAME);
        let part = part_path(&dest);
        fs::write(&part, b"stale").unwrap_or_else(|error| panic!("{error}"));
        download(fixture.storage.path(), None).unwrap_or_else(|error| panic!("{error}"));
        let saved = fs::read(&dest).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved, body);
        assert!(!part.exists());
    }

    #[test]
    fn leftover_part_at_or_past_size_hint_is_unusable() {
        assert!(!part_is_unusable(0, 100));
        assert!(!part_is_unusable(99, 100));
        assert!(part_is_unusable(100, 100));
        assert!(part_is_unusable(101, 100));
        assert!(!part_is_unusable(50, 0));
    }
}
