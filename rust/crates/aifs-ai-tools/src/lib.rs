//! Deterministic AI tools that emit proposal patches, never SQL or filesystem ops.

use aifs_domain::{
    evidence::keys, AssetId, Bundle, BundleConstraint, BundleKind, EntryKind, Evidence, FileFamily,
    ObservedEntry, Placement, ProposalRevision, RelativePath, RevisionPatch, WorkspaceSnapshot,
};
use aifs_planner::validate;

/// Model id recorded on assistant-authored revisions produced by these tools.
pub const MOCK_ASSISTANT_MODEL: &str = "mock-tools";

/// One tool invocation produced by interpreting user intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCall {
    /// Find assets by relative path or evidence text.
    Search {
        /// Free-text needle.
        query: String,
    },
    /// Describe a matching bundle and its constraint.
    InspectBundle {
        /// Free-text needle (label, path, or relationship language).
        query: String,
    },
    /// Summarise proposed destination folders.
    ShowStructure,
    /// Move a family of assets into one folder, keeping file names.
    GroupFamily {
        /// Family key (`audio`, `selected`, or a path substring).
        family: String,
        /// Destination folder relative to the session root.
        folder: String,
    },
    /// Rename media files from extracted tags.
    ApplyNamingTemplate {
        /// Template id (`year_subject`).
        template: String,
    },
    /// Explain a placement or bundle in the current revision.
    Explain {
        /// Free-text needle.
        query: String,
    },
    /// Run plan validation against the current revision.
    Validate,
}

/// Result of executing tools against a frozen revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// Assistant-facing summary (never executable code).
    pub message: String,
    /// Patches to apply with [`ProposalRevision::with_patches`].
    pub patches: Vec<RevisionPatch>,
}

/// Interprets a chat utterance into tool calls. Keyword-based until a model worker exists.
pub fn interpret(utterance: &str) -> Vec<ToolCall> {
    let lower = utterance.to_ascii_lowercase();
    if looks_like_search(&lower) {
        return vec![ToolCall::Search {
            query: extract_quoted_or_rest(utterance, &["find", "search", "where is", "where"]),
        }];
    }
    if looks_like_validate(&lower) {
        return vec![ToolCall::Validate];
    }
    if looks_like_structure(&lower) {
        return vec![ToolCall::ShowStructure];
    }
    if looks_like_inspect(&lower) {
        return vec![ToolCall::InspectBundle {
            query: extract_quoted_or_rest(utterance, &["inspect", "explain", "why", "keep"]),
        }];
    }
    if looks_like_naming(&lower) {
        return vec![ToolCall::ApplyNamingTemplate {
            template: "year_subject".into(),
        }];
    }
    if looks_like_podcast(&lower) {
        return vec![ToolCall::GroupFamily {
            family: "audio".into(),
            folder: "Podcasts".into(),
        }];
    }
    if looks_like_group(&lower) {
        let folder = extract_quoted_or_rest(utterance, &["group", "move", "put"]);
        let folder = if folder.is_empty() {
            "Grouped".into()
        } else {
            first_path_token(&folder)
        };
        return vec![ToolCall::GroupFamily {
            family: "selected".into(),
            folder,
        }];
    }
    vec![ToolCall::Explain {
        query: utterance.trim().to_string(),
    }]
}

/// Executes interpreted tools against a snapshot and revision.
pub fn execute(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
    calls: &[ToolCall],
) -> ToolOutput {
    let mut messages = Vec::new();
    let mut patches = Vec::new();
    for call in calls {
        match call {
            ToolCall::Search { query } => messages.push(format_search(snapshot, query)),
            ToolCall::InspectBundle { query } => {
                messages.push(format_inspect(snapshot, revision, query));
            }
            ToolCall::ShowStructure => messages.push(format_structure(revision)),
            ToolCall::GroupFamily { family, folder } => {
                let (msg, extra) = group_family(snapshot, revision, family, folder);
                messages.push(msg);
                patches.extend(extra);
            }
            ToolCall::ApplyNamingTemplate { template } => {
                let (msg, extra) = apply_naming(snapshot, revision, template);
                messages.push(msg);
                patches.extend(extra);
            }
            ToolCall::Explain { query } => {
                messages.push(format_inspect(snapshot, revision, query));
            }
            ToolCall::Validate => messages.push(format_validate(snapshot, revision)),
        }
    }
    if messages.is_empty() {
        messages.push("I did not find a matching tool for that request.".into());
    }
    ToolOutput {
        message: messages.join("\n\n"),
        patches,
    }
}

fn looks_like_search(lower: &str) -> bool {
    lower.contains("find ")
        || lower.contains("search ")
        || lower.starts_with("find")
        || lower.starts_with("search")
        || lower.contains("where is")
}

