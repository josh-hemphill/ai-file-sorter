//! Heuristic organisation proposals and validation of revisions into operation plans.

pub mod compare;

pub use compare::{ExpectedPlan, destination_map, diff_plan};

use aifs_domain::{
    AssetId, BundleConstraint, EntryKind, FileFamily, ObservedEntry, Operation, OperationPlan,
    Placement, PlanId, PlanIssue, PlanIssueSeverity, PlannedOperation, ProposalRevision,
    RelativePath, RelativePathError, ReviewState, RevisionAuthor, SuggestionOrigin, Timestamp,
    WorkspaceSnapshot, category_date_suffix, escape_path_segment, evidence::keys,
    is_generic_camera_stem,
};
use aifs_protocol::{CategoryWhitelist, FolderStyle, ProposalPolicy};
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

    apply_bundle_constraints(snapshot, policy, &mut destinations);

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
    if let Some(role) = covering_role(snapshot, &entry.path)
        && matches!(
            role.kind,
            aifs_domain::DirectoryRoleKind::Library | aifs_domain::DirectoryRoleKind::WeakArchive
        )
    {
        if matches!(entry.family, FileFamily::Image | FileFamily::RawImage)
            && is_generic_camera_stem(entry.stem())
        {
            let file_name = file_name_for(snapshot, entry, policy);
            let destination = with_file_name(&entry.path, &file_name);
            let origin = if destination == entry.path {
                SuggestionOrigin::Unchanged
            } else {
                SuggestionOrigin::Heuristic
            };
            let rationale = if origin == SuggestionOrigin::Heuristic {
                Some("rename generic camera filename".to_owned())
            } else {
                Some(role.reason.clone())
            };
            return (destination, origin, rationale);
        }
        return (
            entry.path.clone(),
            SuggestionOrigin::Unchanged,
            Some(role.reason.clone()),
        );
    }

    let Some(folder) = folder_for(snapshot, entry, policy) else {
        return (
            entry.path.clone(),
            SuggestionOrigin::Unchanged,
            Some("no allowed category for this file".to_owned()),
        );
    };
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
        .max_by_key(|role| {
            let preserve = matches!(
                role.kind,
                aifs_domain::DirectoryRoleKind::Library
                    | aifs_domain::DirectoryRoleKind::WeakArchive
            );
            (preserve, role.root.as_str().len())
        })
}

fn folder_for(
    snapshot: &WorkspaceSnapshot,
    entry: &ObservedEntry,
    policy: &ProposalPolicy,
) -> Option<RelativePath> {
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
    if let Some(hinted) = model_folder_hint(snapshot, entry, policy) {
        folder = hinted;
    }
    if policy.use_subfolders
        && let Some(artist) = evidence_text(snapshot, entry.id, keys::MEDIA_ARTIST)
        && matches!(entry.family, FileFamily::Audio | FileFamily::Video)
    {
        folder = format!("{folder}/{}", sanitize_segment(&artist, "Unknown"));
    }
    if policy.use_subfolders
        && let Some(date) = category_date_suffix(
            entry,
            evidence_text(snapshot, entry.id, keys::IMAGE_CAPTURED_ON).as_deref(),
        )
    {
        folder = format!("{folder}/{}", sanitize_segment(&date, "Unknown"));
    }
    folder = apply_category_whitelist(folder, entry.family, &policy.whitelist);
    if folder.is_empty() {
        return None;
    }
    match RelativePath::parse(&folder) {
        Ok(path) => Some(path),
        Err(_) => folder
            .split('/')
            .next()
            .filter(|top| !top.is_empty())
            .and_then(|top| RelativePath::parse(top).ok()),
    }
}

fn model_folder_hint(
    snapshot: &WorkspaceSnapshot,
    entry: &ObservedEntry,
    policy: &ProposalPolicy,
) -> Option<String> {
    let category = evidence_text(snapshot, entry.id, keys::CATEGORY)?;
    let top = sanitize_segment(&category, "");
    if top.is_empty() {
        return None;
    }
    let mut folder = top;
    if let Some(sub) = evidence_text(snapshot, entry.id, keys::CATEGORY_SUB) {
        let sub = sanitize_segment(&sub, "");
        if !sub.is_empty() {
            folder = format!("{folder}/{sub}");
        }
    }
    let allowed = apply_category_whitelist(folder, entry.family, &policy.whitelist);
    if allowed.is_empty() {
        None
    } else {
        Some(allowed)
    }
}

