//! Shared GGUF catalog: several slots can point at one downloaded file.

use std::path::{Path, PathBuf};

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
pub const GEMMA_TEXT_BYTES: u64 = 2_490_000_000;

/// Catalog size hint for [`GEMMA_MMPROJ_FILENAME`] (bytes), for UI copy.
pub const GEMMA_MMPROJ_BYTES: u64 = 851_000_000;

/// One downloadable blob that one or more catalog ids require.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogArtifact {
    /// Stable id such as `gemma-text-q4`.
    pub id: &'static str,
    /// Filename under the model storage directory.
    pub filename: &'static str,
    /// Approximate size shown in Setup (not used as a skip threshold).
    pub expected_bytes: u64,
}

/// Gemma 3 4B instruct GGUF shared by the text and vision catalog ids.
pub const ARTIFACT_GEMMA_TEXT: CatalogArtifact = CatalogArtifact {
    id: "gemma-text-q4",
    filename: GEMMA_TEXT_FILENAME,
    expected_bytes: GEMMA_TEXT_BYTES,
};

/// Gemma 3 projector used only by the vision catalog id.
pub const ARTIFACT_GEMMA_MMPROJ: CatalogArtifact = CatalogArtifact {
    id: "gemma-mmproj-f16",
    filename: GEMMA_MMPROJ_FILENAME,
    expected_bytes: GEMMA_MMPROJ_BYTES,
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

/// `{base}/{filename}` for downloads.
pub fn catalog_download_url(filename: &str) -> String {
    let base = std::env::var(CATALOG_BASE_ENV).unwrap_or_else(|_| DEFAULT_CATALOG_BASE.to_string());
    let trimmed = base.trim_end_matches('/');
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

/// Bytes on disk for a finished file, or `0` when missing.
pub fn artifact_bytes_on_disk(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .unwrap_or(0)
}

/// True when every artifact for this catalog id is on disk.
pub fn catalog_id_is_downloaded(storage_dir: &Path, catalog_id: &str) -> bool {
    let Some(entry) = catalog_entry(catalog_id) else {
        return false;
    };
    entry
        .artifacts
        .iter()
        .all(|artifact| artifact_is_present(&artifact_path(storage_dir, artifact.filename)))
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
    }

    #[test]
    fn tiny_fixture_counts_as_downloaded() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = artifact_path(dir.path(), GEMMA_TEXT_FILENAME);
        assert!(!artifact_is_present(&path));
        fs::write(&path, b"gguf").unwrap_or_else(|error| panic!("{error}"));
        assert!(artifact_is_present(&path));
        assert!(catalog_id_is_downloaded(dir.path(), "gemma-3-4b-it"));
        assert!(!catalog_id_is_downloaded(
            dir.path(),
            "gemma-3-4b-it-mmproj"
        ));
    }

    #[test]
    fn catalog_url_uses_override_base() {
        let url = catalog_download_url(GEMMA_TEXT_FILENAME);
        assert!(url.ends_with(GEMMA_TEXT_FILENAME), "{url}");
    }
}