fn looks_like_validate(lower: &str) -> bool {
    lower.contains("validate") || lower.contains("check the plan") || lower.contains("is this safe")
}

fn looks_like_structure(lower: &str) -> bool {
    lower.contains("show structure")
        || lower.contains("current structure")
        || lower.contains("proposed tree")
        || lower.contains("show the tree")
}

fn looks_like_inspect(lower: &str) -> bool {
    lower.contains("inspect")
        || lower.contains("explain")
        || lower.contains("why ")
        || lower.contains("keep raw")
        || lower.contains("together")
}

fn looks_like_naming(lower: &str) -> bool {
    lower.contains("rename")
        || lower.contains("naming")
        || lower.contains("filename")
        || (lower.contains("tag") && lower.contains("name"))
        || lower.contains("from metadata")
}

fn looks_like_podcast(lower: &str) -> bool {
    lower.contains("podcast")
}

fn looks_like_group(lower: &str) -> bool {
    lower.contains("group") || lower.contains("move ") || lower.contains("put ")
}

fn extract_quoted_or_rest(utterance: &str, verbs: &[&str]) -> String {
    if let Some(start) = utterance.find('"') {
        if let Some(end) = utterance[start + 1..].find('"') {
            return utterance[start + 1..start + 1 + end].to_string();
        }
    }
    let lower = utterance.to_ascii_lowercase();
    for verb in verbs {
        if let Some(idx) = lower.find(verb) {
            return utterance[idx + verb.len()..]
                .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
                .to_string();
        }
    }
    utterance.trim().to_string()
}

fn first_path_token(value: &str) -> String {
    value
        .split_whitespace()
        .next()
        .unwrap_or(value)
        .trim_matches(|c: char| c.is_ascii_punctuation())
        .to_string()
}

fn format_search(snapshot: &WorkspaceSnapshot, query: &str) -> String {
    let q = query.to_ascii_lowercase();
    let hits: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|entry| {
            entry.path.as_str().to_ascii_lowercase().contains(&q)
                || snapshot
                    .evidence_for(entry.id)
                    .any(|ev| evidence_matches(ev, &q))
        })
        .take(20)
        .collect();
    if hits.is_empty() {
        return format!("No assets matched `{query}`.");
    }
    let lines: Vec<_> = hits
        .iter()
        .map(|entry| format!("- {} (`{}`)", entry.path.as_str(), entry.id))
        .collect();
    format!("Found {} asset(s):\n{}", hits.len(), lines.join("\n"))
}

fn evidence_matches(evidence: &Evidence, query: &str) -> bool {
    evidence.facts.iter().any(|(k, v)| {
        k.to_ascii_lowercase().contains(query) || v.to_ascii_lowercase().contains(query)
    })
}

fn format_inspect(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
    query: &str,
) -> String {
    let q = query.to_ascii_lowercase();
    let bundle = snapshot
        .bundles
        .iter()
        .find(|bundle| bundle_matches(snapshot, bundle, &q));
    let Some(bundle) = bundle else {
        if let Some(entry) = snapshot
            .entries
            .iter()
            .find(|entry| entry.path.as_str().to_ascii_lowercase().contains(&q))
        {
            return format_placement(snapshot, revision, entry);
        }
        return format!("No bundle or asset matched `{query}`.");
    };
    let members: Vec<_> = bundle
        .members
        .iter()
        .filter_map(|id| snapshot.entry(*id))
        .map(|entry| {
            let dest = revision
                .placement(entry.id)
                .map(|p| p.destination.as_str())
                .unwrap_or("(unplaced)");
            format!("- {} → {dest}", entry.path.as_str())
        })
        .collect();
    format!(
        "Bundle **{}** ({}, {}):\n{}\n{}",
        bundle.label,
        bundle_kind_label(bundle.kind),
        constraint_label(&bundle.constraint),
        bundle.reason,
        members.join("\n")
    )
}

fn bundle_matches(snapshot: &WorkspaceSnapshot, bundle: &Bundle, query: &str) -> bool {
    if query.is_empty() {
        return bundle.is_hard();
    }
    let wants_sidecar = query.contains("raw")
        || query.contains("jpeg")
        || query.contains("sidecar")
        || query.contains("together")
        || query.contains("pair");
    if wants_sidecar && matches!(bundle.kind, BundleKind::SidecarGroup) {
        return true;
    }
    bundle.label.to_ascii_lowercase().contains(query)
        || bundle.reason.to_ascii_lowercase().contains(query)
        || bundle.members.iter().any(|id| {
            snapshot
                .entry(*id)
                .is_some_and(|entry| entry.path.as_str().to_ascii_lowercase().contains(query))
        })
}

