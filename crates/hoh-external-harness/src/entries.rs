//! The window's `@` (HOH-41, `hoh/workspace/entries`): the files and folders of one folder the
//! engine knows whose relative path matches what the person typed, as T3's
//! `projects.searchEntries` answers it. Names and relative paths only; no file is opened.
//!
//! The walk never leaves the folder: it does not follow a link into a folder, and a link is listed
//! only when what it points to is inside the folder. `.git` is never entered; the folder's own
//! `.gitignore` hides what it names plainly. At most [`MAX_SCANNED`] entries are looked at (T3
//! `WORKSPACE_INDEX_MAX_ENTRIES`), folder by folder from the top, so a huge tree is cut deep, not
//! near the top.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use hoh_protocol::{WorkspaceEntry, WorkspaceEntryKind};

/// Entries looked at in one search (T3 `WORKSPACE_INDEX_MAX_ENTRIES`).
pub(crate) const MAX_SCANNED: usize = 25_000;
/// The most entries one answer has (T3 `PROJECT_SEARCH_ENTRIES_MAX_LIMIT`).
pub(crate) const MAX_LIMIT: u32 = 200;
/// The longest query (T3 `PROJECT_SEARCH_ENTRIES_QUERY_MAX_LENGTH`).
pub(crate) const MAX_QUERY_CHARS: usize = 256;

/// The entries of `root` (canonical) matching `query`, the best `limit` of them, and whether more
/// matched or the walk was cut.
pub(crate) fn search(root: &Path, query: &str, limit: usize) -> (Vec<WorkspaceEntry>, bool) {
    let ignored = Ignored::of(root);
    let query = query.trim().to_lowercase();
    let mut found: Vec<(u8, WorkspaceEntry)> = Vec::new();
    let mut scanned = 0;
    let mut cut = false;
    let mut folders = VecDeque::from([PathBuf::new()]);
    'walk: while let Some(relative) = folders.pop_front() {
        let Ok(listing) = std::fs::read_dir(root.join(&relative)) else {
            continue;
        };
        let mut names: Vec<_> = listing.filter_map(Result::ok).collect();
        names.sort_by_key(std::fs::DirEntry::file_name);
        for entry in names {
            if scanned == MAX_SCANNED {
                cut = true;
                break 'walk;
            }
            scanned += 1;
            // A name that is not UTF-8 cannot be written into the prompt.
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = relative.join(&name);
            let Some(text) = path.to_str().map(|text| text.replace('\\', "/")) else {
                continue;
            };
            if name == ".git" || ignored.hides(&name, relative.as_os_str().is_empty()) {
                continue;
            }
            // The entry itself, not what a link points to.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let kind = if kind.is_symlink() {
                match inside(root, &root.join(&path)) {
                    Some(true) => WorkspaceEntryKind::Directory,
                    Some(false) => WorkspaceEntryKind::File,
                    None => continue,
                }
            } else if kind.is_dir() {
                folders.push_back(path);
                WorkspaceEntryKind::Directory
            } else {
                WorkspaceEntryKind::File
            };
            if let Some(rank) = rank(&text, &name, &query) {
                found.push((rank, WorkspaceEntry { path: text, kind }));
            }
        }
    }
    found.sort_by(|(a, left), (b, right)| {
        a.cmp(b)
            .then(left.path.len().cmp(&right.path.len()))
            .then(left.path.cmp(&right.path))
    });
    let truncated = cut || found.len() > limit;
    found.truncate(limit);
    (
        found.into_iter().map(|(_, entry)| entry).collect(),
        truncated,
    )
}

/// Where a link inside `root` leads: `Some(true)` to a folder inside `root`, `Some(false)` to a
/// file inside it, `None` outside it or nowhere.
fn inside(root: &Path, link: &Path) -> Option<bool> {
    let target = std::fs::canonicalize(link).ok()?;
    if !target.starts_with(root) {
        return None;
    }
    Some(std::fs::metadata(&target).ok()?.is_dir())
}

/// How well `path` (relative) matches `query` (lowercase): 0 its name holds the query, 1 the path
/// does, 2 the query's characters come in order in the path; `None` no match. An empty query
/// matches everything.
fn rank(path: &str, name: &str, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let path = path.to_lowercase();
    if name.to_lowercase().contains(query) {
        return Some(0);
    }
    if path.contains(query) {
        return Some(1);
    }
    let mut wanted = query.chars().peekable();
    for c in path.chars() {
        if wanted.peek() == Some(&c) {
            wanted.next();
        }
    }
    wanted.peek().is_none().then_some(2)
}

/// The plain names of the folder's own `.gitignore`: `name` anywhere, `/name` at the top only
/// (a trailing `/` is dropped). Patterns with wildcards, negations or inner slashes are not read.
struct Ignored {
    anywhere: Vec<String>,
    top: Vec<String>,
}

