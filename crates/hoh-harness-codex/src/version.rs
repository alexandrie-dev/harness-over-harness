//! Codex CLI versions whose generated app-server v2 schema this adapter was checked against.

pub const SUPPORTED_MIN: (u64, u64, u64) = (0, 156, 0);
pub const SUPPORTED_MAX_EXCLUSIVE: (u64, u64, u64) = (0, 167, 0);
pub const SUPPORTED_RANGE_TEXT: &str = "0.156.0–0.166.x";

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
    "Codex CLI не найден. Установите Codex и перезапустите окно.".to_owned()
}

pub fn unsupported_version_message(version: &str) -> String {
    format!("Codex CLI версии {version} не поддерживается, нужна {SUPPORTED_RANGE_TEXT}.")
}

pub fn unrecognized_version_message(output: &str) -> String {
    let shown: String = output.trim().chars().take(80).collect();
    format!(
        "Не удалось определить версию Codex CLI (ответ: «{shown}»), нужна {SUPPORTED_RANGE_TEXT}."
    )
}

pub fn protocol_message(version: &str, what: &str) -> String {
    format!(
        "Codex CLI версии {version} ответил не так, как ожидалось ({what}); поддерживается {SUPPORTED_RANGE_TEXT}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_bounds_the_cli_banner() {
        assert_eq!(parse_version("codex-cli 0.166.0"), Some((0, 166, 0)));
        assert_eq!(parse_version("v0.156.1-beta"), Some((0, 156, 1)));
        assert!(is_supported((0, 156, 0)));
        assert!(is_supported((0, 166, 99)));
        assert!(!is_supported((0, 155, 9)));
        assert!(!is_supported((0, 167, 0)));
    }
}
