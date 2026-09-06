//! Line framing helpers. One JSON document per line, no embedded newlines.

use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;

/// Maximum accepted line length; protects the engine from a runaway client.
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Framing/parsing failure.
#[derive(Debug, Error)]
pub enum CodecError {
    /// Line exceeds [`MAX_LINE_BYTES`].
    #[error("line of {0} bytes exceeds the {MAX_LINE_BYTES} byte limit")]
    TooLong(usize),
    /// Line is blank.
    #[error("blank line")]
    Blank,
    /// JSON error.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Serialises a message to a single line (without the trailing newline).
pub fn encode_line<T: Serialize>(value: &T) -> Result<String, CodecError> {
    let json = serde_json::to_string(value)?;
    debug_assert!(
        !json.contains('\n'),
        "serde_json compact output never contains newlines"
    );
    Ok(json)
}

/// Parses one line into a message.
pub fn decode_line<T: DeserializeOwned>(line: &str) -> Result<T, CodecError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(CodecError::TooLong(line.len()));
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(CodecError::Blank);
    }
    Ok(serde_json::from_str(trimmed)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Request, PROTOCOL_VERSION};

    #[test]
    fn round_trips_requests() {
        let request = Request {
            id: "1".into(),
            command: Command::Hello {
                client: "c".into(),
                protocol_version: PROTOCOL_VERSION,
            },
        };
        let line = encode_line(&request).unwrap_or_default();
        let parsed: Request = decode_line(&format!("{line}\r\n")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed, request);
    }

    #[test]
    fn rejects_blank_and_oversized_lines() {
        assert!(matches!(
            decode_line::<Request>("   "),
            Err(CodecError::Blank)
        ));
        let huge = "x".repeat(MAX_LINE_BYTES + 1);
        assert!(matches!(
            decode_line::<Request>(&huge),
            Err(CodecError::TooLong(_))
        ));
    }
}
