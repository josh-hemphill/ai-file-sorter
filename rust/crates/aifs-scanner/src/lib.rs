//! Walk a session root into a [`WorkspaceSnapshot`].

use aifs_domain::{
    AssetId, EntryKind, FileFamily, FileIdentity, LockState, ObservedEntry, RelativePath,
    SessionId, SkipReason, SkippedEntry, Timestamp, WorkspaceSnapshot,
};
use aifs_protocol::ScanOptions;
use aifs_relationships::{detect_project, should_skip_traversal, DetectedProject};
use sha2::{Digest, Sha256};
use std::fs::{self, DirEntry, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use thiserror::Error;

const JUNK_NAMES: &[&str] = &[".ds_store", "thumbs.db", "desktop.ini"];
const JUNK_PREFIXES: &[&str] = &["~$"];
const MACOS_BUNDLE_EXTENSIONS: &[&str] = &[
    "app",
    "utm",
    "vmwarevm",
    "pvm",
    "vbox",
    "pkg",
    "mpkg",
    "prefpane",
    "plugin",
    "framework",
    "kext",
    "qlgenerator",
    "mdimporter",
    "wdgt",
    "scptd",
    "nib",
    "xib",
];

/// Failures that abort a scan before a snapshot can be produced.
#[derive(Debug, Error)]
pub enum ScanError {
    /// Root does not exist or is not a directory.
    #[error("invalid scan root '{path}': {message}")]
    InvalidRoot {
        /// Path that was requested.
        path: PathBuf,
        /// Reason.
        message: String,
    },
    /// I/O failure opening the root itself.
    #[error("failed to scan '{path}': {source}")]
    Io {
        /// Path.
        path: PathBuf,
        /// Source error.
        #[source]
        source: io::Error,
    },
}

/// Walks `root` and returns entries, skipped items, and project matches.
///
/// Relationship bundles and media evidence are filled in later by the engine.
pub fn scan(
    root: &Path,
    options: &ScanOptions,
    session: SessionId,
    mut on_progress: impl FnMut(u64, &str),
) -> Result<WorkspaceSnapshot, ScanError> {
    let root = normalize_root(root)?;
    let mut snapshot = WorkspaceSnapshot::new(session, root.clone());
    let mut seen: u64 = 0;

    if let Some(detected) = detect_project(&root) {
        snapshot
            .projects
            .push(detected.clone().into_match(RelativePath::session_root()));
        if options.protect_projects && should_skip_traversal(&detected) {
            skip_immediate_children(
                &root,
                &root,
                SkipReason::ProtectedProject {
                    rule_id: detected.rule_id,
                },
                &mut snapshot,
            );
            finalize_snapshot(&mut snapshot);
            return Ok(snapshot);
        }
    }

    let mut pending = vec![PendingDir {
        path: root.clone(),
        depth: 0,
    }];

    while let Some(dir) = pending.pop() {
        let read_dir = match fs::read_dir(&dir.path) {
            Ok(iter) => iter,
            Err(error) => {
                if dir.path == root {
                    return Err(ScanError::Io {
                        path: dir.path,
                        source: error,
                    });
                }
                push_skipped_path(
                    &root,
                    &dir.path,
                    SkipReason::Error {
                        message: error.to_string(),
                    },
                    &mut snapshot,
                );
                continue;
            }
        };

        let mut children: Vec<DirEntry> = read_dir.filter_map(Result::ok).collect();
        children.sort_by_key(|entry| entry.file_name());

        for child in children {
            seen += 1;
            match classify_child(&root, &child, options) {
                ChildAction::Skip(skipped) => snapshot.skipped.push(skipped),
                ChildAction::Include {
                    entry,
                    enqueue_dir,
                    skip_children,
                    project,
                } => {
                    on_progress(seen, entry.path.as_str());
                    if let Some(detected) = project {
                        let already = snapshot
                            .projects
                            .iter()
                            .any(|existing| existing.root == entry.path);
                        if !already {
                            snapshot
                                .projects
                                .push(detected.into_match(entry.path.clone()));
                        }
                    }
                    if let Some(reason) = skip_children {
                        skip_immediate_children(
                            &root,
                            &entry.path.resolve(&root),
                            reason,
                            &mut snapshot,
                        );
                    }
                    snapshot.entries.push(*entry);
                    if enqueue_dir {
                        let child_depth = dir.depth.saturating_add(1);
                        let can_descend = options.recursive
                            && (options.max_depth == 0 || child_depth < options.max_depth);
                        if can_descend {
                            pending.push(PendingDir {
                                path: child.path(),
                                depth: child_depth,
                            });
                        }
                    }
                }
            }
        }
    }

    finalize_snapshot(&mut snapshot);
    Ok(snapshot)
}

struct PendingDir {
    path: PathBuf,
    depth: u32,
}

enum ChildAction {
    Skip(SkippedEntry),
    Include {
        entry: Box<ObservedEntry>,
        enqueue_dir: bool,
        skip_children: Option<SkipReason>,
        project: Option<DetectedProject>,
    },
}

fn classify_child(root: &Path, child: &DirEntry, options: &ScanOptions) -> ChildAction {
    let path = child.path();
    let name = child.file_name().to_string_lossy().into_owned();
    let relative = match RelativePath::from_root_and_path(root, &path) {
        Ok(path) => path,
        Err(error) => {
            return skip(
                relative_best_effort(root, &path),
                SkipReason::Error {
                    message: error.to_string(),
                },
            );
        }
    };

    if is_junk_name(&name) {
        return skip(relative, SkipReason::Junk);
    }

    let file_type = match child.file_type() {
        Ok(file_type) => file_type,
        Err(error) => {
            return skip(
                relative,
                SkipReason::Error {
                    message: error.to_string(),
                },
            );
        }
    };

    if file_type.is_symlink() {
        return skip(relative, SkipReason::Symlink);
    }

    let metadata = match child.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            return skip(
                relative,
                SkipReason::Error {
                    message: error.to_string(),
                },
            );
        }
    };

    let hidden = is_hidden(&name, &metadata);
    if hidden && !options.include_hidden {
        return skip(relative, SkipReason::Hidden);
    }

    let macos_bundle = file_type.is_dir() && is_macos_bundle(&name);
    let kind = if file_type.is_dir() && !macos_bundle {
        EntryKind::Directory
    } else if file_type.is_file() || macos_bundle {
        EntryKind::File
    } else {
        return skip(
            relative,
            SkipReason::Error {
                message: "unsupported filesystem node".to_owned(),
            },
        );
    };

    let extension = extension_of(&name).unwrap_or("");
    let family = FileFamily::from_extension(extension);
    let (identity, lock) = file_identity(&path, &metadata, kind, options.fingerprint_prefix_bytes);

    let mut enqueue_dir = kind == EntryKind::Directory;
    let mut skip_children = None;
    let mut project = None;
    if kind == EntryKind::Directory {
        project = detect_project(&path);
        if options.protect_projects {
            if let Some(detected) = &project {
                if should_skip_traversal(detected) {
                    enqueue_dir = false;
                    skip_children = Some(SkipReason::ProtectedProject {
                        rule_id: detected.rule_id.clone(),
                    });
                }
            }
        }
    }

    ChildAction::Include {
        entry: Box::new(ObservedEntry {
            id: AssetId::new(),
            path: relative,
            kind,
            family,
            identity,
            is_hidden: hidden,
            lock,
        }),
        enqueue_dir,
        skip_children,
        project,
    }
}

