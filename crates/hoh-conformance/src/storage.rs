//! Storage guards of the committed Session fixtures (`suite.ts`: `assertSessionFixtureStorage`,
//! `unknownToolCallIds`, `systemPromptPrecedesRequests`).
use crate::{
    Result, corpus,
    normalize::{Normalizer, scrub_system, scrub_tools},
    read, records,
};
use serde_json::Value;
use std::path::Path;

/// The current Session format generation of the committed fixtures.
const CURRENT: u64 = 4;

/// Tool calls whose structured result reports `UNKNOWN_TOOL` (a placeholder when the call id is absent).
pub fn unknown_tool_call_ids(raw: &str) -> Result<Vec<String>> {
    Ok(records(raw)?
        .iter()
        .filter(|r| r["type"] == "tool/result")
        .filter(|r| r["data"]["error"]["code"] == "UNKNOWN_TOOL")
        .map(|r| {
            r["data"]["message"]["source"]["callId"]
                .as_str()
                .unwrap_or("<missing callId>")
                .to_owned()
        })
        .collect())
}

/// Whether the first `request/header` (if any) follows a `system/message`.
pub fn system_prompt_precedes_requests(raw: &str) -> Result<bool> {
    let rows = records(raw)?;
    let first = |kind: &str| rows.iter().position(|r| r["type"] == kind);
    Ok(match first("request/header") {
        None => true,
        Some(header) => first("system/message").is_some_and(|prompt| prompt < header),
    })
}

fn unchanged(rows: &[Value], scrub: impl Fn(&mut Value)) -> bool {
    rows.iter().all(|row| {
        let mut scrubbed = row.clone();
        scrub(&mut scrubbed);
        &scrubbed == row
    })
}

/// Check the selected fixtures of one scenario directory: no `UNKNOWN_TOOL`, no non-canonical macOS
/// cwd token; current-generation fixtures also carry scrubbed prompts and tools, a prompt before
/// every request, and are a fixed point of the identity redaction.
pub fn assert_session_fixture_storage(dir: &Path, scenario: &str) -> Result<()> {
    let mut current = Vec::new();
    for path in corpus::fixture_files(dir)? {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let raw = read(&path)?;
        let rows = records(&raw)?;
        let version = rows
            .first()
            .and_then(|h| h.get("version"))
            .map_or(Some(0), Value::as_u64);
        if !unknown_tool_call_ids(&raw)?.is_empty() {
            return Err(format!("{scenario}/{file} contains UNKNOWN_TOOL"));
        }
        if raw.contains("/private{{cwd}}") {
            return Err(format!(
                "{scenario}/{file} carries a non-canonical macOS cwd token"
            ));
        }
        if version != Some(CURRENT) {
            continue;
        }
        let retain = rows.iter().any(|r| r["type"] == "developer/message");
        if !unchanged(&rows, scrub_system) {
            return Err(format!(
                "{scenario}/{file} carries an unscrubbed system prompt"
            ));
        }
        if !unchanged(&rows, |r| scrub_tools(r, retain)) {
            return Err(format!("{scenario}/{file} carries unscrubbed tool schemas"));
        }
        if !system_prompt_precedes_requests(&raw)? {
            return Err(format!(
                "{scenario}/{file} has a request/header with no preceding system/message"
            ));
        }
        current.push(rows);
    }
    let mut redacted = current.clone();
    Normalizer::new()?.identities(&mut redacted);
    if redacted != current {
        return Err(format!("{scenario}: identity redaction fixed point"));
    }
    Ok(())
}
