//! Observed filesystem entries and their stable identity.

use crate::evidence::{Evidence, keys};
use crate::ids::AssetId;
use crate::path::RelativePath;
use crate::time::{Timestamp, parse_iso_date, parse_iso_year_month, utc_year_month_label};
use serde::{Deserialize, Serialize};

/// Kind of filesystem node that was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link or reparse point (never followed).
    Symlink,
}

/// Coarse family used for default placement and for choosing extractors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFamily {
    /// Bitmap/vector image.
    Image,
    /// Camera RAW image.
    RawImage,
    /// Text-like document (pdf, docx, md, ...).
    Document,
    /// Tabular document.
    Spreadsheet,
    /// Slide deck.
    Presentation,
    /// Ebook container.
    Ebook,
    /// Audio.
    Audio,
    /// Video.
    Video,
    /// Subtitle or lyrics track.
    Subtitle,
    /// Compressed archive or disk image.
    Archive,
    /// Source code or config.
    Code,
    /// Font.
    Font,
    /// Executable or installer.
    Executable,
    /// Machine data (db, json dumps, ...).
    Data,
    /// Metadata sidecar (xmp, thm, aae, ...).
    Sidecar,
    /// Anything else.
    Generic,
}

impl FileFamily {
    /// Classifies by lower-cased extension without the leading dot.
    pub fn from_extension(extension: &str) -> Self {
        match extension.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif"
            | "avif" | "svg" | "psd" | "ico" => Self::Image,
            "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" | "rw2" | "pef" | "srw" => {
                Self::RawImage
            }
            "pdf" | "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "tex" | "rst" | "pages"
            | "html" | "htm" => Self::Document,
            "xls" | "xlsx" | "ods" | "csv" | "tsv" | "numbers" => Self::Spreadsheet,
            "ppt" | "pptx" | "odp" | "key" => Self::Presentation,
            "epub" | "mobi" | "azw" | "azw3" | "fb2" | "cbz" | "cbr" => Self::Ebook,
            "mp3" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "wav" | "aif" | "aiff"
            | "alac" | "ape" | "wma" | "wv" => Self::Audio,
            "mp4" | "m4v" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "flv" | "mpg" | "mpeg"
            | "mts" | "m2ts" | "3gp" => Self::Video,
            "srt" | "vtt" | "ass" | "ssa" | "sub" | "lrc" => Self::Subtitle,
            "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "iso" | "dmg"
            | "z01" | "z02" | "001" | "002" => Self::Archive,
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "vue" | "go" | "java" | "kt" | "c"
            | "h" | "cpp" | "hpp" | "cs" | "rb" | "php" | "swift" | "sh" | "ps1" | "toml"
            | "yaml" | "yml" | "json" | "xml" | "ini" | "cfg" => Self::Code,
            "ttf" | "otf" | "woff" | "woff2" => Self::Font,
            "exe" | "msi" | "app" | "apk" | "deb" | "rpm" | "appimage" | "pkg" | "bat" => {
                Self::Executable
            }
            "db" | "sqlite" | "sqlite3" | "parquet" | "bin" | "dat" => Self::Data,
            "xmp" | "thm" | "aae" | "cue" | "nfo" | "torrent" | "lrv" => Self::Sidecar,
            _ => Self::Generic,
        }
    }

    /// Default top-level folder used by the heuristic planner.
    pub fn default_folder(&self) -> &'static str {
        match self {
            Self::Image | Self::RawImage => "Pictures",
            Self::Document | Self::Spreadsheet | Self::Presentation => "Documents",
            Self::Ebook => "Books",
            Self::Audio => "Music",
            Self::Video | Self::Subtitle => "Videos",
            Self::Archive => "Archives",
            Self::Code => "Code",
            Self::Font => "Fonts",
            Self::Executable => "Installers",
            Self::Data | Self::Sidecar | Self::Generic => "Other",
        }
    }

    /// True for office, text, and ebook families that use the document analysis slot.
    pub fn is_document_like(self) -> bool {
        matches!(
            self,
            Self::Document | Self::Spreadsheet | Self::Presentation | Self::Ebook
        )
    }
}

