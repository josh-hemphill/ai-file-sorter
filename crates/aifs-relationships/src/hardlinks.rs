//! Same-inode hard-link edges.

use aifs_domain::{Confidence, EntryKind, Relationship, RelationshipKind, WorkspaceSnapshot};
use std::collections::HashMap;

/// Records hard-link relationships for files that share a device and inode.
pub fn detect_hard_links(snapshot: &mut WorkspaceSnapshot) {
    let mut groups: HashMap<(u64, u64), Vec<_>> = HashMap::new();
    for entry in snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        if let (Some(device), Some(inode)) = (entry.identity.device, entry.identity.inode) {
            groups.entry((device, inode)).or_default().push(entry.id);
        }
    }
    for ids in groups.into_values() {
        if ids.len() < 2 {
            continue;
        }
        for pair in ids.windows(2) {
            snapshot.relationships.push(Relationship {
                from: pair[0],
                to: pair[1],
                kind: RelationshipKind::HardLink,
                confidence: Confidence::CERTAIN,
                detector: "inode".to_owned(),
                note: None,
            });
        }
    }
}