impl Ignored {
    fn of(root: &Path) -> Self {
        let mut ignored = Ignored {
            anywhere: Vec::new(),
            top: Vec::new(),
        };
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
        for line in text.lines().map(str::trim) {
            if line.is_empty()
                || line.starts_with(['#', '!'])
                || line.contains(['*', '?', '[', '\\'])
            {
                continue;
            }
            let at_top = line.starts_with('/');
            let name = line.trim_start_matches('/').trim_end_matches('/');
            if name.is_empty() || name.contains('/') {
                continue;
            }
            if at_top {
                ignored.top.push(name.to_owned());
            } else {
                ignored.anywhere.push(name.to_owned());
            }
        }
        ignored
    }

    fn hides(&self, name: &str, at_top: bool) -> bool {
        self.anywhere.iter().any(|ignored| ignored == name)
            || (at_top && self.top.iter().any(|ignored| ignored == name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tree() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        for folder in [
            "src/nested",
            ".git/objects",
            "node_modules/pkg",
            "build",
            "src/build",
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        for file in [
            "README.md",
            "src/main.rs",
            "src/nested/deep.rs",
            ".git/HEAD",
            "node_modules/pkg/index.js",
            "build/out.bin",
            "src/build/keep.rs",
        ] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "node_modules/\n/build\n*.log\n").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "outside").unwrap();
        std::fs::create_dir(dir.path().join("elsewhere")).unwrap();
        std::fs::write(dir.path().join("elsewhere/hidden.rs"), "outside").unwrap();
        let root = root.canonicalize().unwrap();
        (dir, root)
    }

    fn paths(entries: &[WorkspaceEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.path.as_str()).collect()
    }

    #[test]
    fn relative_paths_of_the_folder_only_without_git_and_what_gitignore_names() {
        let (_dir, root) = tree();
        let (entries, truncated) = search(&root, "", 200);
        assert!(!truncated);
        assert_eq!(
            paths(&entries),
            [
                "src",
                "README.md",
                "src/build",
                ".gitignore",
                "src/nested",
                "src/main.rs",
                "src/build/keep.rs",
                "src/nested/deep.rs",
            ]
        );
        assert!(entries.iter().all(|entry| !entry.path.starts_with('/')));
    }

    #[test]
    fn a_link_out_of_the_folder_is_neither_listed_nor_followed() {
        let (dir, root) = tree();
        symlink(dir.path().join("secret.txt"), root.join("leak.txt")).unwrap();
        symlink(dir.path().join("elsewhere"), root.join("away")).unwrap();
        symlink("../..", root.join("src/up")).unwrap();
        symlink(root.join("src/main.rs"), root.join("main-link.rs")).unwrap();
        symlink(root.join("src"), root.join("src-link")).unwrap();
        let (entries, _) = search(&root, "", 200);
        let found = paths(&entries);
        for outside in ["leak.txt", "away", "away/hidden.rs", "src/up"] {
            assert!(!found.contains(&outside), "{outside}: {found:?}");
        }
        assert!(
            found.iter().all(|path| !path.contains("hidden")),
            "{found:?}"
        );
        // A link inside is listed with the kind of what it points to, and not entered.
        let link = entries
            .iter()
            .find(|entry| entry.path == "src-link")
            .unwrap();
        assert_eq!(link.kind, WorkspaceEntryKind::Directory);
        assert!(
            !found.iter().any(|path| path.starts_with("src-link/")),
            "{found:?}"
        );
        assert!(found.contains(&"main-link.rs"));
    }

    #[test]
    fn the_query_matches_names_then_paths_then_characters_in_order() {
        let (_dir, root) = tree();
        let (entries, _) = search(&root, "MAIN", 200);
        assert_eq!(paths(&entries), ["src/main.rs"]);
        let (entries, _) = search(&root, "nested", 200);
        assert_eq!(paths(&entries), ["src/nested", "src/nested/deep.rs"]);
        let (entries, _) = search(&root, "sndp", 200);
        assert_eq!(paths(&entries), ["src/nested/deep.rs"]);
        let (entries, _) = search(&root, "..", 200);
        assert!(entries.is_empty(), "{:?}", paths(&entries));
    }

    #[test]
    fn the_limit_cuts_the_answer_and_says_so() {
        let (_dir, root) = tree();
        let (entries, truncated) = search(&root, "", 3);
        assert_eq!(entries.len(), 3);
        assert!(truncated);
    }

    #[test]
    fn a_huge_folder_is_cut_at_the_scan_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for index in 0..MAX_SCANNED + 5 {
            std::fs::write(root.join(format!("f{index:05}")), "").unwrap();
        }
        let (entries, truncated) = search(&root, "", 200);
        assert_eq!(entries.len(), 200);
        assert!(truncated);
    }
}
