use url::Url;

pub const MESSAGES_FILES_BETA: &str = "files-api-2025-04-14";
pub const MESSAGES_TOOL_CHANGES_BETA: &str = "mid-conversation-tool-changes-2026-07-01";

pub fn messages_api_root(base_url: &str) -> Result<String, url::ParseError> {
    let trimmed = base_url.trim_end_matches('/');
    let parsed = Url::parse(trimmed)?;
    if parsed.path().trim_end_matches('/').ends_with("/v1") {
        Ok(trimmed.into())
    } else {
        Ok(format!("{trimmed}/v1"))
    }
}
