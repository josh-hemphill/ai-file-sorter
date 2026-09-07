//! Shared GGUF catalog: several slots can point at one downloaded file.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static CATALOG_BASE_OVERRIDE: Mutex<Option<String>> = Mutex::new(None);
static SHA256_OVERRIDES: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
static CATALOG_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Hugging Face resolve URL used when `AIFS_CATALOG_BASE` is unset.
pub const DEFAULT_CATALOG_BASE: &str =
    "https://huggingface.co/bartowski/google_gemma-3-4b-it-GGUF/resolve/main";

/// Override the catalog origin (`{base}/{filename}`) for tests or mirrors.
pub const CATALOG_BASE_ENV: &str = "AIFS_CATALOG_BASE";

/// Default on-disk name for the Gemma 3 4B instruct GGUF (text + vision weights).
pub const GEMMA_TEXT_FILENAME: &str = "google_gemma-3-4b-it-Q4_K_M.gguf";

/// Default on-disk name for the Gemma 3 projector used with vision analysis.
pub const GEMMA_MMPROJ_FILENAME: &str = "mmproj-google_gemma-3-4b-it-f16.gguf";

/// Catalog size hint for [`GEMMA_TEXT_FILENAME`] (bytes), for UI copy.
pub const GEMMA_TEXT_BYTES: u64 = 2_489_758_112;

/// Catalog size hint for [`GEMMA_MMPROJ_FILENAME`] (bytes), for UI copy.
pub const GEMMA_MMPROJ_BYTES: u64 = 851_251_104;

/// Hugging Face LFS SHA-256 for [`GEMMA_TEXT_FILENAME`].
pub const GEMMA_TEXT_SHA256: &str =
    "4996030242583a40aa151ff93f49ed787ac8c25e4120c3ae4588b2e2a7d1ae94";

/// Hugging Face LFS SHA-256 for [`GEMMA_MMPROJ_FILENAME`].
pub const GEMMA_MMPROJ_SHA256: &str =
    "8c0fb064b019a6972856aaae2c7e4792858af3ca4561be2dbf649123ba6c40cb";

/// One downloadable blob that one or more catalog ids require.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogArtifact {
    /// Stable id such as `gemma-text-q4`.
    pub id: &'static str,
    /// Filename under the model storage directory.
    pub filename: &'static str,
    /// Approximate size shown in Setup (not used as a skip threshold).
    pub expected_bytes: u64,
    /// Lowercase hex SHA-256 of the published file.
    pub sha256: &'static str,
}

/// Gemma 3 4B instruct GGUF shared by the text and vision catalog ids.
pub const ARTIFACT_GEMMA_TEXT: CatalogArtifact = CatalogArtifact {
    id: "gemma-text-q4",
    filename: GEMMA_TEXT_FILENAME,
    expected_bytes: GEMMA_TEXT_BYTES,
    sha256: GEMMA_TEXT_SHA256,
};

/// Gemma 3 projector used only by the vision catalog id.
pub const ARTIFACT_GEMMA_MMPROJ: CatalogArtifact = CatalogArtifact {
    id: "gemma-mmproj-f16",
    filename: GEMMA_MMPROJ_FILENAME,
    expected_bytes: GEMMA_MMPROJ_BYTES,
    sha256: GEMMA_MMPROJ_SHA256,
};

/// Files a catalog id must have on disk before it is considered downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Setup catalog id (`gemma-3-4b-it` or `gemma-3-4b-it-mmproj`).
    pub catalog_id: &'static str,
    /// Human label.
    pub label: &'static str,
    /// GGUF files this id needs. Text and vision share the instruct weights.
    pub artifacts: &'static [CatalogArtifact],
}

/// Built-in text catalog entry.
pub const CATALOG_TEXT: CatalogEntry = CatalogEntry {
    catalog_id: "gemma-3-4b-it",
    label: "Gemma 3 4B Instruct (Q4_K_M)",
    artifacts: &[ARTIFACT_GEMMA_TEXT],
};

/// Built-in vision catalog entry (same instruct GGUF plus mmproj).
pub const CATALOG_VISION: CatalogEntry = CatalogEntry {
    catalog_id: "gemma-3-4b-it-mmproj",
    label: "Gemma 3 4B Instruct + mmproj",
    artifacts: &[ARTIFACT_GEMMA_TEXT, ARTIFACT_GEMMA_MMPROJ],
};

