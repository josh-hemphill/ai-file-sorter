//! Directory roles for mixed nested trees: libraries, inboxes, and leftover archives.

use aifs_domain::{
    Bundle, BundleConstraint, BundleId, BundleKind, DirectoryRoleKind, DirectoryRoleMatch,
    EntryKind, FileFamily, ObservedEntry, ProjectStrength, RelativePath, WorkspaceSnapshot,
};

use crate::folder_shape::{
    BROAD_NAMES, FolderShape, files_look_like_camera_dump, folder_shape, is_date_folder_name,
    is_dump_shaped, is_year_name, path_has_broad_segment,
};

/// Classifies directories and emits PreserveLayout bundles for units that must not flatten.
pub fn classify_directories(snapshot: &mut WorkspaceSnapshot) {
    let mut directories: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Directory)
        .map(|entry| (entry.id, entry.path.clone()))
        .collect();
    directories.sort_by_key(|(_, root)| root.as_str().matches('/').count());

    for (dir_id, root) in directories {
        if snapshot
            .projects
            .iter()
            .any(|project| project.root == root && project.strength == ProjectStrength::Strong)
        {
            continue;
        }
        if ancestor_freezes_layout(snapshot, &root) {
            continue;
        }
        let name = root.file_name().to_ascii_lowercase();
        let files: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File && entry.path.starts_with(&root))
            .cloned()
            .collect();
        let session_root_name = snapshot
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let parent_name = root.parent().map(|parent| parent.file_name().to_owned());
        let child_dirs: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|entry| {
                entry.kind == EntryKind::Directory && entry.path.parent().as_ref() == Some(&root)
            })
            .map(|entry| entry.path.clone())
            .collect();
        let Some(kind) = role_for(
            &name,
            &root,
            &files,
            &session_root_name,
            parent_name.as_deref(),
            &child_dirs,
            snapshot,
        ) else {
            continue;
        };
        let reason = role_reason(kind, &root);
        snapshot.directory_roles.push(DirectoryRoleMatch {
            root: root.clone(),
            kind,
            reason: reason.clone(),
        });
        if matches!(
            kind,
            DirectoryRoleKind::Library | DirectoryRoleKind::WeakArchive
        ) && !files.is_empty()
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

fn ancestor_freezes_layout(snapshot: &WorkspaceSnapshot, root: &RelativePath) -> bool {
    let files: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File && entry.path.starts_with(root))
        .cloned()
        .collect();
    let parent_name = root.parent().map(|parent| parent.file_name().to_owned());
    let shape = folder_shape(
        &root.file_name().to_ascii_lowercase(),
        parent_name.as_deref(),
        &files,
    );
    if is_dump_shaped(shape) {
        return false;
    }
    snapshot.directory_roles.iter().any(|role| {
        matches!(
            role.kind,
            DirectoryRoleKind::Library | DirectoryRoleKind::WeakArchive
        ) && root.starts_with(&role.root)
            && root != &role.root
    })
}

fn role_for(
    name: &str,
    root: &RelativePath,
    files: &[ObservedEntry],
    session_root_name: &str,
    parent_name: Option<&str>,
    child_dirs: &[RelativePath],
    snapshot: &WorkspaceSnapshot,
) -> Option<DirectoryRoleKind> {
    let shape = folder_shape(name, parent_name, files);
    match shape {
        FolderShape::BroadInbox => return Some(DirectoryRoleKind::BroadInbox),
        FolderShape::ArchiveName => return Some(DirectoryRoleKind::WeakArchive),
        FolderShape::CameraDump => return Some(DirectoryRoleKind::BroadInbox),
        FolderShape::EventOrDate => {
            return event_or_date_role(name, root, files, session_root_name);
        }
        FolderShape::LibraryName => {
            return Some(library_named_role(root, files, child_dirs, snapshot));
        }
        FolderShape::Unknown => {}
    }
    if files_are_mostly_media(files) && !files_look_like_camera_dump(files) {
        return Some(DirectoryRoleKind::Library);
    }
    None
}

fn event_or_date_role(
    name: &str,
    root: &RelativePath,
    files: &[ObservedEntry],
    session_root_name: &str,
) -> Option<DirectoryRoleKind> {
    let under_inbox = path_has_broad_segment(root)
        || (root.parent().is_none() && BROAD_NAMES.contains(&session_root_name));
    if is_date_folder_name(name) && under_inbox {
        return None;
    }
    if files.iter().any(|entry| {
        matches!(
            entry.family,
            FileFamily::Image | FileFamily::RawImage | FileFamily::Video | FileFamily::Audio
        )
    }) {
        return Some(DirectoryRoleKind::Library);
    }
    if is_year_name(name) {
        return Some(DirectoryRoleKind::WeakArchive);
    }
    None
}