fn format_placement(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
    entry: &ObservedEntry,
) -> String {
    let dest = revision
        .placement(entry.id)
        .map(|placement| {
            format!(
                "{} ({})",
                placement.destination.as_str(),
                placement.rationale.as_deref().unwrap_or("no rationale")
            )
        })
        .unwrap_or_else(|| "(unplaced)".into());
    let evidence: Vec<_> = snapshot
        .evidence_for(entry.id)
        .flat_map(|bag| bag.facts.iter().map(|(k, v)| format!("{k}={v}")))
        .collect();
    if evidence.is_empty() {
        format!("{} is proposed at {dest}.", entry.path.as_str())
    } else {
        format!(
            "{} is proposed at {dest}. Evidence: {}.",
            entry.path.as_str(),
            evidence.join(", ")
        )
    }
}

fn bundle_kind_label(kind: BundleKind) -> &'static str {
    match kind {
        BundleKind::Project => "project",
        BundleKind::SidecarGroup => "sidecar group",
        BundleKind::Series => "series",
        BundleKind::ArchiveParts => "archive parts",
        BundleKind::Manual => "manual",
    }
}

fn constraint_label(constraint: &BundleConstraint) -> String {
    match constraint {
        BundleConstraint::Soft => "soft".into(),
        BundleConstraint::MoveTogether => "must move together".into(),
        BundleConstraint::PreserveLayout { root } => {
            format!("preserve layout at {}", root.as_str())
        }
        BundleConstraint::Protected { reason } => format!("protected ({reason})"),
    }
}

