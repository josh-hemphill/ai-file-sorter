//! Parse assistant chat replies into `RevisionPatch`es. Model text is untrusted.

use aifs_domain::{
    AssetId, BundleConstraint, EntryKind, ProjectStrength, ProposalRevision, RevisionPatch,
    WorkspaceSnapshot, evidence::keys,
};
use serde::Deserialize;

const CHAT_CONTEXT_FILES: usize = 48;
const CHAT_CONTEXT_CHARS: usize = 3500;
const CHAT_FACT_CHARS: usize = 80;
const UNIT_SAMPLE_LIMIT: usize = 4;

/// Model JSON that named a `patches` array (possibly empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedChat {
    pub message: Option<String>,
    pub patches: Vec<RevisionPatch>,
}

#[derive(Debug, Deserialize)]
struct ChatReplyJson {
    #[serde(default)]
    message: Option<String>,
    patches: Vec<RevisionPatch>,
}

/// Extracts a chat JSON object or patch array from `text`. Garbage yields `None`.
pub(crate) fn parse_chat_reply(text: &str) -> Option<ParsedChat> {
    let mut empty_object = None;
    let mut patched_object = None;
    for object in json_slices(text, '{', '}') {
        let Ok(parsed) = serde_json::from_str::<ChatReplyJson>(object) else {
            continue;
        };
        let chat = ParsedChat {
            message: parsed
                .message
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            patches: parsed.patches,
        };
        if chat.patches.is_empty() {
            empty_object = Some(chat);
        } else {
            patched_object = Some(chat);
        }
    }
    if patched_object.is_some() {
        return patched_object;
    }
    if empty_object.is_some() {
        return empty_object;
    }
    let mut patched_array = None;
    for array in json_slices(text, '[', ']') {
        let Ok(patches) = serde_json::from_str::<Vec<RevisionPatch>>(array) else {
            continue;
        };
        if patches.is_empty() {
            continue;
        }
        patched_array = Some(ParsedChat {
            message: None,
            patches,
        });
    }
    patched_array
}

/// Placement list the chat worker can copy asset ids from.
pub(crate) fn chat_context(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
    utterance: &str,
) -> String {
    let mut lines = vec![
        "Reply with one JSON object: {\"message\":\"brief\",\"patches\":[...]}.".to_owned(),
        "Patch ops: set_destination, move_to_folder, rename, accept, reject, reopen.".to_owned(),
        "folder and destination are root-relative; never '..', absolute paths, SQL, or shell."
            .to_owned(),
        "Move layout units as a whole. Do not patch files inside a unit unless asked to break it up."
            .to_owned(),
        "This prompt is a page, not the whole tree; prefer units, then matching loose files."
            .to_owned(),
        "Units:".to_owned(),
    ];
    let mut unit_lines = Vec::new();
    for project in &snapshot.projects {
        if project.strength != ProjectStrength::Strong {
            continue;
        }
        unit_lines.push(format!(
            "- {} protected ({}) · {}",
            project.root.as_str(),
            project.rule_id,
            project.reason
        ));
    }
    for bundle in &snapshot.bundles {
        let BundleConstraint::PreserveLayout { root } = &bundle.constraint else {
            continue;
        };
        let files = snapshot
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File && entry.path.starts_with(root))
            .count();
        let samples = unit_child_samples(snapshot, root);
        let sample_bit = if samples.is_empty() {
            String::new()
        } else {
            format!(" · {samples}")
        };
        unit_lines.push(format!(
            "- {} library-unit · {files} files{sample_bit}",
            root.as_str()
        ));
    }
    if unit_lines.is_empty() {
        lines.push("- (none)".to_owned());
    } else {
        lines.extend(unit_lines);
    }
    lines.push("Loose files (id path dest · category · description):".to_owned());
    let mut loose: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File && !snapshot.defers_content_analysis(entry))
        .collect();
    rank_loose_files(&mut loose, snapshot, utterance);
    let mut listed = 0usize;
    for entry in loose.into_iter().take(CHAT_CONTEXT_FILES) {
        lines.push(format_loose_file(snapshot, revision, entry));
        listed += 1;
    }
    if listed == 0 {
        lines.push("- (none; inspect a unit or search by name)".to_owned());
    }
    truncate_lines(&lines, CHAT_CONTEXT_CHARS)
}