fn library_named_role(
    root: &RelativePath,
    files: &[ObservedEntry],
    child_dirs: &[RelativePath],
    snapshot: &WorkspaceSnapshot,
) -> DirectoryRoleKind {
    if child_dirs.is_empty() {
        if files_look_like_camera_dump(files) {
            return DirectoryRoleKind::BroadInbox;
        }
        return DirectoryRoleKind::Library;
    }
    let mut dump_kids = 0usize;
    let mut keep_kids = 0usize;
    for child in child_dirs {
        let child_files: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File && entry.path.starts_with(child))
            .cloned()
            .collect();
        let child_shape = folder_shape(
            &child.file_name().to_ascii_lowercase(),
            Some(&root.file_name().to_ascii_lowercase()),
            &child_files,
        );
        if is_dump_shaped(child_shape) {
            dump_kids += 1;
        } else {
            keep_kids += 1;
        }
    }
    if dump_kids > 0 && keep_kids == 0 {
        return DirectoryRoleKind::BroadInbox;
    }
    if dump_kids > 0 && keep_kids > 0 {
        return DirectoryRoleKind::Mixed;
    }
    DirectoryRoleKind::Library
}

fn files_are_mostly_media(files: &[ObservedEntry]) -> bool {
    if files.len() < 3 {
        return false;
    }
    let media = files
        .iter()
        .filter(|entry| {
            matches!(
                entry.family,
                FileFamily::Audio | FileFamily::Video | FileFamily::Image | FileFamily::RawImage
            )
        })
        .count();
    media * 2 >= files.len()
}

