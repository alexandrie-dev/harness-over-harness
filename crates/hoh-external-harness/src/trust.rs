//! Profile-owned folder trust. An absent or unreadable record always means no trust.
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FILE: &str = "external-harness-folder-trust.json";
static SAVING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub digest: String,
}

/// A missing file has its own marker, distinct from an empty file. Project MCP configuration
/// is covered too: it can introduce executable servers after the initial approval.
pub(crate) fn snapshot(folder: &Path) -> std::io::Result<Snapshot> {
    use std::os::unix::fs::MetadataExt;
    let path = folder.canonicalize()?;
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_dir() {
        return Err(std::io::Error::other("folder is not a directory"));
    }
    let mut hash = Sha256::new();
    // Claude discovers MCP files all the way to the filesystem root, even when
    // the selected folder is not a Git repository.
    for root in path.ancestors() {
        let root_bytes = root.as_os_str().as_encoded_bytes();
        hash.update((root_bytes.len() as u64).to_be_bytes());
        hash.update(root_bytes);
        match read_setting(&root.join(".mcp.json")) {
            Ok(bytes) => {
                hash.update([1]);
                hash.update((bytes.len() as u64).to_be_bytes());
                hash.update(bytes);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => hash.update([0]),
            Err(error) => return Err(error),
        }
    }
    for root in settings_roots(&path)? {
        let identity = std::fs::metadata(&root)?;
        let root_bytes = root.as_os_str().as_encoded_bytes();
        hash.update((root_bytes.len() as u64).to_be_bytes());
        hash.update(root_bytes);
        hash.update(identity.dev().to_be_bytes());
        hash.update(identity.ino().to_be_bytes());
        for directory in [".claude", ".codex"] {
            match std::fs::symlink_metadata(root.join(directory)) {
                Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                    return Err(std::io::Error::other(
                        "settings directory must be a directory",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        for name in [
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".mcp.json",
            ".codex/config.toml",
        ] {
            hash.update((name.len() as u64).to_be_bytes());
            hash.update(name.as_bytes());
            match read_setting(&root.join(name)) {
                Ok(bytes) => {
                    hash.update([1]);
                    hash.update((bytes.len() as u64).to_be_bytes());
                    hash.update(bytes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => hash.update([0]),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(Snapshot {
        path,
        device: metadata.dev(),
        inode: metadata.ino(),
        digest: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}

/// Git worktrees may read local settings from the main checkout as well as the worktree root.
/// Inspect Git's metadata directly: discovering trust must never execute repository programs.
fn settings_roots(folder: &Path) -> std::io::Result<BTreeSet<PathBuf>> {
    let mut roots = BTreeSet::from([folder.to_path_buf()]);
    for ancestor in folder.ancestors() {
        let dotgit = ancestor.join(".git");
        let meta = match std::fs::symlink_metadata(&dotgit) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::other("symlinked Git metadata"));
        }
        // Codex can load project configuration in intermediate directories too.
        // Only include these after finding the repository boundary: an unrelated
        // directory outside Git must not pull in settings from the user's home.
        for root in folder.ancestors() {
            roots.insert(root.to_path_buf());
            if root == ancestor {
                break;
            }
        }
        let gitdir = if meta.is_dir() {
            dotgit
        } else if meta.is_file() {
            let bytes = read_setting(&dotgit)?;
            let text = std::str::from_utf8(&bytes).map_err(std::io::Error::other)?;
            let target = text
                .trim()
                .strip_prefix("gitdir: ")
                .ok_or_else(|| std::io::Error::other("invalid gitdir"))?;
            ancestor.join(target).canonicalize()?
        } else {
            return Err(std::io::Error::other("invalid Git metadata"));
        };
        match read_setting(&gitdir.join("commondir")) {
            Ok(bytes) => {
                let common = std::str::from_utf8(&bytes)
                    .map_err(std::io::Error::other)?
                    .trim();
                let common = gitdir.join(common).canonicalize()?;
                if common.file_name().is_none_or(|name| name != ".git") {
                    return Err(std::io::Error::other(
                        "cannot resolve main checkout from Git common directory",
                    ));
                }
                roots.insert(
                    common
                        .parent()
                        .ok_or_else(|| std::io::Error::other("invalid Git common directory"))?
                        .canonicalize()?,
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        break;
    }
    Ok(roots)
}

fn read_setting(path: &Path) -> std::io::Result<Vec<u8>> {
    use rustix::fs::{Mode, OFlags, open};
    const LIMIT: u64 = 1024 * 1024;
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    let file = std::fs::File::from(fd);
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > LIMIT {
        return Err(std::io::Error::other(
            "settings must be a regular file under 1 MiB",
        ));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(std::io::Error::other("settings file is too large"));
    }
    Ok(bytes)
}

async fn load(profile: &Path) -> Vec<Snapshot> {
    match tokio::fs::read(profile.join(FILE)).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

async fn save(profile: &Path, entries: &[Snapshot]) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(entries).map_err(std::io::Error::other)?;
    hoh_atomic_write::write_file_atomic(
        &profile.join(FILE),
        bytes,
        hoh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: None,
        },
    )
    .await
}

pub(crate) async fn status(profile: &Path, folder: &Path) -> std::io::Result<(bool, bool)> {
    let _guard = SAVING.lock().await;
    let mut entries = load(profile).await;
    let current = match snapshot(folder) {
        Ok(current) => current,
        Err(error) => {
            if let Ok(path) = folder.canonicalize() {
                let previous = entries.len();
                entries.retain(|entry| entry.path != path);
                if entries.len() != previous {
                    save(profile, &entries).await?;
                    return Ok((false, true));
                }
                return Ok((false, false));
            }
            return Err(error);
        }
    };
    let Some(saved) = entries.iter().find(|entry| entry.path == current.path) else {
        return Ok((false, false));
    };
    if *saved == current {
        return Ok((true, false));
    }
    entries.retain(|entry| entry.path != current.path);
    save(profile, &entries).await?;
    Ok((false, true))
}

pub(crate) async fn set(profile: &Path, folder: &Path, trusted: bool) -> std::io::Result<()> {
    let _guard = SAVING.lock().await;
    let current = snapshot(folder)?;
    let mut entries = load(profile).await;
    entries.retain(|entry| entry.path != current.path);
    if trusted {
        entries.push(current);
    }
    save(profile, &entries).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parent_mcp_changes_revoke_trust_outside_and_above_git() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        let repo = parent.join("repo");
        let sub = repo.join("sub");
        let profile = temp.path().join("profile");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&profile).unwrap();
        for git_repo in [false, true] {
            if git_repo {
                git(&repo, &["init", "-q"]);
            }
            set(&profile, &sub, true).await.unwrap();
            std::fs::write(parent.join(".mcp.json"), format!("{git_repo}")).unwrap();
            assert_eq!(status(&profile, &sub).await.unwrap(), (false, true));
        }
    }

    fn git(cwd: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[tokio::test]
    async fn git_subfolder_and_worktree_include_all_roots_in_trust() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("worktree");
        let profile = temp.path().join("profile");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&profile).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("tracked"), "ok").unwrap();
        git(&repo, &["add", "tracked"]);
        git(&repo, &["commit", "-qm", "initial"]);
        let repo_sub = repo.join("sub");
        std::fs::create_dir_all(&repo_sub).unwrap();
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        set(&profile, &repo_sub, true).await.unwrap();
        std::fs::write(repo.join(".claude/settings.local.json"), "changed").unwrap();
        assert_eq!(status(&profile, &repo_sub).await.unwrap(), (false, true));
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-qb",
                "other",
                worktree.to_str().unwrap(),
            ],
        );
        let middle = worktree.join("middle");
        let sub = middle.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        for root in [&repo, &worktree, &middle, &sub] {
            std::fs::create_dir_all(root.join(".claude")).unwrap();
            for file in [
                ".claude/settings.json",
                ".claude/settings.local.json",
                ".mcp.json",
                ".codex/config.toml",
            ] {
                if let Some(parent) = root.join(file).parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                set(&profile, &sub, true).await.unwrap();
                std::fs::write(root.join(file), "changed again").unwrap();
                assert_eq!(
                    status(&profile, &sub).await.unwrap(),
                    (false, true),
                    "{}",
                    root.join(file).display()
                );
            }
        }
    }

    #[test]
    fn snapshot_resolves_aliases_and_tracks_file_contents_and_absence() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("project");
        std::fs::create_dir_all(folder.join(".claude")).unwrap();
        std::os::unix::fs::symlink(&folder, temp.path().join("alias")).unwrap();
        let initial = snapshot(&folder).unwrap();
        assert_eq!(
            initial,
            snapshot(&temp.path().join("alias/../project")).unwrap()
        );
        std::fs::write(folder.join(".claude/settings.json"), "").unwrap();
        let empty = snapshot(&folder).unwrap();
        assert_ne!(initial, empty);
        std::fs::write(folder.join(".claude/settings.json"), "{}").unwrap();
        assert_ne!(empty, snapshot(&folder).unwrap());
        let before_mcp = snapshot(&folder).unwrap();
        std::fs::write(folder.join(".mcp.json"), "{}").unwrap();
        assert_ne!(before_mcp, snapshot(&folder).unwrap());
    }

    #[tokio::test]
    async fn trust_persists_and_is_revoked_on_change_even_if_contents_are_restored() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let folder = temp.path().join("project");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(folder.join(".claude")).unwrap();
        let settings = folder.join(".claude/settings.json");
        std::fs::write(&settings, "{}").unwrap();
        assert_eq!(status(&profile, &folder).await.unwrap(), (false, false));
        set(&profile, &folder, true).await.unwrap();
        assert_eq!(status(&profile, &folder).await.unwrap(), (true, false));
        std::fs::write(&settings, "{ } ").unwrap();
        assert_eq!(status(&profile, &folder).await.unwrap(), (false, true));
        std::fs::write(&settings, "{}").unwrap();
        assert_eq!(status(&profile, &folder).await.unwrap(), (false, false));
    }

    #[tokio::test]
    async fn symlinked_settings_revoke_trust_without_following_them() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let folder = temp.path().join("project");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(folder.join(".claude")).unwrap();
        set(&profile, &folder, true).await.unwrap();
        std::os::unix::fs::symlink("/dev/null", folder.join(".claude/settings.json")).unwrap();
        assert_eq!(status(&profile, &folder).await.unwrap(), (false, true));
        assert_eq!(status(&profile, &folder).await.unwrap(), (false, false));
        assert!(set(&profile, &folder, true).await.is_err());
        std::fs::remove_file(folder.join(".claude/settings.json")).unwrap();
        assert_eq!(status(&profile, &folder).await.unwrap(), (false, false));
    }
}
