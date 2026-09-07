//! Prompts for local infer. Model output is evidence, never a path or SQL.

use aifs_domain::{
    Evidence, ObservedEntry, description_looks_like_screenshot, evidence::keys,
    filename_looks_like_screenshot,
};
use aifs_protocol::FolderStyle;

const EVIDENCE_CHARS: usize = 1500;
const FACT_CHARS: usize = 160;
const DOCUMENT_TEXT_CHARS: usize = 400;

/// System prompt for folder labels.
pub const CATEGORIZE_SYSTEM: &str = "You label files for a desktop organizer. \
Reply with one JSON object only: \
{\"category\":\"Top\",\"sub\":\"Optional\",\"description\":\"short\",\"suggested_name\":\"file.ext\"}. \
category is a single folder name, never a path. Do not invent SQL or filesystem commands.";

/// System prompt for local describe when a bitmap may be attached.
#[cfg(any(test, feature = "llama"))]
pub const DESCRIBE_SYSTEM: &str = "You describe an image for a desktop organizer. \
When a bitmap is attached, use what you see plus filename and EXIF facts. \
When no bitmap is attached, use only the filename and EXIF facts. \
Reply with one JSON object only: {\"description\":\"short caption\"}. \
Do not invent SQL or filesystem commands.";

/// System prompt for hosted describe. Pixels are never uploaded.
pub const DESCRIBE_SYSTEM_TEXT: &str = "You describe an image for a desktop organizer. \
Use only the filename, path, and any EXIF or metadata facts. \
Reply with one JSON object only: {\"description\":\"short caption\"}. \
Do not invent SQL or filesystem commands.";

/// System prompt for assistant chat. Patches are JSON; the engine applies them.
pub const CHAT_SYSTEM: &str = "You help organize files. Reply with one JSON object only: \
{\"message\":\"brief\",\"patches\":[{\"op\":\"move_to_folder\",\"assets\":[\"uuid\"],\"folder\":\"Folder\"}]}. \
ops: set_destination, move_to_folder, rename, accept, reject, reopen. \
folder and destination are root-relative, never '..', never absolute, never SQL or shell. \
Use \"patches\":[] when you are only answering. Do not emit filesystem operations.";

/// True when filename or description evidence looks like a screenshot or UI capture.
pub fn looks_like_screenshot(entry: &ObservedEntry, evidence: &[Evidence]) -> bool {
    if filename_looks_like_screenshot(entry.path.file_name()) {
        return true;
    }
    evidence.iter().any(|bag| {
        bag.fact(keys::DESCRIPTION)
            .is_some_and(description_looks_like_screenshot)
    })
}

/// Builds the system prompt for categorize, including whitelist and folder style.
pub fn categorize_system(allowed_categories: &[String], style: FolderStyle) -> String {
    let mut prompt = CATEGORIZE_SYSTEM.to_owned();
    if style == FolderStyle::Refined {
        prompt.push_str(
            " Prefer specific folders when evidence supports them (Screenshots, Podcasts).",
        );
    }
    if !allowed_categories.is_empty() {
        prompt.push_str(" category must be one of: ");
        prompt.push_str(&allowed_categories.join(", "));
        prompt.push('.');
    }
    prompt
}

/// Builds the user turn for categorize.
pub fn categorize_user(
    entry: &ObservedEntry,
    evidence: &[Evidence],
    allowed_categories: &[String],
    style: FolderStyle,
) -> String {
    let mut user = format!(
        "Relative path: {}\nFamily: {:?}\n{}",
        entry.path.as_str(),
        entry.family,
        format_evidence(evidence)
    );
    if looks_like_screenshot(entry, evidence) {
        user.push_str("\nThis looks like a screenshot or UI capture.");
        if style == FolderStyle::Refined {
            user.push_str(" Prefer category Screenshots.");
        }
    }
    if !allowed_categories.is_empty() {
        user.push_str("\nAllowed categories: ");
        user.push_str(&allowed_categories.join(", "));
    }
    if style == FolderStyle::Refined {
        user.push_str("\nFolder style: refined.");
    }
    user
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

    fn file(path: &str, family: aifs_domain::FileFamily) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: aifs_domain::RelativePath::parse(path).unwrap_or_else(|error| panic!("{error}")),
            kind: aifs_domain::EntryKind::File,
            family,
            identity: aifs_domain::FileIdentity::default(),
            is_hidden: false,
            lock: aifs_domain::LockState::Readable,
        }
    }

    #[test]
    fn prompt_includes_path_as_data_not_as_a_command() {
        let entry = file("notes.txt", aifs_domain::FileFamily::Document);
        let user = categorize_user(&entry, &[], &[], FolderStyle::Consistent);
        assert!(user.contains("notes.txt"));
        assert!(!user.contains("rm "));
        assert!(CATEGORIZE_SYSTEM.contains("JSON"));
        assert!(DESCRIBE_SYSTEM.contains("EXIF"));
        assert!(DESCRIBE_SYSTEM.contains("bitmap"));
        assert!(DESCRIBE_SYSTEM_TEXT.contains("EXIF"));
        assert!(!DESCRIBE_SYSTEM_TEXT.contains("bitmap"));
        assert!(CHAT_SYSTEM.contains("SQL"));
        assert!(CHAT_SYSTEM.contains("patches"));
        let described = describe_user(&entry, &[]);
        assert!(described.contains("notes.txt"));
    }

    #[test]
    fn categorize_user_includes_description_whitelist_and_screenshot_hint() {
        let shot = file("Screenshot.png", aifs_domain::FileFamily::Image);
        let bag = Evidence::new(
            shot.id,
            EvidenceSource::LocalModel {
                model: "gemma".into(),
            },
            Confidence::new(0.55),
        )
        .with_fact(keys::DESCRIPTION, "a settings panel UI capture");
        let allowed = vec!["Screenshots".into(), "Pictures".into()];
        let user = categorize_user(&shot, &[bag], &allowed, FolderStyle::Refined);
        assert!(user.contains("a settings panel UI capture"));
        assert!(user.contains("Allowed categories: Screenshots, Pictures"));
        assert!(user.contains("screenshot or UI capture"));
        assert!(user.contains("Prefer category Screenshots"));
        assert!(user.contains("Folder style: refined"));
        let system = categorize_system(&allowed, FolderStyle::Refined);
        assert!(system.contains("category must be one of: Screenshots, Pictures"));
        assert!(system.contains("Screenshots, Podcasts"));
        assert!(looks_like_screenshot(&shot, &[]));
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
            &[],
            FolderStyle::Consistent,
        );
        assert!(user.len() <= EVIDENCE_CHARS + 80, "{}", user.len());
    }
}