fn skip(path: RelativePath, reason: SkipReason) -> ChildAction {
    ChildAction::Skip(SkippedEntry { path, reason })
}

fn relative_best_effort(root: &Path, path: &Path) -> RelativePath {
    RelativePath::from_root_and_path(root, path).unwrap_or_else(|_| {
        let fallback = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_owned());
        RelativePath::parse(&fallback).unwrap_or_else(|_| RelativePath::session_root())
    })
}

fn skip_immediate_children(
    root: &Path,
    dir: &Path,
    reason: SkipReason,
    snapshot: &mut WorkspaceSnapshot,
) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    for child in read_dir.flatten() {
        push_skipped_path(root, &child.path(), reason.clone(), snapshot);
    }
}

fn push_skipped_path(
    root: &Path,
    path: &Path,
    reason: SkipReason,
    snapshot: &mut WorkspaceSnapshot,
) {
    if let Ok(relative) = RelativePath::from_root_and_path(root, path) {
        snapshot.skipped.push(SkippedEntry {
            path: relative,
            reason,
        });
    }
}

fn normalize_root(path: &Path) -> Result<PathBuf, ScanError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(source) => {
                return Err(ScanError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    };
    let metadata = fs::metadata(&absolute).map_err(|source| ScanError::InvalidRoot {
        path: absolute.clone(),
        message: source.to_string(),
    })?;
    if !metadata.is_dir() {
        return Err(ScanError::InvalidRoot {
            path: absolute,
            message: "not a directory".to_owned(),
        });
    }
    Ok(absolute)
}

