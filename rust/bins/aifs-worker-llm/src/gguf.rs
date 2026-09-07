//! Resolves catalog and local GGUF paths. Does not read model weights.

use aifs_protocol::{
    ModelBackend, artifact_is_present, artifact_path, catalog_entry, catalog_id_is_downloaded,
};
use std::path::{Path, PathBuf};

/// Local weights (and optional projector) the llama.cpp backend can load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GgufFiles {
    /// Instruct / text GGUF. Catalog text and vision share this file.
    pub weights: PathBuf,
    /// Optional mmproj GGUF for vision. Not required for text infer.
    pub mmproj: Option<PathBuf>,
    /// Model label for evidence (`catalog_id` or file name).
    pub label: String,
}

/// True when this backend is a local GGUF (catalog or path), not hosted.
pub fn is_local_gguf(backend: &ModelBackend) -> bool {
    matches!(
        backend,
        ModelBackend::Catalog { .. } | ModelBackend::LocalGguf { .. }
    )
}

/// Locates weights on disk. Missing files are an error so the engine can fall back.
pub fn resolve_gguf(backend: &ModelBackend, storage_dir: &str) -> Result<GgufFiles, String> {
    match backend {
        ModelBackend::Catalog { catalog_id } => resolve_catalog(catalog_id, Path::new(storage_dir)),
        ModelBackend::LocalGguf { path, mmproj } => resolve_local(path, mmproj.as_deref()),
        ModelBackend::Off => Err("cannot load an off slot".to_owned()),
        ModelBackend::OpenAi { .. }
        | ModelBackend::Gemini { .. }
        | ModelBackend::CustomEndpoint { .. } => {
            Err("hosted backends are not local GGUF files".to_owned())
        }
    }
}

fn resolve_catalog(catalog_id: &str, storage_dir: &Path) -> Result<GgufFiles, String> {
    let entry =
        catalog_entry(catalog_id).ok_or_else(|| format!("unknown catalog id {catalog_id}"))?;
    if !catalog_id_is_downloaded(storage_dir, catalog_id) {
        return Err(format!(
            "{catalog_id} is not fully downloaded under {}",
            storage_dir.display()
        ));
    }
    let mut weights = None;
    let mut mmproj = None;
    for artifact in entry.artifacts {
        let path = artifact_path(storage_dir, artifact.filename);
        if artifact.id.contains("mmproj") {
            mmproj = Some(path);
        } else {
            weights = Some(path);
        }
    }
    let weights = weights.ok_or_else(|| format!("{catalog_id} has no weights GGUF"))?;
    Ok(GgufFiles {
        weights,
        mmproj,
        label: catalog_id.to_owned(),
    })
}

fn resolve_local(path: &str, mmproj: Option<&str>) -> Result<GgufFiles, String> {
    let weights = PathBuf::from(path);
    if !artifact_is_present(&weights) {
        return Err(format!("{} was not found", weights.display()));
    }
    let mmproj = match mmproj.map(str::trim).filter(|value| !value.is_empty()) {
        Some(proj) => {
            let path = PathBuf::from(proj);
            if !artifact_is_present(&path) {
                return Err(format!("{} was not found", path.display()));
            }
            Some(path)
        }
        None => None,
    };
    let label = weights
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_owned();
    Ok(GgufFiles {
        weights,
        mmproj,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_protocol::GEMMA_TEXT_FILENAME;
    use std::fs;

    #[test]
    fn catalog_text_and_vision_share_the_instruct_gguf() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        fs::write(dir.path().join(GEMMA_TEXT_FILENAME), b"gguf")
            .unwrap_or_else(|error| panic!("{error}"));
        let text = resolve_gguf(
            &ModelBackend::Catalog {
                catalog_id: "gemma-3-4b-it".into(),
            },
            &dir.path().display().to_string(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(text.mmproj.is_none());
        assert_eq!(text.label, "gemma-3-4b-it");

        fs::write(
            dir.path().join(aifs_protocol::GEMMA_MMPROJ_FILENAME),
            b"proj",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let vision = resolve_gguf(
            &ModelBackend::Catalog {
                catalog_id: "gemma-3-4b-it-mmproj".into(),
            },
            &dir.path().display().to_string(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(text.weights, vision.weights);
        assert!(vision.mmproj.is_some());
        assert!(is_local_gguf(&ModelBackend::Catalog {
            catalog_id: "gemma-3-4b-it".into(),
        }));
        assert!(!is_local_gguf(&ModelBackend::OpenAi {
            model: "gpt-4.1-mini".into(),
        }));
    }

    #[test]
    fn missing_catalog_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let error = resolve_gguf(
            &ModelBackend::Catalog {
                catalog_id: "gemma-3-4b-it".into(),
            },
            &dir.path().display().to_string(),
        )
        .err()
        .unwrap_or_else(|| panic!("missing GGUF must fail"));
        assert!(error.contains("not fully downloaded"), "{error}");
    }
}