/// Date folder segment: images `YYYY-MM-DD` from EXIF, documents `YYYY-MM`.
pub fn category_date_suffix(entry: &ObservedEntry, captured_on: Option<&str>) -> Option<String> {
    match entry.family {
        FileFamily::Image | FileFamily::RawImage => {
            let value = captured_on?;
            parse_iso_date(value).map(|_| value.to_owned())
        }
        FileFamily::Document
        | FileFamily::Spreadsheet
        | FileFamily::Presentation
        | FileFamily::Ebook => {
            if let Some(value) = captured_on {
                if parse_iso_date(value).is_some() {
                    return Some(value[..7].to_owned());
                }
                if parse_iso_year_month(value).is_some() {
                    return Some(value.to_owned());
                }
            }
            entry
                .identity
                .modified
                .and_then(|stamp| utc_year_month_label(stamp.as_millis()))
        }
        _ => None,
    }
}

/// True when the file name looks like a screenshot or UI capture.
pub fn filename_looks_like_screenshot(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    name.contains("screenshot")
        || name.contains("screen-shot")
        || name.contains("screen_shot")
        || name.contains("screen shot")
}

/// True when a caption looks like a screenshot or UI capture.
pub fn description_looks_like_screenshot(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("screenshot") || text.contains("ui capture") || text.contains("screen capture")
}

/// True when the file name or description evidence looks like a screenshot or UI capture.
pub fn looks_like_screenshot(entry: &ObservedEntry, evidence: &[Evidence]) -> bool {
    filename_looks_like_screenshot(entry.path.file_name())
        || evidence.iter().any(|bag| {
            bag.fact(keys::DESCRIPTION)
                .is_some_and(description_looks_like_screenshot)
        })
}

/// Identity captured at scan time so apply/undo can detect that a file changed underneath.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIdentity {
    /// Filesystem device id when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
    /// Inode / file index when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
    /// Size in bytes.
    pub size: u64,
    /// Modification time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<Timestamp>,
    /// Optional content hash (hex) of a size-bounded prefix or full file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_fingerprint: Option<String>,
}

impl FileIdentity {
    /// Returns true when the two identities plausibly describe the same unchanged file.
    pub fn matches(&self, other: &FileIdentity) -> bool {
        if self.size != other.size {
            return false;
        }
        if let (Some(a), Some(b)) = (&self.content_fingerprint, &other.content_fingerprint) {
            return a == b;
        }
        if let (Some(a), Some(b)) = (self.inode, other.inode)
            && a != b
        {
            return false;
        }
        match (self.modified, other.modified) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }
}

/// Whether a file could be read when scanned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LockState {
    /// Readable.
    Readable,
    /// Another process holds an exclusive lock or hydration is required.
    Locked {
        /// Human-readable reason.
        reason: String,
    },
}

/// A file or directory seen under the session root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedEntry {
    /// Stable id assigned at first observation.
    pub id: AssetId,
    /// Path relative to the session root.
    pub path: RelativePath,
    /// Node kind.
    pub kind: EntryKind,
    /// Coarse family.
    pub family: FileFamily,
    /// Identity snapshot.
    pub identity: FileIdentity,
    /// True for dot-files or hidden-attribute entries.
    #[serde(default)]
    pub is_hidden: bool,
    /// Read/lock state.
    pub lock: LockState,
}

impl ObservedEntry {
    /// Lower-cased extension without dot, if any.
    pub fn extension(&self) -> Option<String> {
        let name = self.path.file_name();
        let index = name.rfind('.')?;
        if index == 0 || index + 1 == name.len() {
            return None;
        }
        Some(name[index + 1..].to_ascii_lowercase())
    }

