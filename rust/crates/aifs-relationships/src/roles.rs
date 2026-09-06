//! Directory roles for mixed nested trees: libraries, inboxes, and leftover archives.

use aifs_domain::{
    Bundle, BundleConstraint, BundleId, BundleKind, DirectoryRoleKind, DirectoryRoleMatch,
    EntryKind, FileFamily, ProjectStrength, RelativePath, WorkspaceSnapshot,
};

const LIBRARY_NAMES: &[&str] = &[
    "music", "photos", "pictures", "videos", "movies", "library", "media",
];
const BROAD_NAMES: &[&str] = &[
    "downloads",
    "desktop",
    "inbox",
    "unsorted",
    "dump",
    "temp",
    "tmp",
    "incoming",
];
const ARCHIVE_NAMES: &[&str] = &["old", "archive", "archives", "backup", "bak", "final", "finals"];

/// Classifies directories and emits PreserveLayout bundles for units that must not flatten.
pub fn classify_directories(snapshot: &mut WorkspaceSnapshot) {
    let directories: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Directory)
        .map(|entry| (entry.id, entry.path.clone()))
        .collect();

    for (dir_id, root) in directories {
        if snapshot.projects.iter().any(|project| {
            project.root == root && project.strength == ProjectStrength::Strong
        }) {
            continue;
        }
        let name = root.file_name().to_ascii_lowercase();
        let files: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File && entry.path.starts_with(&root))
            .cloned()
            .collect();
        let Some(kind) = role_for(&name, &root, &files) else {
            continue;
        };
        let reason = role_reason(kind, &root);
        snapshot.directory_roles.push(DirectoryRoleMatch {
            root: root.clone(),
            kind,
            reason: reason.clone(),
        });
        if matches!(kind, DirectoryRoleKind::Library | DirectoryRoleKind::WeakArchive)
            && !files.is_empty()
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

fn role_for(
    name: &str,
    root: &RelativePath,
    files: &[aifs_domain::ObservedEntry],
) -> Option<DirectoryRoleKind> {
    if BROAD_NAMES.contains(&name) {
        return Some(DirectoryRoleKind::BroadInbox);
    }
    if ARCHIVE_NAMES.contains(&name) {
        return Some(DirectoryRoleKind::WeakArchive);
    }
    if is_year_name(name) && !path_has_broad_segment(root) {
        return Some(DirectoryRoleKind::WeakArchive);
    }
    if LIBRARY_NAMES.contains(&name) {
        return Some(DirectoryRoleKind::Library);
    }
    if files.len() >= 3 {
        let media = files
            .iter()
            .filter(|entry| {
                matches!(
                    entry.family,
                    FileFamily::Audio | FileFamily::Video | FileFamily::Image | FileFamily::RawImage
                )
            })
            .count();
        if media * 2 >= files.len() {
            return Some(DirectoryRoleKind::Library);
        }
    }
    None
}

fn is_year_name(name: &str) -> bool {
    name.len() == 4
        && name.chars().all(|ch| ch.is_ascii_digit())
        && (name.starts_with("19") || name.starts_with("20"))
}

fn path_has_broad_segment(path: &RelativePath) -> bool {
    path.as_str().split('/').any(|segment| {
        BROAD_NAMES.contains(&segment.to_ascii_lowercase().as_str())
    })
}

fn role_reason(kind: DirectoryRoleKind, root: &RelativePath) -> String {
    match kind {
        DirectoryRoleKind::Library => {
            format!("{} looks like an existing library; keep its layout", root.as_str())
        }
        DirectoryRoleKind::WeakArchive => format!(
            "{} looks like a previous archive attempt; keep path context",
            root.as_str()
        ),
        DirectoryRoleKind::BroadInbox => {
            format!("{} is a generic dump; files can be organised independently", root.as_str())
        }
        DirectoryRoleKind::Mixed => format!("{} has mixed contents", root.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        AssetId, FileIdentity, LockState, ObservedEntry, RelativePath, SessionId,
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

    fn dir(path: &str) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(path).unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::Directory,
            family: FileFamily::Generic,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    #[test]
    fn music_folder_is_a_library_unit() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Music"),
            dir("Music/Ada"),
            file("Music/Ada/night.mp3", FileFamily::Audio),
        ];
        classify_directories(&mut snapshot);
        assert!(snapshot
            .directory_roles
            .iter()
            .any(|role| role.kind == DirectoryRoleKind::Library && role.root.as_str() == "Music"));
        assert!(snapshot.bundles.iter().any(|bundle| {
            bundle.kind == BundleKind::Folder
                && matches!(bundle.constraint, BundleConstraint::PreserveLayout { .. })
        }));
    }

    #[test]
    fn old_archive_keeps_layout() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("old"),
            dir("old/2019"),
            file("old/2019/invoice.txt", FileFamily::Document),
        ];
        classify_directories(&mut snapshot);
        assert!(snapshot
            .directory_roles
            .iter()
            .any(|role| role.kind == DirectoryRoleKind::WeakArchive));
    }

    #[test]
    fn downloads_is_a_broad_inbox() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![dir("Downloads"), file("Downloads/a.txt", FileFamily::Document)];
        classify_directories(&mut snapshot);
        assert_eq!(
            snapshot.directory_roles[0].kind,
            DirectoryRoleKind::BroadInbox
        );
        assert!(snapshot.bundles.is_empty());
    }

    #[test]
    fn year_folder_inside_an_inbox_is_not_an_archive() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Downloads"),
            dir("Downloads/2024"),
            file("Downloads/2024/notes.txt", FileFamily::Document),
        ];
        classify_directories(&mut snapshot);
        assert!(
            !snapshot.directory_roles.iter().any(|role| {
                role.kind == DirectoryRoleKind::WeakArchive && role.root.as_str() == "Downloads/2024"
            }),
            "dated folders under a dump must stay organisable"
        );
    }
}