fn apply_category_whitelist(
    folder: String,
    family: FileFamily,
    whitelist: &CategoryWhitelist,
) -> String {
    let mut parts: Vec<String> = folder
        .split('/')
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if parts.is_empty() {
        return folder;
    }
    if !whitelist.allows_top(&parts[0]) {
        let fallback = family.default_folder();
        if whitelist.allows_top(fallback) {
            parts[0] = fallback.to_owned();
        } else {
            return String::new();
        }
    }
    if parts.len() > 1 {
        let top = parts[0].clone();
        let sub = parts[1].clone();
        let allowed_subs = if !whitelist.global_subcategories.is_empty() {
            Some(whitelist.global_subcategories.as_slice())
        } else if !whitelist.branching.is_empty() {
            Some(
                whitelist
                    .branching
                    .get(&top)
                    .or_else(|| {
                        whitelist
                            .branching
                            .iter()
                            .find(|(key, _)| key.eq_ignore_ascii_case(&top))
                            .map(|(_, value)| value)
                    })
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )
        } else {
            None
        };
        if let Some(allowed) = allowed_subs
            && (allowed.is_empty() || !allowed.iter().any(|name| name.eq_ignore_ascii_case(&sub)))
        {
            parts.truncate(1);
        }
    }
    parts.join("/")
}

fn file_name_for(
    snapshot: &WorkspaceSnapshot,
    entry: &ObservedEntry,
    policy: &ProposalPolicy,
) -> String {
    let original = entry.path.file_name().to_owned();
    let ext = original_extension(entry);
    if policy.rename_media
        && matches!(entry.family, FileFamily::Audio | FileFamily::Video)
        && let Some(title) = evidence_text(snapshot, entry.id, keys::MEDIA_TITLE)
    {
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
    if matches!(entry.family, FileFamily::Image | FileFamily::RawImage)
        && is_generic_camera_stem(entry.stem())
    {
        if let Some(name) = evidence_text(snapshot, entry.id, keys::SUGGESTED_NAME)
            && let Some(stem) = usable_image_stem(&name)
        {
            return with_extension(&stem, &ext, &original);
        }
        if let Some(description) = evidence_text(snapshot, entry.id, keys::DESCRIPTION) {
            let stem = slug_from_caption(&description);
            if !stem.is_empty() && !is_generic_camera_stem(&stem) {
                return with_extension(&stem, &ext, &original);
            }
        }
    }
    original
}

const CAPTION_SLUG_WORDS: usize = 6;
const CAPTION_SLUG_STOP: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "at", "to", "for", "and", "with",
];

fn slug_from_caption(text: &str) -> String {
    let words: Vec<String> = text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| word.len() >= 2)
        .map(|word| word.to_ascii_lowercase())
        .filter(|word| !CAPTION_SLUG_STOP.contains(&word.as_str()))
        .take(CAPTION_SLUG_WORDS)
        .collect();
    if words.is_empty() {
        String::new()
    } else {
        sanitize_segment(&words.join("-"), "")
    }
}

fn usable_image_stem(suggested: &str) -> Option<String> {
    let name = suggested.rsplit(['/', '\\']).next().unwrap_or(suggested);
    let stem = strip_filename_extension(name);
    let stem = sanitize_segment(stem, "");
    if stem.is_empty() || is_generic_camera_stem(&stem) {
        None
    } else {
        Some(stem)
    }
}

fn strip_filename_extension(name: &str) -> &str {
    let Some(index) = name.rfind('.') else {
        return name;
    };
    if index == 0 {
        return name;
    }
    let ext = &name[index + 1..];
    let looks_like_ext =
        (2..=4).contains(&ext.len()) && ext.chars().all(|ch| ch.is_ascii_alphanumeric());
    if looks_like_ext { &name[..index] } else { name }
}

fn stem_from_file_name(name: &str) -> String {
    match name.rfind('.') {
        Some(index) if index > 0 => name[..index].to_owned(),
        _ => name.to_owned(),
    }
}

fn with_file_name(path: &RelativePath, file_name: &str) -> RelativePath {
    path.with_file_name(file_name)
        .unwrap_or_else(|_| path.clone())
}

fn shared_rename_stem(
    snapshot: &WorkspaceSnapshot,
    members: &[AssetId],
    policy: &ProposalPolicy,
) -> Option<String> {
    for id in members {
        let Some(entry) = snapshot.entry(*id) else {
            continue;
        };
        let name = file_name_for(snapshot, entry, policy);
        if name != entry.path.file_name() {
            return Some(stem_from_file_name(&name));
        }
    }
    None
}

