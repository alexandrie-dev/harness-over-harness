//! Folder browsing for the T3 client's «Add project → Local folder» (lead 22.11 20:43): the names of
//! the subfolders that match what the person typed, never files. Port of T3's
//! `apps/server/src/workspace/WorkspaceEntries.ts` (`browse`, `resolveBrowseTarget`) and
//! `os-jank.ts` (`expandHomePath`). Pure, so it is tested without a window; the Tauri command
//! (`native::browse_folders`) only answers the main window.
use serde::Serialize;
use std::cmp::Ordering;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    pub full_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderListing {
    pub parent_path: String,
    pub entries: Vec<FolderEntry>,
}

/// `~` and `~/…` are the person's home folder.
fn expand_home(input: &str, home: &Path) -> PathBuf {
    if input == "~" {
        return home.to_owned();
    }
    match input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        Some(rest) => home.join(rest),
        None => PathBuf::from(input),
    }
}

fn explicit_relative(value: &str) -> bool {
    matches!(value, "." | "..")
        || ["./", "../", ".\\", "..\\"]
            .iter()
            .any(|prefix| value.starts_with(prefix))
}

fn windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}

/// `path.resolve` without the file system: `.` and `..` are taken by their text.
fn resolve(base: &Path, path: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for component in base.join(path).components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    out
}

/// The subfolders of the folder `partial` names (or that hold what it starts), filtered by the
/// typed tail. `cwd` is the current project's folder, needed only for `./` and `../`. A path that
/// is neither absolute, `~` nor explicitly relative is taken from the home folder (T3 takes it from
/// its server's working folder; the window has none).
pub fn list_folders(
    partial: &str,
    cwd: Option<&str>,
    home: &Path,
) -> Result<FolderListing, String> {
    if windows_absolute(partial) {
        return Err(format!(
            "Windows paths are not supported on this computer: '{partial}'."
        ));
    }
    let target = if explicit_relative(partial) {
        let cwd = cwd.filter(|cwd| !cwd.is_empty()).ok_or_else(|| {
            format!("A current project is required to browse relative workspace path '{partial}'.")
        })?;
        resolve(&expand_home(cwd, home), Path::new(partial))
    } else {
        resolve(home, &expand_home(partial, home))
    };
    let ends_with_separator = partial.ends_with(['/', '\\']) || partial == "~";
    let (parent, prefix) = if ends_with_separator {
        (target, String::new())
    } else {
        let prefix = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        (
            target.parent().map(Path::to_path_buf).unwrap_or(target),
            prefix,
        )
    };
    let read = match std::fs::read_dir(&parent) {
        Ok(read) => read,
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            return Ok(FolderListing {
                parent_path: parent.display().to_string(),
                entries: Vec::new(),
            });
        }
        Err(_) => return Err(format!("Unable to read folder '{}'.", parent.display())),
    };
    let show_hidden = ends_with_separator || prefix.starts_with('.');
    let lower_prefix = prefix.to_lowercase();
    let mut entries: Vec<FolderEntry> = read
        .flatten()
        // The entry's own type: a link to a folder is not followed, a file never shows.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.to_lowercase().starts_with(&lower_prefix)
                && (show_hidden || !name.starts_with('.'))
        })
        .map(|name| FolderEntry {
            full_path: parent.join(&name).display().to_string(),
            name,
        })
        .collect();
    entries.sort_by(|left, right| compare_names(&left.name, &right.name));
    Ok(FolderListing {
        parent_path: parent.display().to_string(),
        entries,
    })
}

/// Close to `localeCompare`: letters before case, then the exact text.
fn compare_names(left: &str, right: &str) -> Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}
