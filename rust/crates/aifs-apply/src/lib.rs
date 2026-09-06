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
    apply_plan_with_progress(snapshot, plan, dry_run, |_| {})
}

/// Applies a plan, invoking `on_progress` after the journal is created and after each op.
pub fn apply_plan_with_progress(
    snapshot: &WorkspaceSnapshot,
    plan: &OperationPlan,
    dry_run: bool,
    on_progress: impl FnMut(&ApplyJournal),
) -> ApplyJournal {
    apply_plan_with_prefix_and_progress(
        snapshot,
        plan,
        dry_run,
        DEFAULT_FINGERPRINT_PREFIX,
        on_progress,
    )
}

/// Applies a plan using a specific fingerprint prefix when re-checking identities.
pub fn apply_plan_with_prefix(
    snapshot: &WorkspaceSnapshot,
    plan: &OperationPlan,
    dry_run: bool,
    fingerprint_prefix_bytes: u64,
) -> ApplyJournal {
    apply_plan_with_prefix_and_progress(snapshot, plan, dry_run, fingerprint_prefix_bytes, |_| {})
}

/// Applies a plan with a fingerprint prefix and a progress sink.
pub fn apply_plan_with_prefix_and_progress(
    snapshot: &WorkspaceSnapshot,
    plan: &OperationPlan,
    dry_run: bool,
    fingerprint_prefix_bytes: u64,
    mut on_progress: impl FnMut(&ApplyJournal),
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
    on_progress(&journal);

    if dry_run {
        journal.status = JournalStatus::Completed;
        journal.finished_at = Some(Timestamp::now());
        on_progress(&journal);
        return journal;
    }

    for index in 0..journal.entries.len() {
        let operation = journal.entries[index].operation.clone();
        match run_operation(&snapshot.root, &operation, fingerprint_prefix_bytes) {
            Ok(RunOutcome::Done) => {
                journal.entries[index].state = JournalState::Done;
            }
            Ok(RunOutcome::Skipped(reason)) => {
                journal.entries[index].state = JournalState::Skipped { reason };
            }
            Err(message) => {
                journal.entries[index].state = JournalState::Failed {
                    message: message.clone(),
                };
                journal.entries[index].updated_at = Timestamp::now();
                journal.status = JournalStatus::Failed;
                journal.finished_at = Some(Timestamp::now());
                on_progress(&journal);
                return journal;
            }
        }
        journal.entries[index].updated_at = Timestamp::now();
        on_progress(&journal);
    }
    journal.status = JournalStatus::Completed;
    journal.finished_at = Some(Timestamp::now());
    on_progress(&journal);
    journal
}

/// Reverses a completed (or partially completed) journal.
pub fn undo_journal(snapshot: &WorkspaceSnapshot, journal: &ApplyJournal) -> ApplyJournal {
    undo_journal_with_progress(snapshot, journal, |_| {})
}

/// Undo with a progress sink after each reversed operation.
pub fn undo_journal_with_progress(
    snapshot: &WorkspaceSnapshot,
    journal: &ApplyJournal,
    on_progress: impl FnMut(&ApplyJournal),
) -> ApplyJournal {
    undo_journal_with_prefix_and_progress(
        snapshot,
        journal,
        DEFAULT_FINGERPRINT_PREFIX,
        on_progress,
    )
}

/// Undo using a specific fingerprint prefix.
pub fn undo_journal_with_prefix(
    snapshot: &WorkspaceSnapshot,
    journal: &ApplyJournal,
    fingerprint_prefix_bytes: u64,
) -> ApplyJournal {
    undo_journal_with_prefix_and_progress(snapshot, journal, fingerprint_prefix_bytes, |_| {})
}

/// Undo with a fingerprint prefix and a progress sink.
pub fn undo_journal_with_prefix_and_progress(
    snapshot: &WorkspaceSnapshot,
    journal: &ApplyJournal,
    fingerprint_prefix_bytes: u64,
    mut on_progress: impl FnMut(&ApplyJournal),
) -> ApplyJournal {
    let mut next = journal.clone();
    if journal.dry_run {
        next.status = JournalStatus::Undone;
        next.finished_at = Some(Timestamp::now());
        on_progress(&next);
        return next;
    }
    on_progress(&next);
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
                on_progress(&next);
                return next;
            }
        }
        on_progress(&next);
    }
    next.status = JournalStatus::Undone;
    next.finished_at = Some(Timestamp::now());
    on_progress(&next);
    next
}

enum RunOutcome {
    Done,
    Skipped(String),
}

fn run_operation(root: &Path, operation: &Operation, prefix: u64) -> Result<RunOutcome, String> {
    match operation {
        Operation::CreateDirectory { path } => {
            let dir = path.resolve(root);
            if dir.exists() {
                return Ok(RunOutcome::Skipped("directory already exists".to_owned()));
            }
            fs::create_dir_all(dir).map_err(|error| error.to_string())?;
            Ok(RunOutcome::Done)
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
            move_file(&src, &dest)?;
            Ok(RunOutcome::Done)
        }
        Operation::RemoveEmptyDirectory { path } => {
            let dir = path.resolve(root);
            match fs::remove_dir(&dir) {
                Ok(()) => Ok(RunOutcome::Done),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Ok(RunOutcome::Skipped("directory already absent".to_owned()))
                }
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
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
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
    if src == dest {
        return Ok(());
    }
    if dest.exists() {
        return Err(format!("destination already exists: {}", dest.display()));
    }
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
        AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, Operation,
        RelativePath, ReviewState, SessionId,
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

    #[test]
    fn refuses_to_overwrite_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), dir.path().to_path_buf());
        snapshot
            .entries
            .push(scan_like(dir.path(), "note.txt", b"hello"));
        let revision = accept_all(&propose(&snapshot, &ProposalPolicy::default()))
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, _) = validate(&snapshot, &revision);
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        fs::create_dir_all(dir.path().join("Documents")).unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("Documents/note.txt"), b"keep-me")
            .unwrap_or_else(|e| panic!("{e}"));
        let applied = apply_plan(&snapshot, &plan, false);
        assert_eq!(applied.status, JournalStatus::Failed);
        assert_eq!(
            fs::read(dir.path().join("note.txt")).ok(),
            Some(b"hello".to_vec())
        );
        assert_eq!(
            fs::read(dir.path().join("Documents/note.txt")).ok(),
            Some(b"keep-me".to_vec())
        );
    }

    #[test]
    fn undo_does_not_delete_a_preexisting_destination_folder() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::create_dir_all(dir.path().join("Documents")).unwrap_or_else(|e| panic!("{e}"));
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), dir.path().to_path_buf());
        snapshot.entries.push(ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse("Documents").unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::Directory,
            family: FileFamily::Generic,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        });
        snapshot
            .entries
            .push(scan_like(dir.path(), "note.txt", b"hello"));
        let revision = accept_all(&propose(&snapshot, &ProposalPolicy::default()))
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, _) = validate(&snapshot, &revision);
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        assert!(
            !plan.operations.iter().any(|op| matches!(
                op.operation,
                Operation::CreateDirectory { ref path } if path.as_str() == "Documents"
            )),
            "planner must not mkdir a folder that already exists in the snapshot"
        );
        let applied = apply_plan(&snapshot, &plan, false);
        assert_eq!(applied.status, JournalStatus::Completed);
        let undone = undo_journal(&snapshot, &applied);
        assert_eq!(undone.status, JournalStatus::Undone);
        assert!(
            dir.path().join("Documents").is_dir(),
            "preexisting Documents/ must survive undo"
        );
        assert!(dir.path().join("note.txt").exists());
    }
}
