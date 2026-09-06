//! Facts extracted about an asset. Evidence is untrusted input to planners and models.

use crate::ids::AssetId;
use crate::relationship::Confidence;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a fact came from. Model-produced evidence is kept distinct so it can be
/// shown, discounted, or excluded from remote prompts.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum EvidenceSource {
    /// Names, sizes, timestamps.
    Filesystem,
    /// Embedded audio/video tags.
    MediaTags,
    /// Image EXIF/XMP.
    Exif,
    /// Document properties or extracted text summary.
    DocumentMetadata,
    /// A deterministic detector.
    Detector {
        /// Detector name.
        name: String,
    },
    /// A model running on this machine.
    LocalModel {
        /// Model identifier.
        model: String,
    },
    /// A remote model.
    RemoteModel {
        /// Provider/model identifier.
        model: String,
    },
    /// Entered by the user.
    User,
}

/// Well-known evidence keys so producers and consumers agree without a shared enum.
pub mod keys {
    /// Media title.
    pub const MEDIA_TITLE: &str = "media.title";
    /// Media artist / performer.
    pub const MEDIA_ARTIST: &str = "media.artist";
    /// Media album / show.
    pub const MEDIA_ALBUM: &str = "media.album";
    /// Media year (4 digits).
    pub const MEDIA_YEAR: &str = "media.year";
    /// Media genre.
    pub const MEDIA_GENRE: &str = "media.genre";
    /// Track number.
    pub const MEDIA_TRACK: &str = "media.track";
    /// Duration in whole seconds.
    pub const MEDIA_DURATION_SECONDS: &str = "media.duration_seconds";
    /// Capture date as `YYYY-MM-DD`.
    pub const IMAGE_CAPTURED_ON: &str = "image.captured_on";
    /// Camera make/model.
    pub const IMAGE_CAMERA: &str = "image.camera";
    /// GPS latitude.
    pub const IMAGE_LATITUDE: &str = "image.latitude";
    /// GPS longitude.
    pub const IMAGE_LONGITUDE: &str = "image.longitude";
    /// Short natural-language description.
    pub const DESCRIPTION: &str = "description";
    /// Suggested filename (without folder).
    pub const SUGGESTED_NAME: &str = "suggested_name";
    /// Suggested top-level category from a model (untrusted).
    pub const CATEGORY: &str = "category";
    /// Suggested subcategory from a model (untrusted).
    pub const CATEGORY_SUB: &str = "category.sub";
}

/// A bag of facts about one asset from one source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// Asset the facts describe.
    pub asset: AssetId,
    /// Producer.
    pub source: EvidenceSource,
    /// Confidence in the whole bag.
    pub confidence: Confidence,
    /// Key/value facts; keys should come from [`keys`] where possible.
    pub facts: BTreeMap<String, String>,
}

impl Evidence {
    /// Creates an empty bag for an asset.
    pub fn new(asset: AssetId, source: EvidenceSource, confidence: Confidence) -> Self {
        Self {
            asset,
            source,
            confidence,
            facts: BTreeMap::new(),
        }
    }

    /// Adds a fact, dropping blank values.
    pub fn with_fact(mut self, key: &str, value: impl Into<String>) -> Self {
        let value = value.into();
        if !value.trim().is_empty() {
            self.facts.insert(key.to_owned(), value.trim().to_owned());
        }
        self
    }

    /// Looks up a fact.
    pub fn fact(&self, key: &str) -> Option<&str> {
        self.facts.get(key).map(String::as_str)
    }

    /// True when no facts were recorded.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_facts_are_dropped() {
        let evidence = Evidence::new(
            AssetId::new(),
            EvidenceSource::MediaTags,
            Confidence::CERTAIN,
        )
        .with_fact(keys::MEDIA_TITLE, "  Night Drive ")
        .with_fact(keys::MEDIA_ALBUM, "   ");
        assert_eq!(evidence.fact(keys::MEDIA_TITLE), Some("Night Drive"));
        assert_eq!(evidence.fact(keys::MEDIA_ALBUM), None);
        assert!(!evidence.is_empty());
    }

    #[test]
    fn model_sources_serialize_with_tag() {
        let source = EvidenceSource::RemoteModel {
            model: "gpt".into(),
        };
        let json = serde_json::to_string(&source).unwrap_or_default();
        assert_eq!(json, r#"{"source":"remote_model","model":"gpt"}"#);
    }
}
