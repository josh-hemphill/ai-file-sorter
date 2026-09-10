//! Root-relative, forward-slash paths with the traversal rules every layer relies on.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Reasons a relative path is rejected.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum RelativePathError {
    /// The path was empty after normalisation.
    #[error("relative path is empty")]
    Empty,
    /// The path is absolute or has a drive/UNC prefix.
    #[error("path must be relative: {0}")]
    Absolute(String),
    /// The path contains a `..` segment.
    #[error("path escapes the root with '..': {0}")]
    ParentTraversal(String),
    /// A segment contains a control character (including NUL).
    #[error("segment '{segment}' contains forbidden character {character:?}")]
    ForbiddenCharacter {
        /// Offending segment.
        segment: String,
        /// Offending character.
        character: char,
    },
    /// A segment is a reserved device name on Windows.
    #[error("segment '{0}' is a reserved name")]
    ReservedName(String),
    /// A segment ends with a space or dot, which Windows silently strips.
    #[error("segment '{0}' ends with a space or dot")]
    TrailingSpaceOrDot(String),
    /// The value is not a single file name (empty, `.`/`..`, or contains a separator).
    #[error("not a single file name: {0}")]
    NotAFileName(String),
}

const RESERVED_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A normalised path relative to a session root. Always uses `/` separators and never
/// contains `.`/`..` segments, so it can be joined onto a root without escaping it.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelativePath(String);

impl RelativePath {
    /// Path that refers to the session root itself (a project detected at the scanned folder).
    pub fn session_root() -> Self {
        Self(".".to_owned())
    }

    /// True when this path is the session root sentinel (`.`).
    pub fn is_session_root(&self) -> bool {
        self.0 == "."
    }

    /// Parses and validates a candidate path. Backslashes are treated as separators so
    /// user-typed Windows paths still normalise. Song-title punctuation (`?`, `:`, `|`,
    /// quotes) is kept; use [`escape_path_segment`] when *creating* a Windows-safe name.
    pub fn parse(value: &str) -> Result<Self, RelativePathError> {
        let unified = value.replace('\\', "/");
        let trimmed = unified.trim();
        if trimmed.is_empty() {
            return Err(RelativePathError::Empty);
        }
        if trimmed == "." {
            return Ok(Self::session_root());
        }
        if trimmed.starts_with('/') || has_drive_prefix(trimmed) {
            return Err(RelativePathError::Absolute(value.to_owned()));
        }

        let mut segments = Vec::new();
        for segment in trimmed.split('/') {
            match segment {
                "" | "." => continue,
                ".." => return Err(RelativePathError::ParentTraversal(value.to_owned())),
                other => {
                    validate_segment(other)?;
                    segments.push(other.to_owned());
                }
            }
        }
        if segments.is_empty() {
            return Err(RelativePathError::Empty);
        }
        Ok(Self(segments.join("/")))
    }

    /// Builds a relative path from a filesystem path that is already below `root`.
    pub fn from_root_and_path(root: &Path, path: &Path) -> Result<Self, RelativePathError> {
        let stripped = path
            .strip_prefix(root)
            .map_err(|_| RelativePathError::Absolute(path.display().to_string()))?;
        if stripped.as_os_str().is_empty() {
            return Ok(Self::session_root());
        }
        let mut segments = Vec::new();
        for component in stripped.components() {
            match component {
                Component::Normal(segment) => segments.push(segment.to_string_lossy().into_owned()),
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(RelativePathError::ParentTraversal(
                        path.display().to_string(),
                    ));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(RelativePathError::Absolute(path.display().to_string()));
                }
            }
        }
        Self::parse(&segments.join("/"))
    }

    /// Returns the `/`-separated string form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the final segment.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Returns the parent path, or `None` when the path is a single segment.
    pub fn parent(&self) -> Option<RelativePath> {
        let index = self.0.rfind('/')?;
        Some(RelativePath(self.0[..index].to_owned()))
    }

    /// Appends a validated segment or sub-path.
    pub fn join(&self, child: &str) -> Result<RelativePath, RelativePathError> {
        RelativePath::parse(&format!("{}/{}", self.0, child))
    }

    /// Parses a single file-name segment: no separators, and not empty/`.`/`..`.
    pub fn parse_file_name(name: &str) -> Result<String, RelativePathError> {
        if name.is_empty() {
            return Err(RelativePathError::Empty);
        }
        if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            return Err(RelativePathError::NotAFileName(name.to_owned()));
        }
        validate_segment(name)?;
        Ok(name.to_owned())
    }

    /// Replaces the final segment, keeping the parent folder.
    pub fn with_file_name(&self, name: &str) -> Result<RelativePath, RelativePathError> {
        let name = Self::parse_file_name(name)?;
        match self.parent() {
            Some(parent) => parent.join(&name),
            None => RelativePath::parse(&name),
        }
    }

    /// Returns true when `self` is `ancestor` or lives below it.
    pub fn starts_with(&self, ancestor: &RelativePath) -> bool {
        if ancestor.is_session_root() {
            return true;
        }
        self == ancestor || self.0.starts_with(&format!("{}/", ancestor.0))
    }

    /// Returns the path segments in order.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Resolves the path under a concrete root directory.
    pub fn resolve(&self, root: &Path) -> PathBuf {
        if self.is_session_root() {
            return root.to_path_buf();
        }
        let mut out = root.to_path_buf();
        for segment in self.segments() {
            out.push(segment);
        }
        out
    }

    /// Case-folded form used for collision detection on case-insensitive filesystems.
    pub fn case_fold(&self) -> String {
        self.0.to_lowercase()
    }
}

