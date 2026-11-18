//! Shared parser for the exit-status markers emitted by shell-tool renderers.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum ParsedExitStatus {
    ExitCode { body: String, exit_code: u64 },
    Signal { body: String, signal: String },
}

fn split_marker<'a>(text: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
    let before_close = text.strip_suffix(']')?;
    let start = before_close.rfind(prefix)?;
    Some((&text[..start], &before_close[start + prefix.len()..]))
}

pub fn parse_exit_status(text: &str) -> ParsedExitStatus {
    if let Some((body, signal)) = split_marker(text, "\n[killed by signal: ")
        && !signal.is_empty()
        && !signal.contains(']')
        && !signal.contains('\n')
    {
        return ParsedExitStatus::Signal {
            body: body.to_owned(),
            signal: signal.to_owned(),
        };
    }
    if let Some((body, digits)) = split_marker(text, "\n[exit code: ")
        && !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(exit_code) = digits.parse()
    {
        return ParsedExitStatus::ExitCode {
            body: body.to_owned(),
            exit_code,
        };
    }
    ParsedExitStatus::ExitCode {
        body: text.to_owned(),
        exit_code: 0,
    }
}
