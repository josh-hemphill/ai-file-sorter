//! Numbered-sequence (burst / episode) grouping.

use aifs_domain::{
    Bundle, BundleConstraint, BundleId, BundleKind, Confidence, EntryKind, ObservedEntry,
    Relationship, RelationshipKind, WorkspaceSnapshot,
};
use std::collections::HashMap;

use crate::sidecars::parent_key;

const MIN_SERIES_LEN: usize = 3;
const MAX_SERIES_LEN: usize = 24;

/// Groups files in the same folder that share a stem prefix and trailing digits.
pub fn detect_series(snapshot: &mut WorkspaceSnapshot) {
    let files: Vec<ObservedEntry> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .cloned()
        .collect();
    let mut groups: HashMap<(String, String, String), Vec<(ObservedEntry, u32)>> = HashMap::new();
    for entry in files {
        let Some((prefix, number)) = split_numbered_stem(entry.stem()) else {
            continue;
        };
        let ext = entry.extension().unwrap_or_default();
        groups
            .entry((parent_key(&entry), prefix, ext))
            .or_default()
            .push((entry, number));
    }

    let mut new_bundles = Vec::new();
    let mut new_relationships = Vec::new();
    for ((_, prefix, ext), mut members) in groups {
        members.sort_by_key(|(_, number)| *number);
        members.dedup_by_key(|(_, number)| *number);
        for run in consecutive_runs(&members) {
            if run.len() < MIN_SERIES_LEN || run.len() > MAX_SERIES_LEN {
                continue;
            }
            let ids: Vec<_> = run.iter().map(|(entry, _)| entry.id).collect();
            for window in ids.windows(2) {
                new_relationships.push(Relationship {
                    from: window[0],
                    to: window[1],
                    kind: RelationshipKind::SeriesMember,
                    confidence: Confidence::new(0.7),
                    detector: "numbered-series".to_owned(),
                    note: None,
                });
            }
            let label = if ext.is_empty() {
                prefix.clone()
            } else {
                format!("{prefix}*.{ext}")
            };
            new_bundles.push(Bundle {
                id: BundleId::new(),
                kind: BundleKind::Series,
                label,
                members: ids,
                anchor: run.first().map(|(entry, _)| entry.id),
                constraint: BundleConstraint::Soft,
                reason: "Numbered burst in the same folder.".to_owned(),
            });
        }
    }
    snapshot.bundles.extend(new_bundles);
    snapshot.relationships.extend(new_relationships);
}

fn consecutive_runs(members: &[(ObservedEntry, u32)]) -> Vec<&[(ObservedEntry, u32)]> {
    let mut runs = Vec::new();
    let mut start = 0;
    for index in 1..=members.len() {
        let split =
            index == members.len() || members[index].1 != members[index - 1].1.saturating_add(1);
        if split {
            runs.push(&members[start..index]);
            start = index;
        }
    }
    runs
}

fn split_numbered_stem(stem: &str) -> Option<(String, u32)> {
    let bytes = stem.as_bytes();
    let mut idx = bytes.len();
    while idx > 0 && bytes[idx - 1].is_ascii_digit() {
        idx -= 1;
    }
    if idx == 0 || idx == bytes.len() {
        return None;
    }
    let number: u32 = stem[idx..].parse().ok()?;
    Some((stem[..idx].to_ascii_lowercase(), number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{AssetId, FileFamily, FileIdentity, LockState, RelativePath, SessionId};
    use std::path::PathBuf;

    fn image(path: &str) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(path).unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::File,
            family: FileFamily::Image,
            identity: FileIdentity {
                size: 10,
                ..FileIdentity::default()
            },
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    fn snapshot_with(entries: Vec<ObservedEntry>) -> WorkspaceSnapshot {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = entries;
        snapshot
    }

    fn series_sizes(snapshot: &WorkspaceSnapshot) -> Vec<usize> {
        let mut sizes: Vec<_> = snapshot
            .bundles
            .iter()
            .filter(|bundle| bundle.kind == BundleKind::Series)
            .map(|bundle| bundle.members.len())
            .collect();
        sizes.sort_unstable();
        sizes
    }

    #[test]
    fn three_consecutive_stills_form_a_burst() {
        let mut snapshot = snapshot_with(vec![
            image("IMG_001.jpg"),
            image("IMG_002.jpg"),
            image("IMG_003.jpg"),
        ]);
        detect_series(&mut snapshot);
        assert_eq!(series_sizes(&snapshot), vec![3]);
    }

    #[test]
    fn a_long_camera_roll_is_not_one_series() {
        let entries: Vec<_> = (1..=30)
            .map(|index| image(&format!("IMG_{index:03}.jpg")))
            .collect();
        let mut snapshot = snapshot_with(entries);
        detect_series(&mut snapshot);
        assert!(
            snapshot
                .bundles
                .iter()
                .all(|bundle| bundle.kind != BundleKind::Series),
            "a 30-file consecutive roll must not be treated as a burst, got {:?}",
            snapshot.bundles
        );
    }

    #[test]
    fn gapped_stills_split_into_separate_bursts() {
        let mut snapshot = snapshot_with(vec![
            image("IMG_001.jpg"),
            image("IMG_002.jpg"),
            image("IMG_003.jpg"),
            image("IMG_020.jpg"),
            image("IMG_021.jpg"),
            image("IMG_022.jpg"),
        ]);
        detect_series(&mut snapshot);
        assert_eq!(series_sizes(&snapshot), vec![3, 3]);
    }
}
