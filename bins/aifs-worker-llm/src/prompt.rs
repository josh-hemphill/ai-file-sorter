//! Prompts for local infer. Model output is evidence, never a path or SQL.

use aifs_domain::{Evidence, ObservedEntry, evidence::keys};

const EVIDENCE_CHARS: usize = 1500;
const FACT_CHARS: usize = 160;
const DOCUMENT_TEXT_CHARS: usize = 400;

/// System prompt for folder labels.
pub const CATEGORIZE_SYSTEM: &str = "You label files for a desktop organizer. \
Reply with one JSON object only: \
{\"category\":\"Top\",\"sub\":\"Optional\",\"description\":\"short\",\"suggested_name\":\"file.ext\"}. \
category is a single folder name, never a path. Do not invent SQL or filesystem commands.";

/// System prompt for image captions.
pub const DESCRIBE_SYSTEM: &str = "You describe an image for a desktop organizer using the filename \
and any EXIF facts. Reply with one JSON object only: {\"description\":\"short caption\"}. \
Do not invent SQL or filesystem commands.";

/// System prompt for assistant chat (tools still run in the engine).
pub const CHAT_SYSTEM: &str = "You help organize files. Speak briefly. Do not emit SQL, shell, \
or filesystem operations. The engine applies its own revision tools.";

/// Builds the user turn for categorize.
pub fn categorize_user(entry: &ObservedEntry, evidence: &[Evidence]) -> String {
    format!(
        "Relative path: {}\nFamily: {:?}\n{}",
        entry.path.as_str(),
        entry.family,
        format_evidence(evidence)
    )
}

/// Builds the user turn for describe.
pub fn describe_user(entry: &ObservedEntry, evidence: &[Evidence]) -> String {
    format!(
        "Relative path: {}\nFamily: {:?}\n{}",
        entry.path.as_str(),
        entry.family,
        format_evidence(evidence)
    )
}

fn format_evidence(evidence: &[Evidence]) -> String {
    let mut lines = Vec::new();
    for bag in evidence {
        for (key, value) in &bag.facts {
            let limit = if key == keys::DOCUMENT_TEXT {
                DOCUMENT_TEXT_CHARS
            } else {
                FACT_CHARS
            };
            lines.push(format!("{key}: {}", truncate(value, limit)));
        }
    }
    let joined = if lines.is_empty() {
        "Evidence: none".to_owned()
    } else {
        format!("Evidence:\n{}", lines.join("\n"))
    };
    truncate(&joined, EVIDENCE_CHARS)
}

fn truncate(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{AssetId, Confidence, EvidenceSource};

    #[test]
    fn prompt_includes_path_as_data_not_as_a_command() {
        let entry = ObservedEntry {
            id: AssetId::new(),
            path: aifs_domain::RelativePath::parse("notes.txt")
                .unwrap_or_else(|error| panic!("{error}")),
            kind: aifs_domain::EntryKind::File,
            family: aifs_domain::FileFamily::Document,
            identity: aifs_domain::FileIdentity::default(),
            is_hidden: false,
            lock: aifs_domain::LockState::Readable,
        };
        let user = categorize_user(&entry, &[]);
        assert!(user.contains("notes.txt"));
        assert!(!user.contains("rm "));
        assert!(CATEGORIZE_SYSTEM.contains("JSON"));
        assert!(DESCRIBE_SYSTEM.contains("EXIF"));
        assert!(CHAT_SYSTEM.contains("SQL"));
        let described = describe_user(&entry, &[]);
        assert!(described.contains("notes.txt"));
    }

    #[test]
    fn evidence_text_is_capped() {
        let long = "x".repeat(800);
        let bag = Evidence::new(
            AssetId::new(),
            EvidenceSource::DocumentMetadata,
            Confidence::new(1.0),
        )
        .with_fact(keys::DOCUMENT_TEXT, long);
        let user = categorize_user(
            &ObservedEntry {
                id: bag.asset,
                path: aifs_domain::RelativePath::parse("a.pdf")
                    .unwrap_or_else(|error| panic!("{error}")),
                kind: aifs_domain::EntryKind::File,
                family: aifs_domain::FileFamily::Document,
                identity: aifs_domain::FileIdentity::default(),
                is_hidden: false,
                lock: aifs_domain::LockState::Readable,
            },
            &[bag],
        );
        assert!(user.len() <= EVIDENCE_CHARS + 80, "{}", user.len());
    }
}
