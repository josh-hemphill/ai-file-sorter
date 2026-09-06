//! Durable record of an apply run. Written before each mutation so a crash mid-run can
//! be recovered, and kept afterwards so the run can be undone.

use crate::ids::{JournalId, PlanId};
use crate::plan::Operation;
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Lifecycle of one journaled operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JournalState {
    /// Recorded, not yet executed.
    Intended,
    /// Executed successfully.
    Done,
    /// Skipped because a precondition failed.
    Skipped {
        /// Why.
        reason: String,
    },
    /// Execution failed.
    Failed {
        /// Error text.
        message: String,
    },
    /// Reversed by rollback or undo.
    RolledBack,
}

/// One journaled operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Plan sequence number.
    pub seq: u32,
    /// The operation.
    pub operation: Operation,
    /// Current state.
    pub state: JournalState,
    /// Last state change.
    pub updated_at: Timestamp,
}

/// Overall journal status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalStatus {
    /// Apply in progress.
    Running,
    /// All operations done or skipped.
    Completed,
    /// Stopped after a failure; some operations may be done.
    Failed,
    /// Every done operation was reversed.
    Undone,
}

/// Journal for one apply run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyJournal {
    /// Journal id.
    pub id: JournalId,
    /// Plan that was applied.
    pub plan: PlanId,
    /// Absolute root.
    pub root: PathBuf,
    /// Start time.
    pub started_at: Timestamp,
    /// Finish time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    /// Status.
    pub status: JournalStatus,
    /// True when no filesystem mutation was performed.
    #[serde(default)]
    pub dry_run: bool,
    /// Entries in execution order.
    pub entries: Vec<JournalEntry>,
}

impl ApplyJournal {
    /// Number of operations that completed.
    pub fn done_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.state, JournalState::Done))
            .count()
    }

    /// Number of operations that failed.
    pub fn failed_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.state, JournalState::Failed { .. }))
            .count()
    }

    /// Number of operations that were skipped.
    pub fn skipped_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.state, JournalState::Skipped { .. }))
            .count()
    }
}
