use crate::store::DEFAULT_ROOT_PREFIX;
use indexmap::IndexMap;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub type WarnFn = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SweepRoot {
    pub path: String,
    pub prune_when_empty: bool,
}

#[derive(Clone)]
pub struct SweepOptions {
    pub roots: Vec<SweepRoot>,
    pub cutoff_ms: i64,
    pub warn: WarnFn,
}

#[derive(Clone, Debug)]
struct ResolvedRoot {
    path: String,
    identity: String,
}

fn warn_safely(warn: &WarnFn, message: String) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| warn(message)));
}

fn node_fs_error(error: &io::Error, syscall: &str, path: &Path) -> String {
    format!(
        "Error: {}",
        hoh_error::node_fs_error_message(
            error,
            syscall,
            Some(path.to_string_lossy().as_ref()),
            None,
        )
    )
}

#[cfg(unix)]
fn is_trusted_directory(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_dir()
        && metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.mode() & 0o022 == 0
}

#[cfg(not(unix))]
fn is_trusted_directory(metadata: &std::fs::Metadata) -> bool {
    metadata.is_dir()
}

#[cfg(unix)]
fn root_identity(_path: &Path, metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!("{}:{}", metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn root_identity(path: &Path, _: &std::fs::Metadata) -> String {
    path.to_string_lossy().to_lowercase()
}

#[cfg(unix)]
async fn has_protected_ancestors(path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let current_uid = rustix::process::geteuid().as_raw();
    let mut child = path.to_path_buf();
    let mut child_metadata = tokio::fs::symlink_metadata(&child).await?;
    loop {
        let Some(parent) = child.parent() else {
            return Ok(true);
        };
        if parent == child {
            return Ok(true);
        }
        let metadata = tokio::fs::symlink_metadata(parent).await?;
        if !metadata.is_dir() {
            return Ok(false);
        }
        let writable_by_others = metadata.mode() & 0o022 != 0;
        let sticky = metadata.mode() & 0o1000 != 0;
        if writable_by_others && !sticky {
            return Ok(false);
        }
        if writable_by_others && child_metadata.uid() != current_uid {
            return Ok(false);
        }
        child = parent.to_path_buf();
        child_metadata = metadata;
    }
}

#[cfg(not(unix))]
async fn has_protected_ancestors(_: &Path) -> io::Result<bool> {
    Ok(true)
}

async fn resolve_root(path: &Path, allow_symlink: bool, warn: &WarnFn) -> Option<ResolvedRoot> {
    let initial = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                warn_safely(
                    warn,
                    format!(
                        "spill-local: failed to inspect root {}: {}",
                        path.display(),
                        node_fs_error(&error, "lstat", path)
                    ),
                );
            }
            return None;
        }
    };
    if initial.file_type().is_symlink() {
        if !allow_symlink {
            return None;
        }
    } else if !is_trusted_directory(&initial) {
        warn_safely(
            warn,
            format!(
                "spill-local: skipped unsafe root {}: expected a directory owned by the current user and not writable by group or others",
                path.display()
            ),
        );
        return None;
    }
    let canonical = match tokio::fs::canonicalize(path).await {
        Ok(path) => path,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                warn_safely(
                    warn,
                    format!(
                        "spill-local: failed to resolve root {}: {}",
                        path.display(),
                        node_fs_error(&error, "realpath", path)
                    ),
                );
            }
            return None;
        }
    };
    let metadata = match tokio::fs::symlink_metadata(&canonical).await {
        Ok(metadata) => metadata,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                warn_safely(
                    warn,
                    format!(
                        "spill-local: failed to resolve root {}: {}",
                        path.display(),
                        node_fs_error(&error, "lstat", &canonical)
                    ),
                );
            }
            return None;
        }
    };
    let protected = match has_protected_ancestors(&canonical).await {
        Ok(protected) => protected,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                warn_safely(
                    warn,
                    format!(
                        "spill-local: failed to inspect ancestors of root {}: {}",
                        canonical.display(),
                        node_fs_error(&error, "lstat", &canonical)
                    ),
                );
            }
            return None;
        }
    };
    if !is_trusted_directory(&metadata) || !protected {
        warn_safely(
            warn,
            format!(
                "spill-local: skipped unsafe root {}: expected a current-user-owned directory with protected write and ancestor permissions",
                canonical.display()
            ),
        );
        return None;
    }
    Some(ResolvedRoot {
        identity: root_identity(&canonical, &metadata),
        path: canonical.to_string_lossy().into_owned(),
    })
}

