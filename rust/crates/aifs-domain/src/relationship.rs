//! Typed relationships between assets and the bundles/constraints they imply.

use crate::ids::{AssetId, BundleId};
use crate::path::RelativePath;
use serde::{Deserialize, Serialize};

/// Confidence in a detector or model output, clamped to `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Confidence(f32);

impl Confidence {
    /// Certain, e.g. derived from filesystem facts.
    pub const CERTAIN: Confidence = Confidence(1.0);

    /// Clamps the value into range.
    pub fn new(value: f32) -> Self {
        Self(value.clamp(0.0, 1.0))
    }

    /// Raw value.
    pub const fn value(&self) -> f32 {
        self.0
    }
}

/// Why two assets are linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    /// Metadata or companion file (RAW+JPEG, XMP, THM).
    Sidecar,
    /// Subtitle or lyrics for a media file.
    Subtitle,
    /// Cover art for an album/video.
    CoverArt,
    /// Both belong to one detected project.
    ProjectMember,
    /// Part of a numbered sequence (bursts, scans, episodes).
    SeriesMember,
    /// One archive split across several part files.
    ArchivePart,
    /// Derived export of the other (docx -> pdf).
    Derived,
    /// Same content.
    Duplicate,
    /// Same inode.
    HardLink,
}

/// A directed, typed edge between two assets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relationship {
    /// Source asset.
    pub from: AssetId,
    /// Target asset.
    pub to: AssetId,
    /// Edge type.
    pub kind: RelationshipKind,
    /// Detector confidence.
    pub confidence: Confidence,
    /// Which detector produced the edge.
    pub detector: String,
    /// Optional human-readable justification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What kind of grouping a bundle represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleKind {
    /// Detected project root (Unity, git, cargo, ...).
    Project,
    /// Primary file plus its sidecars/subtitles/cover art.
    SidecarGroup,
    /// Numbered sequence.
    Series,
    /// Split archive parts.
    ArchiveParts,
    /// User-defined group.
    Manual,
}

/// How strictly members must stay together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BundleConstraint {
    /// Members may be placed independently; grouping is a hint only.
    Soft,
    /// Members must end up in the same destination folder.
    MoveTogether,
    /// Members must keep their layout relative to the bundle root; the root moves as a unit.
    PreserveLayout {
        /// Root folder of the layout.
        root: RelativePath,
    },
    /// Members must not move at all.
    Protected {
        /// Why they are protected.
        reason: String,
    },
}

/// A set of assets that should be reviewed and organised as one item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bundle {
    /// Bundle id.
    pub id: BundleId,
    /// Grouping type.
    pub kind: BundleKind,
    /// Human label, e.g. project name or primary file stem.
    pub label: String,
    /// Member assets; the anchor is included.
    pub members: Vec<AssetId>,
    /// Member that gives the bundle its name/destination, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AssetId>,
    /// Constraint applied by the planner.
    pub constraint: BundleConstraint,
    /// Why the detector grouped these.
    pub reason: String,
}

impl Bundle {
    /// True when the bundle forbids splitting members across folders.
    pub fn is_hard(&self) -> bool {
        !matches!(self.constraint, BundleConstraint::Soft)
    }

    /// True when the planner must not move members at all.
    pub fn is_protected(&self) -> bool {
        matches!(self.constraint, BundleConstraint::Protected { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_is_clamped() {
        assert_eq!(Confidence::new(1.7).value(), 1.0);
        assert_eq!(Confidence::new(-0.2).value(), 0.0);
    }

    #[test]
    fn bundle_hardness_follows_constraint() {
        let soft = Bundle {
            id: BundleId::new(),
            kind: BundleKind::Series,
            label: "IMG".into(),
            members: vec![],
            anchor: None,
            constraint: BundleConstraint::Soft,
            reason: String::new(),
        };
        let protected = Bundle {
            constraint: BundleConstraint::Protected {
                reason: "git".into(),
            },
            ..soft.clone()
        };
        assert!(!soft.is_hard());
        assert!(protected.is_hard());
        assert!(protected.is_protected());
    }
}
