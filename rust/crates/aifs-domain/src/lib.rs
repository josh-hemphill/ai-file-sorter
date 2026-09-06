//! Core domain model shared by the engine, workers, CLI, and desktop shell.
//!
//! Everything here is plain data with `serde` support and no I/O. The vocabulary
//! follows the rewrite design notes in `rust/docs/domain-model.md`:
//!
//! * [`entry::ObservedEntry`] — what the scanner saw on disk, keyed by an opaque
//!   [`ids::AssetId`] rather than by path.
//! * [`relationship::Bundle`] / [`relationship::Relationship`] — how entries relate
//!   (sidecars, project members, series, duplicates) and which constraints follow.
//! * [`evidence::Evidence`] — facts extracted by detectors, tag readers, or models.
//! * [`proposal::ProposalRevision`] — an immutable candidate organisation state.
//! * [`plan::OperationPlan`] — validated filesystem operations derived from a revision.
//! * [`journal::ApplyJournal`] — the durable record written while a plan is applied.

pub mod entry;
pub mod evidence;
pub mod ids;
pub mod journal;
pub mod path;
pub mod plan;
pub mod proposal;
pub mod relationship;
pub mod snapshot;
pub mod time;

pub use entry::{EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry};
pub use evidence::{Evidence, EvidenceSource};
pub use ids::{AssetId, BundleId, JournalId, PlanId, RevisionId, SessionId};
pub use journal::{ApplyJournal, JournalEntry, JournalState, JournalStatus};
pub use path::{RelativePath, RelativePathError};
pub use plan::{Operation, OperationPlan, PlanIssue, PlanIssueSeverity, PlannedOperation};
pub use proposal::{
    Placement, ProposalRevision, ReviewState, RevisionAuthor, RevisionPatch, SuggestionOrigin,
};
pub use relationship::{
    Bundle, BundleConstraint, BundleKind, Confidence, Relationship, RelationshipKind,
};
pub use snapshot::{ProjectMatch, ProjectStrength, SkipReason, SkippedEntry, WorkspaceSnapshot};
pub use time::Timestamp;
