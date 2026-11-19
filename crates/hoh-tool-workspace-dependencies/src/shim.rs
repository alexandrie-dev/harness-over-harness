#[cfg(not(windows))]
pub(crate) use hoh_util_workspace_path::node_path as host_path;

#[cfg(windows)]
pub(crate) use win32_path as host_path;

// SHIM(hoh-util-workspace-path): the dependency currently exposes only Node path.posix;
// replace this module when it provides host path.win32 operations.
#[cfg(any(windows, test))]
pub(crate) mod win32_path {
    fn separators(value: &str) -> impl Iterator<Item = &str> {
        value.split(['/', '\\'])
    }

    pub fn is_absolute(path: &str) -> bool {
        let bytes = path.as_bytes();
        path.starts_with(['/', '\\'])
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'))
    }

    fn normalize(path: &str) -> String {
        let bytes = path.as_bytes();
        let drive = (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
            .then(|| path[..2].to_owned());
        let unc = (path.starts_with("\\\\") || path.starts_with("//")).then(|| {
            separators(path)
                .filter(|part| !part.is_empty())
                .take(2)
                .collect::<Vec<_>>()
        });
        let absolute = is_absolute(path);
        let skip = if unc.is_some() { 2 } else { 0 };
        let mut parts = Vec::new();
        for part in separators(path).filter(|part| !part.is_empty()).skip(skip) {
            if drive.as_deref().is_some_and(|drive| part == drive) || part == "." {
                continue;
            }
            if part == ".." {
                if parts.last().is_some_and(|part| *part != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push(part);
                }
            } else {
                parts.push(part);
            }
        }
        let prefix = if let Some(parts) = unc {
            format!(
                "\\\\{}\\{}",
                parts.first().unwrap_or(&""),
                parts.get(1).unwrap_or(&"")
            )
        } else if let Some(drive) = drive {
            if absolute {
                format!("{drive}\\")
            } else {
                drive
            }
        } else if absolute {
            "\\".to_owned()
        } else {
            String::new()
        };
        let body = parts.join("\\");
        if prefix.is_empty() && body.is_empty() {
            ".".to_owned()
        } else if prefix.ends_with('\\') || body.is_empty() {
            format!("{prefix}{body}")
        } else {
            format!("{prefix}\\{body}")
        }
    }

    pub fn join(parts: &[&str]) -> String {
        normalize(
            &parts
                .iter()
                .copied()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\\"),
        )
    }

    pub fn dirname(path: &str) -> String {
        let normalized = normalize(path);
        if (normalized.len() == 3
            && normalized.as_bytes().get(1) == Some(&b':')
            && normalized.ends_with('\\'))
            || (normalized.starts_with("\\\\")
                && separators(&normalized)
                    .filter(|part| !part.is_empty())
                    .count()
                    <= 2)
        {
            return normalized;
        }
        let trimmed = normalized.trim_end_matches(['/', '\\']);
        match trimmed.rfind(['/', '\\']) {
            Some(index) if index == 2 && trimmed.as_bytes().get(1) == Some(&b':') => {
                trimmed[..=index].to_owned()
            }
            Some(0) => "\\".to_owned(),
            Some(index) => trimmed[..index].to_owned(),
            None if trimmed.len() == 2 && trimmed.ends_with(':') => trimmed.to_owned(),
            None => ".".to_owned(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{dirname, is_absolute, join};

        #[test]
        fn keeps_drive_and_unc_share_roots_for_staging_paths() {
            assert!(is_absolute(r"C:\runtime"));
            assert!(is_absolute(r"\\server\share\runtime"));
            assert_eq!(dirname(r"C:\"), r"C:\");
            assert_eq!(dirname(r"\\server\share"), r"\\server\share");
            assert_eq!(
                join(&[r"C:\runtime", "dependencies", "python"]),
                r"C:\runtime\dependencies\python"
            );
        }
    }
}
