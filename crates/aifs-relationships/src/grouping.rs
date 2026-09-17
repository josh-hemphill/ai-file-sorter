//! Applies untrusted directory.grouping evidence after deterministic roles.

use aifs_domain::{
    Bundle, BundleConstraint, BundleId, BundleKind, DirectoryRoleKind, DirectoryRoleMatch,
    EntryKind, WorkspaceSnapshot, evidence::keys,
};

const GROUPING_DETECTOR_REASON: &str = "model grouping of this folder path";

/// Upgrades unlabeled directories from `directory.grouping` evidence.
///
/// Strong deterministic roles (dump names, event albums) are not overridden.
pub fn apply_grouping_evidence(snapshot: &mut WorkspaceSnapshot) {
    let mut directories: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Directory)
        .map(|entry| (entry.id, entry.path.clone()))
        .collect();
    directories.sort_by_key(|(_, root)| root.as_str().matches('/').count());

    for (dir_id, root) in directories {
        if snapshot.covering_layout_root(&root).is_some() {
            continue;
        }
        if snapshot
            .directory_roles
            .iter()
            .any(|role| role.root == root)
        {
            continue;
        }
        let Some(grouping) = snapshot
            .evidence_for(dir_id)
            .find_map(|bag| bag.fact(keys::DIRECTORY_GROUPING).map(str::to_owned))
        else {
            continue;
        };
        let Some(kind) = role_for_grouping(&grouping) else {
            continue;
        };
        let reason = snapshot
            .evidence_for(dir_id)
            .find_map(|bag| bag.fact(keys::DIRECTORY_GROUPING_REASON).map(str::to_owned))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| GROUPING_DETECTOR_REASON.to_owned());
        let has_files = snapshot
            .entries
            .iter()
            .any(|entry| entry.kind == EntryKind::File && entry.path.starts_with(&root));
        snapshot.directory_roles.push(DirectoryRoleMatch {
            root: root.clone(),
            kind,
            reason: reason.clone(),
        });
        if matches!(
            kind,
            DirectoryRoleKind::Library | DirectoryRoleKind::WeakArchive
        ) && has_files
        {
            let members: Vec<_> = snapshot
                .entries
                .iter()
                .filter(|entry| entry.path.starts_with(&root))
                .map(|entry| entry.id)
                .collect();
            snapshot.bundles.push(Bundle {
                id: BundleId::new(),
                kind: BundleKind::Folder,
                label: root.as_str().to_owned(),
                members,
                anchor: Some(dir_id),
                constraint: BundleConstraint::PreserveLayout { root },
                reason,
            });
        }
    }
}

fn role_for_grouping(grouping: &str) -> Option<DirectoryRoleKind> {
    match grouping {
        "camera_dump" | "broad_inbox" => Some(DirectoryRoleKind::BroadInbox),
        "event_or_date" | "library" => Some(DirectoryRoleKind::Library),
        "archive" => Some(DirectoryRoleKind::WeakArchive),
        "mixed" => Some(DirectoryRoleKind::Mixed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        AssetId, Confidence, Evidence, EvidenceSource, FileFamily, FileIdentity, LockState,
        ObservedEntry, RelativePath, SessionId,
    };
    use std::path::PathBuf;

    fn file(path: &str, family: FileFamily) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(path).unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::File,
            family,
            identity: FileIdentity {
                size: 10,
                ..FileIdentity::default()
            },
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    fn dir(path: &str) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(path).unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::Directory,
            family: FileFamily::Generic,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    fn grouping(asset: aifs_domain::AssetId, label: &str) -> Evidence {
        Evidence::new(
            asset,
            EvidenceSource::LocalModel {
                model: "stub".into(),
            },
            Confidence::new(0.4),
        )
        .with_fact(keys::DIRECTORY_GROUPING, label)
        .with_fact(keys::DIRECTORY_GROUPING_REASON, "stub reason")
    }

    fn role_kind(snapshot: &WorkspaceSnapshot, root: &str) -> Option<DirectoryRoleKind> {
        snapshot
            .directory_roles
            .iter()
            .find(|role| role.root.as_str() == root)
            .map(|role| role.kind)
    }

    #[test]
    fn unlabeled_export_dump_opens_for_describe() {
        let export = dir("Export");
        let image = file("Export/IMG_001.jpg", FileFamily::Image);
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.evidence.push(grouping(export.id, "camera_dump"));
        snapshot.entries = vec![export, image.clone()];
        apply_grouping_evidence(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Export"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert!(!snapshot.defers_content_analysis(&image));
    }

    #[test]
    fn unlabeled_holiday_album_stays_a_unit() {
        let holiday = dir("Holiday");
        let image = file("Holiday/IMG_001.jpg", FileFamily::Image);
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot
            .evidence
            .push(grouping(holiday.id, "event_or_date"));
        snapshot.entries = vec![holiday, image.clone()];
        apply_grouping_evidence(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Holiday"),
            Some(DirectoryRoleKind::Library)
        );
        assert!(snapshot.defers_content_analysis(&image));
    }

    #[test]
    fn deterministic_dump_role_is_not_overridden() {
        let dcim = dir("DCIM");
        let image = file("DCIM/IMG_001.jpg", FileFamily::Image);
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.directory_roles.push(DirectoryRoleMatch {
            root: dcim.path.clone(),
            kind: DirectoryRoleKind::BroadInbox,
            reason: "dump".into(),
        });
        snapshot.evidence.push(grouping(dcim.id, "event_or_date"));
        snapshot.entries = vec![dcim, image.clone()];
        apply_grouping_evidence(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "DCIM"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert!(!snapshot.defers_content_analysis(&image));
    }
}
