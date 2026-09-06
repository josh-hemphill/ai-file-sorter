//! Request options shared by the engine and its callers.

use aifs_domain::FileFamily;
use serde::{Deserialize, Serialize};

/// Controls a `scan` request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanOptions {
    /// Descend into sub-folders.
    pub recursive: bool,
    /// Maximum depth below the root when recursive (`0` = unlimited).
    pub max_depth: u32,
    /// Include dot-files and hidden entries.
    pub include_hidden: bool,
    /// Skip strong project roots and report them as protected bundles.
    pub protect_projects: bool,
    /// Read media tags and EXIF while scanning.
    pub extract_metadata: bool,
    /// Bytes of each file to hash for the content fingerprint (`0` disables hashing).
    pub fingerprint_prefix_bytes: u64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            recursive: true,
            max_depth: 0,
            include_hidden: false,
            protect_projects: true,
            extract_metadata: true,
            fingerprint_prefix_bytes: 64 * 1024,
        }
    }
}

/// How the heuristic planner names folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FolderStyle {
    /// Broad, stable top-level folders (`Documents`, `Pictures`, ...).
    #[default]
    Consistent,
    /// More specific folders when evidence supports them (`Podcasts`, `Screenshots`).
    Refined,
}

/// Controls a `propose` request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProposalPolicy {
    /// Folder naming style.
    pub style: FolderStyle,
    /// Add a second-level folder (artist, year, topic) when evidence supports it.
    pub use_subfolders: bool,
    /// Propose metadata-based file names for audio/video.
    pub rename_media: bool,
    /// Propose capture-date prefixes for images.
    pub rename_images_with_date: bool,
    /// Families left where they are (e.g. keep `Code` untouched).
    pub pinned_families: Vec<FileFamily>,
    /// Suggested destination folder for protected project bundles, or `None` to leave them.
    pub project_folder: Option<String>,
    /// Optional allowed category names for heuristic (and later model) folders.
    pub whitelist: CategoryWhitelist,
    /// Display language for category names (`en` until translations exist).
    pub category_language: String,
}

fn default_category_language() -> String {
    "en".to_owned()
}

/// Constrains top-level and optional second-level folder names.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CategoryWhitelist {
    /// Allowed top-level folders. Empty means any heuristic name is allowed.
    pub main: Vec<String>,
    /// Second-level names allowed under every main category. Mutually exclusive with [`Self::branching`].
    pub global_subcategories: Vec<String>,
    /// Per-category allowed children. Mutually exclusive with [`Self::global_subcategories`].
    pub branching: std::collections::BTreeMap<String, Vec<String>>,
}

impl CategoryWhitelist {
    /// Returns an error when both subcategory styles are populated.
    pub fn validate(&self) -> Result<(), String> {
        if !self.global_subcategories.is_empty() && !self.branching.is_empty() {
            return Err(
                "Use either global subcategories or per-category branching, not both".to_owned(),
            );
        }
        Ok(())
    }

    /// True when `name` is allowed as a top-level folder.
    pub fn allows_top(&self, name: &str) -> bool {
        self.main.is_empty()
            || self
                .main
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(name))
    }
}

/// Persisted scan/proposal/analysis policy owned by the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Scan options used by the Custom intent.
    pub scan: ScanOptions,
    /// Placement policy used by the Custom intent.
    pub policy: ProposalPolicy,
    /// Run image description when a vision slot is connected.
    pub analyze_images: bool,
    /// Run document analysis when a document slot is connected.
    pub analyze_documents: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            scan: ScanOptions::default(),
            policy: ProposalPolicy::default(),
            analyze_images: false,
            analyze_documents: false,
        }
    }
}

impl Default for ProposalPolicy {
    fn default() -> Self {
        Self {
            style: FolderStyle::Consistent,
            use_subfolders: true,
            rename_media: true,
            rename_images_with_date: false,
            pinned_families: vec![FileFamily::Code],
            project_folder: None,
            whitelist: CategoryWhitelist::default(),
            category_language: default_category_language(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults() {
        let options: ScanOptions =
            serde_json::from_str(r#"{"recursive":false}"#).unwrap_or_else(|e| panic!("{e}"));
        assert!(!options.recursive);
        assert!(options.protect_projects);
        let policy: ProposalPolicy =
            serde_json::from_str(r#"{"style":"refined"}"#).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(policy.style, FolderStyle::Refined);
        assert_eq!(policy.pinned_families, vec![FileFamily::Code]);
        assert_eq!(policy.category_language, "en");
        assert!(policy.whitelist.main.is_empty());
        let settings: AppSettings = serde_json::from_str("{}").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(settings, AppSettings::default());
    }

    #[test]
    fn whitelist_rejects_both_subcategory_styles() {
        let mut whitelist = CategoryWhitelist {
            main: vec!["Documents".into()],
            global_subcategories: vec!["Reports".into()],
            ..CategoryWhitelist::default()
        };
        assert!(whitelist.validate().is_ok());
        whitelist
            .branching
            .insert("Documents".into(), vec!["Notes".into()]);
        assert!(whitelist.validate().is_err());
    }
}