fn unit_child_samples(snapshot: &WorkspaceSnapshot, root: &aifs_domain::RelativePath) -> String {
    let mut names: Vec<&str> = snapshot
        .entries
        .iter()
        .filter_map(|entry| {
            let parent = entry.path.parent()?;
            (parent == *root).then_some(entry.path.file_name())
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    names.truncate(UNIT_SAMPLE_LIMIT);
    names.join(", ")
}

fn rank_loose_files(
    entries: &mut [&aifs_domain::ObservedEntry],
    snapshot: &WorkspaceSnapshot,
    utterance: &str,
) {
    let tokens = utterance_tokens(utterance);
    entries.sort_by(|left, right| {
        score_entry(snapshot, right, &tokens)
            .cmp(&score_entry(snapshot, left, &tokens))
            .then_with(|| left.path.as_str().cmp(right.path.as_str()))
    });
}

fn utterance_tokens(utterance: &str) -> Vec<String> {
    utterance
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn score_entry(
    snapshot: &WorkspaceSnapshot,
    entry: &aifs_domain::ObservedEntry,
    tokens: &[String],
) -> usize {
    if tokens.is_empty() {
        return 0;
    }
    let mut hay = entry.path.as_str().to_ascii_lowercase();
    if let Some(category) = fact(snapshot, entry.id, keys::CATEGORY) {
        hay.push(' ');
        hay.push_str(&category.to_ascii_lowercase());
    }
    if let Some(description) = fact(snapshot, entry.id, keys::DESCRIPTION) {
        hay.push(' ');
        hay.push_str(&description.to_ascii_lowercase());
    }
    tokens
        .iter()
        .filter(|token| hay.contains(token.as_str()))
        .count()
}

fn format_loose_file(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
    entry: &aifs_domain::ObservedEntry,
) -> String {
    let dest = revision
        .placement(entry.id)
        .map(|placement| placement.destination.as_str())
        .unwrap_or("-");
    let category = fact(snapshot, entry.id, keys::CATEGORY).unwrap_or("-");
    let description = fact(snapshot, entry.id, keys::DESCRIPTION)
        .map(|text| truncate_fact(text, CHAT_FACT_CHARS))
        .unwrap_or_else(|| "-".to_owned());
    format!(
        "{} {} {dest} · {category} · {description}",
        entry.id.as_uuid(),
        entry.path.as_str()
    )
}

fn fact<'a>(snapshot: &'a WorkspaceSnapshot, id: AssetId, key: &str) -> Option<&'a str> {
    snapshot.evidence_for(id).find_map(|bag| bag.fact(key))
}

fn truncate_fact(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let mut truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// Drops destination edits for protected or layout-preserving bundle members.
pub(crate) fn drop_blocked_moves(
    snapshot: &WorkspaceSnapshot,
    patches: Vec<RevisionPatch>,
) -> (Vec<RevisionPatch>, usize) {
    let mut kept = Vec::new();
    let mut skipped = 0usize;
    for patch in patches {
        match patch {
            RevisionPatch::MoveToFolder {
                assets,
                folder,
                rationale,
            } => {
                let (assets, dropped) = partition_movable(snapshot, assets);
                skipped += dropped;
                if !assets.is_empty() {
                    kept.push(RevisionPatch::MoveToFolder {
                        assets,
                        folder,
                        rationale,
                    });
                }
            }
            RevisionPatch::SetDestination {
                asset,
                destination,
                rationale,
            } => {
                if blocks_model_move(snapshot, asset) {
                    skipped += 1;
                } else {
                    kept.push(RevisionPatch::SetDestination {
                        asset,
                        destination,
                        rationale,
                    });
                }
            }
            RevisionPatch::Rename { asset, file_name } => {
                if blocks_model_move(snapshot, asset) {
                    skipped += 1;
                } else {
                    kept.push(RevisionPatch::Rename { asset, file_name });
                }
            }
            other => kept.push(other),
        }
    }
    (kept, skipped)
}

fn partition_movable(snapshot: &WorkspaceSnapshot, assets: Vec<AssetId>) -> (Vec<AssetId>, usize) {
    let mut kept = Vec::new();
    let mut skipped = 0usize;
    for asset in assets {
        if blocks_model_move(snapshot, asset) {
            skipped += 1;
        } else {
            kept.push(asset);
        }
    }
    (kept, skipped)
}

fn blocks_model_move(snapshot: &WorkspaceSnapshot, asset: AssetId) -> bool {
    match snapshot.hard_bundle_for(asset) {
        Some(bundle) if bundle.is_protected() => true,
        Some(bundle) if matches!(bundle.constraint, BundleConstraint::PreserveLayout { .. }) => {
            true
        }
        _ => false,
    }
}

fn json_slices(text: &str, open: char, close: char) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut slices = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index].1 != open {
            index += 1;
            continue;
        }
        let Some(end) = scan_balanced(&chars, index, open, close) else {
            index += 1;
            continue;
        };
        let start_byte = chars[index].0;
        let end_byte = chars[end].0 + chars[end].1.len_utf8();
        slices.push(&text[start_byte..end_byte]);
        index = end + 1;
    }
    slices
}

