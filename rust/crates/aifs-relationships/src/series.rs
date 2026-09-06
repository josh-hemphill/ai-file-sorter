//! Numbered-sequence (burst / episode) grouping.

use aifs_domain::{
    Bundle, BundleConstraint, BundleId, BundleKind, Confidence, EntryKind, ObservedEntry,
    Relationship, RelationshipKind, WorkspaceSnapshot,
};
use std::collections::HashMap;

use crate::sidecars::parent_key;

const MIN_SERIES_LEN: usize = 3;

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
        if members.len() < MIN_SERIES_LEN {
            continue;
        }
        let ids: Vec<_> = members.iter().map(|(entry, _)| entry.id).collect();
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
            prefix
        } else {
            format!("{prefix}*.{ext}")
        };
        new_bundles.push(Bundle {
            id: BundleId::new(),
            kind: BundleKind::Series,
            label,
            members: ids,
            anchor: members.first().map(|(entry, _)| entry.id),
            constraint: BundleConstraint::Soft,
            reason: "Numbered sequence in the same folder.".to_owned(),
        });
    }
    snapshot.bundles.extend(new_bundles);
    snapshot.relationships.extend(new_relationships);
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
