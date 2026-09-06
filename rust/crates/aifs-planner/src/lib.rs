//! Heuristic organisation proposals and validation of revisions into operation plans.

pub mod compare;

pub use compare::{destination_map, diff_plan, ExpectedPlan};

use aifs_domain::{
    evidence::keys, AssetId, BundleConstraint, EntryKind, FileFamily, ObservedEntry, Operation,
    OperationPlan, Placement, PlanId, PlanIssue, PlanIssueSeverity, PlannedOperation,
    ProposalRevision, RelativePath, RelativePathError, ReviewState, RevisionAuthor,
    SuggestionOrigin, Timestamp, WorkspaceSnapshot,
};
use aifs_protocol::{FolderStyle, ProposalPolicy};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Builds a root heuristic revision from a snapshot. Placements start as `proposed`.
pub fn propose(snapshot: &WorkspaceSnapshot, policy: &ProposalPolicy) -> ProposalRevision {
    let mut revision = ProposalRevision::new(
        snapshot.session,
        RevisionAuthor::Engine,
        "heuristic proposal",
    );
    let mut destinations: HashMap<AssetId, (RelativePath, SuggestionOrigin, Option<String>)> =
        HashMap::new();

    for entry in snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        destinations.insert(entry.id, destination_for(snapshot, entry, policy));
    }

    apply_bundle_constraints(snapshot, &mut destinations);

    for entry in snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        let Some((destination, origin, rationale)) = destinations.remove(&entry.id) else {
            continue;
        };
        revision.place(Placement {
            asset: entry.id,
            destination,
            rationale,
            origin,
            review: ReviewState::Proposed,
        });
    }
    revision
}

fn destination_for(
    snapshot: &WorkspaceSnapshot,
    entry: &ObservedEntry,
    policy: &ProposalPolicy,
) -> (RelativePath, SuggestionOrigin, Option<String>) {
    if policy.pinned_families.contains(&entry.family) {
        return (
            entry.path.clone(),
            SuggestionOrigin::Unchanged,
            Some(format!("{} files are pinned", folder_label(entry.family))),
        );
    }
    if snapshot
        .hard_bundle_for(entry.id)
        .is_some_and(|bundle| bundle.is_protected())
    {
        return (
            entry.path.clone(),
            SuggestionOrigin::Unchanged,
            Some("protected project member".to_owned()),
        );
    }
    if let Some(role) = covering_role(snapshot, &entry.path) {
        if matches!(
            role.kind,
            aifs_domain::DirectoryRoleKind::Library | aifs_domain::DirectoryRoleKind::WeakArchive
        ) {
            return (
                entry.path.clone(),
                SuggestionOrigin::Unchanged,
                Some(role.reason.clone()),
            );
        }
    }

    let folder = folder_for(snapshot, entry, policy);
    let file_name = file_name_for(snapshot, entry, policy);
    let destination = match folder.join(&file_name) {
        Ok(path) => path,
        Err(_) => entry.path.clone(),
    };
    let origin = if destination == entry.path {
        SuggestionOrigin::Unchanged
    } else {
        SuggestionOrigin::Heuristic
    };
    let rationale = if origin == SuggestionOrigin::Heuristic {
        Some(format!("place in {}", folder))
    } else {
        None
    };
    (destination, origin, rationale)
}

fn covering_role<'a>(
    snapshot: &'a WorkspaceSnapshot,
    path: &RelativePath,
) -> Option<&'a aifs_domain::DirectoryRoleMatch> {
    snapshot
        .directory_roles
        .iter()
        .filter(|role| path.starts_with(&role.root))
        .max_by_key(|role| role.root.as_str().len())
}