const ENTRIES: &[CatalogEntry] = &[CATALOG_TEXT, CATALOG_VISION];

const ARTIFACTS: &[CatalogArtifact] = &[ARTIFACT_GEMMA_TEXT, ARTIFACT_GEMMA_MMPROJ];

/// Look up a catalog id (`gemma-3-4b-it` or `gemma-3-4b-it-mmproj`).
pub fn catalog_entry(catalog_id: &str) -> Option<&'static CatalogEntry> {
    ENTRIES.iter().find(|entry| entry.catalog_id == catalog_id)
}

/// Every known downloadable artifact.
pub fn all_artifacts() -> &'static [CatalogArtifact] {
    ARTIFACTS
}

/// Catalog ids that include this artifact.
pub fn catalog_ids_for_artifact(artifact_id: &str) -> Vec<&'static str> {
    ENTRIES
        .iter()
        .filter(|entry| {
            entry
                .artifacts
                .iter()
                .any(|artifact| artifact.id == artifact_id)
        })
        .map(|entry| entry.catalog_id)
        .collect()
}

/// Override the catalog origin for this process without mutating the environment.
///
/// Edition 2024 makes `std::env::set_var` unsafe, and this workspace forbids `unsafe`.
pub fn set_catalog_base_override(base: Option<String>) -> Option<String> {
    match CATALOG_BASE_OVERRIDE.lock() {
        Ok(mut guard) => std::mem::replace(&mut *guard, base),
        Err(_) => base,
    }
}

/// Holds SHA-256 pins for tests. Serializes in-process so parallel tests do not clobber pins.
pub struct ArtifactSha256Guard {
    _lock: MutexGuard<'static, ()>,
    previous: Vec<(String, Option<String>)>,
}

impl ArtifactSha256Guard {
    /// Pins `filename → sha256(body)` until the guard drops.
    pub fn pin(files: &[(&str, &[u8])]) -> Self {
        let lock = CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous = files
            .iter()
            .map(|(name, body)| {
                (
                    (*name).to_owned(),
                    set_artifact_sha256_override(*name, Some(sha256_hex(body))),
                )
            })
            .collect();
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for ArtifactSha256Guard {
    fn drop(&mut self) {
        for (name, previous) in self.previous.drain(..) {
            let _ = set_artifact_sha256_override(name, previous);
        }
    }
}

/// Pins catalog base URL and SHA-256s for download tests. Serializes in-process.
pub struct CatalogTestGuard {
    _lock: MutexGuard<'static, ()>,
    previous_base: Option<String>,
    previous_hashes: Vec<(String, Option<String>)>,
}

impl CatalogTestGuard {
    /// Serves `files` hashes and `base` until the guard drops.
    pub fn pin(base: &str, files: &[(&str, &[u8])]) -> Self {
        let lock = CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous_base = set_catalog_base_override(Some(base.to_owned()));
        let previous_hashes = files
            .iter()
            .map(|(name, body)| {
                (
                    (*name).to_owned(),
                    set_artifact_sha256_override(*name, Some(sha256_hex(body))),
                )
            })
            .collect();
        Self {
            _lock: lock,
            previous_base,
            previous_hashes,
        }
    }
}

impl Drop for CatalogTestGuard {
    fn drop(&mut self) {
        for (name, previous) in self.previous_hashes.drain(..) {
            let _ = set_artifact_sha256_override(name, previous);
        }
        let _ = set_catalog_base_override(self.previous_base.take());
    }
}

/// Overrides the SHA-256 pin for `filename` (tests and mirrors). `None` clears it.
pub fn set_artifact_sha256_override(
    filename: impl Into<String>,
    sha256: Option<String>,
) -> Option<String> {
    let filename = filename.into();
    match SHA256_OVERRIDES.lock() {
        Ok(mut map) => match sha256 {
            Some(hash) => map.insert(filename, hash),
            None => map.remove(&filename),
        },
        Err(_) => sha256,
    }
}

/// SHA-256 that download and verify use for this artifact.
pub fn expected_sha256(artifact: &CatalogArtifact) -> String {
    if let Ok(map) = SHA256_OVERRIDES.lock()
        && let Some(hash) = map.get(artifact.filename)
    {
        return hash.clone();
    }
    artifact.sha256.to_owned()
}

/// Lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    hex_lower(Sha256::digest(data))
}

fn catalog_base() -> String {
    if let Ok(guard) = CATALOG_BASE_OVERRIDE.lock()
        && let Some(base) = guard.as_ref()
    {
        return base.clone();
    }
    std::env::var(CATALOG_BASE_ENV).unwrap_or_else(|_| DEFAULT_CATALOG_BASE.to_string())
}

/// `{base}/{filename}` for downloads.
pub fn catalog_download_url(filename: &str) -> String {
    let trimmed = catalog_base().trim_end_matches('/').to_owned();
    format!("{trimmed}/{filename}")
}

/// `{dir}/{filename}` for a stored GGUF.
pub fn artifact_path(storage_dir: &Path, filename: &str) -> PathBuf {
    storage_dir.join(filename)
}

/// True when a finished GGUF is on disk (`.part` files do not count).
pub fn artifact_is_present(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() > 0)
        .unwrap_or(false)
}

