//! Shape of a folder: camera dump, date/event album, or named library.

use aifs_domain::{FileFamily, ObservedEntry, RelativePath, is_generic_camera_stem};

pub(crate) const LIBRARY_NAMES: &[&str] = &[
    "music", "photos", "pictures", "videos", "movies", "library", "media",
];
pub(crate) const BROAD_NAMES: &[&str] = &[
    "downloads",
    "desktop",
    "inbox",
    "unsorted",
    "dump",
    "temp",
    "tmp",
    "incoming",
];
pub(crate) const ARCHIVE_NAMES: &[&str] = &[
    "old", "archive", "archives", "backup", "bak", "final", "finals",
];

const DUMP_FOLDER_NAMES: &[&str] = &[
    "dcim", "camera", "img", "dsc", "raw", "recents", "imported", "unfiled", "private", "roll",
];

/// Photo-library tokens that form a camera-dump stack when nested (`Photos/Pictures`).
const PHOTO_DUMP_TOKENS: &[&str] = &["photos", "pictures"];

/// Single-token folders that are not albums even when they contain files.
const GENERIC_FOLDER_NAMES: &[&str] = &[
    "projects",
    "documents",
    "files",
    "work",
    "data",
    "home",
    "users",
    "stuff",
    "misc",
    "other",
    "folder",
    "items",
    "scan",
    "export",
    "output",
    "input",
    "cache",
    "logs",
];

/// How a directory looks before any model label is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FolderShape {
    /// Nested camera roll / DCIM / repeated Pictures token.
    CameraDump,
    /// Human date or event name (keep as a unit).
    EventOrDate,
    /// Downloads-style dump name.
    BroadInbox,
    /// old/backup/archive token.
    ArchiveName,
    /// Music/Pictures/… token; children decide Library vs dump.
    LibraryName,
    /// Not enough signal.
    Unknown,
}

/// Classifies a folder name plus the files that live under it.
pub(crate) fn folder_shape(
    name: &str,
    parent_name: Option<&str>,
    files: &[ObservedEntry],
) -> FolderShape {
    let name = name.to_ascii_lowercase();
    if BROAD_NAMES.contains(&name.as_str()) {
        return FolderShape::BroadInbox;
    }
    if ARCHIVE_NAMES.contains(&name.as_str()) {
        return FolderShape::ArchiveName;
    }
    if is_dump_folder_name(&name, parent_name) {
        return FolderShape::CameraDump;
    }
    if is_date_folder_name(&name) || is_event_folder_name(&name) {
        return FolderShape::EventOrDate;
    }
    if LIBRARY_NAMES.contains(&name.as_str()) {
        return FolderShape::LibraryName;
    }
    if files_look_like_camera_dump(files) {
        return FolderShape::CameraDump;
    }
    FolderShape::Unknown
}

/// True when this child should not inherit a parent library freeze.
pub(crate) fn is_dump_shaped(shape: FolderShape) -> bool {
    matches!(shape, FolderShape::CameraDump | FolderShape::BroadInbox)
}

/// True when the folder name is a camera-roll or nested Photos/Pictures dump stack.
pub(crate) fn is_dump_folder_name(name: &str, parent_name: Option<&str>) -> bool {
    let name = name.to_ascii_lowercase();
    if DUMP_FOLDER_NAMES.contains(&name.as_str()) {
        return true;
    }
    if folder_tokens(&name).any(|token| DUMP_FOLDER_NAMES.contains(&token)) {
        return true;
    }
    if is_dcf_folder(&name) {
        return true;
    }
    if let Some(parent) = parent_name {
        let parent = parent.to_ascii_lowercase();
        if is_nested_photo_dump(&name, &parent) {
            return true;
        }
    }
    false
}

/// True for `2019`, `2019-06`, `2019-06-12`, `20190612`.
pub(crate) fn is_date_folder_name(name: &str) -> bool {
    if is_year_name(name) {
        return true;
    }
    if name.len() == 8 && name.chars().all(|ch| ch.is_ascii_digit()) {
        return looks_like_yyyymmdd(name);
    }
    let normalized = name.replace(['_', '.'], "-");
    let parts: Vec<&str> = normalized.split('-').collect();
    match parts.as_slice() {
        [year, month] => is_year_name(year) && is_month(month),
        [year, month, day] => is_year_name(year) && is_month(month) && is_day(day),
        _ => false,
    }
}

/// Human event/album name: letterful and not a camera/library/dump/generic token.
pub(crate) fn is_event_folder_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if is_reserved_folder_name(&name) || folder_tokens(&name).any(is_reserved_folder_name) {
        return false;
    }
    if is_date_folder_name(&name) || name_contains_year(&name) {
        return true;
    }
    let letters: String = name.chars().filter(|ch| ch.is_ascii_alphabetic()).collect();
    letters.len() >= 3
}

/// Camera-generated file stem (`IMG_1042`, `PXL_20260915_123`, `DSC01234`).
pub(crate) fn is_camera_stem(stem: &str) -> bool {
    is_generic_camera_stem(stem)
}

pub(crate) fn is_year_name(name: &str) -> bool {
    name.len() == 4
        && name.chars().all(|ch| ch.is_ascii_digit())
        && (name.starts_with("19") || name.starts_with("20"))
}

pub(crate) fn path_has_broad_segment(path: &RelativePath) -> bool {
    path.as_str()
        .split('/')
        .any(|segment| BROAD_NAMES.contains(&segment.to_ascii_lowercase().as_str()))
}

