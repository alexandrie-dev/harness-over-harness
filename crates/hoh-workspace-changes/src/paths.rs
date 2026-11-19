//! Canonical, durable, and display path rules.

use std::{
    cmp::Ordering,
    collections::BTreeSet,
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

/// JavaScript-compatible display ordering by UTF-16 code units.
pub fn compare_display(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// Slash-separated form used by git and the public API.
pub fn to_posix(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// Whether `path` is `root` or a descendant of it.
pub fn is_inside(root: &Path, path: &Path) -> bool {
    path == root || path.strip_prefix(root).is_ok()
}

/// Resolve symlinks through the nearest existing ancestor while retaining missing components.
pub async fn canonical_path(path: &Path) -> PathBuf {
    let original = path.to_path_buf();
    let mut missing: Vec<OsString> = Vec::new();
    let mut head = original.clone();
    loop {
        match tokio::fs::canonicalize(&head).await {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return resolved;
            }
            Err(_) => {
                let Some(parent) = head.parent() else {
                    return original;
                };
                if parent == head || parent.parent().is_none_or(|grand| grand == parent) {
                    return original;
                }
                let Some(name) = head.file_name() else {
                    return original;
                };
                missing.push(name.to_os_string());
                head = parent.to_path_buf();
            }
        }
    }
}

/// Canonical raw and symlink-resolved spellings of temporary directories.
pub async fn temporary_roots(candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    for root in candidates {
        roots.insert(root.clone());
        roots.insert(canonical_path(root).await);
    }
    roots.into_iter().collect()
}

pub fn is_temporary_path(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| is_inside(root, path))
}

fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<Component<'_>> = from.components().collect();
    let to: Vec<Component<'_>> = to.components().collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return to.iter().collect();
    }
    let mut relative = PathBuf::new();
    for _ in common..from.len() {
        relative.push("..");
    }
    for component in &to[common..] {
        relative.push(component.as_os_str());
    }
    relative
}

/// Sort key and label for one canonical absolute path.
pub fn display_path_of(absolute: &Path, cwd: &Path, root: &Path, home: Option<&Path>) -> String {
    if is_inside(cwd, absolute) || is_inside(root, absolute) {
        return to_posix(&relative_path(cwd, absolute));
    }
    if let Some(home) = home
        && is_inside(home, absolute)
    {
        return format!("~/{}", to_posix(&relative_path(home, absolute)));
    }
    to_posix(absolute)
}

/// Path relative to the working directory inside it, absolute elsewhere.
pub fn durable_path_of(absolute: &Path, cwd: &Path) -> String {
    if is_inside(cwd, absolute) {
        to_posix(&relative_path(cwd, absolute))
    } else {
        absolute.to_string_lossy().into_owned()
    }
}
