//! Sidecar, subtitle, and cover-art grouping.

use aifs_domain::{
    AssetId, Bundle, BundleConstraint, BundleId, BundleKind, Confidence, EntryKind, FileFamily,
    ObservedEntry, Relationship, RelationshipKind, WorkspaceSnapshot,
};
use std::collections::HashMap;

const COVER_STEMS: &[&str] = &["cover", "folder", "album", "front", "artwork"];

/// Adds sidecar / subtitle / cover-art bundles and edges to `snapshot`.
pub fn detect_sidecars(snapshot: &mut WorkspaceSnapshot) {
    let files: Vec<ObservedEntry> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .cloned()
        .collect();

    let mut groups: HashMap<(String, String), Vec<AssetId>> = HashMap::new();
    for entry in &files {
        let key = (parent_key(entry), grouping_stem(entry));
        groups.entry(key).or_default().push(entry.id);
    }

    let mut new_bundles = Vec::new();
    let mut new_relationships = Vec::new();
    for ((_parent, stem), members) in groups {
        if members.len() < 2 {
            continue;
        }
        let resolved: Vec<&ObservedEntry> = members
            .iter()
            .filter_map(|id| files.iter().find(|entry| entry.id == *id))
            .collect();
        if let Some(bundle) = sidecar_bundle(&resolved, &stem) {
            sidecar_relationships(&files, &bundle, &mut new_relationships);
            new_bundles.push(bundle);
        }
    }

    cover_art_relationships(&files, &mut new_relationships);
    snapshot.bundles.extend(new_bundles);
    snapshot.relationships.extend(new_relationships);
}

fn sidecar_bundle(members: &[&ObservedEntry], stem: &str) -> Option<Bundle> {
    let has_raw = members.iter().any(|e| e.family == FileFamily::RawImage);
    let has_image = members.iter().any(|e| e.family == FileFamily::Image);
    let has_video = members.iter().any(|e| e.family == FileFamily::Video);
    let has_audio = members.iter().any(|e| e.family == FileFamily::Audio);
    let has_subtitle = members.iter().any(|e| e.family == FileFamily::Subtitle);
    let has_sidecar = members.iter().any(|e| e.family == FileFamily::Sidecar);

    let matches = (has_raw && (has_image || has_sidecar))
        || (has_video && has_subtitle)
        || (has_audio && (has_subtitle || has_sidecar))
        || (has_image && has_sidecar);
    if !matches {
        return None;
    }

    let anchor = pick_anchor(members);
    Some(Bundle {
        id: BundleId::new(),
        kind: BundleKind::SidecarGroup,
        label: stem.to_owned(),
        members: members.iter().map(|entry| entry.id).collect(),
        anchor: Some(anchor),
        constraint: BundleConstraint::MoveTogether,
        reason: "Companion files share a stem and should stay together.".to_owned(),
    })
}

fn pick_anchor(members: &[&ObservedEntry]) -> AssetId {
    let preferred = [
        FileFamily::RawImage,
        FileFamily::Video,
        FileFamily::Audio,
        FileFamily::Image,
    ];
    for family in preferred {
        if let Some(entry) = members.iter().find(|entry| entry.family == family) {
            return entry.id;
        }
    }
    members
        .iter()
        .max_by_key(|entry| entry.identity.size)
        .or_else(|| members.first())
        .map(|entry| entry.id)
        .unwrap_or_else(AssetId::new)
}

fn sidecar_relationships(files: &[ObservedEntry], bundle: &Bundle, out: &mut Vec<Relationship>) {
    let Some(anchor) = bundle.anchor else {
        return;
    };
    let Some(primary) = files.iter().find(|entry| entry.id == anchor) else {
        return;
    };
    for member in &bundle.members {
        if *member == anchor {
            continue;
        }
        let Some(other) = files.iter().find(|entry| entry.id == *member) else {
            continue;
        };
        let kind = if other.family == FileFamily::Subtitle {
            RelationshipKind::Subtitle
        } else {
            RelationshipKind::Sidecar
        };
        out.push(Relationship {
            from: primary.id,
            to: other.id,
            kind,
            confidence: Confidence::CERTAIN,
            detector: "sidecar-stem".to_owned(),
            note: None,
        });
    }
}

fn cover_art_relationships(files: &[ObservedEntry], out: &mut Vec<Relationship>) {
    let mut by_parent: HashMap<String, Vec<&ObservedEntry>> = HashMap::new();
    for entry in files {
        by_parent.entry(parent_key(entry)).or_default().push(entry);
    }
    for siblings in by_parent.values() {
        let covers: Vec<&&ObservedEntry> = siblings
            .iter()
            .filter(|entry| {
                entry.family == FileFamily::Image
                    && COVER_STEMS.contains(&entry.stem().to_ascii_lowercase().as_str())
            })
            .collect();
        if covers.is_empty() {
            continue;
        }
        for audio in siblings
            .iter()
            .filter(|entry| entry.family == FileFamily::Audio)
        {
            for cover in &covers {
                out.push(Relationship {
                    from: audio.id,
                    to: cover.id,
                    kind: RelationshipKind::CoverArt,
                    confidence: Confidence::new(0.8),
                    detector: "cover-art-name".to_owned(),
                    note: None,
                });
            }
        }
    }
}

pub(crate) fn parent_key(entry: &ObservedEntry) -> String {
    entry
        .path
        .parent()
        .map(|parent| parent.as_str().to_ascii_lowercase())
        .unwrap_or_default()
}

pub(crate) fn grouping_stem(entry: &ObservedEntry) -> String {
    let stem = entry.stem();
    if entry.family == FileFamily::Subtitle {
        strip_language_suffix(stem).to_ascii_lowercase()
    } else {
        stem.to_ascii_lowercase()
    }
}

fn strip_language_suffix(stem: &str) -> &str {
    let mut remaining = stem;
    loop {
        let Some((head, tail)) = remaining.rsplit_once('.') else {
            return remaining;
        };
        if head.is_empty() || !is_language_tag(tail) {
            return remaining;
        }
        remaining = head;
    }
}

fn is_language_tag(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "forced" | "sdh" | "cc" | "hi" | "hearing-impaired"
    ) {
        return true;
    }
    if let Some((lang, region)) = lower.split_once('-') {
        return is_alpha_len(lang, 2, 3) && is_alpha_len(region, 2, 3);
    }
    is_alpha_len(&lower, 2, 3)
}

fn is_alpha_len(value: &str, min: usize, max: usize) -> bool {
    let len = value.chars().count();
    (min..=max).contains(&len) && value.chars().all(|ch| ch.is_ascii_alphabetic())
}