fn folder_for(
    snapshot: &WorkspaceSnapshot,
    entry: &ObservedEntry,
    policy: &ProposalPolicy,
) -> RelativePath {
    let mut folder = entry.family.default_folder().to_owned();
    if policy.style == FolderStyle::Refined {
        if entry.family == FileFamily::Audio
            && evidence_text(snapshot, entry.id, keys::MEDIA_GENRE)
                .is_some_and(|genre| genre.to_ascii_lowercase().contains("podcast"))
        {
            folder = "Podcasts".to_owned();
        }
        if entry.family == FileFamily::Image
            && entry
                .path
                .file_name()
                .to_ascii_lowercase()
                .contains("screenshot")
        {
            folder = "Screenshots".to_owned();
        }
    }
    if policy.use_subfolders {
        if let Some(artist) = evidence_text(snapshot, entry.id, keys::MEDIA_ARTIST) {
            if matches!(entry.family, FileFamily::Audio | FileFamily::Video) {
                folder = format!("{folder}/{}", sanitize_segment(&artist, "Unknown"));
            }
        }
    }
    RelativePath::parse(&folder).unwrap_or_else(|_| {
        RelativePath::parse(entry.family.default_folder()).unwrap_or_else(|_| entry.path.clone())
    })
}

fn file_name_for(
    snapshot: &WorkspaceSnapshot,
    entry: &ObservedEntry,
    policy: &ProposalPolicy,
) -> String {
    let original = entry.path.file_name().to_owned();
    let ext = entry.extension().unwrap_or_default();
    if policy.rename_media && matches!(entry.family, FileFamily::Audio | FileFamily::Video) {
        if let Some(title) = evidence_text(snapshot, entry.id, keys::MEDIA_TITLE) {
            let artist = evidence_text(snapshot, entry.id, keys::MEDIA_ARTIST);
            let stem = match artist {
                Some(artist) => format!(
                    "{} - {}",
                    sanitize_segment(&artist, "Unknown"),
                    sanitize_segment(&title, "Untitled")
                ),
                None => sanitize_segment(&title, "Untitled"),
            };
            return with_extension(&stem, &ext, &original);
        }
    }
    original
}

fn apply_bundle_constraints(
    snapshot: &WorkspaceSnapshot,
    destinations: &mut HashMap<AssetId, (RelativePath, SuggestionOrigin, Option<String>)>,
) {
    for bundle in &snapshot.bundles {
        match &bundle.constraint {
            BundleConstraint::Protected { reason } => {
                for member in &bundle.members {
                    if let Some(entry) = snapshot.entry(*member) {
                        destinations.insert(
                            *member,
                            (
                                entry.path.clone(),
                                SuggestionOrigin::Unchanged,
                                Some(reason.clone()),
                            ),
                        );
                    }
                }
            }
            BundleConstraint::MoveTogether => {
                let Some(anchor) = bundle.anchor.and_then(|id| destinations.get(&id)) else {
                    continue;
                };
                let folder = anchor.0.parent().unwrap_or_else(|| anchor.0.clone());
                let member_ids = bundle.members.clone();
                for member in member_ids {
                    let Some(entry) = snapshot.entry(member) else {
                        continue;
                    };
                    let dest = folder
                        .join(entry.path.file_name())
                        .unwrap_or_else(|_| entry.path.clone());
                    destinations.insert(
                        member,
                        (
                            dest,
                            SuggestionOrigin::Heuristic,
                            Some(format!("keep with {}", bundle.label)),
                        ),
                    );
                }
            }
            BundleConstraint::PreserveLayout { root } => {
                let Some(anchor_id) = bundle.anchor else {
                    continue;
                };
                let Some(anchor_dest) = destinations.get(&anchor_id).map(|pair| pair.0.clone())
                else {
                    continue;
                };
                let member_ids = bundle.members.clone();
                for member in member_ids {
                    let Some(entry) = snapshot.entry(member) else {
                        continue;
                    };
                    let dest = relocate_under(root, &entry.path, &anchor_dest)
                        .unwrap_or_else(|_| entry.path.clone());
                    destinations.insert(
                        member,
                        (
                            dest,
                            SuggestionOrigin::Heuristic,
                            Some("preserve project layout".to_owned()),
                        ),
                    );
                }
            }
            BundleConstraint::Soft => {}
        }
    }
}