fn has_drive_prefix(value: &str) -> bool {
    let mut chars = value.chars();
    matches!((chars.next(), chars.next()), (Some(letter), Some(':')) if letter.is_ascii_alphabetic())
}

/// Windows-safe encoding of one path segment. Keeps the title readable instead of
/// dropping `?` / `:` / `|` / quotes that music files commonly use.
pub fn escape_path_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        out.push(escape_segment_char(ch));
    }
    let trimmed = out.trim().trim_matches('.').trim();
    trimmed.to_owned()
}

fn escape_segment_char(ch: char) -> char {
    match ch {
        '<' => '\u{FF1C}',
        '>' => '\u{FF1E}',
        ':' => '\u{FF1A}',
        '"' => '\u{FF02}',
        '/' => '\u{FF0F}',
        '\\' => '\u{FF3C}',
        '|' => '\u{FF5C}',
        '?' => '\u{FF1F}',
        '*' => '\u{FF0A}',
        ch if ch.is_control() => ' ',
        ch => ch,
    }
}

fn validate_segment(segment: &str) -> Result<(), RelativePathError> {
    if let Some(character) = segment.chars().find(|ch| ch.is_control()) {
        return Err(RelativePathError::ForbiddenCharacter {
            segment: segment.to_owned(),
            character,
        });
    }
    if segment.ends_with(' ') || segment.ends_with('.') {
        return Err(RelativePathError::TrailingSpaceOrDot(segment.to_owned()));
    }
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .to_ascii_lowercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(RelativePathError::ReservedName(segment.to_owned()));
    }
    Ok(())
}

impl fmt::Debug for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelativePath({:?})", self.0)
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RelativePath {
    type Error = RelativePathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        RelativePath::parse(&value)
    }
}

impl From<RelativePath> for String {
    fn from(value: RelativePath) -> Self {
        value.0
    }
}

impl std::str::FromStr for RelativePath {
    type Err = RelativePathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        RelativePath::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_separators_and_dot_segments() {
        let path =
            RelativePath::parse("Photos\\2024/./trip//IMG_1.jpg").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(path.as_str(), "Photos/2024/trip/IMG_1.jpg");
        assert_eq!(path.file_name(), "IMG_1.jpg");
        assert_eq!(
            path.parent().map(|p| p.as_str().to_owned()),
            Some("Photos/2024/trip".into())
        );
    }

    #[test]
    fn rejects_escapes_and_absolute_paths() {
        assert_eq!(
            RelativePath::parse("../outside.txt"),
            Err(RelativePathError::ParentTraversal("../outside.txt".into()))
        );
        assert!(matches!(
            RelativePath::parse("/etc/passwd"),
            Err(RelativePathError::Absolute(_))
        ));
        assert!(matches!(
            RelativePath::parse("C:\\Users"),
            Err(RelativePathError::Absolute(_))
        ));
        assert_eq!(RelativePath::parse("./"), Err(RelativePathError::Empty));
    }