fn apply_bundle_constraints(
    snapshot: &WorkspaceSnapshot,
    policy: &ProposalPolicy,
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
                let Some(anchor_id) = bundle.anchor else {
                    continue;
                };
                let Some(anchor) = destinations.get(&anchor_id).cloned() else {
                    continue;
                };
                let folder = anchor.0.parent().unwrap_or_else(|| anchor.0.clone());
                let shared_stem = shared_rename_stem(snapshot, &bundle.members, policy);
                let member_ids = bundle.members.clone();
                for member in member_ids {
                    let Some(entry) = snapshot.entry(member) else {
                        continue;
                    };
                    let file_name = match &shared_stem {
                        Some(stem) => {
                            with_extension(stem, &original_extension(entry), entry.path.file_name())
                        }
                        None => entry.path.file_name().to_owned(),
                    };
                    let dest = folder
                        .join(&file_name)
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
                let new_root = preserve_layout_new_root(snapshot, anchor_id, &anchor_dest);
                let member_ids = bundle.members.clone();
                for member in member_ids {
                    let Some(entry) = snapshot.entry(member) else {
                        continue;
                    };
                    let mut dest = relocate_under(root, &entry.path, &new_root)
                        .unwrap_or_else(|_| entry.path.clone());
                    if entry.kind == EntryKind::File {
                        dest = with_file_name(&dest, &file_name_for(snapshot, entry, policy));
                    }
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
    let escaped = escape_path_segment(value);
    if escaped.is_empty() {
        fallback.to_owned()
    } else {
        escaped.chars().take(80).collect()
    }
}

fn preserve_layout_new_root(
    snapshot: &WorkspaceSnapshot,
    anchor_id: AssetId,
    anchor_dest: &RelativePath,
) -> RelativePath {
    if snapshot
        .entry(anchor_id)
        .is_some_and(|entry| entry.kind == EntryKind::File)
    {
        anchor_dest.parent().unwrap_or_else(|| anchor_dest.clone())
    } else {
        anchor_dest.clone()
    }
}

fn original_extension(entry: &ObservedEntry) -> String {
    let Some(ext) = entry.extension() else {
        return String::new();
    };
    let name = entry.path.file_name();
    name[name.len().saturating_sub(ext.len())..].to_owned()
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
    check_preserve_layout_bundles(snapshot, &accepted_by_asset, &mut issues);

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
            && occupant.id != *asset
            && !moving_from.contains(&occupant.path.case_fold())
        {
            issues.push(PlanIssue::error(
                "destination_occupied",
                format!("{dest} already exists and is not moving away"),
                vec![*asset, occupant.id],
            ));
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
    if let BundleConstraint::Protected { reason } = &bundle.constraint
        && placement.destination != entry.path
    {
        issues.push(PlanIssue::error(
            "protected_member",
            reason.clone(),
            vec![entry.id],
        ));
    }
}

fn check_preserve_layout_bundles(
    snapshot: &WorkspaceSnapshot,
    accepted: &HashMap<AssetId, &Placement>,
    issues: &mut Vec<PlanIssue>,
) {
    for bundle in &snapshot.bundles {
        let BundleConstraint::PreserveLayout { root } = &bundle.constraint else {
            continue;
        };
        let files: Vec<&ObservedEntry> = bundle
            .members
            .iter()
            .filter_map(|id| snapshot.entry(*id))
            .filter(|entry| entry.kind == EntryKind::File)
            .collect();
        if files.len() < 2 {
            continue;
        }
        let destinations: Vec<(&ObservedEntry, RelativePath)> = files
            .iter()
            .map(|entry| {
                let dest = accepted
                    .get(&entry.id)
                    .map(|placement| placement.destination.clone())
                    .unwrap_or_else(|| entry.path.clone());
                (*entry, dest)
            })
            .collect();
        if destinations.iter().all(|(entry, dest)| dest == &entry.path) {
            continue;
        }
        let mut new_roots: BTreeSet<String> = BTreeSet::new();
        let mut split = false;
        for (entry, dest) in &destinations {
            match implied_layout_root(root, &entry.path, dest) {
                Some(new_root) => {
                    new_roots.insert(new_root.case_fold());
                }
                None => split = true,
            }
        }
        if split || new_roots.len() != 1 {
            issues.push(PlanIssue::error(
                "bundle_split",
                format!(
                    "hard bundle '{}' must keep members under the same relocated folder",
                    bundle.label
                ),
                files.iter().map(|entry| entry.id).collect(),
            ));
        }
    }
}

fn implied_layout_root(
    old_root: &RelativePath,
    path: &RelativePath,
    dest: &RelativePath,
) -> Option<RelativePath> {
    if path == old_root {
        return Some(dest.clone());
    }
    let rest = path
        .as_str()
        .strip_prefix(&format!("{}/", old_root.as_str()))?;
    if rest.is_empty() {
        return Some(dest.clone());
    }
    if let Some((rest_dir, _)) = rest.rsplit_once('/') {
        let dest_dir = dest.parent()?;
        let suffix = format!("/{rest_dir}");
        let new_root = dest_dir.as_str().strip_suffix(&suffix)?;
        if new_root.is_empty() {
            return None;
        }
        return RelativePath::parse(new_root).ok();
    }
    dest.parent()
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
    let remaining_occupant =
        |dir: &RelativePath| {
            snapshot.entries.iter().any(|entry| {
                entry.kind != EntryKind::Directory
                    && entry.path.starts_with(dir)
                    && !moving.contains(&entry.path.case_fold())
            }) || snapshot.skipped.iter().any(|entry| {
                entry.path.starts_with(dir) && !moving.contains(&entry.path.case_fold())
            }) || snapshot.entries.iter().any(|entry| {
                entry.kind == EntryKind::Directory
                    && entry.path.starts_with(dir)
                    && entry.path != *dir
                    && !emptied_roots.contains(&entry.path)
            })
        };
    let mut removable: Vec<RelativePath> = emptied_roots
        .iter()
        .filter(|dir| !occupied.contains(&dir.case_fold()) && !remaining_occupant(dir))
        .cloned()
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
        LockState, ObservedEntry, SessionId, SkipReason, SkippedEntry,
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
    fn model_category_hint_is_whitelisted() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("clip.mp3", FileFamily::Audio);
        let id = entry.id;
        snapshot.evidence.push(
            Evidence::new(
                id,
                EvidenceSource::LocalModel {
                    model: "stub".into(),
                },
                Confidence::new(0.4),
            )
            .with_fact(keys::CATEGORY, "Documents"),
        );
        snapshot.entries.push(entry);
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "Documents/clip.mp3");
    }

    #[test]
    fn model_category_outside_whitelist_falls_back() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("clip.mp3", FileFamily::Audio);
        let id = entry.id;
        snapshot.evidence.push(
            Evidence::new(
                id,
                EvidenceSource::LocalModel {
                    model: "stub".into(),
                },
                Confidence::new(0.4),
            )
            .with_fact(keys::CATEGORY, "../Etc"),
        );
        snapshot.entries.push(entry);
        let mut policy = ProposalPolicy::default();
        policy.whitelist.main = vec!["Music".into(), "Documents".into()];
        let revision = propose(&snapshot, &policy);
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "Music/clip.mp3");
    }

    #[test]
    fn nested_inbox_name_does_not_flatten_a_library() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("Music/tmp/clip.mp3", FileFamily::Audio);
        let id = entry.id;
        snapshot.entries.push(entry);
        snapshot
            .directory_roles
            .push(aifs_domain::DirectoryRoleMatch {
                root: RelativePath::parse("Music").unwrap_or_else(|e| panic!("{e}")),
                kind: aifs_domain::DirectoryRoleKind::Library,
                reason: "library".into(),
            });
        snapshot
            .directory_roles
            .push(aifs_domain::DirectoryRoleMatch {
                root: RelativePath::parse("Music/tmp").unwrap_or_else(|e| panic!("{e}")),
                kind: aifs_domain::DirectoryRoleKind::BroadInbox,
                reason: "dump".into(),
            });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "Music/tmp/clip.mp3");
    }

    #[test]
    fn generic_camera_stems_rename_in_place_inside_a_library() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let generic = file("Wedding/IMG_001.jpg", FileFamily::Image);
        let named = file("Wedding/ceremony-kiss.jpg", FileFamily::Image);
        let generic_id = generic.id;
        let named_id = named.id;
        snapshot.evidence.push(
            Evidence::new(
                generic_id,
                EvidenceSource::LocalModel {
                    model: "vision".into(),
                },
                Confidence::new(0.6),
            )
            .with_fact(keys::SUGGESTED_NAME, "bride-smiling.jpg")
            .with_fact(keys::DESCRIPTION, "a bride smiling under an arch"),
        );
        snapshot.evidence.push(
            Evidence::new(
                named_id,
                EvidenceSource::LocalModel {
                    model: "vision".into(),
                },
                Confidence::new(0.6),
            )
            .with_fact(keys::DESCRIPTION, "the ceremony kiss"),
        );
        snapshot.entries.push(directory("Wedding"));
        snapshot.entries.push(generic);
        snapshot.entries.push(named);
        let wedding = RelativePath::parse("Wedding").unwrap_or_else(|e| panic!("{e}"));
        snapshot
            .directory_roles
            .push(aifs_domain::DirectoryRoleMatch {
                root: wedding.clone(),
                kind: aifs_domain::DirectoryRoleKind::Library,
                reason: "event album".into(),
            });
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::Folder,
            label: "Wedding".into(),
            members: vec![generic_id, named_id],
            anchor: None,
            constraint: BundleConstraint::PreserveLayout { root: wedding },
            reason: "event album".into(),
        });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let renamed = revision
            .placement(generic_id)
            .unwrap_or_else(|| panic!("generic"));
        assert_eq!(renamed.destination.as_str(), "Wedding/bride-smiling.jpg");
        assert_eq!(renamed.origin, SuggestionOrigin::Heuristic);
        let kept = revision
            .placement(named_id)
            .unwrap_or_else(|| panic!("named"));
        assert_eq!(kept.destination.as_str(), "Wedding/ceremony-kiss.jpg");
        assert_eq!(kept.origin, SuggestionOrigin::Unchanged);
        let accepted = accept_all(&revision).unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &accepted);
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error),
            "in-place rename must not split the album, got {issues:?}"
        );
        assert!(plan.is_some());
    }

    #[test]
    fn generic_camera_stem_falls_back_to_caption_slug() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("Photos/DSC_0001.jpg", FileFamily::Image);
        let id = entry.id;
        snapshot.evidence.push(
            Evidence::new(
                id,
                EvidenceSource::LocalModel {
                    model: "vision".into(),
                },
                Confidence::new(0.6),
            )
            .with_fact(keys::DESCRIPTION, "a bride smiling under an arch"),
        );
        snapshot.entries.push(entry);
        snapshot
            .directory_roles
            .push(aifs_domain::DirectoryRoleMatch {
                root: RelativePath::parse("Photos").unwrap_or_else(|e| panic!("{e}")),
                kind: aifs_domain::DirectoryRoleKind::Library,
                reason: "library".into(),
            });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(
            placement.destination.as_str(),
            "Photos/bride-smiling-under-arch.jpg"
        );
    }

    #[test]
    fn sidecar_family_shares_the_renamed_stem() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let raw = file("IMG_1.CR2", FileFamily::RawImage);
        let jpeg = file("IMG_1.jpg", FileFamily::Image);
        let raw_id = raw.id;
        let jpeg_id = jpeg.id;
        snapshot.evidence.push(
            Evidence::new(
                jpeg_id,
                EvidenceSource::LocalModel {
                    model: "vision".into(),
                },
                Confidence::new(0.6),
            )
            .with_fact(keys::SUGGESTED_NAME, "bride-smiling.jpg"),
        );
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
        assert_eq!(
            revision
                .placement(jpeg_id)
                .map(|placement| placement.destination.as_str()),
            Some("Pictures/bride-smiling.jpg")
        );
        assert_eq!(
            revision
                .placement(raw_id)
                .map(|placement| placement.destination.as_str()),
            Some("Pictures/bride-smiling.CR2")
        );
    }

    #[test]
    fn split_archive_parts_keep_original_names() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let first = file("foo.part1.rar", FileFamily::Archive);
        let second = file("foo.part2.rar", FileFamily::Archive);
        let first_id = first.id;
        let second_id = second.id;
        snapshot.entries.push(first);
        snapshot.entries.push(second);
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::ArchiveParts,
            label: "foo".into(),
            members: vec![first_id, second_id],
            anchor: Some(first_id),
            constraint: BundleConstraint::MoveTogether,
            reason: "split archive".into(),
        });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        assert_eq!(
            revision
                .placement(first_id)
                .map(|placement| placement.destination.as_str()),
            Some("Archives/foo.part1.rar")
        );
        assert_eq!(
            revision
                .placement(second_id)
                .map(|placement| placement.destination.as_str()),
            Some("Archives/foo.part2.rar")
        );
    }

    #[test]
    fn tagged_audio_inside_a_library_stays_put() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("Music/Ada/night.mp3", FileFamily::Audio);
        let id = entry.id;
        snapshot.evidence.push(
            Evidence::new(id, EvidenceSource::MediaTags, Confidence::CERTAIN)
                .with_fact(keys::MEDIA_TITLE, "Night Drive")
                .with_fact(keys::MEDIA_ARTIST, "Ada"),
        );
        snapshot.entries.push(entry);
        snapshot
            .directory_roles
            .push(aifs_domain::DirectoryRoleMatch {
                root: RelativePath::parse("Music").unwrap_or_else(|e| panic!("{e}")),
                kind: aifs_domain::DirectoryRoleKind::Library,
                reason: "library".into(),
            });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "Music/Ada/night.mp3");
        assert_eq!(placement.origin, SuggestionOrigin::Unchanged);
    }

    #[test]
    fn preserve_layout_file_anchor_renames_generic_stems() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let generic = file("Trip/IMG_001.jpg", FileFamily::Image);
        let extra = file("Trip/IMG_002.jpg", FileFamily::Image);
        let generic_id = generic.id;
        let extra_id = extra.id;
        snapshot.evidence.push(
            Evidence::new(
                generic_id,
                EvidenceSource::LocalModel {
                    model: "vision".into(),
                },
                Confidence::new(0.6),
            )
            .with_fact(keys::SUGGESTED_NAME, "v1.2-portrait.jpg"),
        );
        snapshot.entries.push(generic);
        snapshot.entries.push(extra);
        let trip = RelativePath::parse("Trip").unwrap_or_else(|e| panic!("{e}"));
        snapshot
            .directory_roles
            .push(aifs_domain::DirectoryRoleMatch {
                root: trip.clone(),
                kind: aifs_domain::DirectoryRoleKind::Library,
                reason: "event album".into(),
            });
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::Folder,
            label: "Trip".into(),
            members: vec![generic_id, extra_id],
            anchor: Some(generic_id),
            constraint: BundleConstraint::PreserveLayout { root: trip },
            reason: "event album".into(),
        });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        assert_eq!(
            revision
                .placement(generic_id)
                .map(|placement| placement.destination.as_str()),
            Some("Trip/v1.2-portrait.jpg")
        );
        assert_eq!(
            revision
                .placement(extra_id)
                .map(|placement| placement.destination.as_str()),
            Some("Trip/IMG_002.jpg")
        );
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
    fn audio_title_punctuation_is_escaped_not_dropped() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("show.mp3", FileFamily::Audio);
        let id = entry.id;
        snapshot.evidence.push(
            Evidence::new(id, EvidenceSource::MediaTags, Confidence::CERTAIN)
                .with_fact(keys::MEDIA_TITLE, "What Is Love?")
                .with_fact(keys::MEDIA_ARTIST, "Haddaway"),
        );
        snapshot.entries.push(entry);
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(
            placement.destination.as_str(),
            "Music/Haddaway/Haddaway - What Is Love\u{FF1F}.mp3"
        );
    }

    #[test]
    fn image_exif_date_becomes_a_subfolder_when_enabled() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("shot.jpg", FileFamily::Image);
        let id = entry.id;
        snapshot.evidence.push(
            Evidence::new(id, EvidenceSource::Exif, Confidence::CERTAIN)
                .with_fact(keys::IMAGE_CAPTURED_ON, "2021-07-15"),
        );
        snapshot.entries.push(entry);
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(
            placement.destination.as_str(),
            "Pictures/2021-07-15/shot.jpg"
        );

        let no_subs = ProposalPolicy {
            use_subfolders: false,
            ..ProposalPolicy::default()
        };
        let revision = propose(&snapshot, &no_subs);
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "Pictures/shot.jpg");
    }

    #[test]
    fn document_modified_month_becomes_a_subfolder_when_enabled() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let mut entry = file("note.txt", FileFamily::Document);
        entry.identity.modified = Some(Timestamp(1_626_307_200_000));
        let id = entry.id;
        snapshot.entries.push(entry);
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "Documents/2021-07/note.txt");

        let no_subs = ProposalPolicy {
            use_subfolders: false,
            ..ProposalPolicy::default()
        };
        let revision = propose(&snapshot, &no_subs);
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "Documents/note.txt");
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
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error)
        );
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
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error)
        );
        assert!(plan.is_some());
    }

    #[test]
    fn preserve_layout_split_is_rejected() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let night = file("Music/Ada/night.mp3", FileFamily::Audio);
        let day = file("Music/Ada/day.mp3", FileFamily::Audio);
        let night_id = night.id;
        let day_id = day.id;
        snapshot.entries.push(directory("Music"));
        snapshot.entries.push(directory("Music/Ada"));
        snapshot.entries.push(night);
        snapshot.entries.push(day);
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::Folder,
            label: "Music".into(),
            members: vec![night_id, day_id],
            anchor: None,
            constraint: BundleConstraint::PreserveLayout {
                root: RelativePath::parse("Music").unwrap_or_else(|e| panic!("{e}")),
            },
            reason: "library".into(),
        });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let only_night = revision
            .with_patches(
                RevisionAuthor::User,
                "accept one track",
                &[
                    aifs_domain::RevisionPatch::SetDestination {
                        asset: night_id,
                        destination: RelativePath::parse("Documents/night.mp3")
                            .unwrap_or_else(|e| panic!("{e}")),
                        rationale: None,
                    },
                    aifs_domain::RevisionPatch::Accept {
                        assets: vec![night_id],
                    },
                ],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &only_night);
        assert!(plan.is_none());
        assert!(issues.iter().any(|issue| issue.code == "bundle_split"));
    }

    #[test]
    fn preserve_layout_flatten_is_rejected() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let night = file("Music/Ada/night.mp3", FileFamily::Audio);
        let day = file("Music/Ada/day.mp3", FileFamily::Audio);
        let night_id = night.id;
        let day_id = day.id;
        snapshot.entries.push(directory("Music"));
        snapshot.entries.push(directory("Music/Ada"));
        snapshot.entries.push(night);
        snapshot.entries.push(day);
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::Folder,
            label: "Music".into(),
            members: vec![night_id, day_id],
            anchor: None,
            constraint: BundleConstraint::PreserveLayout {
                root: RelativePath::parse("Music").unwrap_or_else(|e| panic!("{e}")),
            },
            reason: "library".into(),
        });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let flattened = revision
            .with_patches(
                RevisionAuthor::User,
                "flatten library",
                &[
                    aifs_domain::RevisionPatch::SetDestination {
                        asset: night_id,
                        destination: RelativePath::parse("Pictures/night.mp3")
                            .unwrap_or_else(|e| panic!("{e}")),
                        rationale: None,
                    },
                    aifs_domain::RevisionPatch::SetDestination {
                        asset: day_id,
                        destination: RelativePath::parse("Pictures/day.mp3")
                            .unwrap_or_else(|e| panic!("{e}")),
                        rationale: None,
                    },
                    aifs_domain::RevisionPatch::Accept {
                        assets: vec![night_id, day_id],
                    },
                ],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &flattened);
        assert!(plan.is_none());
        assert!(issues.iter().any(|issue| issue.code == "bundle_split"));
    }

    #[test]
    fn preserve_layout_conflicting_new_roots_are_rejected() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let night = file("Music/Ada/night.mp3", FileFamily::Audio);
        let day = file("Music/Ada/day.mp3", FileFamily::Audio);
        let night_id = night.id;
        let day_id = day.id;
        snapshot.entries.push(directory("Music"));
        snapshot.entries.push(directory("Music/Ada"));
        snapshot.entries.push(night);
        snapshot.entries.push(day);
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::Folder,
            label: "Music".into(),
            members: vec![night_id, day_id],
            anchor: None,
            constraint: BundleConstraint::PreserveLayout {
                root: RelativePath::parse("Music").unwrap_or_else(|e| panic!("{e}")),
            },
            reason: "library".into(),
        });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let split_roots = revision
            .with_patches(
                RevisionAuthor::User,
                "two new roots",
                &[
                    aifs_domain::RevisionPatch::SetDestination {
                        asset: night_id,
                        destination: RelativePath::parse("Archives/Music/Ada/night.mp3")
                            .unwrap_or_else(|e| panic!("{e}")),
                        rationale: None,
                    },
                    aifs_domain::RevisionPatch::SetDestination {
                        asset: day_id,
                        destination: RelativePath::parse("Other/Music/Ada/day.mp3")
                            .unwrap_or_else(|e| panic!("{e}")),
                        rationale: None,
                    },
                    aifs_domain::RevisionPatch::Accept {
                        assets: vec![night_id, day_id],
                    },
                ],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &split_roots);
        assert!(plan.is_none());
        assert!(issues.iter().any(|issue| issue.code == "bundle_split"));
    }

    #[test]
    fn preserve_layout_unit_move_is_allowed() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let night = file("Music/Ada/night.mp3", FileFamily::Audio);
        let day = file("Music/Ada/day.mp3", FileFamily::Audio);
        let night_id = night.id;
        let day_id = day.id;
        snapshot.entries.push(directory("Music"));
        snapshot.entries.push(directory("Music/Ada"));
        snapshot.entries.push(night);
        snapshot.entries.push(day);
        snapshot.bundles.push(aifs_domain::Bundle {
            id: aifs_domain::BundleId::new(),
            kind: aifs_domain::BundleKind::Folder,
            label: "Music".into(),
            members: vec![night_id, day_id],
            anchor: None,
            constraint: BundleConstraint::PreserveLayout {
                root: RelativePath::parse("Music").unwrap_or_else(|e| panic!("{e}")),
            },
            reason: "library".into(),
        });
        let revision = propose(&snapshot, &ProposalPolicy::default());
        let relocated = revision
            .with_patches(
                RevisionAuthor::User,
                "move library as a unit",
                &[
                    aifs_domain::RevisionPatch::SetDestination {
                        asset: night_id,
                        destination: RelativePath::parse("Archives/Music/Ada/night.mp3")
                            .unwrap_or_else(|e| panic!("{e}")),
                        rationale: None,
                    },
                    aifs_domain::RevisionPatch::SetDestination {
                        asset: day_id,
                        destination: RelativePath::parse("Archives/Music/Ada/day.mp3")
                            .unwrap_or_else(|e| panic!("{e}")),
                        rationale: None,
                    },
                    aifs_domain::RevisionPatch::Accept {
                        assets: vec![night_id, day_id],
                    },
                ],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &relocated);
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error),
            "unit move should plan, got {issues:?}"
        );
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
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error)
        );
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
    fn whitelist_rewrites_disallowed_top_level_and_drops_unknown_subs() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        snapshot.entries.push(file("clip.mp3", FileFamily::Audio));
        let policy = ProposalPolicy {
            style: FolderStyle::Refined,
            use_subfolders: true,
            rename_media: false,
            whitelist: CategoryWhitelist {
                main: vec!["Documents".into(), "Pictures".into()],
                global_subcategories: vec!["Reports".into()],
                ..CategoryWhitelist::default()
            },
            ..ProposalPolicy::default()
        };
        snapshot.evidence.push(
            Evidence::new(
                snapshot.entries[0].id,
                EvidenceSource::MediaTags,
                Confidence::CERTAIN,
            )
            .with_fact(keys::MEDIA_TITLE, "Night")
            .with_fact(keys::MEDIA_ARTIST, "Ada"),
        );
        let revision = propose(&snapshot, &policy);
        let placement = revision
            .placements
            .values()
            .next()
            .unwrap_or_else(|| panic!("p"));
        assert_eq!(placement.destination.as_str(), "clip.mp3");
        assert_eq!(placement.origin, SuggestionOrigin::Unchanged);
    }

    #[test]
    fn invalid_subfolder_does_not_escape_whitelist() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let entry = file("show.mp3", FileFamily::Audio);
        let id = entry.id;
        snapshot.evidence.push(
            Evidence::new(id, EvidenceSource::MediaTags, Confidence::CERTAIN)
                .with_fact(keys::MEDIA_TITLE, "Night")
                .with_fact(keys::MEDIA_ARTIST, "AUX")
                .with_fact(keys::MEDIA_GENRE, "Podcast"),
        );
        snapshot.entries.push(entry);
        let policy = ProposalPolicy {
            style: FolderStyle::Refined,
            use_subfolders: true,
            rename_media: false,
            whitelist: CategoryWhitelist {
                main: vec!["Podcasts".into()],
                ..CategoryWhitelist::default()
            },
            ..ProposalPolicy::default()
        };
        let revision = propose(&snapshot, &policy);
        let placement = revision.placement(id).unwrap_or_else(|| panic!("p"));
        assert!(
            placement.destination.as_str().starts_with("Podcasts/"),
            "must stay under the allowed top, got {}",
            placement.destination.as_str()
        );
        assert!(
            !placement.destination.as_str().starts_with("Music/"),
            "must not fall back to the family default outside the whitelist"
        );
    }

    #[test]
    fn skipped_junk_in_source_folder_is_not_removed() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        snapshot.entries.push(directory("dump"));
        snapshot
            .entries
            .push(file("dump/a.txt", FileFamily::Document));
        snapshot.skipped.push(SkippedEntry {
            path: RelativePath::parse("dump/.DS_Store").unwrap_or_else(|e| panic!("{e}")),
            reason: SkipReason::Junk,
        });
        let revision = accept_all(&propose(&snapshot, &ProposalPolicy::default()))
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &revision);
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error)
        );
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        assert!(
            !plan.operations.iter().any(|planned| matches!(
                planned.operation,
                Operation::RemoveEmptyDirectory { ref path } if path.as_str() == "dump"
            )),
            "skipped junk still occupies dump, so it must not be removed"
        );
    }

    #[test]
    fn leftover_empty_sibling_is_not_removed() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        snapshot.entries.push(directory("dump"));
        snapshot.entries.push(directory("dump/keep-empty"));
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
                    assets: vec![snapshot.entries[2].id],
                }],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &only_text);
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error)
        );
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        assert!(
            !plan.operations.iter().any(|planned| matches!(
                planned.operation,
                Operation::RemoveEmptyDirectory { ref path }
                    if path.as_str() == "dump/keep-empty" || path.as_str() == "dump"
            )),
            "must not delete unrelated empty folders while dump still has files"
        );
    }

    #[test]
    fn preexisting_empty_folder_is_not_removed_when_sibling_files_leave() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        snapshot.entries.push(directory("dump"));
        snapshot.entries.push(directory("dump/keep-empty"));
        snapshot
            .entries
            .push(file("dump/a.txt", FileFamily::Document));
        let revision = accept_all(&propose(&snapshot, &ProposalPolicy::default()))
            .unwrap_or_else(|e| panic!("{e}"));
        let (plan, issues) = validate(&snapshot, &revision);
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error)
        );
        let plan = plan.unwrap_or_else(|| panic!("plan"));
        assert!(
            !plan.operations.iter().any(|planned| matches!(
                planned.operation,
                Operation::RemoveEmptyDirectory { ref path }
                    if path.as_str() == "dump/keep-empty" || path.as_str() == "dump"
            )),
            "pre-existing empty folders must keep their parent, got {:?}",
            plan.operations
        );
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
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != PlanIssueSeverity::Error)
        );
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
