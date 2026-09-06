//! Catalog GGUF downloads with progress. Shared files are fetched once.

use aifs_protocol::{
    artifact_is_present, artifact_path, catalog_download_url, catalog_entry, CatalogArtifact,
    Envelope, Event, RequestId,
};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const BUFFER_SIZE: usize = 64 * 1024;

/// Download every artifact for `catalog_id`, skipping files already on disk.
pub fn download_catalog(
    storage_dir: &Path,
    catalog_id: &str,
    id: &RequestId,
    emit: &mut impl FnMut(Envelope),
) -> Result<(), DownloadError> {
    let entry = catalog_entry(catalog_id).ok_or_else(|| DownloadError::UnknownCatalog {
        catalog_id: catalog_id.to_owned(),
    })?;
    fs::create_dir_all(storage_dir).map_err(|error| DownloadError::Io {
        path: storage_dir.to_path_buf(),
        error,
    })?;
    for artifact in entry.artifacts {
        download_artifact(storage_dir, artifact, id, emit)?;
    }
    Ok(())
}

fn download_artifact(
    storage_dir: &Path,
    artifact: &CatalogArtifact,
    id: &RequestId,
    emit: &mut impl FnMut(Envelope),
) -> Result<(), DownloadError> {
    let dest = artifact_path(storage_dir, artifact.filename);
    if artifact_is_present(&dest) {
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
    let url = catalog_download_url(artifact.filename);
    let part = dest.with_extension("gguf.part");
    if part.exists() {
        let _ = fs::remove_file(&part);
    }
    emit_progress(
        emit,
        id,
        0,
        Some(artifact.expected_bytes),
        format!("Downloading {}", artifact.filename),
    );
    fetch_to_part(&url, &part, artifact.expected_bytes, id, emit)?;
    fs::rename(&part, &dest).map_err(|error| DownloadError::Io {
        path: dest.clone(),
        error,
    })?;
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

fn fetch_to_part(
    url: &str,
    part: &Path,
    size_hint: u64,
    id: &RequestId,
    emit: &mut impl FnMut(Envelope),
) -> Result<(), DownloadError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();
    let response = agent.get(url).call().map_err(|error| DownloadError::Http {
        url: url.to_owned(),
        message: error.to_string(),
    })?;
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .or(Some(size_hint));
    let mut reader = response.into_reader();
    let mut file = File::create(part).map_err(|error| DownloadError::Io {
        path: part.to_path_buf(),
        error,
    })?;
    let mut buffer = [0_u8; BUFFER_SIZE];
    let mut written: u64 = 0;
    let mut last = Instant::now()
        .checked_sub(PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| DownloadError::Io {
                path: part.to_path_buf(),
                error,
            })?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| DownloadError::Io {
                path: part.to_path_buf(),
                error,
            })?;
        written = written.saturating_add(read as u64);
        if last.elapsed() >= PROGRESS_INTERVAL {
            last = Instant::now();
            emit_progress(
                emit,
                id,
                written,
                total,
                format!("Downloading {}", file_name(part)),
            );
        }
    }
    file.flush().map_err(|error| DownloadError::Io {
        path: part.to_path_buf(),
        error,
    })?;
    if written == 0 {
        let _ = fs::remove_file(part);
        return Err(DownloadError::Empty {
            url: url.to_owned(),
        });
    }
    Ok(())
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
            Self::Http { url, message } => write!(formatter, "download {url} failed: {message}"),
            Self::Empty { url } => write!(formatter, "download {url} returned an empty file"),
            Self::Io { path, error } => write!(formatter, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for DownloadError {}
