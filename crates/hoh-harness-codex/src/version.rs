//! Optional Codex version metadata and installation diagnostics.

use serde_json::Value;

pub(crate) fn from_initialize(response: &Value) -> Option<String> {
    let version = response
        .pointer("/serverInfo/version")
        .and_then(Value::as_str)
        .or_else(|| {
            let user_agent = response.get("userAgent")?.as_str()?;
            let (_, version) = user_agent.split_whitespace().next()?.rsplit_once('/')?;
            Some(version)
        })?;
    let version = version.trim().strip_prefix('v').unwrap_or(version.trim());
    if version.len() <= 80
        && version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    {
        Some(version.to_owned())
    } else {
        None
    }
}

pub fn not_installed_message() -> String {
    "Codex CLI не найден. Установите Codex и перезапустите окно.".to_owned()
}

pub(crate) fn missing_code_mode_host_message() -> String {
    "The Codex CLI installation is missing an executable codex-code-mode-host. Install the complete Codex CLI package and restart HoH."
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_optional_metadata_without_interpreting_release_numbers() {
        assert_eq!(
            from_initialize(&json!({"userAgent":"hoh/0.172.0-alpha.1 (Linux)"})).as_deref(),
            Some("0.172.0-alpha.1")
        );
        assert_eq!(
            from_initialize(&json!({"serverInfo":{"version":"0.98.0"}})).as_deref(),
            Some("0.98.0")
        );
        assert_eq!(
            from_initialize(&json!({"serverInfo":{"version":"local build"}})),
            None
        );
    }
}
