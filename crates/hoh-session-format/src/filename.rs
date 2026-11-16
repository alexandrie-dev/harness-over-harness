//! Canonical raw log basename shared by every generation-addressed Session artifact.

use crate::error::SessionFormatError;
use crate::json::{MAX_SAFE_INTEGER, session_format_count_u64};

/// Name the raw JSONL log of one immutable Session format generation. Version zero keeps the
/// original `session.jsonl`; every later generation carries a lowercase numeric `.vN` component
/// before the `.jsonl` suffix.
pub fn session_format_log_filename(version: u64) -> Result<String, SessionFormatError> {
    let generation = session_format_count_u64(version, "Session log generation version")?;
    Ok(if generation == 0 {
        "session.jsonl".to_owned()
    } else {
        format!("session.v{generation}.jsonl")
    })
}

/// Read the generation named by one raw JSONL log basename, matching
/// `^session(?:\.v([1-9][0-9]*))?\.jsonl$`. Temporary, uppercase, leading-zero, `.v0`, and
/// compression-suffixed names are not canonical. `None` when the name is not canonical.
pub fn parse_session_format_log_filename(filename: &str) -> Option<u64> {
    let middle = filename.strip_prefix("session")?.strip_suffix(".jsonl")?;
    if middle.is_empty() {
        return Some(0);
    }
    let digits = middle.strip_prefix(".v")?;
    let mut chars = digits.chars();
    let first = chars.next()?;
    if !('1'..='9').contains(&first) || !chars.all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    // A digit string past u64 is far beyond the safe-integer range as well.
    let version = digits.parse::<u64>().ok()?;
    (version <= MAX_SAFE_INTEGER).then_some(version)
}