fn role_reason(kind: DirectoryRoleKind, root: &RelativePath) -> String {
    match kind {
        DirectoryRoleKind::Library => {
            format!(
                "{} looks like an existing library; keep its layout",
                root.as_str()
            )
        }
        DirectoryRoleKind::WeakArchive => format!(
            "{} looks like a previous archive attempt; keep path context",
            root.as_str()
        ),
        DirectoryRoleKind::BroadInbox => {
            format!(
                "{} is a generic dump; files can be organised independently",
                root.as_str()
            )
        }
        DirectoryRoleKind::Mixed => format!(
            "{} has organised albums and camera dumps; dumps can still be sorted",
            root.as_str()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{AssetId, FileIdentity, LockState, SessionId};
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

    fn role_kind(snapshot: &WorkspaceSnapshot, root: &str) -> Option<DirectoryRoleKind> {
        snapshot
            .directory_roles
            .iter()
            .find(|role| role.root.as_str() == root)
            .map(|role| role.kind)
    }

    fn preserves(snapshot: &WorkspaceSnapshot, root: &str) -> bool {
        snapshot
            .bundles
            .iter()
            .any(|bundle| match &bundle.constraint {
                BundleConstraint::PreserveLayout { root: bundle_root } => {
                    bundle_root.as_str() == root
                }
                _ => false,
            })
    }

    fn entry_named<'a>(snapshot: &'a WorkspaceSnapshot, path: &str) -> &'a ObservedEntry {
        snapshot
            .entries
            .iter()
            .find(|entry| entry.path.as_str() == path)
            .unwrap_or_else(|| panic!("{path}"))
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
        assert!(
            snapshot.directory_roles.iter().any(
                |role| role.kind == DirectoryRoleKind::Library && role.root.as_str() == "Music"
            )
        );
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
        assert!(
            snapshot
                .directory_roles
                .iter()
                .any(|role| role.kind == DirectoryRoleKind::WeakArchive)
        );
    }

    #[test]
    fn downloads_is_a_broad_inbox() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Downloads"),
            file("Downloads/a.txt", FileFamily::Document),
        ];
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
            file("Downloads/2024/a.jpg", FileFamily::Image),
            file("Downloads/2024/b.jpg", FileFamily::Image),
            file("Downloads/2024/c.jpg", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(role_kind(&snapshot, "Downloads/2024"), None);
        assert!(!preserves(&snapshot, "Downloads/2024"));
        assert!(!snapshot.defers_content_analysis(entry_named(&snapshot, "Downloads/2024/a.jpg")));
    }

    #[test]
    fn year_folder_at_a_downloads_scan_root_is_not_an_archive() {
        let mut snapshot =
            WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/Downloads"));
        snapshot.entries = vec![
            dir("2024"),
            file("2024/a.jpg", FileFamily::Image),
            file("2024/b.jpg", FileFamily::Image),
            file("2024/c.jpg", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(role_kind(&snapshot, "2024"), None);
        assert!(!preserves(&snapshot, "2024"));
        assert!(!snapshot.defers_content_analysis(entry_named(&snapshot, "2024/a.jpg")));
    }

    #[test]
    fn nested_year_folder_under_downloads_scan_root_can_still_be_an_archive() {
        let mut snapshot =
            WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/Downloads"));
        snapshot.entries = vec![
            dir("Projects"),
            dir("Projects/2019"),
            file("Projects/2019/notes.txt", FileFamily::Document),
        ];
        classify_directories(&mut snapshot);
        assert!(
            snapshot.directory_roles.iter().any(|role| {
                role.kind == DirectoryRoleKind::WeakArchive && role.root.as_str() == "Projects/2019"
            }),
            "nested year folders that are not dump children should keep archive context"
        );
    }

    #[test]
    fn nested_media_folders_inherit_the_outermost_library_unit() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Pictures"),
            dir("Pictures/Lensa"),
            dir("Pictures/Screenshots"),
            file("Pictures/a.jpg", FileFamily::Image),
            file("Pictures/b.jpg", FileFamily::Image),
            file("Pictures/c.jpg", FileFamily::Image),
            file("Pictures/Lensa/one.jpg", FileFamily::Image),
            file("Pictures/Lensa/two.jpg", FileFamily::Image),
            file("Pictures/Lensa/three.jpg", FileFamily::Image),
            file("Pictures/Screenshots/one.png", FileFamily::Image),
            file("Pictures/Screenshots/two.png", FileFamily::Image),
            file("Pictures/Screenshots/three.png", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        let libraries: Vec<_> = snapshot
            .directory_roles
            .iter()
            .filter(|role| role.kind == DirectoryRoleKind::Library)
            .map(|role| role.root.as_str())
            .collect();
        assert_eq!(libraries, vec!["Pictures"]);
        let units: Vec<_> = snapshot
            .bundles
            .iter()
            .filter(|bundle| matches!(bundle.constraint, BundleConstraint::PreserveLayout { .. }))
            .map(|bundle| bundle.label.as_str())
            .collect();
        assert_eq!(units, vec!["Pictures"]);
    }

    #[test]
    fn nested_pictures_camera_stack_is_a_broad_inbox() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Photos"),
            dir("Photos/Pictures"),
            dir("Photos/Pictures/DCIM"),
            file("Photos/Pictures/DCIM/IMG_001.jpg", FileFamily::Image),
            file("Photos/Pictures/DCIM/IMG_002.jpg", FileFamily::Image),
            file("Photos/Pictures/DCIM/IMG_003.jpg", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Photos"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert_eq!(
            role_kind(&snapshot, "Photos/Pictures"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert_eq!(
            role_kind(&snapshot, "Photos/Pictures/DCIM"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert!(!preserves(&snapshot, "Photos"));
        assert!(!preserves(&snapshot, "Photos/Pictures"));
        assert!(!preserves(&snapshot, "Photos/Pictures/DCIM"));
        assert!(
            !snapshot.defers_content_analysis(entry_named(
                &snapshot,
                "Photos/Pictures/DCIM/IMG_001.jpg"
            ))
        );
    }

    #[test]
    fn event_folder_of_camera_files_stays_a_library_unit() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Photos"),
            dir("Photos/Italy 2019"),
            file("Photos/Italy 2019/IMG_001.jpg", FileFamily::Image),
            file("Photos/Italy 2019/IMG_002.jpg", FileFamily::Image),
            file("Photos/Italy 2019/IMG_003.jpg", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Photos"),
            Some(DirectoryRoleKind::Library)
        );
        assert!(preserves(&snapshot, "Photos"));
        assert_eq!(role_kind(&snapshot, "Photos/Italy 2019"), None);
        assert!(
            snapshot
                .defers_content_analysis(entry_named(&snapshot, "Photos/Italy 2019/IMG_001.jpg"))
        );
    }

    #[test]
    fn mixed_photos_keeps_event_folders_and_opens_dumps() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Photos"),
            dir("Photos/Italy"),
            dir("Photos/DCIM"),
            file("Photos/Italy/IMG_001.jpg", FileFamily::Image),
            file("Photos/Italy/IMG_002.jpg", FileFamily::Image),
            file("Photos/Italy/IMG_003.jpg", FileFamily::Image),
            file("Photos/DCIM/IMG_010.jpg", FileFamily::Image),
            file("Photos/DCIM/IMG_011.jpg", FileFamily::Image),
            file("Photos/DCIM/IMG_012.jpg", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Photos"),
            Some(DirectoryRoleKind::Mixed)
        );
        assert!(!preserves(&snapshot, "Photos"));
        assert_eq!(
            role_kind(&snapshot, "Photos/Italy"),
            Some(DirectoryRoleKind::Library)
        );
        assert!(preserves(&snapshot, "Photos/Italy"));
        assert_eq!(
            role_kind(&snapshot, "Photos/DCIM"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert!(!preserves(&snapshot, "Photos/DCIM"));
        assert!(
            snapshot.defers_content_analysis(entry_named(&snapshot, "Photos/Italy/IMG_001.jpg"))
        );
        assert!(
            !snapshot.defers_content_analysis(entry_named(&snapshot, "Photos/DCIM/IMG_010.jpg"))
        );
    }

    #[test]
    fn leaf_photos_of_camera_files_is_a_broad_inbox() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Photos"),
            file("Photos/IMG_001.jpg", FileFamily::Image),
            file("Photos/IMG_002.jpg", FileFamily::Image),
            file("Photos/IMG_003.jpg", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Photos"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert!(!preserves(&snapshot, "Photos"));
        assert!(!snapshot.defers_content_analysis(entry_named(&snapshot, "Photos/IMG_001.jpg")));
    }

    #[test]
    fn top_level_wedding_album_stays_a_library_unit() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Wedding"),
            file("Wedding/IMG_001.jpg", FileFamily::Image),
            file("Wedding/IMG_002.jpg", FileFamily::Image),
            file("Wedding/IMG_003.jpg", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Wedding"),
            Some(DirectoryRoleKind::Library)
        );
        assert!(preserves(&snapshot, "Wedding"));
        assert!(snapshot.defers_content_analysis(entry_named(&snapshot, "Wedding/IMG_001.jpg")));
    }

    #[test]
    fn movies_videos_stays_a_library_unit() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Movies"),
            dir("Movies/Videos"),
            file("Movies/Videos/clip.mp4", FileFamily::Video),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Movies"),
            Some(DirectoryRoleKind::Library)
        );
        assert!(preserves(&snapshot, "Movies"));
        assert_eq!(role_kind(&snapshot, "Movies/Videos"), None);
        assert!(snapshot.defers_content_analysis(entry_named(&snapshot, "Movies/Videos/clip.mp4")));
    }

    #[test]
    fn photos_videos_and_leaf_numbered_videos_stay_library_units() {
        let mut nested = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        nested.entries = vec![
            dir("Photos"),
            dir("Photos/Videos"),
            file("Photos/Videos/clip.mp4", FileFamily::Video),
        ];
        classify_directories(&mut nested);
        assert_eq!(
            role_kind(&nested, "Photos"),
            Some(DirectoryRoleKind::Library)
        );
        assert!(preserves(&nested, "Photos"));
        assert_eq!(role_kind(&nested, "Photos/Videos"), None);

        let mut leaf = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        leaf.entries = vec![
            dir("Videos"),
            file("Videos/video_1.mp4", FileFamily::Video),
            file("Videos/video_2.mp4", FileFamily::Video),
            file("Videos/video_3.mp4", FileFamily::Video),
        ];
        classify_directories(&mut leaf);
        assert_eq!(role_kind(&leaf, "Videos"), Some(DirectoryRoleKind::Library));
        assert!(preserves(&leaf, "Videos"));
        assert!(leaf.defers_content_analysis(entry_named(&leaf, "Videos/video_1.mp4")));
    }

    #[test]
    fn camera_roll_and_new_folder_of_stills_are_dumps() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp"));
        snapshot.entries = vec![
            dir("Camera Roll"),
            file("Camera Roll/IMG_001.jpg", FileFamily::Image),
            file("Camera Roll/IMG_002.jpg", FileFamily::Image),
            file("Camera Roll/IMG_003.jpg", FileFamily::Image),
            dir("New Folder"),
            file("New Folder/IMG_010.jpg", FileFamily::Image),
            file("New Folder/IMG_011.jpg", FileFamily::Image),
            file("New Folder/IMG_012.jpg", FileFamily::Image),
        ];
        classify_directories(&mut snapshot);
        assert_eq!(
            role_kind(&snapshot, "Camera Roll"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert!(!preserves(&snapshot, "Camera Roll"));
        assert!(
            !snapshot.defers_content_analysis(entry_named(&snapshot, "Camera Roll/IMG_001.jpg"))
        );
        assert_eq!(
            role_kind(&snapshot, "New Folder"),
            Some(DirectoryRoleKind::BroadInbox)
        );
        assert!(!preserves(&snapshot, "New Folder"));
        assert!(
            !snapshot.defers_content_analysis(entry_named(&snapshot, "New Folder/IMG_010.jpg"))
        );
    }
}