async fn read_dir_names(path: &Path) -> io::Result<Vec<String>> {
    let mut directory = tokio::fs::read_dir(path).await?;
    let mut names = Vec::new();
    while let Some(entry) = directory.next_entry().await? {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    Ok(names)
}

fn is_session_dir_name(name: &str) -> bool {
    name.len() == 20
        && name.starts_with("session-")
        && name[8..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_default_root_name(name: &str) -> bool {
    name.strip_prefix(DEFAULT_ROOT_PREFIX)
        .is_some_and(|suffix| {
            suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn modified_ms(metadata: &std::fs::Metadata) -> Option<i64> {
    metadata.modified().ok().map(system_time_ms)
}

fn system_time_ms(value: SystemTime) -> i64 {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(error) => {
            let duration = error.duration();
            let milliseconds = duration.as_nanos().saturating_add(999_999) / 1_000_000;
            -i64::try_from(milliseconds).unwrap_or(i64::MAX)
        }
    }
}

async fn unlink_idempotent(path: &Path, warn: &WarnFn) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != io::ErrorKind::NotFound
    {
        warn_safely(
            warn,
            format!(
                "spill-local: failed to delete {}: {}",
                path.display(),
                node_fs_error(&error, "unlink", path)
            ),
        );
    }
}

async fn sweep_session_dir(directory: &Path, cutoff_ms: i64, warn: &WarnFn) -> bool {
    let names = match read_dir_names(directory).await {
        Ok(names) => names,
        Err(error) => {
            warn_safely(
                warn,
                format!(
                    "spill-local: failed to read {}: {}",
                    directory.display(),
                    node_fs_error(&error, "scandir", directory)
                ),
            );
            return false;
        }
    };
    let mut remaining = names.len();
    for name in names {
        let path = directory.join(name);
        let metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                remaining -= 1;
                continue;
            }
            Err(error) => {
                warn_safely(
                    warn,
                    format!(
                        "spill-local: failed to stat {}: {}",
                        path.display(),
                        node_fs_error(&error, "lstat", &path)
                    ),
                );
                continue;
            }
        };
        if !metadata.is_file() || modified_ms(&metadata).is_none_or(|mtime| mtime >= cutoff_ms) {
            continue;
        }
        unlink_idempotent(&path, warn).await;
        remaining -= 1;
    }
    remaining == 0
}

pub async fn sweep_spill_roots(options: SweepOptions) {
    let mut roots = IndexMap::<String, SweepRoot>::new();
    for candidate in options.roots {
        let Some(resolved) = resolve_root(Path::new(&candidate.path), false, &options.warn).await
        else {
            continue;
        };
        let prune_when_empty = roots
            .get(&resolved.identity)
            .is_none_or(|existing| existing.prune_when_empty)
            && candidate.prune_when_empty;
        roots.insert(
            resolved.identity,
            SweepRoot {
                path: resolved.path,
                prune_when_empty,
            },
        );
    }
    for root in roots.into_values() {
        let root_path = PathBuf::from(&root.path);
        let entries = match read_dir_names(&root_path).await {
            Ok(entries) => entries,
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    warn_safely(
                        &options.warn,
                        format!(
                            "spill-local: failed to read root {}: {}",
                            root_path.display(),
                            node_fs_error(&error, "scandir", &root_path)
                        ),
                    );
                }
                continue;
            }
        };
        let mut root_empty = true;
        for name in entries {
            if !is_session_dir_name(&name) {
                root_empty = false;
                continue;
            }
            let directory = root_path.join(name);
            let metadata = match tokio::fs::symlink_metadata(&directory).await {
                Ok(metadata) => metadata,
                Err(error) => {
                    if error.kind() != io::ErrorKind::NotFound {
                        warn_safely(
                            &options.warn,
                            format!(
                                "spill-local: failed to stat {}: {}",
                                directory.display(),
                                node_fs_error(&error, "lstat", &directory)
                            ),
                        );
                    }
                    continue;
                }
            };
            if !is_trusted_directory(&metadata) {
                warn_safely(
                    &options.warn,
                    format!(
                        "spill-local: skipped unsafe session directory {}",
                        directory.display()
                    ),
                );
                root_empty = false;
                continue;
            }
            if !sweep_session_dir(&directory, options.cutoff_ms, &options.warn).await {
                root_empty = false;
                continue;
            }
            if let Err(error) = tokio::fs::remove_dir(&directory).await {
                root_empty = false;
                if !matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) {
                    warn_safely(
                        &options.warn,
                        format!(
                            "spill-local: failed to prune {}: {}",
                            directory.display(),
                            node_fs_error(&error, "rmdir", &directory)
                        ),
                    );
                }
            }
        }
        if root.prune_when_empty
            && root_empty
            && let Err(error) = tokio::fs::remove_dir(&root_path).await
            && !matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            )
        {
            warn_safely(
                &options.warn,
                format!(
                    "spill-local: failed to prune root {}: {}",
                    root_path.display(),
                    node_fs_error(&error, "rmdir", &root_path)
                ),
            );
        }
    }
}

