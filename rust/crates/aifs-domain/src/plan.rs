//! Validated operation plans derived from an accepted revision.

use crate::entry::FileIdentity;
use crate::ids::{AssetId, PlanId, RevisionId};
use crate::path::RelativePath;
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One filesystem mutation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    /// Create a folder (and parents).
    CreateDirectory {
        /// Folder path.
        path: RelativePath,
    },
    /// Move/rename an asset.
    Move {
        /// Asset.
        asset: AssetId,
        /// Current path.
        from: RelativePath,
        /// Destination path.
        to: RelativePath,
        /// Identity expected at `from` before moving.
        expected: FileIdentity,
    },
    /// Remove a folder left empty by moves.
    RemoveEmptyDirectory {
        /// Folder path.
        path: RelativePath,
    },
}

/// Operation with ordering metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedOperation {
    /// Sequence number; operations run in ascending order.
    pub seq: u32,
    /// The mutation.
    pub operation: Operation,
}

/// Severity of a validation finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanIssueSeverity {
    /// Plan cannot be applied.
    Error,
    /// Plan can be applied but the user should know.
    Warning,
}

/// A validation finding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanIssue {
    /// Severity.
    pub severity: PlanIssueSeverity,
    /// Stable machine-readable code (e.g. `destination_collision`).
    pub code: String,
    /// Human-readable text.
    pub message: String,
    /// Affected assets.
    #[serde(default)]
    pub assets: Vec<AssetId>,
}

impl PlanIssue {
    /// Builds an error.
    pub fn error(code: &str, message: impl Into<String>, assets: Vec<AssetId>) -> Self {
        Self {
            severity: PlanIssueSeverity::Error,
            code: code.to_owned(),
            message: message.into(),
            assets,
        }
    }

    /// Builds a warning.
    pub fn warning(code: &str, message: impl Into<String>, assets: Vec<AssetId>) -> Self {
        Self {
            severity: PlanIssueSeverity::Warning,
            code: code.to_owned(),
            message: message.into(),
            assets,
        }
    }
}

/// A frozen, validated set of operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPlan {
    /// Plan id.
    pub id: PlanId,
    /// Revision it was derived from.
    pub revision: RevisionId,
    /// Absolute root.
    pub root: PathBuf,
    /// Creation time.
    pub created_at: Timestamp,
    /// Ordered operations.
    pub operations: Vec<PlannedOperation>,
    /// Non-fatal findings recorded at plan time.
    #[serde(default)]
    pub warnings: Vec<PlanIssue>,
}

impl OperationPlan {
    /// Number of move operations.
    pub fn move_count(&self) -> usize {
        self.operations
            .iter()
            .filter(|planned| matches!(planned.operation, Operation::Move { .. }))
            .count()
    }
}
