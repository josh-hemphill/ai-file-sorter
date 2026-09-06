//! Deterministic relationship detectors: projects, sidecars, series, archives, hard links.

pub mod archives;
pub mod hardlinks;
pub mod projects;
pub mod series;
pub mod sidecars;

pub use projects::{detect_project, should_skip_traversal, DetectedProject};

use aifs_domain::{
    Bundle, BundleConstraint, BundleId, BundleKind, ProjectStrength, WorkspaceSnapshot,
};

/// Fills `snapshot.bundles` and `snapshot.relationships` from already-observed entries
/// and project matches. Does not touch the filesystem except via project detection that
/// already happened during the scan.
pub fn enrich(snapshot: &mut WorkspaceSnapshot, protect_projects: bool) {
    add_project_bundles(snapshot, protect_projects);
    sidecars::detect_sidecars(snapshot);
    archives::detect_archive_parts(snapshot);
    series::detect_series(snapshot);
    hardlinks::detect_hard_links(snapshot);
}

fn add_project_bundles(snapshot: &mut WorkspaceSnapshot, protect_projects: bool) {
    if !protect_projects {
        return;
    }
    let projects = snapshot.projects.clone();
    for project in projects {
        if project.strength != ProjectStrength::Strong {
            continue;
        }
        let members: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|entry| entry.path.starts_with(&project.root))
            .map(|entry| entry.id)
            .collect();
        if members.is_empty() {
            continue;
        }
        let anchor = members.first().copied();
        snapshot.bundles.push(Bundle {
            id: BundleId::new(),
            kind: BundleKind::Project,
            label: project.name.clone(),
            members,
            anchor,
            constraint: BundleConstraint::Protected {
                reason: project.reason.clone(),
            },
            reason: format!("{} ({})", project.name, project.rule_id),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelationshipKind,
        RelativePath, SessionId,
    };
    use std::path::PathBuf;

    fn file(path: &str, family: FileFamily) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(path).unwrap_or_else(|e| panic!("{e}")),
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

    fn snapshot_with(entries: Vec<ObservedEntry>) -> WorkspaceSnapshot {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/inbox"));
        snapshot.entries = entries;
        snapshot
    }

    #[test]
    fn raw_jpeg_xmp_form_a_move_together_bundle() {
        let mut snapshot = snapshot_with(vec![
            file("trip/IMG_1.CR2", FileFamily::RawImage),
            file("trip/IMG_1.jpg", FileFamily::Image),
            file("trip/IMG_1.xmp", FileFamily::Sidecar),
        ]);
        enrich(&mut snapshot, true);
        let bundle = snapshot
            .bundles
            .iter()
            .find(|bundle| bundle.kind == BundleKind::SidecarGroup)
            .unwrap_or_else(|| panic!("sidecar bundle"));
        assert_eq!(bundle.members.len(), 3);
        assert!(bundle.is_hard());
        assert_eq!(
            snapshot
                .relationships
                .iter()
                .filter(|rel| rel.kind == RelationshipKind::Sidecar)
                .count(),
            2
        );
    }

    #[test]
    fn video_and_language_subtitle_group() {
        let mut snapshot = snapshot_with(vec![
            file("movie.mp4", FileFamily::Video),
            file("movie.en.srt", FileFamily::Subtitle),
        ]);
        enrich(&mut snapshot, true);
        assert!(snapshot
            .bundles
            .iter()
            .any(|bundle| bundle.kind == BundleKind::SidecarGroup && bundle.members.len() == 2));
        assert!(snapshot
            .relationships
            .iter()
            .any(|rel| rel.kind == RelationshipKind::Subtitle));
    }

    #[test]
    fn split_zip_parts_move_together() {
        let mut snapshot = snapshot_with(vec![
            file("backup.zip", FileFamily::Archive),
            file("backup.z01", FileFamily::Archive),
            file("backup.z02", FileFamily::Archive),
        ]);
        enrich(&mut snapshot, true);
        let bundle = snapshot
            .bundles
            .iter()
            .find(|bundle| bundle.kind == BundleKind::ArchiveParts)
            .unwrap_or_else(|| panic!("archive bundle"));
        assert_eq!(bundle.members.len(), 3);
        assert_eq!(bundle.constraint, BundleConstraint::MoveTogether);
    }

    #[test]
    fn numbered_photos_form_a_soft_series() {
        let mut snapshot = snapshot_with(vec![
            file("IMG_001.jpg", FileFamily::Image),
            file("IMG_002.jpg", FileFamily::Image),
            file("IMG_003.jpg", FileFamily::Image),
        ]);
        enrich(&mut snapshot, true);
        let bundle = snapshot
            .bundles
            .iter()
            .find(|bundle| bundle.kind == BundleKind::Series)
            .unwrap_or_else(|| panic!("series bundle"));
        assert_eq!(bundle.members.len(), 3);
        assert!(!bundle.is_hard());
    }
}
