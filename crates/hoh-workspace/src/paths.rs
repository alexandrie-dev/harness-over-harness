use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathPlatform {
    Posix,
    Windows,
}

impl PathPlatform {
    pub const fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Posix
        }
    }
}

pub fn fully_qualified_workspace_path_for(path: &str, platform: PathPlatform) -> bool {
    match platform {
        PathPlatform::Posix => path.starts_with('/'),
        PathPlatform::Windows => {
            let bytes = path.as_bytes();
            let drive = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\');
            let unc = bytes.len() >= 3
                && matches!(bytes[0], b'/' | b'\\')
                && matches!(bytes[1], b'/' | b'\\')
                && !matches!(bytes[2], b'/' | b'\\')
                && path[2..]
                    .split(['/', '\\'])
                    .filter(|part| !part.is_empty())
                    .count()
                    >= 2;
            drive || unc
        }
    }
}

pub fn fully_qualified_workspace_path(path: &str) -> bool {
    fully_qualified_workspace_path_for(path, PathPlatform::host())
}

pub fn default_workspace_title_for(path: &str, platform: PathPlatform) -> String {
    match platform {
        PathPlatform::Posix => {
            let title = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            if title.is_empty() { "/" } else { title }.to_owned()
        }
        PathPlatform::Windows => {
            let bytes = path.as_bytes();
            if bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\')
                && bytes[3..].iter().all(|byte| matches!(byte, b'/' | b'\\'))
            {
                return path[..3].to_owned();
            }
            let trimmed = path.trim_end_matches(['/', '\\']);
            let title = trimmed.rsplit(['/', '\\']).next().unwrap_or("");
            if !title.is_empty() {
                title.to_owned()
            } else if path.starts_with("\\\\") || path.starts_with("//") {
                path.to_owned()
            } else {
                path.get(..3).unwrap_or(path).to_owned()
            }
        }
    }
}

pub fn default_workspace_title(path: &str) -> String {
    default_workspace_title_for(path, PathPlatform::host())
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspacePathError {
    #[error("Workspace path is not fully qualified: '{0}'")]
    NotFullyQualified(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub async fn realpath_normalize(path: &str) -> Result<String, WorkspacePathError> {
    if !fully_qualified_workspace_path(path) {
        return Err(WorkspacePathError::NotFullyQualified(path.to_owned()));
    }
    let canonical = tokio::fs::canonicalize(Path::new(path))
        .await?
        .to_string_lossy()
        .into_owned();
    #[cfg(windows)]
    let canonical = canonical
        .strip_prefix(r"\\?\UNC\")
        .map(|path| format!(r"\\{path}"))
        .or_else(|| canonical.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or(canonical);
    Ok(canonical)
}
