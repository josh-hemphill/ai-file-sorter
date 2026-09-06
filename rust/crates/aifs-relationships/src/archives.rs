//! Split-archive part grouping.

use aifs_domain::{
    Bundle, BundleConstraint, BundleId, BundleKind, Confidence, EntryKind, ObservedEntry,
    Relationship, RelationshipKind, WorkspaceSnapshot,
};
use std::collections::HashMap;

use crate::sidecars::parent_key;

/// Groups numbered archive parts that share a base name.
pub fn detect_archive_parts(snapshot: &mut WorkspaceSnapshot) {
    let files: Vec<ObservedEntry> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .cloned()
        .collect();
    let mut groups: HashMap<(String, String), Vec<ObservedEntry>> = HashMap::new();
    for entry in files {
        if let Some(base) = archive_group_key(entry.path.file_name()) {
            groups
                .entry((parent_key(&entry), base))
                .or_default()
                .push(entry);
        }
    }

    let mut new_bundles = Vec::new();
    let mut new_relationships = Vec::new();
    for ((_, base), mut members) in groups {
        if members.len() < 2 {
            continue;
        }
        members.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
        let ids: Vec<_> = members.iter().map(|entry| entry.id).collect();
        let anchor = members
            .iter()
            .find(|entry| is_archive_anchor(entry.path.file_name()))
            .or_else(|| members.first())
            .map(|entry| entry.id);
        for window in ids.windows(2) {
            new_relationships.push(Relationship {
                from: window[0],
                to: window[1],
                kind: RelationshipKind::ArchivePart,
                confidence: Confidence::CERTAIN,
                detector: "archive-parts".to_owned(),
                note: None,
            });
        }
        new_bundles.push(Bundle {
            id: BundleId::new(),
            kind: BundleKind::ArchiveParts,
            label: base,
            members: ids,
            anchor,
            constraint: BundleConstraint::MoveTogether,
            reason: "Split archive parts must be kept together.".to_owned(),
        });
    }
    snapshot.bundles.extend(new_bundles);
    snapshot.relationships.extend(new_relationships);
}

fn archive_group_key(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    if let Some(base) = part_nn_base(&lower) {
        return Some(base);
    }
    if let Some(base) = numeric_extension_base(&lower) {
        return Some(base);
    }
    if let Some(stem) = split_volume_stem(&lower) {
        return Some(stem);
    }
    for ext in [".zip", ".rar", ".7z"] {
        if let Some(stem) = lower.strip_suffix(ext) {
            if !stem.is_empty() {
                return Some(stem.to_owned());
            }
        }
    }
    None
}

fn part_nn_base(lower: &str) -> Option<String> {
    let idx = lower.rfind(".part")?;
    let after = &lower[idx + 5..];
    let (num, ext) = after.split_once('.')?;
    if num.is_empty() || !num.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    if !matches!(ext, "rar" | "zip" | "7z") {
        return None;
    }
    let head = &lower[..idx];
    if head.is_empty() {
        return None;
    }
    Some(format!("{head}.{ext}"))
}

fn numeric_extension_base(lower: &str) -> Option<String> {
    let (stem, ext) = lower.rsplit_once('.')?;
    if ext.len() < 2 || ext.len() > 3 || !ext.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_owned())
}

fn split_volume_stem(lower: &str) -> Option<String> {
    let (stem, ext) = lower.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    if (ext.starts_with('z') || ext.starts_with('r'))
        && ext.len() == 3
        && ext[1..].chars().all(|ch| ch.is_ascii_digit())
    {
        return Some(stem.to_owned());
    }
    None
}

fn is_archive_anchor(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".zip") || lower.ends_with(".rar") || lower.ends_with(".7z")
}
