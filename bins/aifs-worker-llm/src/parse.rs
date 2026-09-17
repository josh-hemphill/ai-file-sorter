//! Turns model text into untrusted evidence facts. Never treats output as a path or SQL.

use aifs_domain::escape_path_segment;
use aifs_domain::evidence::keys;
use aifs_domain::{EntryKind, ObservedEntry};
use serde::Deserialize;

const DESCRIPTION_CHARS: usize = 400;
const NAME_CHARS: usize = 80;
const CATEGORY_CHARS: usize = 80;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedInfer {
    pub category: Option<String>,
    pub category_sub: Option<String>,
    pub description: Option<String>,
    pub suggested_name: Option<String>,
    pub grouping: Option<String>,
    pub grouping_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InferJson {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default, rename = "category.sub")]
    category_sub: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    suggested_name: Option<String>,
    #[serde(default)]
    grouping: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// Parses a JSON object out of model text. Garbage yields `None`.
pub fn parse_infer_json(text: &str) -> Option<ParsedInfer> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    let parsed: InferJson = serde_json::from_str(&text[start..=end]).ok()?;
    let category = sanitize_label(parsed.category.as_deref());
    let category_sub = sanitize_label(parsed.category_sub.as_deref())
        .or_else(|| sanitize_label(parsed.sub.as_deref()));
    let description = sanitize_description(parsed.description.as_deref());
    let suggested_name = filename_only(parsed.suggested_name.as_deref());
    let grouping = parse_grouping(parsed.grouping.as_deref());
    let grouping_reason = sanitize_description(parsed.reason.as_deref());
    if category.is_none() && description.is_none() && suggested_name.is_none() && grouping.is_none()
    {
        return None;
    }
    Some(ParsedInfer {
        category,
        category_sub,
        description,
        suggested_name,
        grouping,
        grouping_reason,
    })
}

/// True when parsed JSON has the field this infer kind requires.
pub fn infer_has_required_field(
    parsed: &ParsedInfer,
    entry: &ObservedEntry,
    want_category: bool,
) -> bool {
    if entry.kind == EntryKind::Directory {
        parsed.grouping.is_some()
    } else if want_category {
        parsed.category.is_some()
    } else {
        parsed.description.is_some()
    }
}

fn parse_grouping(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.contains('/') || value.contains('\\') || value.contains("..") {
        return None;
    }
    let normalized = value.to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "camera_dump" | "event_or_date" | "library" | "broad_inbox" | "archive" | "mixed" => {
            Some(normalized)
        }
        _ => None,
    }
}

fn sanitize_label(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.contains('/') || value.contains('\\') {
        return None;
    }
    let escaped = escape_path_segment(value);
    let trimmed: String = escaped.chars().take(CATEGORY_CHARS).collect();
    let trimmed = trimmed.trim().trim_matches('.').trim();
    if trimmed.is_empty() || trimmed == ".." {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn sanitize_description(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let text: String = value
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n')
        .take(DESCRIPTION_CHARS)
        .collect();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn filename_only(value: Option<&str>) -> Option<String> {
    let value = value?.replace('\\', "/");
    let name = value.rsplit('/').next().unwrap_or("").trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let escaped = escape_path_segment(name);
    let trimmed: String = escaped.chars().take(NAME_CHARS).collect();
    let trimmed = trimmed.trim().trim_matches('.');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Copies parsed facts onto an evidence builder callback.
pub fn apply_parsed(parsed: &ParsedInfer, mut set: impl FnMut(&'static str, &str)) {
    if let Some(category) = &parsed.category {
        set(keys::CATEGORY, category);
    }
    if let Some(sub) = &parsed.category_sub {
        set(keys::CATEGORY_SUB, sub);
    }
    if let Some(description) = &parsed.description {
        set(keys::DESCRIPTION, description);
    }
    if let Some(name) = &parsed.suggested_name {
        set(keys::SUGGESTED_NAME, name);
    }
    if let Some(grouping) = &parsed.grouping {
        set(keys::DIRECTORY_GROUPING, grouping);
    }
    if let Some(reason) = &parsed.grouping_reason {
        set(keys::DIRECTORY_GROUPING_REASON, reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_json_even_with_prose_around_it() {
        let parsed = parse_infer_json(
            "sure, here:\n{\"category\":\"Documents\",\"sub\":\"Notes\",\"description\":\"a memo\"}\n",
        )
        .unwrap_or_else(|| panic!("expected json"));
        assert_eq!(parsed.category.as_deref(), Some("Documents"));
        assert_eq!(parsed.category_sub.as_deref(), Some("Notes"));
        assert_eq!(parsed.description.as_deref(), Some("a memo"));
        let mut keys_seen = Vec::new();
        apply_parsed(&parsed, |key, _value| keys_seen.push(key));
        assert!(keys_seen.contains(&"category"));
    }

    #[test]
    fn drops_path_like_categories_and_names() {
        let parsed = parse_infer_json(
            r#"{"category":"../Etc","suggested_name":"/tmp/pwned.txt","description":"ok"}"#,
        )
        .unwrap_or_else(|| panic!("expected json"));
        assert!(parsed.category.is_none(), "{parsed:?}");
        assert_eq!(parsed.suggested_name.as_deref(), Some("pwned.txt"));
        let nested = parse_infer_json(r#"{"category":"Documents/Notes","description":"ok"}"#)
            .unwrap_or_else(|| panic!("expected json"));
        assert!(nested.category.is_none(), "{nested:?}");
    }

    #[test]
    fn reads_directory_grouping_labels() {
        let parsed = parse_infer_json(r#"{"grouping":"camera_dump","reason":"nested DCIM roll"}"#)
            .unwrap_or_else(|| panic!("expected json"));
        assert_eq!(parsed.grouping.as_deref(), Some("camera_dump"));
        assert_eq!(parsed.grouping_reason.as_deref(), Some("nested DCIM roll"));
        assert!(parse_infer_json(r#"{"grouping":"../Etc"}"#).is_none());
        assert!(parse_infer_json(r#"{"grouping":"not_a_label"}"#).is_none());
        let spaced = parse_infer_json(r#"{"grouping":"event-or-date"}"#)
            .unwrap_or_else(|| panic!("normalized"));
        assert_eq!(spaced.grouping.as_deref(), Some("event_or_date"));
    }

    #[test]
    fn garbage_is_none() {
        assert!(parse_infer_json("Documents folder please").is_none());
    }

    #[test]
    fn song_title_punctuation_is_escaped_not_stripped() {
        let parsed = parse_infer_json(
            r#"{"category":"Music?","suggested_name":"What Is Love?.mp3","description":"ok"}"#,
        )
        .unwrap_or_else(|| panic!("expected json"));
        assert_eq!(parsed.category.as_deref(), Some("Music\u{FF1F}"));
        assert_eq!(
            parsed.suggested_name.as_deref(),
            Some("What Is Love\u{FF1F}.mp3")
        );
    }
}
