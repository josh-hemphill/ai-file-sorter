//! Carry extract/analyze evidence across scans of the same session root.

use aifs_domain::{AssetId, EvidenceSource, ObservedEntry, WorkspaceSnapshot, evidence::keys};
use std::collections::HashMap;
use std::path::Path;

/// How often extract/analyze flush evidence into the session snapshot.
pub const CHECKPOINT_EVERY: usize = 8;

/// True when `stored` and `requested` name the same directory.
pub fn roots_match(stored: &Path, requested: &Path) -> bool {
    match (stored.canonicalize(), requested.canonicalize()) {
        (Ok(stored), Ok(requested)) => stored == requested,
        _ => stored == requested,
    }
}

/// Copies prior evidence onto `snapshot` for files whose path and identity still match.
pub fn carry_evidence(snapshot: &mut WorkspaceSnapshot, prior: &WorkspaceSnapshot) {
    let mut old_to_new: HashMap<AssetId, AssetId> = HashMap::new();
    for entry in &snapshot.entries {
        if let Some(old) = matching_entry(prior, entry) {
            old_to_new.insert(old.id, entry.id);
        }
    }
    for bag in &prior.evidence {
        let Some(&new_id) = old_to_new.get(&bag.asset) else {
            continue;
        };
        if snapshot
            .evidence
            .iter()
            .any(|existing| existing.asset == new_id && existing.source == bag.source)
        {
            continue;
        }
        let mut bag = bag.clone();
        bag.asset = new_id;
        snapshot.evidence.push(bag);
    }
}

/// True when extract already recorded media/EXIF/document facts for `entry`.
pub fn has_extract_evidence(snapshot: &WorkspaceSnapshot, entry: &ObservedEntry) -> bool {
    snapshot.evidence.iter().any(|bag| {
        bag.asset == entry.id
            && matches!(
                bag.source,
                EvidenceSource::MediaTags | EvidenceSource::Exif | EvidenceSource::DocumentMetadata
            )
            && !bag.is_empty()
    })
}

/// True when categorize already produced a category for `entry`.
pub fn has_category_evidence(snapshot: &WorkspaceSnapshot, entry: &ObservedEntry) -> bool {
    has_fact(snapshot, entry, keys::CATEGORY)
}

/// True when describe already produced a caption for `entry`.
pub fn has_description_evidence(snapshot: &WorkspaceSnapshot, entry: &ObservedEntry) -> bool {
    has_fact(snapshot, entry, keys::DESCRIPTION)
}

/// Files in `snapshot` that already have extract evidence.
pub fn extract_done_count(snapshot: &WorkspaceSnapshot) -> usize {
    snapshot
        .entries
        .iter()
        .filter(|entry| has_extract_evidence(snapshot, entry))
        .count()
}

fn has_fact(snapshot: &WorkspaceSnapshot, entry: &ObservedEntry, key: &str) -> bool {
    snapshot
        .evidence_for(entry.id)
        .any(|bag| bag.fact(key).is_some())
}

fn matching_entry<'a>(
    prior: &'a WorkspaceSnapshot,
    entry: &ObservedEntry,
) -> Option<&'a ObservedEntry> {
    prior
        .entries
        .iter()
        .find(|old| old.path == entry.path && old.identity.matches(&entry.identity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        Confidence, EntryKind, Evidence, FileFamily, FileIdentity, LockState, RelativePath,
        SessionId,
    };
    use std::path::PathBuf;

    fn file(path: &str, size: u64) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(path).unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::Audio,
            identity: FileIdentity {
                size,
                ..FileIdentity::default()
            },
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    #[test]
    fn carry_remaps_evidence_for_matching_paths() {
        let mut prior = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/inbox"));
        let old = file("show.mp3", 12);
        let bag = Evidence::new(old.id, EvidenceSource::MediaTags, Confidence::CERTAIN)
            .with_fact(keys::MEDIA_TITLE, "Night Drive");
        prior.entries.push(old.clone());
        prior.evidence.push(bag);

        let mut next = WorkspaceSnapshot::new(prior.session, PathBuf::from("/tmp/inbox"));
        let new = file("show.mp3", 12);
        next.entries.push(new.clone());
        carry_evidence(&mut next, &prior);
        assert!(has_extract_evidence(&next, &new));
        assert_eq!(
            next.evidence_for(new.id)
                .next()
                .and_then(|bag| bag.fact(keys::MEDIA_TITLE)),
            Some("Night Drive")
        );
    }

    #[test]
    fn changed_identity_is_not_carried() {
        let mut prior = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/inbox"));
        let old = file("show.mp3", 12);
        prior.entries.push(old.clone());
        prior.evidence.push(
            Evidence::new(old.id, EvidenceSource::MediaTags, Confidence::CERTAIN)
                .with_fact(keys::MEDIA_TITLE, "Night Drive"),
        );
        let mut next = WorkspaceSnapshot::new(prior.session, PathBuf::from("/tmp/inbox"));
        let changed = file("show.mp3", 99);
        next.entries.push(changed.clone());
        carry_evidence(&mut next, &prior);
        assert!(!has_extract_evidence(&next, &changed));
    }
}