fn format_structure(revision: &ProposalRevision) -> String {
    let mut folders: Vec<_> = revision.placements.values().map(folder_of).collect();
    folders.sort();
    folders.dedup();
    if folders.is_empty() {
        return "The current proposal has no placements yet.".into();
    }
    format!(
        "Proposed folders:\n{}",
        folders
            .iter()
            .map(|folder| format!("- {folder}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn folder_of(placement: &Placement) -> String {
    placement
        .destination
        .parent()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| ".".into())
}

fn group_family(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
    family: &str,
    folder: &str,
) -> (String, Vec<RevisionPatch>) {
    let Ok(dest_folder) = RelativePath::parse(folder) else {
        return (
            format!("`{folder}` is not a valid destination folder."),
            Vec::new(),
        );
    };
    let mut assets: Vec<AssetId> = Vec::new();
    for placement in revision.placements.values() {
        let Some(entry) = snapshot.entry(placement.asset) else {
            continue;
        };
        if entry.kind != EntryKind::File {
            continue;
        }
        if !family_matches(family, entry) {
            continue;
        }
        assets.push(placement.asset);
    }
    if assets.is_empty() {
        return (
            format!("No {family} assets to move into `{folder}`."),
            Vec::new(),
        );
    }
    let moved = assets.len();
    (
        format!("Moved {moved} {family} asset(s) into `{folder}`."),
        vec![RevisionPatch::MoveToFolder {
            assets,
            folder: dest_folder,
            rationale: Some(format!("chat: group {family}")),
        }],
    )
}

fn family_matches(family: &str, entry: &ObservedEntry) -> bool {
    match family {
        "audio" => entry.family == FileFamily::Audio,
        "selected" => true,
        other => entry
            .path
            .as_str()
            .to_ascii_lowercase()
            .contains(&other.to_ascii_lowercase()),
    }
}

fn apply_naming(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
    template: &str,
) -> (String, Vec<RevisionPatch>) {
    let _ = template;
    let mut patches = Vec::new();
    for placement in revision.placements.values() {
        let Some(entry) = snapshot.entry(placement.asset) else {
            continue;
        };
        let Some(name) = name_from_evidence(snapshot, entry) else {
            continue;
        };
        if name == placement.destination.file_name() {
            continue;
        }
        patches.push(RevisionPatch::Rename {
            asset: placement.asset,
            file_name: name,
        });
    }
    (
        format!(
            "Applied metadata naming template to {} file(s).",
            patches.len()
        ),
        patches,
    )
}

fn name_from_evidence(snapshot: &WorkspaceSnapshot, entry: &ObservedEntry) -> Option<String> {
    let year = snapshot
        .evidence_for(entry.id)
        .find_map(|ev| ev.fact(keys::MEDIA_YEAR).map(str::to_owned))?;
    let subject = snapshot
        .evidence_for(entry.id)
        .find_map(|ev| {
            ev.fact(keys::MEDIA_TITLE)
                .or_else(|| ev.fact(keys::MEDIA_ARTIST))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| entry.stem().to_owned());
    let ext = entry.extension()?;
    let slug = sanitize_filename(&format!("{year}_{subject}"));
    let name = format!("{slug}.{ext}");
    RelativePath::parse_file_name(&name).ok()?;
    Some(name)
}

fn sanitize_filename(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch.is_whitespace() && !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "untitled".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn format_validate(snapshot: &WorkspaceSnapshot, revision: &ProposalRevision) -> String {
    let (plan, issues) = validate(snapshot, revision);
    if issues.is_empty() {
        let ops = plan.map(|p| p.operations.len()).unwrap_or(0);
        return format!(
            "Revision `{}` is valid with {} placement(s) and {ops} planned operation(s).",
            revision.id,
            revision.placements.len()
        );
    }
    let lines: Vec<_> = issues
        .iter()
        .map(|issue| format!("- {:?}: {} ({})", issue.severity, issue.message, issue.code))
        .collect();
    format!("Validation findings:\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        AssetId, Confidence, EntryKind, Evidence, EvidenceSource, FileIdentity, LockState,
        Placement, RelativePath, ReviewState, RevisionAuthor, SessionId, SuggestionOrigin,
        WorkspaceSnapshot,
    };
    use std::path::PathBuf;

    fn snapshot_with_audio() -> (WorkspaceSnapshot, ProposalRevision) {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/inbox"));
        let asset = AssetId::new();
        snapshot.entries.push(ObservedEntry {
            id: asset,
            path: RelativePath::parse("inbox/show.mp3").unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::Audio,
            identity: FileIdentity {
                size: 12,
                ..FileIdentity::default()
            },
            is_hidden: false,
            lock: LockState::Readable,
        });
        snapshot.evidence.push(
            Evidence::new(asset, EvidenceSource::MediaTags, Confidence::CERTAIN)
                .with_fact(keys::MEDIA_YEAR, "2024")
                .with_fact(keys::MEDIA_TITLE, "Ada Night"),
        );
        let mut revision = ProposalRevision::new(snapshot.session, RevisionAuthor::Engine, "seed");
        revision.place(Placement {
            asset,
            destination: RelativePath::parse("Music/show.mp3").unwrap_or_else(|e| panic!("{e}")),
            rationale: Some("audio".into()),
            origin: SuggestionOrigin::Heuristic,
            review: ReviewState::Proposed,
        });
        (snapshot, revision)
    }

    #[test]
    fn podcast_utterance_moves_audio_into_podcasts() {
        let (snapshot, revision) = snapshot_with_audio();
        let calls = interpret("Move podcasts away from music, but keep seasons shallow.");
        assert!(matches!(
            calls.first(),
            Some(ToolCall::GroupFamily { family, folder })
                if family == "audio" && folder == "Podcasts"
        ));
        let output = execute(&snapshot, &revision, &calls);
        assert_eq!(output.patches.len(), 1);
        match &output.patches[0] {
            RevisionPatch::MoveToFolder { assets, folder, .. } => {
                assert_eq!(assets.len(), 1);
                assert_eq!(folder.as_str(), "Podcasts");
            }
            other => panic!("unexpected patch {other:?}"),
        }
        assert!(output.message.contains("Podcasts"));
    }

    #[test]
    fn naming_template_uses_year_and_title() {
        let (snapshot, revision) = snapshot_with_audio();
        let output = execute(
            &snapshot,
            &revision,
            &[ToolCall::ApplyNamingTemplate {
                template: "year_subject".into(),
            }],
        );
        match &output.patches[0] {
            RevisionPatch::Rename { file_name, .. } => {
                assert_eq!(file_name, "2024_Ada_Night.mp3");
            }
            other => panic!("unexpected patch {other:?}"),
        }
    }

    #[test]
    fn search_finds_relative_path() {
        let (snapshot, revision) = snapshot_with_audio();
        let output = execute(
            &snapshot,
            &revision,
            &[ToolCall::Search {
                query: "show.mp3".into(),
            }],
        );
        assert!(output.message.contains("inbox/show.mp3"));
        assert!(output.patches.is_empty());
    }

    #[test]
    fn inspect_together_finds_sidecar_language() {
        let (mut snapshot, revision) = snapshot_with_audio();
        snapshot.bundles.push(Bundle {
            id: aifs_domain::BundleId::new(),
            kind: BundleKind::SidecarGroup,
            label: "photo".into(),
            members: snapshot.entries.iter().map(|e| e.id).collect(),
            anchor: snapshot.entries.first().map(|e| e.id),
            constraint: BundleConstraint::MoveTogether,
            reason: "RAW and JPEG share a stem".into(),
        });
        let calls = interpret("Keep RAW and JPEG pairs together.");
        assert!(matches!(
            calls.first(),
            Some(ToolCall::InspectBundle { .. })
        ));
        let output = execute(&snapshot, &revision, &calls);
        assert!(output.message.contains("must move together"));
        assert!(output.patches.is_empty());
    }
}