    #[test]
    fn rejects_windows_hostile_names() {
        assert!(matches!(
            RelativePath::parse("a/CON.txt"),
            Err(RelativePathError::ReservedName(_))
        ));
        assert!(matches!(
            RelativePath::parse("a/b. "),
            Err(RelativePathError::TrailingSpaceOrDot(_))
        ));
        assert!(matches!(
            RelativePath::parse("a/\u{0001}b"),
            Err(RelativePathError::ForbiddenCharacter { .. })
        ));
    }

    #[test]
    fn keeps_song_title_punctuation_in_observed_paths() {
        let path =
            RelativePath::parse(r#"Music/What Is Love?.mp3"#).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(path.as_str(), "Music/What Is Love?.mp3");
        let colon = RelativePath::parse("Music/Love: Live.flac").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(colon.as_str(), "Music/Love: Live.flac");
        let pipe = RelativePath::parse("a/b|c").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pipe.as_str(), "a/b|c");
    }

    #[test]
    fn escape_path_segment_maps_windows_illegal_title_chars() {
        assert_eq!(
            escape_path_segment(r#"What Is Love?"#),
            "What Is Love\u{FF1F}"
        );
        assert_eq!(escape_path_segment("AC/DC"), "AC\u{FF0F}DC");
        assert_eq!(
            escape_path_segment("Live: Tokyo | 2024"),
            "Live\u{FF1A} Tokyo \u{FF5C} 2024"
        );
        assert_eq!(escape_path_segment("  ..  "), "");
        assert_eq!(escape_path_segment("ok"), "ok");
    }

    #[test]
    fn ancestry_is_segment_aware() {
        let parent = RelativePath::parse("Docs").unwrap_or_else(|e| panic!("{e}"));
        let child = RelativePath::parse("Docs/a.txt").unwrap_or_else(|e| panic!("{e}"));
        let sibling = RelativePath::parse("Documents/a.txt").unwrap_or_else(|e| panic!("{e}"));
        assert!(child.starts_with(&parent));
        assert!(!sibling.starts_with(&parent));
    }

    #[test]
    fn serde_uses_plain_strings_and_validates() {
        let path: RelativePath =
            serde_json::from_str("\"Music/song.mp3\"").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            serde_json::to_string(&path).ok(),
            Some("\"Music/song.mp3\"".to_owned())
        );
        assert!(serde_json::from_str::<RelativePath>("\"../x\"").is_err());
    }

    #[test]
    fn from_root_and_path_strips_root() {
        let root = Path::new("/data/inbox");
        let path = RelativePath::from_root_and_path(root, Path::new("/data/inbox/a/b.txt"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(path.as_str(), "a/b.txt");
        assert!(RelativePath::from_root_and_path(root, Path::new("/data/other/b.txt")).is_err());
        let at_root =
            RelativePath::from_root_and_path(root, root).unwrap_or_else(|e| panic!("{e}"));
        assert!(at_root.is_session_root());
        assert_eq!(at_root.resolve(root), root);
    }

    #[test]
    fn session_root_is_an_ancestor_of_every_path() {
        let root = RelativePath::session_root();
        let child = RelativePath::parse("Docs/a.txt").unwrap_or_else(|e| panic!("{e}"));
        assert!(child.starts_with(&root));
        assert!(root.starts_with(&root));
        assert_eq!(RelativePath::parse(".").ok(), Some(root));
    }

    #[test]
    fn with_file_name_rejects_multi_segment_and_dot_names() {
        let path = RelativePath::parse("Music/track.mp3").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            path.with_file_name("2024_song.mp3")
                .map(|p| p.as_str().to_owned())
                .ok(),
            Some("Music/2024_song.mp3".into())
        );
        assert_eq!(
            path.with_file_name("nested/song.mp3"),
            Err(RelativePathError::NotAFileName("nested/song.mp3".into()))
        );
        assert_eq!(
            path.with_file_name("nested\\song.mp3"),
            Err(RelativePathError::NotAFileName("nested\\song.mp3".into()))
        );
        assert_eq!(
            path.with_file_name("."),
            Err(RelativePathError::NotAFileName(".".into()))
        );
        assert_eq!(
            path.with_file_name(".."),
            Err(RelativePathError::NotAFileName("..".into()))
        );
        assert_eq!(path.with_file_name(""), Err(RelativePathError::Empty));
        let top = RelativePath::parse("track.mp3").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            top.with_file_name("renamed.mp3")
                .map(|p| p.as_str().to_owned())
                .ok(),
            Some("renamed.mp3".into())
        );
        assert!(top.with_file_name("a/b").is_err());
    }
}
