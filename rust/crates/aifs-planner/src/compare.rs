//! Compare a proposal against a golden destination map (fixture regression).

use aifs_domain::{BundleKind, EntryKind, ProposalRevision, WorkspaceSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Frozen expectations for a fixture directory. Keys are source relative paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedPlan {
    /// Source path → proposed destination.
    pub destinations: BTreeMap<String, String>,
    /// Relative roots of strong protected projects.
    #[serde(default)]
    pub protected_projects: Vec<String>,
    /// Stems of sidecar bundles that must be detected.
    #[serde(default)]
    pub sidecar_stems: Vec<String>,
}

/// Builds a sorted source→destination map for file entries.
pub fn destination_map(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for entry in snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        if let Some(placement) = revision.placement(entry.id) {
            map.insert(
                entry.path.as_str().to_owned(),
                placement.destination.as_str().to_owned(),
            );
        }
    }
    map
}

/// Differences between a live proposal and a golden fixture. Empty means a match.
pub fn diff_plan(
    snapshot: &WorkspaceSnapshot,
    revision: &ProposalRevision,
    expected: &ExpectedPlan,
) -> Vec<String> {
    let mut diffs = Vec::new();
    let actual = destination_map(snapshot, revision);
    for (path, dest) in &expected.destinations {
        match actual.get(path) {
            Some(got) if got == dest => {}
            Some(got) => diffs.push(format!("{path}: expected {dest}, got {got}")),
            None => diffs.push(format!("{path}: missing from proposal (expected {dest})")),
        }
    }
    for (path, dest) in &actual {
        if !expected.destinations.contains_key(path) {
            diffs.push(format!("{path}: unexpected placement {dest}"));
        }
    }
    let mut projects: Vec<_> = snapshot
        .projects
        .iter()
        .map(|project| project.root.as_str().to_owned())
        .collect();
    projects.sort();
    let mut expected_projects = expected.protected_projects.clone();
    expected_projects.sort();
    if projects != expected_projects {
        diffs.push(format!(
            "protected projects: expected {expected_projects:?}, got {projects:?}"
        ));
    }
    for stem in &expected.sidecar_stems {
        let found = snapshot.bundles.iter().any(|bundle| {
            bundle.kind == BundleKind::SidecarGroup && bundle.label.eq_ignore_ascii_case(stem)
        });
        if !found {
            diffs.push(format!("missing sidecar bundle for stem {stem}"));
        }
    }
    diffs
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{
        AssetId, Bundle, BundleConstraint, BundleId, EntryKind, FileFamily, FileIdentity,
        LockState, ObservedEntry, Placement, RelativePath, ReviewState, RevisionAuthor, SessionId,
        SuggestionOrigin,
    };
    use std::path::PathBuf;

    #[test]
    fn reports_destination_and_sidecar_mismatches() {
        let mut snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/in"));
        let id = AssetId::new();
        snapshot.entries.push(ObservedEntry {
            id,
            path: RelativePath::parse("a.txt").unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::Document,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        });
        snapshot.bundles.push(Bundle {
            id: BundleId::new(),
            kind: BundleKind::SidecarGroup,
            label: "photo".into(),
            members: vec![id],
            anchor: Some(id),
            constraint: BundleConstraint::MoveTogether,
            reason: String::new(),
        });
        let mut revision = ProposalRevision::new(snapshot.session, RevisionAuthor::Engine, "t");
        revision.place(Placement {
            asset: id,
            destination: RelativePath::parse("Documents/a.txt").unwrap_or_else(|e| panic!("{e}")),
            rationale: None,
            origin: SuggestionOrigin::Heuristic,
            review: ReviewState::Proposed,
        });
        let expected = ExpectedPlan {
            destinations: BTreeMap::from([("a.txt".into(), "Other/a.txt".into())]),
            protected_projects: vec!["app".into()],
            sidecar_stems: vec!["photo".into(), "clip".into()],
        };
        let diffs = diff_plan(&snapshot, &revision, &expected);
        assert!(diffs.iter().any(|d| d.contains("expected Other/a.txt")));
        assert!(diffs.iter().any(|d| d.contains("protected projects")));
        assert!(diffs.iter().any(|d| d.contains("clip")));
        assert!(!diffs.iter().any(|d| d.contains("stem photo")));
    }
}
