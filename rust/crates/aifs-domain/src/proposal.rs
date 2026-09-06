//! Proposal revisions: immutable candidate organisation states that users and
//! assistants iterate on before anything touches disk.

use crate::ids::{AssetId, RevisionId, SessionId};
use crate::path::RelativePath;
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Who produced a suggestion.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum SuggestionOrigin {
    /// Deterministic planner rules.
    Heuristic,
    /// Model on this machine.
    LocalModel {
        /// Model id.
        model: String,
    },
    /// Remote model.
    RemoteModel {
        /// Provider/model id.
        model: String,
    },
    /// Typed or dragged by the user.
    User,
    /// Left where it already is.
    Unchanged,
}

/// Review decision for a placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    /// Not yet decided.
    #[default]
    Proposed,
    /// Approved for apply.
    Accepted,
    /// Excluded from apply.
    Rejected,
}

/// Where one asset should end up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// Asset being placed.
    pub asset: AssetId,
    /// Destination relative to the session root (folder + file name).
    pub destination: RelativePath,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// Producer.
    pub origin: SuggestionOrigin,
    /// Review decision.
    #[serde(default)]
    pub review: ReviewState,
}

/// Who created a revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "author", rename_all = "snake_case")]
pub enum RevisionAuthor {
    /// Engine heuristics.
    Engine,
    /// Interactive user edit.
    User,
    /// Assistant tool call.
    Assistant {
        /// Provider/model id.
        model: String,
    },
    /// Imported from a file or another machine.
    Import,
}

/// A single edit to a revision. Assistants and the UI both speak this vocabulary, so
/// every change is auditable and replayable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RevisionPatch {
    /// Set the full destination path for one asset.
    SetDestination {
        /// Asset.
        asset: AssetId,
        /// New destination.
        destination: RelativePath,
        /// Optional rationale.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rationale: Option<String>,
    },
    /// Move assets into a folder, keeping their file names.
    MoveToFolder {
        /// Assets.
        assets: Vec<AssetId>,
        /// Destination folder.
        folder: RelativePath,
        /// Optional rationale.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rationale: Option<String>,
    },
    /// Rename an asset in place.
    Rename {
        /// Asset.
        asset: AssetId,
        /// New file name (no folder).
        file_name: String,
    },
    /// Mark assets accepted.
    Accept {
        /// Assets.
        assets: Vec<AssetId>,
    },
    /// Mark assets rejected.
    Reject {
        /// Assets.
        assets: Vec<AssetId>,
    },
    /// Reset assets to proposed.
    Reopen {
        /// Assets.
        assets: Vec<AssetId>,
    },
}

/// An immutable organisation proposal. Revisions form a chain through `parent`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalRevision {
    /// Revision id.
    pub id: RevisionId,
    /// Owning session.
    pub session: SessionId,
    /// Previous revision, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<RevisionId>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Author.
    pub author: RevisionAuthor,
    /// Free-form summary of what changed.
    #[serde(default)]
    pub summary: String,
    /// Placements keyed by asset id (one per asset).
    pub placements: BTreeMap<AssetId, Placement>,
}

impl ProposalRevision {
    /// Creates a root revision.
    pub fn new(session: SessionId, author: RevisionAuthor, summary: impl Into<String>) -> Self {
        Self {
            id: RevisionId::new(),
            session,
            parent: None,
            created_at: Timestamp::now(),
            author,
            summary: summary.into(),
            placements: BTreeMap::new(),
        }
    }

    /// Inserts or replaces a placement.
    pub fn place(&mut self, placement: Placement) {
        self.placements.insert(placement.asset, placement);
    }

    /// Looks up the placement for an asset.
    pub fn placement(&self, asset: AssetId) -> Option<&Placement> {
        self.placements.get(&asset)
    }

    /// Placements that are accepted for apply.
    pub fn accepted(&self) -> impl Iterator<Item = &Placement> {
        self.placements
            .values()
            .filter(|placement| placement.review == ReviewState::Accepted)
    }

    /// Returns a new child revision with the patches applied. Unknown assets are ignored
    /// so a stale assistant patch cannot invent placements.
    pub fn with_patches(
        &self,
        author: RevisionAuthor,
        summary: impl Into<String>,
        patches: &[RevisionPatch],
    ) -> Result<ProposalRevision, PatchError> {
        let mut next = ProposalRevision {
            id: RevisionId::new(),
            session: self.session,
            parent: Some(self.id),
            created_at: Timestamp::now(),
            author: author.clone(),
            summary: summary.into(),
            placements: self.placements.clone(),
        };
        let origin = match &author {
            RevisionAuthor::Assistant { model } => SuggestionOrigin::RemoteModel {
                model: model.clone(),
            },
            RevisionAuthor::Engine => SuggestionOrigin::Heuristic,
            RevisionAuthor::User | RevisionAuthor::Import => SuggestionOrigin::User,
        };
        for patch in patches {
            apply_patch(&mut next, patch, &origin)?;
        }
        Ok(next)
    }
}

/// Patch application failure.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatchError {
    /// Asset is not part of the revision.
    #[error("asset {0} is not part of this revision")]
    UnknownAsset(AssetId),
    /// Path problem.
    #[error(transparent)]
    Path(#[from] crate::path::RelativePathError),
}

