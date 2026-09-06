//! Journaled apply/undo. Filesystem and SQLite cannot share a transaction, so each
//! operation is recorded as `intended` before it runs.

use aifs_domain::{
    ApplyJournal, FileIdentity, JournalEntry, JournalId, JournalState, JournalStatus, Operation,
    OperationPlan, Timestamp, WorkspaceSnapshot,
};
use aifs_scanner::read_identity;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use thiserror::Error;

const DEFAULT_FINGERPRINT_PREFIX: u64 = 64 * 1024;

/// Apply failures that abort the remaining operations.
#[derive(Debug, Error)]
pub enum ApplyError {
    /// I/O failure.
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Applies `plan` under `snapshot.root`. `dry_run` records intended operations only.
pub fn apply_plan(
    snapshot: &WorkspaceSnapshot,
    plan: &OperationPlan,
    dry_run: bool,
) -> ApplyJournal {
    apply_plan_with_prefix(snapshot, plan, dry_run, DEFAULT_FINGERPRINT_PREFIX)
}

/// Applies a plan using a specific fingerprint prefix when re-checking identities.
pub fn apply_plan_with_prefix(
    snapshot: &WorkspaceSnapshot,
    plan: &OperationPlan,
    dry_run: bool,
    fingerprint_prefix_bytes: u64,
) -> ApplyJournal {
    let mut journal = ApplyJournal {
        id: JournalId::new(),
        plan: plan.id,
        root: snapshot.root.clone(),
        started_at: Timestamp::now(),
        finished_at: None,
        status: JournalStatus::Running,
        dry_run,
        entries: plan
            .operations
            .iter()
            .map(|planned| JournalEntry {
                seq: planned.seq,
                operation: planned.operation.clone(),
                state: JournalState::Intended,
                updated_at: Timestamp::now(),
            })
            .collect(),
    };

    if dry_run {
        journal.status = JournalStatus::Completed;
        journal.finished_at = Some(Timestamp::now());
        return journal;
    }

    for index in 0..journal.entries.len() {
        let operation = journal.entries[index].operation.clone();
        match run_operation(&snapshot.root, &operation, fingerprint_prefix_bytes) {
            Ok(()) => {
                journal.entries[index].state = JournalState::Done;
            }
            Err(message) => {
                journal.entries[index].state = JournalState::Failed {
                    message: message.clone(),
                };
                journal.status = JournalStatus::Failed;
                journal.finished_at = Some(Timestamp::now());
                return journal;
            }
        }
        journal.entries[index].updated_at = Timestamp::now();
    }
    journal.status = JournalStatus::Completed;
    journal.finished_at = Some(Timestamp::now());
    journal
}

/// Reverses a completed (or partially completed) journal.
pub fn undo_journal(snapshot: &WorkspaceSnapshot, journal: &ApplyJournal) -> ApplyJournal {
    undo_journal_with_prefix(snapshot, journal, DEFAULT_FINGERPRINT_PREFIX)
}

/// Undo using a specific fingerprint prefix.
pub fn undo_journal_with_prefix(
    snapshot: &WorkspaceSnapshot,
    journal: &ApplyJournal,
    fingerprint_prefix_bytes: u64,
) -> ApplyJournal {
    let mut next = journal.clone();
    if journal.dry_run {
        next.status = JournalStatus::Undone;
        next.finished_at = Some(Timestamp::now());
        return next;
    }
    for index in (0..next.entries.len()).rev() {
        if !matches!(next.entries[index].state, JournalState::Done) {
            continue;
        }
        let operation = next.entries[index].operation.clone();
        match reverse_operation(&snapshot.root, &operation, fingerprint_prefix_bytes) {
            Ok(()) => {
                next.entries[index].state = JournalState::RolledBack;
                next.entries[index].updated_at = Timestamp::now();
            }
            Err(message) => {
                next.entries[index].state = JournalState::Failed { message };
                next.status = JournalStatus::Failed;
                next.finished_at = Some(Timestamp::now());
                return next;
            }
        }
    }
    next.status = JournalStatus::Undone;
    next.finished_at = Some(Timestamp::now());
    next
}

fn run_operation(root: &Path, operation: &Operation, prefix: u64) -> Result<(), String> {
    match operation {
        Operation::CreateDirectory { path } => {
            fs::create_dir_all(path.resolve(root)).map_err(|error| error.to_string())
        }
        Operation::Move {
            from, to, expected, ..
        } => {
            let src = from.resolve(root);
            let dest = to.resolve(root);
            verify_identity(&src, expected, prefix)?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            move_file(&src, &dest)
        }
        Operation::RemoveEmptyDirectory { path } => {
            let dir = path.resolve(root);
            match fs::remove_dir(&dir) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    }
}

fn reverse_operation(root: &Path, operation: &Operation, prefix: u64) -> Result<(), String> {
    match operation {
        Operation::CreateDirectory { path } => {
            let dir = path.resolve(root);
            match fs::remove_dir(&dir) {
                Ok(()) | Err(_) => Ok(()),
            }
        }
        Operation::Move {
            from, to, expected, ..
        } => {
            let src = to.resolve(root);
            let dest = from.resolve(root);
            let current = read_identity(&src, prefix).map_err(|error| error.to_string())?;
            if !identities_compatible(expected, &current) {
                return Err(format!(
                    "{} changed since apply; refusing to undo",
                    to.as_str()
                ));
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            move_file(&src, &dest)
        }
        Operation::RemoveEmptyDirectory { path } => {
            fs::create_dir_all(path.resolve(root)).map_err(|error| error.to_string())
        }
    }
}

fn verify_identity(path: &Path, expected: &FileIdentity, prefix: u64) -> Result<(), String> {
    let current = read_identity(path, prefix).map_err(|error| error.to_string())?;
    if identities_compatible(expected, &current) {
        Ok(())
    } else {
        Err(format!("{} changed since scan", path.display()))
    }
}

fn identities_compatible(expected: &FileIdentity, current: &FileIdentity) -> bool {
    expected.matches(current)
}

fn move_file(src: &Path, dest: &Path) -> Result<(), String> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device(&error) => copy_verify_delete(src, dest),
        Err(error) => Err(error.to_string()),
    }
}

fn is_cross_device(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::CrossesDevices || error.raw_os_error() == Some(18)
}

fn copy_verify_delete(src: &Path, dest: &Path) -> Result<(), String> {
    let source_hash = hash_file(src).map_err(|error| error.to_string())?;
    fs::copy(src, dest).map_err(|error| error.to_string())?;
    let dest_hash = hash_file(dest).map_err(|error| error.to_string())?;
    if source_hash != dest_hash {
        let _ = fs::remove_file(dest);
        return Err(format!("copy of {} failed verification", src.display()));
    }
    fs::remove_file(src).map_err(|error| error.to_string())
}

fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelativePath,
        ReviewState, SessionId,
    };
    use aifs_planner::{accept_all, propose, validate};
    use aifs_protocol::ProposalPolicy;
    use std::fs;

    fn scan_like(root: &Path, relative: &str, bytes: &[u8]) -> ObservedEntry {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{e}"));
        }
        fs::write(&path, bytes).unwrap_or_else(|e| panic!("{e}"));
        let identity =
            read_identity(&path, DEFAULT_FINGERPRINT_PREFIX).unwrap_or_else(|e| panic!("{e}"));
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(relative).unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::from_extension(
                Path::new(relative)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or(""),
            ),
            identity,
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    #[test]
    fn dry_run_does_not_move_and_apply_then_undo_restores() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), dir.path().to_path_buf());
        snapshot
            .entries
            .push(scan_like(dir.path(), "note.txt", b"hello"));
        let revision = accept_all(&propose(&snapshot, &ProposalPolicy::default()))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            revision.placements.values().next().map(|p| p.review),
            Some(ReviewState::Accepted)
        );
        let (plan, issues) = validate(&snapshot, &revision);
        assert!(issues
            .iter()
            .all(|issue| issue.severity != aifs_domain::PlanIssueSeverity::Error));
        let plan = plan.unwrap_or_else(|| panic!("plan"));

        let dry = apply_plan(&snapshot, &plan, true);
        assert!(dry.dry_run);
        assert!(dir.path().join("note.txt").exists());
        assert!(!dir.path().join("Documents/note.txt").exists());

        let applied = apply_plan(&snapshot, &plan, false);
        assert_eq!(applied.status, JournalStatus::Completed);
        assert!(!dir.path().join("note.txt").exists());
        assert_eq!(
            fs::read(dir.path().join("Documents/note.txt")).ok(),
            Some(b"hello".to_vec())
        );

        let undone = undo_journal(&snapshot, &applied);
        assert_eq!(undone.status, JournalStatus::Undone);
        assert!(dir.path().join("note.txt").exists());
        assert_eq!(
            fs::read(dir.path().join("note.txt")).ok(),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn identity_mismatch_skips_apply() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), dir.path().to_path_buf());
        snapshot
            .entries
            .push(scan_like(dir.path(), "note.txt", b"hello"));
        let revision = accept_all(&propose(&snapshot, &ProposalPolicy::default()))
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, _) = validate(&snapshot, &revision);
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        fs::write(dir.path().join("note.txt"), b"changed").unwrap_or_else(|e| panic!("{e}"));
        let applied = apply_plan(&snapshot, &plan, false);
        assert_eq!(applied.status, JournalStatus::Failed);
        assert!(dir.path().join("note.txt").exists());
        let _ = FileIdentity::default();
    }
}
