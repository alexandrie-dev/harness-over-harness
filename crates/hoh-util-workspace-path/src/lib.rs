//! Port of `@deepseek-ai/dsh-util-workspace-path` (`packages/util/workspace-path`).

pub mod file_address;
pub mod node_path;
pub use file_address::*;

use serde::{Deserialize, Serialize};

fn is_windows_style_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || value.starts_with("\\\\")
}

/// Whether a path has a POSIX, Windows drive, or UNC absolute prefix.
pub fn is_absolute_workspace_path(path: &str) -> bool {
    path.starts_with('/') || is_windows_style_path(path)
}

/// Join a relative spelling to a known workspace without resolving dot segments.
pub fn resolve_workspace_path(cwd: Option<&str>, path: &str) -> String {
    if is_absolute_workspace_path(path) {
        return path.to_owned();
    }
    let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) else {
        return path.to_owned();
    };
    let separator = if is_windows_style_path(cwd) && cwd.contains('\\') {
        '\\'
    } else {
        '/'
    };
    let base = cwd.trim_end_matches(['/', '\\']);
    let mut relative = path;
    // The source regexp repeats an optional dot followed by either separator.
    loop {
        if relative.starts_with(['/', '\\']) {
            relative = &relative[1..];
        } else if relative.starts_with("./") || relative.starts_with(".\\") {
            relative = &relative[2..];
        } else {
            break;
        }
    }
    format!("{base}{separator}{relative}")
}

/// Abbreviate only a POSIX home directory and its descendants.
pub fn abbreviate_home_path(path: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return path.to_owned();
    };
    if is_windows_style_path(path) || is_windows_style_path(home) {
        return path.to_owned();
    }
    let root = home.trim_end_matches('/');
    if root.is_empty() || root == "/" {
        return path.to_owned();
    }
    if path.trim_end_matches('/') == root {
        return "~".to_owned();
    }
    if path.starts_with(&format!("{root}/")) {
        return format!("~{}", &path[root.len()..]);
    }
    path.to_owned()
}

/// Read the final non-empty segment, treating both slash spellings as separators.
pub fn workspace_title_of(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_owned()
}

/// The directory prefix and final name returned by [`path_parts_of`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathParts {
    pub directory: String,
    pub name: String,
}

/// Split a display path, dropping trailing separators except for separator-only paths.
pub fn path_parts_of(path: &str) -> PathParts {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return PathParts {
            directory: String::new(),
            name: path.to_owned(),
        };
    }
    let cut = trimmed
        .rfind(['/', '\\'])
        .map_or(0, |separator| separator + 1);
    PathParts {
        directory: trimmed[..cut].to_owned(),
        name: trimmed[cut..].to_owned(),
    }
}

/// Address a path within its Session, shortening only the exact workspace prefix.
pub fn file_address_for(session_id: &str, cwd: Option<&str>, path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if !is_absolute_workspace_path(&normalized) {
        return session_file_address(session_id, &normalized);
    }
    let root = cwd.unwrap_or("").replace('\\', "/");
    let root = root.trim_end_matches('/');
    if !root.is_empty() {
        if normalized == root {
            return session_file_address(session_id, "");
        }
        if let Some(relative) = normalized.strip_prefix(&format!("{root}/")) {
            return session_file_address(session_id, relative);
        }
    }
    session_file_address(session_id, &normalized)
}

/// Strip a workspace directory prefix for display, keeping the remaining spelling.
pub fn relativize_to_cwd(text: &str, cwd: Option<&str>) -> String {
    let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) else {
        return text.to_owned();
    };
    let root = cwd.trim_end_matches(['/', '\\']);
    for separator in ['/', '\\'] {
        if let Some(relative) = text.strip_prefix(&format!("{root}{separator}")) {
            return relative.to_owned();
        }
    }
    text.to_owned()
}

/// A failed WHATWG URL construction, corresponding to JavaScript's `TypeError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileMediaUrlError {
    #[error("Invalid URL")]
    InvalidUrl(#[from] url::ParseError),
}

/// Address a decoded native file path through the application's file route.
///
/// Returns `Ok(None)` for unsupported transports or rejected native paths.
///
/// # Errors
/// Returns [`FileMediaUrlError`] when URL construction would throw a `TypeError`.
pub fn file_media_url(base: &str, path: &str) -> Result<Option<String>, FileMediaUrlError> {
    if !(base.starts_with("http:")
        || base.starts_with("https:")
        || base.starts_with("dsh-app://app/"))
        || !is_absolute_workspace_path(path)
        || path
            .as_bytes()
            .get(..2)
            .is_some_and(|prefix| prefix.iter().all(|b| matches!(b, b'/' | b'\\')))
        || path.chars().any(|ch| ch <= '\u{1f}' || ch == '\u{7f}')
    {
        return Ok(None);
    }
    let base = url::Url::parse(base)?;
    let relative = format!(
        "api/file?path={}",
        file_address::encode_component(path, false)
    );
    Ok(Some(base.join(&relative)?.into()))
}