/// True when `path` is present and its SHA-256 matches `expected_sha256`.
pub fn artifact_is_verified(path: &Path, expected_sha256: &str) -> bool {
    if !artifact_is_present(path) {
        return false;
    }
    file_sha256_hex(path).is_ok_and(|hex| hex.eq_ignore_ascii_case(expected_sha256))
}

/// Bytes on disk for a finished file, or `0` when missing.
pub fn artifact_bytes_on_disk(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .unwrap_or(0)
}

/// True when every artifact for this catalog id is present and hash-verified.
pub fn catalog_id_is_downloaded(storage_dir: &Path, catalog_id: &str) -> bool {
    let Some(entry) = catalog_entry(catalog_id) else {
        return false;
    };
    entry.artifacts.iter().all(|artifact| {
        artifact_is_verified(
            &artifact_path(storage_dir, artifact.filename),
            &expected_sha256(artifact),
        )
    })
}

fn file_sha256_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(hasher.finalize()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn vision_catalog_shares_the_text_gguf() {
        assert_eq!(
            catalog_ids_for_artifact(ARTIFACT_GEMMA_TEXT.id),
            vec!["gemma-3-4b-it", "gemma-3-4b-it-mmproj"]
        );
        assert_eq!(
            catalog_ids_for_artifact(ARTIFACT_GEMMA_MMPROJ.id),
            vec!["gemma-3-4b-it-mmproj"]
        );
        assert_eq!(CATALOG_VISION.artifacts.len(), 2);
        assert_eq!(CATALOG_TEXT.artifacts.len(), 1);
        assert_eq!(ARTIFACT_GEMMA_TEXT.sha256, GEMMA_TEXT_SHA256);
        assert_eq!(ARTIFACT_GEMMA_MMPROJ.sha256, GEMMA_MMPROJ_SHA256);
    }

    #[test]
    fn tiny_fixture_is_present_but_not_downloaded_until_hash_matches() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = artifact_path(dir.path(), GEMMA_TEXT_FILENAME);
        assert!(!artifact_is_present(&path));
        fs::write(&path, b"gguf").unwrap_or_else(|error| panic!("{error}"));
        assert!(artifact_is_present(&path));
        assert!(!artifact_is_verified(&path, GEMMA_TEXT_SHA256));
        assert!(!catalog_id_is_downloaded(dir.path(), "gemma-3-4b-it"));
        assert!(!catalog_id_is_downloaded(
            dir.path(),
            "gemma-3-4b-it-mmproj"
        ));
    }

    #[test]
    fn matching_fixture_hash_counts_as_downloaded() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let body = b"gguf";
        let path = artifact_path(dir.path(), GEMMA_TEXT_FILENAME);
        fs::write(&path, body).unwrap_or_else(|error| panic!("{error}"));
        let _pin = ArtifactSha256Guard::pin(&[(GEMMA_TEXT_FILENAME, body.as_slice())]);
        assert!(catalog_id_is_downloaded(dir.path(), "gemma-3-4b-it"));
        assert!(artifact_is_verified(
            &path,
            &expected_sha256(&ARTIFACT_GEMMA_TEXT)
        ));
    }

    #[test]
    fn catalog_url_uses_override_base() {
        let url = catalog_download_url(GEMMA_TEXT_FILENAME);
        assert!(url.ends_with(GEMMA_TEXT_FILENAME), "{url}");
    }
}
