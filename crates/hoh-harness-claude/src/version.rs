//! The range of Claude Code versions this package speaks, and the window's words about it.
//!
//! The stream-json control protocol (`initialize`, `interrupt`, `can_use_tool`) is not published
//! by Anthropic as a stable interface: T3 Code reaches it through the Agent SDK, HoH speaks it
//! directly. So the range is checked at every discovery and every start, and a version outside
//! it gets no chat at all.

/// The oldest supported `claude --version` (inclusive).
pub const SUPPORTED_MIN: (u64, u64, u64) = (2, 0, 0);
/// The first unsupported major version (exclusive upper bound).
pub const SUPPORTED_MAX_EXCLUSIVE: (u64, u64, u64) = (3, 0, 0);
/// The range as the window shows it.
pub const SUPPORTED_RANGE_TEXT: &str = "2.0.0–2.x";

/// The first `major.minor.patch` in `text` (`2.0.31 (Claude Code)` → `(2, 0, 31)`).
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    text.split_whitespace().find_map(|token| {
        let token = token.trim_start_matches('v');
        let mut parts = token.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch: String = parts
            .next()?
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        Some((major, minor, patch.parse().ok()?))
    })
}

pub fn is_supported(version: (u64, u64, u64)) -> bool {
    version >= SUPPORTED_MIN && version < SUPPORTED_MAX_EXCLUSIVE
}

pub fn format_version(version: (u64, u64, u64)) -> String {
    format!("{}.{}.{}", version.0, version.1, version.2)
}

pub fn not_installed_message() -> String {
    "Claude Code не найден. Установите Claude Code и перезапустите окно.".to_owned()
}

pub fn unsupported_version_message(version: &str) -> String {
    format!("Claude Code версии {version} не поддерживается, нужна {SUPPORTED_RANGE_TEXT}.")
}

pub fn unrecognized_version_message(output: &str) -> String {
    let shown: String = output.trim().chars().take(80).collect();
    format!(
        "Не удалось определить версию Claude Code (ответ: «{shown}»), нужна {SUPPORTED_RANGE_TEXT}."
    )
}

pub fn protocol_message(version: &str, what: &str) -> String {
    format!(
        "Claude Code версии {version} ответил не так, как ожидалось ({what}); поддерживается {SUPPORTED_RANGE_TEXT}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_cli_banner() {
        assert_eq!(parse_version("2.0.31 (Claude Code)\n"), Some((2, 0, 31)));
        assert_eq!(parse_version("v2.1.0-beta.1"), Some((2, 1, 0)));
        assert_eq!(parse_version("claude 1.0.128"), Some((1, 0, 128)));
        assert_eq!(parse_version("hello"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn the_range_is_half_open() {
        assert!(is_supported((2, 0, 0)));
        assert!(is_supported((2, 99, 3)));
        assert!(!is_supported((1, 9, 9)));
        assert!(!is_supported((3, 0, 0)));
    }

    #[test]
    fn messages_name_the_version_and_the_range() {
        let text = unsupported_version_message("1.0.128");
        assert!(text.contains("1.0.128") && text.contains(SUPPORTED_RANGE_TEXT));
    }
}