async fn discover_default_root_records(warn: &WarnFn, base: &Path) -> Vec<ResolvedRoot> {
    let entries = match read_dir_names(base).await {
        Ok(entries) => entries,
        Err(error) => {
            warn_safely(
                warn,
                format!(
                    "spill-local: failed to scan {} for default roots: {}",
                    base.display(),
                    node_fs_error(&error, "scandir", base)
                ),
            );
            return vec![];
        }
    };
    let mut roots = Vec::new();
    for name in entries {
        if !is_default_root_name(&name) {
            continue;
        }
        if let Some(root) = resolve_root(&base.join(name), false, warn).await {
            roots.push(root);
        }
    }
    roots
}

fn default_roots_base(base: Option<&Path>) -> PathBuf {
    base.map_or_else(std::env::temp_dir, Path::to_path_buf)
}

pub async fn discover_default_roots(warn: WarnFn, base: Option<&Path>) -> Vec<String> {
    let base = default_roots_base(base);
    discover_default_root_records(&warn, &base)
        .await
        .into_iter()
        .map(|root| root.path)
        .collect()
}

pub async fn gather_sweep_roots(
    active_root: &str,
    warn: WarnFn,
    default_base: Option<&Path>,
) -> Vec<SweepRoot> {
    let base = default_roots_base(default_base);
    let (discovered, active) = tokio::join!(
        discover_default_root_records(&warn, &base),
        resolve_root(Path::new(active_root), true, &warn)
    );
    let mut roots = IndexMap::<String, SweepRoot>::new();
    for root in discovered {
        roots.insert(
            root.identity,
            SweepRoot {
                path: root.path,
                prune_when_empty: true,
            },
        );
    }
    if let Some(active) = active {
        roots.insert(
            active.identity,
            SweepRoot {
                path: active.path,
                prune_when_empty: false,
            },
        );
    }
    roots.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn converts_pre_epoch_file_times_with_floor_millisecond_semantics() {
        assert_eq!(system_time_ms(UNIX_EPOCH - Duration::from_micros(1)), -1);
        assert_eq!(system_time_ms(UNIX_EPOCH - Duration::from_millis(1)), -1);
        assert_eq!(system_time_ms(UNIX_EPOCH + Duration::from_micros(1_999)), 1);
    }

    #[test]
    fn defaults_discovery_to_the_os_temp_directory() {
        assert_eq!(default_roots_base(None), std::env::temp_dir());
    }
}