fn scan_balanced(chars: &[(usize, char)], start: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (index, &(_, ch)) in chars.iter().enumerate().skip(start) {
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn truncate_lines(lines: &[String], max_chars: usize) -> String {
    let mut out = String::new();
    for line in lines {
        let extra = if out.is_empty() {
            line.chars().count()
        } else {
            1 + line.chars().count()
        };
        if !out.is_empty() && out.chars().count() + extra > max_chars {
            break;
        }
        if out.is_empty() && line.chars().count() > max_chars {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        AssetId, Bundle, BundleConstraint, BundleId, BundleKind, EntryKind, FileFamily,
        FileIdentity, LockState, ObservedEntry, Placement, RelativePath, ReviewState,
        RevisionAuthor, SessionId, SuggestionOrigin, WorkspaceSnapshot, evidence::keys,
    };
    use std::path::PathBuf;

    fn audio_snapshot() -> (WorkspaceSnapshot, ProposalRevision, AssetId) {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/inbox"));
        let asset = AssetId::new();
        snapshot.entries.push(ObservedEntry {
            id: asset,
            path: RelativePath::parse("show.mp3").unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::Audio,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        });
        let mut revision = ProposalRevision::new(snapshot.session, RevisionAuthor::Engine, "p");
        revision.placements.insert(
            asset,
            Placement {
                asset,
                destination: RelativePath::parse("Music/show.mp3")
                    .unwrap_or_else(|e| panic!("{e}")),
                rationale: None,
                origin: SuggestionOrigin::Heuristic,
                review: ReviewState::Proposed,
            },
        );
        (snapshot, revision, asset)
    }

    #[test]
    fn parse_object_patches_and_ignore_prose() {
        let asset = AssetId::new();
        let text = format!(
            "Sure.\n{{\"message\":\"Moved\",\"patches\":[{{\"op\":\"move_to_folder\",\"assets\":[\"{}\"],\"folder\":\"Broadcasts\"}}]}}",
            asset.as_uuid()
        );
        let parsed = parse_chat_reply(&text).unwrap_or_else(|| panic!("expected json"));
        assert_eq!(parsed.message.as_deref(), Some("Moved"));
        match &parsed.patches[..] {
            [RevisionPatch::MoveToFolder { assets, folder, .. }] => {
                assert_eq!(assets, &vec![asset]);
                assert_eq!(folder.as_str(), "Broadcasts");
            }
            other => panic!("{other:?}"),
        }
        assert!(parse_chat_reply("Grouping audio into Podcasts.").is_none());
        assert!(
            parse_chat_reply(r#"{"choices":[{"message":{"content":"hi"}}]}"#).is_none(),
            "OpenAI envelopes must not count as a chat reply"
        );
        assert!(
            parse_chat_reply(r#"{"message":"hi"}"#).is_none(),
            "objects without a patches key must not skip keyword fallback"
        );
        let empty = parse_chat_reply(r#"{"message":"Just thinking.","patches":[]}"#)
            .unwrap_or_else(|| panic!("empty patches is parseable"));
        assert_eq!(empty.message.as_deref(), Some("Just thinking."));
        assert!(empty.patches.is_empty());
    }

    #[test]
    fn parse_skips_schema_echo_and_stray_brackets() {
        let asset = AssetId::new();
        let echoed = format!(
            "Reply with {{\"message\":\"brief\",\"patches\":[]}}. stray [] then\n\
{{\"message\":\"Moved\",\"patches\":[{{\"op\":\"move_to_folder\",\"assets\":[\"{}\"],\"folder\":\"Broadcasts\"}}]}}",
            asset.as_uuid()
        );
        let parsed = parse_chat_reply(&echoed).unwrap_or_else(|| panic!("expected later object"));
        assert_eq!(parsed.message.as_deref(), Some("Moved"));
        match &parsed.patches[..] {
            [RevisionPatch::MoveToFolder { folder, .. }] => {
                assert_eq!(folder.as_str(), "Broadcasts");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            parse_chat_reply("I will not emit [] this turn.").is_none(),
            "stray empty arrays must not skip keyword fallback"
        );
        let example = format!(
            "[{{ \"op\":\"move_to_folder\",\"assets\":[\"{}\"],\"folder\":\"Example\" }}] then \
[{{ \"op\":\"move_to_folder\",\"assets\":[\"{}\"],\"folder\":\"Broadcasts\" }}]",
            asset.as_uuid(),
            asset.as_uuid()
        );
        let last = parse_chat_reply(&example).unwrap_or_else(|| panic!("expected last array"));
        match &last.patches[..] {
            [RevisionPatch::MoveToFolder { folder, .. }] => {
                assert_eq!(folder.as_str(), "Broadcasts");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parent_traversal_folder_is_not_a_patch() {
        let asset = AssetId::new();
        let text = format!(
            "{{\"patches\":[{{\"op\":\"move_to_folder\",\"assets\":[\"{}\"],\"folder\":\"../etc\"}}]}}",
            asset.as_uuid()
        );
        assert!(parse_chat_reply(&text).is_none());
        let absolute = format!(
            "{{\"patches\":[{{\"op\":\"move_to_folder\",\"assets\":[\"{}\"],\"folder\":\"/etc\"}}]}}",
            asset.as_uuid()
        );
        assert!(parse_chat_reply(&absolute).is_none());
    }

    #[test]
    fn drop_blocked_moves_skips_protected_members() {
        let (mut snapshot, _, asset) = audio_snapshot();
        snapshot.bundles.push(Bundle {
            id: BundleId::new(),
            kind: BundleKind::Project,
            label: "repo".into(),
            members: vec![asset],
            anchor: Some(asset),
            constraint: BundleConstraint::Protected {
                reason: "git".into(),
            },
            reason: "git".into(),
        });
        let (kept, skipped) = drop_blocked_moves(
            &snapshot,
            vec![RevisionPatch::MoveToFolder {
                assets: vec![asset],
                folder: RelativePath::parse("Podcasts").unwrap_or_else(|e| panic!("{e}")),
                rationale: None,
            }],
        );
        assert!(kept.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn drop_blocked_moves_skips_preserve_layout_members() {
        let (mut snapshot, _, asset) = audio_snapshot();
        snapshot.bundles.push(Bundle {
            id: BundleId::new(),
            kind: BundleKind::Folder,
            label: "album".into(),
            members: vec![asset],
            anchor: Some(asset),
            constraint: BundleConstraint::PreserveLayout {
                root: RelativePath::parse("album").unwrap_or_else(|e| panic!("{e}")),
            },
            reason: "folder".into(),
        });
        let (kept, skipped) = drop_blocked_moves(
            &snapshot,
            vec![RevisionPatch::Rename {
                asset,
                file_name: "x.mp3".into(),
            }],
        );
        assert!(kept.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn chat_context_lists_hyphenated_ids() {
        let (snapshot, revision, asset) = audio_snapshot();
        let context = chat_context(&snapshot, &revision, "group audio");
        assert!(context.contains(&asset.as_uuid().to_string()));
        assert!(context.contains("show.mp3"));
        assert!(context.contains("patches"));
        assert!(context.contains("Units:"));
        assert!(context.contains("Loose files"));
    }

    #[test]
    fn chat_context_summarises_layout_units_instead_of_their_files() {
        let (mut snapshot, mut revision, loose) = audio_snapshot();
        let album = aifs_domain::AssetId::new();
        let track = aifs_domain::AssetId::new();
        snapshot.entries.push(aifs_domain::ObservedEntry {
            id: album,
            path: RelativePath::parse("Pictures").unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::Directory,
            family: FileFamily::Generic,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        });
        snapshot.entries.push(aifs_domain::ObservedEntry {
            id: track,
            path: RelativePath::parse("Pictures/shot.jpg").unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::Image,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        });
        snapshot.bundles.push(Bundle {
            id: BundleId::new(),
            kind: BundleKind::Folder,
            label: "Pictures".into(),
            members: vec![album, track],
            anchor: Some(album),
            constraint: BundleConstraint::PreserveLayout {
                root: RelativePath::parse("Pictures").unwrap_or_else(|e| panic!("{e}")),
            },
            reason: "library".into(),
        });
        snapshot.evidence.push(
            aifs_domain::Evidence::new(
                loose,
                aifs_domain::EvidenceSource::LocalModel {
                    model: "stub".into(),
                },
                aifs_domain::Confidence::new(0.5),
            )
            .with_fact(keys::DESCRIPTION, "late-night mix"),
        );
        revision.placements.insert(
            track,
            Placement {
                asset: track,
                destination: RelativePath::parse("Pictures/shot.jpg")
                    .unwrap_or_else(|e| panic!("{e}")),
                rationale: None,
                origin: SuggestionOrigin::Heuristic,
                review: ReviewState::Proposed,
            },
        );
        let context = chat_context(&snapshot, &revision, "pictures library");
        assert!(context.contains("library-unit"));
        assert!(context.contains("Pictures"));
        assert!(!context.contains(&track.as_uuid().to_string()));
        assert!(context.contains("late-night mix"));
        assert!(context.contains(&loose.as_uuid().to_string()));
    }
}