fn relocate_under(
    old_root: &RelativePath,
    path: &RelativePath,
    new_root: &RelativePath,
) -> Result<RelativePath, RelativePathError> {
    if path == old_root {
        return Ok(new_root.clone());
    }
    let prefix = format!("{}/", old_root.as_str());
    if let Some(rest) = path.as_str().strip_prefix(&prefix) {
        new_root.join(rest)
    } else {
        Ok(path.clone())
    }
}

fn evidence_text(snapshot: &WorkspaceSnapshot, asset: AssetId, key: &str) -> Option<String> {
    snapshot
        .evidence_for(asset)
        .find_map(|evidence| evidence.fact(key).map(ToOwned::to_owned))
}

fn folder_label(family: FileFamily) -> &'static str {
    family.default_folder()
}

fn sanitize_segment(value: &str, fallback: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let trimmed = out.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn with_extension(stem: &str, ext: &str, fallback: &str) -> String {
    if stem.is_empty() {
        return fallback.to_owned();
    }
    if ext.is_empty() {
        stem.to_owned()
    } else {
        format!("{stem}.{ext}")
    }
}

/// Validates a revision into an operation plan. Errors mean the plan must not be applied.
pub fn validate(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
) -> (Option<OperationPlan>, Vec<PlanIssue>) {
    let mut issues = Vec::new();
    let accepted: Vec<&Placement> = revision.accepted().collect();
    if accepted.is_empty() {
        issues.push(PlanIssue::error(
            "nothing_accepted",
            "Approve at least one proposed change before Validate. Nothing is moved until you Apply.",
            vec![],
        ));
        return (None, issues);
    }

    let index = snapshot.entry_index();
    let accepted_by_asset: HashMap<AssetId, &Placement> = accepted
        .iter()
        .map(|placement| (placement.asset, *placement))
        .collect();
    check_move_together_bundles(snapshot, &accepted_by_asset, &mut issues);

    let mut moves: Vec<(AssetId, RelativePath, RelativePath)> = Vec::new();
    for placement in accepted {
        let Some(entry) = index.get(&placement.asset) else {
            issues.push(PlanIssue::error(
                "unknown_asset",
                "accepted placement refers to an asset missing from the snapshot",
                vec![placement.asset],
            ));
            continue;
        };
        if entry.path == placement.destination {
            continue;
        }
        check_protected_member(snapshot, entry, placement, &mut issues);
        moves.push((entry.id, entry.path.clone(), placement.destination.clone()));
    }

    let mut seen_dest: BTreeMap<String, AssetId> = BTreeMap::new();
    for (asset, _, dest) in &moves {
        let key = dest.case_fold();
        if let Some(other) = seen_dest.insert(key, *asset) {
            issues.push(PlanIssue::error(
                "destination_collision",
                format!("{dest} is claimed by more than one asset"),
                vec![*asset, other],
            ));
        }
    }

    let moving_from: BTreeSet<String> = moves.iter().map(|(_, from, _)| from.case_fold()).collect();
    for (asset, _, dest) in &moves {
        if let Some(occupant) = snapshot
            .entries
            .iter()
            .find(|entry| entry.path.case_fold() == dest.case_fold())
        {
            if occupant.id != *asset && !moving_from.contains(&occupant.path.case_fold()) {
                issues.push(PlanIssue::error(
                    "destination_occupied",
                    format!("{dest} already exists and is not moving away"),
                    vec![*asset, occupant.id],
                ));
            }
        }
    }

    if issues
        .iter()
        .any(|issue| issue.severity == PlanIssueSeverity::Error)
    {
        return (None, issues);
    }

    let ordered = match order_moves(&moves) {
        Ok(ordered) => ordered,
        Err(assets) => {
            issues.push(PlanIssue::error(
                "move_cycle",
                "moves form a cycle and need an explicit temp path",
                assets,
            ));
            return (None, issues);
        }
    };

    let mut operations = Vec::new();
    let mut seq = 0u32;
    let mut created = BTreeSet::new();
    for (_, _, dest) in &ordered {
        if let Some(parent) = dest.parent() {
            if snapshot_has_directory(snapshot, &parent) {
                continue;
            }
            if created.insert(parent.case_fold()) {
                operations.push(PlannedOperation {
                    seq,
                    operation: Operation::CreateDirectory { path: parent },
                });
                seq += 1;
            }
        }
    }
    for (asset, from, to) in &ordered {
        let Some(entry) = index.get(asset) else {
            continue;
        };
        operations.push(PlannedOperation {
            seq,
            operation: Operation::Move {
                asset: *asset,
                from: from.clone(),
                to: to.clone(),
                expected: entry.identity.clone(),
            },
        });
        seq += 1;
    }
    for dir in empty_directories_to_remove(snapshot, &ordered) {
        operations.push(PlannedOperation {
            seq,
            operation: Operation::RemoveEmptyDirectory { path: dir },
        });
        seq += 1;
    }

    let warnings: Vec<PlanIssue> = issues
        .iter()
        .filter(|issue| issue.severity == PlanIssueSeverity::Warning)
        .cloned()
        .collect();
    let plan = OperationPlan {
        id: PlanId::new(),
        revision: revision.id,
        root: snapshot.root.clone(),
        created_at: Timestamp::now(),
        operations,
        warnings: warnings.clone(),
    };
    (Some(plan), issues)
}

fn snapshot_has_directory(snapshot: &WorkspaceSnapshot, path: &RelativePath) -> bool {
    if path.is_session_root() {
        return true;
    }
    snapshot
        .entries
        .iter()
        .any(|entry| entry.path == *path && matches!(entry.kind, EntryKind::Directory))
}

fn destination_folder(path: &RelativePath) -> String {
    path.parent()
        .map(|parent| parent.case_fold())
        .unwrap_or_default()
}

fn check_move_together_bundles(
    snapshot: &WorkspaceSnapshot,
    accepted: &HashMap<AssetId, &Placement>,
    issues: &mut Vec<PlanIssue>,
) {
    for bundle in &snapshot.bundles {
        if bundle.constraint != BundleConstraint::MoveTogether {
            continue;
        }
        let mut folders: BTreeMap<String, Vec<AssetId>> = BTreeMap::new();
        for member in &bundle.members {
            let Some(entry) = snapshot.entry(*member) else {
                continue;
            };
            if entry.kind != EntryKind::File {
                continue;
            }
            let dest = accepted
                .get(member)
                .map(|placement| placement.destination.clone())
                .unwrap_or_else(|| entry.path.clone());
            folders
                .entry(destination_folder(&dest))
                .or_default()
                .push(*member);
        }
        if folders.len() > 1 {
            let assets: Vec<AssetId> = folders.into_values().flatten().collect();
            issues.push(PlanIssue::error(
                "bundle_split",
                format!(
                    "hard bundle '{}' must keep members in the same destination folder",
                    bundle.label
                ),
                assets,
            ));
        }
    }
}

fn check_protected_member(
    snapshot: &WorkspaceSnapshot,
    entry: &ObservedEntry,
    placement: &Placement,
    issues: &mut Vec<PlanIssue>,
) {
    let Some(bundle) = snapshot.hard_bundle_for(entry.id) else {
        return;
    };
    if let BundleConstraint::Protected { reason } = &bundle.constraint {
        if placement.destination != entry.path {
            issues.push(PlanIssue::error(
                "protected_member",
                reason.clone(),
                vec![entry.id],
            ));
        }
    }
}

fn order_moves(
    moves: &[(AssetId, RelativePath, RelativePath)],
) -> Result<Vec<(AssetId, RelativePath, RelativePath)>, Vec<AssetId>> {
    let mut remaining: Vec<(AssetId, RelativePath, RelativePath)> = moves.to_vec();
    let mut ordered = Vec::new();
    while !remaining.is_empty() {
        let from_set: BTreeSet<String> = remaining
            .iter()
            .map(|(_, from, _)| from.case_fold())
            .collect();
        let idx = remaining
            .iter()
            .position(|(_, _, to)| !from_set.contains(&to.case_fold()));
        match idx {
            Some(idx) => ordered.push(remaining.remove(idx)),
            None => {
                return Err(remaining.into_iter().map(|(asset, _, _)| asset).collect());
            }
        }
    }
    Ok(ordered)
}

fn empty_directories_to_remove(
    snapshot: &WorkspaceSnapshot,
    moves: &[(AssetId, RelativePath, RelativePath)],
) -> Vec<RelativePath> {
    let moving: BTreeSet<String> = moves.iter().map(|(_, from, _)| from.case_fold()).collect();
    let mut occupied: BTreeSet<String> = BTreeSet::new();
    for (_, _, to) in moves {
        occupied.insert(to.case_fold());
        let mut parent = to.parent();
        while let Some(dir) = parent {
            occupied.insert(dir.case_fold());
            parent = dir.parent();
        }
    }
    let mut emptied_roots: BTreeSet<RelativePath> = BTreeSet::new();
    for (_, from, _) in moves {
        let mut parent = from.parent();
        while let Some(dir) = parent {
            if dir.is_session_root() {
                break;
            }
            emptied_roots.insert(dir.clone());
            parent = dir.parent();
        }
    }
    let mut candidates = emptied_roots.clone();
    for entry in &snapshot.entries {
        if entry.kind != EntryKind::Directory || entry.path.is_session_root() {
            continue;
        }
        if emptied_roots
            .iter()
            .any(|root| entry.path.starts_with(root))
        {
            candidates.insert(entry.path.clone());
        }
    }
    let mut removable: Vec<RelativePath> = candidates
        .into_iter()
        .filter(|dir| {
            if occupied.contains(&dir.case_fold()) {
                return false;
            }
            !snapshot.entries.iter().any(|entry| {
                entry.kind == EntryKind::File
                    && entry.path.starts_with(dir)
                    && !moving.contains(&entry.path.case_fold())
            })
        })
        .collect();
    removable.sort_by_key(|path| std::cmp::Reverse(path.as_str().len()));
    removable
}

/// Marks every placement accepted. Used by the CLI organise shortcut.
pub fn accept_all(
    revision: &ProposalRevision,
) -> Result<ProposalRevision, aifs_domain::PatchError> {
    let assets: Vec<AssetId> = revision.placements.keys().copied().collect();
    revision.with_patches(
        RevisionAuthor::Engine,
        "accept all heuristic placements",
        &[aifs_domain::RevisionPatch::Accept { assets }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        AssetId, Confidence, EntryKind, Evidence, EvidenceSource, FileFamily, FileIdentity,
        LockState, ObservedEntry, SessionId,
    };
    use std::path::PathBuf;

    fn file(path: &str, family: FileFamily) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(path).unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family,
            identity: FileIdentity {
                size: 4,
                ..FileIdentity::default()
            },
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    #[test]
    fn text_file_goes_to_documents() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        snapshot
            .entries
            .push(file("note.txt", FileFamily::Document));
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision
            .placements
            .values()
            .next()
            .unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "Documents/note.txt");
        assert_eq!(placement.review, ReviewState::Proposed);
    }

    #[test]
    fn audio_uses_tags_for_name_and_artist_folder() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("show.mp3", FileFamily::Audio);
        let id = entry.id;
        snapshot.evidence.push(
            Evidence::new(id, EvidenceSource::MediaTags, Confidence::CERTAIN)
                .with_fact(keys::MEDIA_TITLE, "Night Drive")
                .with_fact(keys::MEDIA_ARTIST, "Ada"),
        );
        snapshot.entries.push(entry);
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(
            placement.destination.as_str(),
            "Music/Ada/Ada - Night Drive.mp3"
        );
    }

    #[test]
    fn plan_requires_accept() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        snapshot
            .entries
            .push(file("note.txt", FileFamily::Document));
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let (plan, issues) = validate(&snapshot, &revision);
        assert!(plan.is_none());
        assert!(issues.iter().any(|issue| issue.code == "nothing_accepted"));
        let accepted = accept_all(&revision).unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &accepted);
        assert!(issues
            .iter()
            .all(|issue| issue.severity != PlanIssueSeverity::Error));
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        assert!(plan.move_count() == 1);
    }

    #[test]
    fn move_together_split_is_rejected() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let raw = file("IMG_1.CR2", FileFamily::RawImage);
        let jpeg = file("IMG_1.jpg", FileFamily::Image);
        let raw_id = raw.id;
        let jpeg_id = jpeg.id;
        snapshot.entries.push(raw);
        snapshot.entries.push(jpeg);
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::SidecarGroup,
            label: "IMG_1".into(),
            members: vec![raw_id, jpeg_id],
            anchor: Some(raw_id),
            constraint: BundleConstraint::MoveTogether,
            reason: "sidecar".into(),
        });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let only_raw = revision
            .with_patches(
                RevisionAuthor::User,
                "accept raw only",
                &[aifs_domain::RevisionPatch::Accept {
                    assets: vec![raw_id],
                }],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &only_raw);
        assert!(plan.is_none());
        assert!(issues.iter().any(|issue| issue.code == "bundle_split"));

        let both = accept_all(&revision).unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &both);
        assert!(issues
            .iter()
            .all(|issue| issue.severity != PlanIssueSeverity::Error));
        assert!(plan.is_some());
    }

    fn directory(path: &str) -> ObservedEntry {
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
    fn emptied_source_directories_are_removed() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        snapshot.entries.push(directory("dump"));
        snapshot.entries.push(directory("dump/nested"));
        snapshot
            .entries
            .push(file("dump/nested/a.txt", FileFamily::Document));
        snapshot
            .entries
            .push(file("keep.txt", FileFamily::Document));
        let revision = accept_all(&propose(&snapshot, &ProposalPolicy::default()))
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &revision);
        assert!(issues
            .iter()
            .all(|issue| issue.severity != PlanIssueSeverity::Error));
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        let removed: Vec<String> = plan
            .operations
            .iter()
            .filter_map(|planned| match &planned.operation {
                Operation::RemoveEmptyDirectory { path } => Some(path.as_str().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            removed.contains(&"dump/nested".to_owned()),
            "nested emptied dir should be removed first, got {removed:?}"
        );
        assert!(
            removed.contains(&"dump".to_owned()),
            "parent emptied dir should be removed, got {removed:?}"
        );
        assert!(
            !removed.iter().any(|path| path == "Documents"),
            "destination folders must not be cleaned, got {removed:?}"
        );
        let nested_idx = removed
            .iter()
            .position(|path| path == "dump/nested")
            .unwrap_or_else(|| panic!("nested"));
        let dump_idx = removed
            .iter()
            .position(|path| path == "dump")
            .unwrap_or_else(|| panic!("dump"));
        assert!(nested_idx < dump_idx, "remove deepest directories first");
    }

    #[test]
    fn occupied_source_folder_is_not_removed() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        snapshot.entries.push(directory("dump"));
        snapshot
            .entries
            .push(file("dump/a.txt", FileFamily::Document));
        snapshot
            .entries
            .push(file("dump/keep.bin", FileFamily::Generic));
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let only_text = revision
            .with_patches(
                RevisionAuthor::User,
                "accept dump text only",
                &[aifs_domain::RevisionPatch::Accept {
                    assets: vec![snapshot.entries[1].id],
                }],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &only_text);
        assert!(issues
            .iter()
            .all(|issue| issue.severity != PlanIssueSeverity::Error));
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        assert!(
            !plan.operations.iter().any(|planned| matches!(
                planned.operation,
                Operation::RemoveEmptyDirectory { ref path } if path.as_str() == "dump"
            )),
            "dump still has keep.bin"
        );
    }
}