    /// File name without its final extension.
    pub fn stem(&self) -> &str {
        let name = self.path.file_name();
        match name.rfind('.') {
            Some(index) if index > 0 => &name[..index],
            _ => name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_classification_and_default_folders() {
        assert_eq!(FileFamily::from_extension("JPG"), FileFamily::Image);
        assert_eq!(FileFamily::from_extension("cr2"), FileFamily::RawImage);
        assert_eq!(FileFamily::from_extension("srt"), FileFamily::Subtitle);
        assert_eq!(FileFamily::from_extension("weird"), FileFamily::Generic);
        assert_eq!(FileFamily::Audio.default_folder(), "Music");
        assert!(FileFamily::Document.is_document_like());
        assert!(FileFamily::Ebook.is_document_like());
        assert!(!FileFamily::Image.is_document_like());
    }

    #[test]
    fn identity_matching_prefers_fingerprint_then_inode_then_mtime() {
        let base = FileIdentity {
            device: Some(1),
            inode: Some(10),
            size: 5,
            modified: Some(Timestamp(100)),
            content_fingerprint: None,
        };
        let same = base.clone();
        let moved_mtime = FileIdentity {
            modified: Some(Timestamp(200)),
            ..base.clone()
        };
        let bigger = FileIdentity {
            size: 6,
            ..base.clone()
        };
        let hashed_a = FileIdentity {
            content_fingerprint: Some("aa".into()),
            ..base.clone()
        };
        let hashed_b = FileIdentity {
            content_fingerprint: Some("bb".into()),
            modified: Some(Timestamp(100)),
            ..base.clone()
        };
        assert!(base.matches(&same));
        assert!(!base.matches(&moved_mtime));
        assert!(!base.matches(&bigger));
        assert!(!hashed_a.matches(&hashed_b));
    }

    #[test]
    fn extension_and_stem_handle_dotfiles() {
        let entry = ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(".bashrc").unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::Generic,
            identity: FileIdentity::default(),
            is_hidden: true,
            lock: LockState::Readable,
        };
        assert_eq!(entry.extension(), None);
        assert_eq!(entry.stem(), ".bashrc");
    }

    #[test]
    fn screenshot_filename_and_caption_detectors() {
        assert!(filename_looks_like_screenshot("Screenshot 2024.png"));
        assert!(filename_looks_like_screenshot("screen-shot.jpg"));
        assert!(filename_looks_like_screenshot("screen_shot.webp"));
        assert!(!filename_looks_like_screenshot("DSC_0001.jpg"));
        assert!(description_looks_like_screenshot(
            "A UI capture of settings"
        ));
        assert!(description_looks_like_screenshot(
            "this is a screenshot of a menu"
        ));
        assert!(!description_looks_like_screenshot("a cat on a sofa"));
        let shot = ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse("desk.jpg").unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::Image,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        };
        let bag = crate::Evidence::new(
            shot.id,
            crate::EvidenceSource::LocalModel {
                model: "vision".into(),
            },
            crate::Confidence::new(0.5),
        )
        .with_fact(keys::DESCRIPTION, "a settings panel UI capture");
        assert!(looks_like_screenshot(&shot, &[bag]));
        assert!(!looks_like_screenshot(&shot, &[]));
    }

    fn dated_file(path: &str, family: FileFamily, modified: Option<Timestamp>) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(path).unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::File,
            family,
            identity: FileIdentity {
                modified,
                ..FileIdentity::default()
            },
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    #[test]
    fn category_date_suffix_uses_exif_for_images_and_month_for_documents() {
        let image = dated_file("shot.jpg", FileFamily::Image, Some(Timestamp(0)));
        assert_eq!(
            category_date_suffix(&image, Some("2021-07-15")).as_deref(),
            Some("2021-07-15")
        );
        assert!(category_date_suffix(&image, Some("not-a-date")).is_none());
        assert!(category_date_suffix(&image, None).is_none());

        let document = dated_file(
            "note.txt",
            FileFamily::Document,
            Some(Timestamp(1_626_307_200_000)),
        );
        assert_eq!(
            category_date_suffix(&document, None).as_deref(),
            Some("2021-07")
        );
        assert_eq!(
            category_date_suffix(&document, Some("2021-07-15")).as_deref(),
            Some("2021-07")
        );
        assert!(
            category_date_suffix(&dated_file("note.txt", FileFamily::Document, None), None)
                .is_none()
        );
    }
}