fn is_junk_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    JUNK_NAMES.contains(&lower.as_str())
        || JUNK_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

fn is_macos_bundle(name: &str) -> bool {
    extension_of(name).is_some_and(|ext| MACOS_BUNDLE_EXTENSIONS.contains(&ext))
}

fn extension_of(name: &str) -> Option<&str> {
    let index = name.rfind('.')?;
    if index == 0 || index + 1 == name.len() {
        return None;
    }
    Some(&name[index + 1..])
}

fn is_hidden(name: &str, metadata: &Metadata) -> bool {
    name.starts_with('.') || hidden_attribute(metadata)
}

#[cfg(windows)]
fn hidden_attribute(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn hidden_attribute(_metadata: &Metadata) -> bool {
    false
}

fn file_identity(
    path: &Path,
    metadata: &Metadata,
    kind: EntryKind,
    fingerprint_prefix_bytes: u64,
) -> (FileIdentity, LockState) {
    let mut identity = FileIdentity {
        device: None,
        inode: None,
        size: if kind == EntryKind::File {
            metadata.len()
        } else {
            0
        },
        modified: metadata.modified().ok().map(Timestamp::from_system_time),
        content_fingerprint: None,
    };
    inode_from_metadata(metadata, &mut identity);

    if kind != EntryKind::File || fingerprint_prefix_bytes == 0 {
        return (identity, LockState::Readable);
    }

    match hash_prefix(path, fingerprint_prefix_bytes) {
        Ok(hex) => {
            identity.content_fingerprint = Some(hex);
            (identity, LockState::Readable)
        }
        Err(error) => (
            identity,
            LockState::Locked {
                reason: error.to_string(),
            },
        ),
    }
}

fn hash_prefix(path: &Path, prefix_bytes: u64) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut remaining = prefix_bytes;
    let mut buffer = [0u8; 8192];
    while remaining > 0 {
        let want = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = file.read(&mut buffer[..want])?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(TABLE[(byte >> 4) as usize] as char);
        out.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(unix)]
fn inode_from_metadata(metadata: &Metadata, identity: &mut FileIdentity) {
    use std::os::unix::fs::MetadataExt;
    identity.device = Some(metadata.dev());
    identity.inode = Some(metadata.ino());
}

#[cfg(windows)]
fn inode_from_metadata(metadata: &Metadata, identity: &mut FileIdentity) {
    use std::os::windows::fs::MetadataExt;
    identity.device = Some(u64::from(metadata.volume_serial_number().unwrap_or(0)));
    identity.inode = metadata.file_index();
}

#[cfg(not(any(unix, windows)))]
fn inode_from_metadata(_metadata: &Metadata, _identity: &mut FileIdentity) {}

fn finalize_snapshot(snapshot: &mut WorkspaceSnapshot) {
    snapshot
        .entries
        .sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    snapshot
        .skipped
        .sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    snapshot
        .projects
        .sort_by(|a, b| a.root.as_str().cmp(b.root.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::ProjectStrength;
    use std::fs;
    use std::io::Write;

    fn scan_tree(root: &Path, options: ScanOptions) -> WorkspaceSnapshot {
        scan(root, &options, SessionId::new(), |_, _| {}).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn skips_junk_hidden_and_symlinks() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("keep.txt"), b"ok").unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join(".secret"), b"no").unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("Thumbs.db"), b"junk").unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("~$lock.docx"), b"lock").unwrap_or_else(|e| panic!("{e}"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("keep.txt"), dir.path().join("alias"))
                .unwrap_or_else(|e| panic!("{e}"));
        }

        let snapshot = scan_tree(dir.path(), ScanOptions::default());
        let names: Vec<_> = snapshot
            .entries
            .iter()
            .map(|entry| entry.path.as_str().to_owned())
            .collect();
        assert_eq!(names, vec!["keep.txt"]);
        assert!(snapshot
            .skipped
            .iter()
            .any(|skipped| skipped.path.as_str() == "Thumbs.db"
                && matches!(skipped.reason, SkipReason::Junk)));
        assert!(snapshot
            .skipped
            .iter()
            .any(|skipped| skipped.path.as_str() == ".secret"
                && matches!(skipped.reason, SkipReason::Hidden)));
        #[cfg(unix)]
        {
            assert!(snapshot
                .skipped
                .iter()
                .any(|skipped| skipped.path.as_str() == "alias"
                    && matches!(skipped.reason, SkipReason::Symlink)));
        }
        let keep = snapshot
            .entries
            .iter()
            .find(|entry| entry.path.as_str() == "keep.txt")
            .unwrap_or_else(|| panic!("keep.txt"));
        assert!(keep.identity.content_fingerprint.is_some());
        assert_eq!(keep.lock, LockState::Readable);
    }

    #[test]
    fn unity_project_is_recorded_and_not_traversed() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("loose.txt"), b"x").unwrap_or_else(|e| panic!("{e}"));
        let unity = dir.path().join("Game");
        fs::create_dir_all(unity.join("Assets/Scenes")).unwrap_or_else(|e| panic!("{e}"));
        fs::create_dir_all(unity.join("ProjectSettings")).unwrap_or_else(|e| panic!("{e}"));
        fs::write(unity.join("ProjectSettings/ProjectVersion.txt"), "2022")
            .unwrap_or_else(|e| panic!("{e}"));
        fs::write(unity.join("Assets/Scenes/Main.unity"), b"scene")
            .unwrap_or_else(|e| panic!("{e}"));

        let snapshot = scan_tree(dir.path(), ScanOptions::default());
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.path.as_str() == "loose.txt"));
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.path.as_str() == "Game" && entry.kind == EntryKind::Directory));
        assert!(!snapshot
            .entries
            .iter()
            .any(|entry| entry.path.as_str().starts_with("Game/")));
        let project = snapshot
            .projects
            .iter()
            .find(|project| project.rule_id == "unity")
            .unwrap_or_else(|| panic!("unity project"));
        assert_eq!(project.strength, ProjectStrength::Strong);
        assert!(snapshot.skipped.iter().any(|skipped| matches!(
            skipped.reason,
            SkipReason::ProtectedProject { ref rule_id } if rule_id == "unity"
        )));
    }

    #[test]
    fn fingerprints_are_stable_for_the_same_bytes() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mut file = fs::File::create(dir.path().join("a.bin")).unwrap_or_else(|e| panic!("{e}"));
        file.write_all(&[1, 2, 3, 4])
            .unwrap_or_else(|e| panic!("{e}"));
        drop(file);
        fs::copy(dir.path().join("a.bin"), dir.path().join("b.bin"))
            .unwrap_or_else(|e| panic!("{e}"));
        let snapshot = scan_tree(dir.path(), ScanOptions::default());
        let hashes: Vec<_> = snapshot
            .entries
            .iter()
            .filter_map(|entry| entry.identity.content_fingerprint.clone())
            .collect();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes[0], hashes[1]);
    }

    #[test]
    fn rejects_missing_roots() {
        let err = scan(
            Path::new("/definitely-not-a-real-aifs-root"),
            &ScanOptions::default(),
            SessionId::new(),
            |_, _| {},
        );
        assert!(matches!(err, Err(ScanError::InvalidRoot { .. })));
    }
}