fn apply_patch(
    revision: &mut ProposalRevision,
    patch: &RevisionPatch,
    origin: &SuggestionOrigin,
) -> Result<(), PatchError> {
    match patch {
        RevisionPatch::SetDestination {
            asset,
            destination,
            rationale,
        } => {
            let placement = placement_mut(revision, *asset)?;
            placement.destination = destination.clone();
            placement.origin = origin.clone();
            placement.review = ReviewState::Proposed;
            if rationale.is_some() {
                placement.rationale = rationale.clone();
            }
        }
        RevisionPatch::MoveToFolder {
            assets,
            folder,
            rationale,
        } => {
            for asset in assets {
                let placement = placement_mut(revision, *asset)?;
                placement.destination = folder.join(placement.destination.file_name())?;
                placement.origin = origin.clone();
                placement.review = ReviewState::Proposed;
                if rationale.is_some() {
                    placement.rationale = rationale.clone();
                }
            }
        }
        RevisionPatch::Rename { asset, file_name } => {
            let placement = placement_mut(revision, *asset)?;
            placement.destination = placement.destination.with_file_name(file_name)?;
            placement.origin = origin.clone();
            placement.review = ReviewState::Proposed;
        }
        RevisionPatch::Accept { assets } => set_review(revision, assets, ReviewState::Accepted)?,
        RevisionPatch::Reject { assets } => set_review(revision, assets, ReviewState::Rejected)?,
        RevisionPatch::Reopen { assets } => set_review(revision, assets, ReviewState::Proposed)?,
    }
    Ok(())
}

fn set_review(
    revision: &mut ProposalRevision,
    assets: &[AssetId],
    state: ReviewState,
) -> Result<(), PatchError> {
    for asset in assets {
        placement_mut(revision, *asset)?.review = state;
    }
    Ok(())
}

fn placement_mut(
    revision: &mut ProposalRevision,
    asset: AssetId,
) -> Result<&mut Placement, PatchError> {
    revision
        .placements
        .get_mut(&asset)
        .ok_or(PatchError::UnknownAsset(asset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::RelativePathError;

    fn revision_with(asset: AssetId, destination: &str) -> ProposalRevision {
        let mut revision = ProposalRevision::new(SessionId::new(), RevisionAuthor::Engine, "seed");
        revision.place(Placement {
            asset,
            destination: RelativePath::parse(destination).unwrap_or_else(|e| panic!("{e}")),
            rationale: None,
            origin: SuggestionOrigin::Heuristic,
            review: ReviewState::Accepted,
        });
        revision
    }

    #[test]
    fn patches_produce_child_revision_and_reset_review() {
        let asset = AssetId::new();
        let base = revision_with(asset, "Other/a.txt");
        let next = base
            .with_patches(
                RevisionAuthor::Assistant {
                    model: "mock".into(),
                },
                "move",
                &[RevisionPatch::MoveToFolder {
                    assets: vec![asset],
                    folder: RelativePath::parse("Documents/Notes")
                        .unwrap_or_else(|e| panic!("{e}")),
                    rationale: Some("notes".into()),
                }],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(next.parent, Some(base.id));
        let placement = next.placement(asset).unwrap_or_else(|| panic!("missing"));
        assert_eq!(placement.destination.as_str(), "Documents/Notes/a.txt");
        assert_eq!(placement.review, ReviewState::Proposed);
        assert_eq!(
            placement.origin,
            SuggestionOrigin::RemoteModel {
                model: "mock".into()
            }
        );
        assert_eq!(
            base.placement(asset).map(|p| p.review),
            Some(ReviewState::Accepted)
        );
    }

    #[test]
    fn rename_keeps_folder_and_unknown_assets_fail() {
        let asset = AssetId::new();
        let base = revision_with(asset, "Music/track.mp3");
        let renamed = base
            .with_patches(
                RevisionAuthor::User,
                "rename",
                &[RevisionPatch::Rename {
                    asset,
                    file_name: "2024_song.mp3".into(),
                }],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            renamed.placement(asset).map(|p| p.destination.as_str()),
            Some("Music/2024_song.mp3")
        );
        let unknown = AssetId::new();
        assert_eq!(
            base.with_patches(
                RevisionAuthor::User,
                "x",
                &[RevisionPatch::Accept {
                    assets: vec![unknown]
                }]
            )
            .err(),
            Some(PatchError::UnknownAsset(unknown))
        );
        let nested = base.with_patches(
            RevisionAuthor::User,
            "bad-rename",
            &[RevisionPatch::Rename {
                asset,
                file_name: "folder/track.mp3".into(),
            }],
        );
        assert!(
            matches!(
                nested,
                Err(PatchError::Path(RelativePathError::NotAFileName(_)))
            ),
            "rename must reject a multi-segment name, got {nested:?}"
        );
        assert!(base
            .with_patches(
                RevisionAuthor::User,
                "dot",
                &[RevisionPatch::Rename {
                    asset,
                    file_name: ".".into(),
                }],
            )
            .is_err());
    }

    #[test]
    fn patch_vocabulary_is_tagged_json() {
        let patch = RevisionPatch::Accept { assets: vec![] };
        assert_eq!(
            serde_json::to_string(&patch).ok(),
            Some(r#"{"op":"accept","assets":[]}"#.into())
        );
    }
}