/// True when a majority of media files use camera-generated stems.
pub(crate) fn files_look_like_camera_dump(files: &[ObservedEntry]) -> bool {
    let media: Vec<_> = files
        .iter()
        .filter(|entry| {
            matches!(
                entry.family,
                FileFamily::Image | FileFamily::RawImage | FileFamily::Video
            )
        })
        .collect();
    if media.len() < 3 {
        return false;
    }
    let camera = media
        .iter()
        .filter(|entry| is_camera_stem(entry.stem()))
        .count();
    camera * 2 >= media.len()
}

/// True for a leaf `Photos`/`Pictures` folder of real camera stems.
pub(crate) fn is_leaf_photo_camera_dump(
    name: &str,
    files: &[ObservedEntry],
    has_child_dirs: bool,
) -> bool {
    !has_child_dirs
        && PHOTO_DUMP_TOKENS.contains(&name.to_ascii_lowercase().as_str())
        && files_look_like_camera_dump(files)
}

fn is_nested_photo_dump(name: &str, parent: &str) -> bool {
    PHOTO_DUMP_TOKENS.contains(&name) && PHOTO_DUMP_TOKENS.contains(&parent)
}

fn folder_tokens(name: &str) -> impl Iterator<Item = &str> {
    name.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
}

fn is_reserved_folder_name(name: &str) -> bool {
    is_dump_folder_name(name, None)
        || LIBRARY_NAMES.contains(&name)
        || BROAD_NAMES.contains(&name)
        || ARCHIVE_NAMES.contains(&name)
        || GENERIC_FOLDER_NAMES.contains(&name)
}

fn name_contains_year(name: &str) -> bool {
    name.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(is_year_name)
}

fn is_dcf_folder(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < 5 || bytes.len() > 8 {
        return false;
    }
    bytes[..3].iter().all(|ch| ch.is_ascii_digit())
        && bytes[3..].iter().all(|ch| ch.is_ascii_alphabetic())
}

fn is_month(value: &str) -> bool {
    matches!(
        value,
        "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12"
    )
}

fn is_day(value: &str) -> bool {
    let Ok(day) = value.parse::<u8>() else {
        return false;
    };
    (1..=31).contains(&day) && value.len() == 2
}

fn looks_like_yyyymmdd(name: &str) -> bool {
    is_year_name(&name[..4]) && is_month(&name[4..6]) && is_day(&name[6..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{AssetId, EntryKind, FileIdentity, LockState};

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

    #[test]
    fn camera_stems_match_common_makers() {
        assert!(is_camera_stem("IMG_1042"));
        assert!(is_camera_stem("DSC01234"));
        assert!(is_camera_stem("PXL_20260915_123456"));
        assert!(is_camera_stem("IMG-20260915-WA0001"));
        assert!(is_camera_stem("VID_0001"));
        assert!(!is_camera_stem("Italy-sunset"));
        assert!(!is_camera_stem("cover"));
        assert!(!is_camera_stem("video_1"));
        assert!(!is_camera_stem("movie_1"));
        assert!(!is_camera_stem("description1"));
        assert!(!is_camera_stem("photo_album1"));
    }

    #[test]
    fn nested_pictures_token_is_a_dump() {
        assert!(is_dump_folder_name("pictures", Some("photos")));
        assert!(is_dump_folder_name("pictures", Some("pictures")));
        assert!(!is_dump_folder_name("pictures", Some("home")));
        assert!(!is_dump_folder_name("videos", Some("movies")));
        assert!(!is_dump_folder_name("music", Some("library")));
        assert!(!is_dump_folder_name("videos", Some("photos")));
        assert!(is_dump_folder_name("dcim", Some("photos")));
        assert!(is_dump_folder_name("100CANON", None));
        assert!(is_dump_folder_name("Camera Roll", None));
    }

    #[test]
    fn date_and_event_names_are_organized() {
        assert!(is_date_folder_name("2019"));
        assert!(is_date_folder_name("2019-06"));
        assert!(is_date_folder_name("2019-06-12"));
        assert!(is_event_folder_name("Italy"));
        assert!(is_event_folder_name("Wedding"));
        assert!(is_event_folder_name("Italy 2019"));
        assert!(!is_event_folder_name("dcim"));
        assert!(!is_event_folder_name("pictures"));
        assert!(!is_event_folder_name("projects"));
        assert!(!is_event_folder_name("Camera Roll"));
        assert!(!is_event_folder_name("New Folder"));
    }

    #[test]
    fn flat_camera_files_are_a_dump_shape() {
        let files = vec![
            image("roll/IMG_001.jpg"),
            image("roll/IMG_002.jpg"),
            image("roll/IMG_003.jpg"),
        ];
        assert_eq!(
            folder_shape("roll", Some("photos"), &files),
            FolderShape::CameraDump
        );
        assert_eq!(
            folder_shape("Italy", Some("photos"), &files),
            FolderShape::EventOrDate
        );
        assert_eq!(
            folder_shape("projects", None, &files),
            FolderShape::CameraDump
        );
        assert!(is_leaf_photo_camera_dump("Photos", &files, false));
        assert!(!is_leaf_photo_camera_dump("Videos", &files, false));
        assert!(!is_leaf_photo_camera_dump("Photos", &files, true));
    }
}
