//! The scanner's view of a session root at one point in time.

use crate::entry::ObservedEntry;
use crate::evidence::Evidence;
use crate::ids::{AssetId, SessionId};
use crate::path::RelativePath;
use crate::relationship::{Bundle, Relationship};
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// How sure a project detector is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStrength {
    /// Marker is suggestive only; traversal continues.
    Weak,
    /// Marker is definitive; the tree is protected and not traversed.
    Strong,
}

/// A recognised project root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMatch {
    /// Root folder of the project relative to the session root.
    pub root: RelativePath,
    /// Rule id (e.g. `unity`, `git`).
    pub rule_id: String,
    /// Display name.
    pub name: String,
    /// Strength.
    pub strength: ProjectStrength,
    /// Why moving members independently is unsafe.
    pub reason: String,
}

/// Why the scanner did not include an entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// Lives inside a strong project root.
    ProtectedProject {
        /// Rule id.
        rule_id: String,
    },
    /// Symlink/reparse point.
    Symlink,
    /// Hidden entry with hidden files disabled.
    Hidden,
    /// Known junk (Thumbs.db, .DS_Store, ...).
    Junk,
    /// Directory depth limit reached.
    DepthLimit,
    /// I/O failure.
    Error {
        /// Error text.
        message: String,
    },
}

/// An entry the scanner saw but excluded, so the UI can show what was ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedEntry {
    /// Path relative to the root.
    pub path: RelativePath,
    /// Why.
    pub reason: SkipReason,
}

/// Immutable result of scanning + relationship detection + extraction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Owning session.
    pub session: SessionId,
    /// Absolute root that was scanned.
    pub root: PathBuf,
    /// When the scan finished.
    pub captured_at: Timestamp,
    /// Included entries.
    pub entries: Vec<ObservedEntry>,
    /// Excluded entries.
    #[serde(default)]
    pub skipped: Vec<SkippedEntry>,
    /// Recognised projects (strong ones also appear as bundles).
    #[serde(default)]
    pub projects: Vec<ProjectMatch>,
    /// Detected bundles.
    #[serde(default)]
    pub bundles: Vec<Bundle>,
    /// Detected relationships.
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    /// Extracted evidence.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

impl WorkspaceSnapshot {
    /// Creates an empty snapshot for a root.
    pub fn new(session: SessionId, root: PathBuf) -> Self {
        Self {
            session,
            root,
            captured_at: Timestamp::now(),
            entries: Vec::new(),
            skipped: Vec::new(),
            projects: Vec::new(),
            bundles: Vec::new(),
            relationships: Vec::new(),
            evidence: Vec::new(),
        }
    }

    /// Looks up an entry by id.
    pub fn entry(&self, id: AssetId) -> Option<&ObservedEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Builds an id → entry index.
    pub fn entry_index(&self) -> HashMap<AssetId, &ObservedEntry> {
        self.entries.iter().map(|entry| (entry.id, entry)).collect()
    }

    /// Returns all evidence recorded for an asset.
    pub fn evidence_for(&self, id: AssetId) -> impl Iterator<Item = &Evidence> {
        self.evidence
            .iter()
            .filter(move |evidence| evidence.asset == id)
    }

    /// Returns the hard bundle containing an asset, if any.
    pub fn hard_bundle_for(&self, id: AssetId) -> Option<&Bundle> {
        self.bundles
            .iter()
            .find(|bundle| bundle.is_hard() && bundle.members.contains(&id))
    }
}
