//! Host-workspace discovery for `@file` completion. The index contains paths
//! only: selected values remain ordinary prompt text and file contents stay
//! behind the model-facing `read` tool.
//!
//! Port of `src/search.ts`.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use hoh_error::{BoxError, SharedError, node_fs_error_message};
use hoh_file_reference::{FileReferenceCandidate, FileReferenceKind};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_util_workspace_path::node_path;
use tokio::sync::watch;

pub use hoh_file_reference::grammar::{active_at_token, format_file_mention};

/// Default maximum file and directory candidates rendered for one query.
pub const DEFAULT_FILE_SEARCH_MAX_RESULTS: u64 = 20;
/// Default maximum entries retained in one workspace search index.
pub const DEFAULT_FILE_SEARCH_MAX_ENTRIES: u64 = 50_000;
/// Directory basenames omitted from traversal unless the deployment overrides
/// them: version-control and dependency stores plus build-output names that no
/// ecosystem also uses for sources. Generated files carry the basenames of the
/// sources that produced them, so an unfiltered tree both spends the entry
/// budget twice and ranks `dist/x.js` beside `src/x.ts` for every query.
///
/// `lib` is deliberately absent: Ruby gems and many npm packages keep their
/// sources there, and excluding it would make `@` miss those sources entirely
/// and silently. A workspace that builds into `lib` adds it through
/// `excludedDirectories`.
pub const DEFAULT_FILE_SEARCH_EXCLUDED_DIRECTORIES: [&str; 15] = [
    ".git",
    "node_modules",
    "dist",
    "build",
    "out",
    "coverage",
    "target",
    ".next",
    ".nuxt",
    ".turbo",
    ".venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".gradle",
];

/// `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Resolved limits and exclusions for one workspace index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSearchConfig {
    /// Maximum ranked candidates returned for one query.
    pub max_results: u64,
    /// Maximum indexed files and directories.
    pub max_entries: u64,
    /// Directory basenames never traversed or offered.
    pub excluded_directories: Vec<String>,
}

struct RankedPath {
    candidate: FileReferenceCandidate,
    score: i64,
}

type IndexResult = Result<Arc<Vec<FileReferenceCandidate>>, SharedError>;

struct Generation {
    id: u64,
    controller: AbortController,
    result: watch::Receiver<Option<IndexResult>>,
}

/// A completed traversal and the invalidation counter it observed at its start.
struct SettledIndex {
    entries: Arc<Vec<FileReferenceCandidate>>,
    started_at: u64,
}

#[derive(Default)]
struct State {
    settled: Option<SettledIndex>,
    generation: Option<Generation>,
    /// Monotonic invalidation counter; a settled index below it is stale.
    invalidations: u64,
    next_generation: u64,
    disposed: bool,
}

struct Inner {
    root: String,
    max_results: usize,
    max_entries: usize,
    excluded_directories: HashSet<String>,
    state: Mutex<State>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Cancellable, reusable fuzzy index rooted at one agent working directory.
/// Directory-scoped queries list live state; bare fuzzy queries share one
/// bounded traversal. Only the first query of a workspace waits for that
/// traversal — an invalidated index keeps answering while its replacement
/// builds behind the caret.
#[derive(Clone)]
pub struct WorkspaceFileSearch {
    inner: Arc<Inner>,
}

/// Validate the limits of one index (`validateConfig` of the search constructor).
fn validate(config: &FileSearchConfig) -> Result<(), BoxError> {
    if config.max_results == 0 || config.max_results > MAX_SAFE_INTEGER {
        return Err("file search maxResults must be a positive safe integer".into());
    }
    if config.max_entries == 0 || config.max_entries > MAX_SAFE_INTEGER {
        return Err("file search maxEntries must be a positive safe integer".into());
    }
    if config
        .excluded_directories
        .iter()
        .any(|name| name.is_empty() || name.contains('/') || name.contains('\\'))
    {
        return Err(
            "file search excludedDirectories entries must be non-empty directory basenames".into(),
        );
    }
    Ok(())
}

impl WorkspaceFileSearch {
    /// # Errors
    /// A limit that is not a positive safe integer, or an excluded entry that is not a basename.
    pub fn new(root: impl Into<String>, config: FileSearchConfig) -> Result<Self, BoxError> {
        validate(&config)?;
        Ok(Self {
            inner: Arc::new(Inner {
                root: root.into(),
                max_results: usize::try_from(config.max_results).unwrap_or(usize::MAX),
                max_entries: usize::try_from(config.max_entries).unwrap_or(usize::MAX),
                excluded_directories: config.excluded_directories.into_iter().collect(),
                state: Mutex::new(State::default()),
            }),
        })
    }

    /// Return ranked path candidates for the current token.
    ///
    /// `raw_query` is the path text following `@` or `@"`; `signal` cancels this caller's wait
    /// without killing an index shared by a newer query. Returns at most `maxResults`
    /// deterministic candidates.
    pub async fn list(
        &self,
        raw_query: &str,
        signal: &AbortSignal,
    ) -> Result<Vec<FileReferenceCandidate>, BoxError> {
        signal.throw_if_aborted()?;
        if lock(&self.inner.state).disposed {
            return Ok(Vec::new());
        }
        let query = raw_query.replace('\\', "/");
        let slash = query.rfind('/');
        if query.is_empty() || slash.is_some() {
            let (directory, fragment) = match slash {
                None => ("", ""),
                Some(slash) => (&query[..=slash], &query[slash + 1..]),
            };
            return self.list_directory(directory, fragment, signal).await;
        }
        let indexed = self.index_for(signal).await?;
        let visible: Vec<&FileReferenceCandidate> = indexed
            .iter()
            .filter(|candidate| visible_for_global_query(&candidate.path, &query))
            .collect();
        Ok(rank_candidates(visible, &query, self.inner.max_results))
    }

    /// Mark the index stale so a later bare query observes a fresh tree.
    ///
    /// The stale entries are kept and keep answering: a rebuild costs one
    /// traversal of the whole workspace, and putting that in front of the caret
    /// is what a caller invalidating on every tool result would otherwise pay.
    pub fn invalidate(&self) {
        lock(&self.inner.state).invalidations += 1;
    }

    /// Abort traversal and make later queries return no candidates.
    pub fn dispose(&self) {
        let mut state = lock(&self.inner.state);
        if state.disposed {
            return;
        }
        state.disposed = true;
        if let Some(generation) = state.generation.take() {
            generation
                .controller
                .abort_with("file search index disposed");
        }
        state.settled = None;
    }

    /// The entries a bare fuzzy query ranks. Only the first query of a workspace
    /// waits for a traversal; afterwards a stale index answers immediately and
    /// its replacement builds in the background.
    async fn index_for(
        &self,
        signal: &AbortSignal,
    ) -> Result<Arc<Vec<FileReferenceCandidate>>, SharedError> {
        let (settled, stale) = {
            let state = lock(&self.inner.state);
            match &state.settled {
                None => (None, false),
                Some(settled) => (
                    Some(settled.entries.clone()),
                    settled.started_at < state.invalidations,
                ),
            }
        };
        match settled {
            None => wait_for_index(self.ensure_index(), signal).await,
            Some(entries) => {
                if stale {
                    // A background refresh failure is not this caller's error: the stale
                    // entries still answer and `started_at` stays behind, so the next bare
                    // query starts a fresh attempt.
                    drop(self.ensure_index());
                }
                Ok(entries)
            }
        }
    }

    fn ensure_index(&self) -> watch::Receiver<Option<IndexResult>> {
        let mut state = lock(&self.inner.state);
        if let Some(generation) = &state.generation {
            return generation.result.clone();
        }
        let controller = AbortController::new();
        let signal = controller.signal();
        let started_at = state.invalidations;
        let id = state.next_generation;
        state.next_generation += 1;
        let (sender, receiver) = watch::channel(None);
        state.generation = Some(Generation {
            id,
            controller,
            result: receiver.clone(),
        });
        drop(state);

        let inner = self.inner.clone();
        tokio::spawn(async move {
            let scanned = scan_workspace(&inner, &signal).await.map(Arc::new);
            {
                let mut state = lock(&inner.state);
                match &scanned {
                    Ok(entries) => {
                        // Disposal aborts this traversal; the guard covers a scan that finished
                        // its last directory in the instant before the abort landed.
                        if !state.disposed {
                            state.generation = None;
                            state.settled = Some(SettledIndex {
                                entries: entries.clone(),
                                started_at,
                            });
                        }
                    }
                    Err(_) => {
                        if state
                            .generation
                            .as_ref()
                            .is_some_and(|generation| generation.id == id)
                        {
                            state.generation = None;
                        }
                    }
                }
            }
            sender.send_replace(Some(scanned));
        });
        receiver
    }

    async fn list_directory(
        &self,
        display_directory: &str,
        fragment: &str,
        signal: &AbortSignal,
    ) -> Result<Vec<FileReferenceCandidate>, BoxError> {
        let inner = &self.inner;
        if display_directory
            .split('/')
            .any(|segment| inner.excluded_directories.contains(segment))
        {
            return Ok(Vec::new());
        }
        let Some(absolute) =
            resolve_display_directory(&inner.root, display_directory, signal).await?
        else {
            return Ok(Vec::new());
        };
        let entries = read_directory(&absolute, signal).await?;
        let mut candidates = Vec::new();
        for entry in entries {
            if entry.name.starts_with('.') && !fragment.starts_with('.') {
                continue;
            }
            if entry.is_directory {
                if inner.excluded_directories.contains(&entry.name) {
                    continue;
                }
                candidates.push(FileReferenceCandidate {
                    path: format!("{display_directory}{}", entry.name),
                    kind: FileReferenceKind::Directory,
                });
            } else if entry.is_file {
                candidates.push(FileReferenceCandidate {
                    path: format!("{display_directory}{}", entry.name),
                    kind: FileReferenceKind::File,
                });
            }
        }
        Ok(rank_candidates(
            candidates.iter().collect(),
            fragment,
            inner.max_results,
        ))
    }
}

async fn scan_workspace(
    inner: &Inner,
    signal: &AbortSignal,
) -> Result<Vec<FileReferenceCandidate>, SharedError> {
    let mut indexed: Vec<FileReferenceCandidate> = Vec::new();
    let mut directories: Vec<(String, String)> = vec![(inner.root.clone(), String::new())];
    let mut cursor = 0;
    while cursor < directories.len() && indexed.len() < inner.max_entries {
        signal.throw_if_aborted()?;
        let (absolute, relative) = directories[cursor].clone();
        // The root is not a subtree: an unreadable branch costs its own
        // candidates, but an unreadable root means the traversal learned
        // nothing. Letting that settle would publish an empty index over
        // entries that are still good and leave no invalidation to retry from.
        let entries = if cursor == 0 {
            read_workspace_root(&absolute, signal).await?
        } else {
            read_directory(&absolute, signal).await?
        };
        for entry in entries {
            signal.throw_if_aborted()?;
            let path = if relative.is_empty() {
                entry.name.clone()
            } else {
                format!("{relative}/{}", entry.name)
            };
            if entry.is_directory {
                if inner.excluded_directories.contains(&entry.name) {
                    continue;
                }
                indexed.push(FileReferenceCandidate {
                    path: path.clone(),
                    kind: FileReferenceKind::Directory,
                });
                directories.push((node_path::join(&[&absolute, &entry.name]), path));
            } else if entry.is_file {
                indexed.push(FileReferenceCandidate {
                    path,
                    kind: FileReferenceKind::File,
                });
            }
            if indexed.len() >= inner.max_entries {
                break;
            }
        }
        cursor += 1;
    }
    Ok(indexed)
}

async fn resolve_display_directory(
    root: &str,
    display_directory: &str,
    signal: &AbortSignal,
) -> Result<Option<String>, BoxError> {
    let resolved_root = node_path::resolve(root, &[]);
    let target = if display_directory.is_empty() {
        "."
    } else {
        display_directory
    };
    let absolute = node_path::resolve(&resolved_root, &[target]);
    let from_root = node_path::relative(&resolved_root, &resolved_root, &absolute);
    if from_root == ".." || from_root.starts_with("../") {
        return Ok(None);
    }
    if node_path::is_absolute(&from_root) {
        return Ok(None);
    }
    let mut current = resolved_root;
    for segment in from_root.split('/').filter(|segment| !segment.is_empty()) {
        signal.throw_if_aborted()?;
        current = node_path::join(&[&current, segment]);
        match tokio::fs::symlink_metadata(Path::new(&current)).await {
            Ok(status) => {
                signal.throw_if_aborted()?;
                if status.file_type().is_symlink() || !status.is_dir() {
                    return Ok(None);
                }
            }
            Err(_) => {
                signal.throw_if_aborted()?;
                return Ok(None);
            }
        }
    }
    Ok(Some(absolute))
}

/// One directory entry as `readdir` reports it (`Dirent`): symbolic links are neither files
/// nor directories.
struct Entry {
    name: String,
    is_directory: bool,
    is_file: bool,
}

async fn read_entries(absolute: &str) -> std::io::Result<Vec<Entry>> {
    let mut reader = tokio::fs::read_dir(Path::new(absolute)).await?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        let file_type = entry.file_type().await.ok();
        entries.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_directory: file_type.is_some_and(|kind| kind.is_dir()),
            is_file: file_type.is_some_and(|kind| kind.is_file()),
        });
    }
    entries.sort_by(|left, right| compare_text(&left.name, &right.name));
    Ok(entries)
}

async fn read_workspace_root(
    absolute: &str,
    signal: &AbortSignal,
) -> Result<Vec<Entry>, SharedError> {
    signal.throw_if_aborted()?;
    let entries = read_entries(absolute).await.map_err(|error| {
        SharedError::new(node_fs_error_message(
            &error,
            "scandir",
            Some(absolute),
            None,
        ))
    })?;
    signal.throw_if_aborted()?;
    Ok(entries)
}

async fn read_directory(absolute: &str, signal: &AbortSignal) -> Result<Vec<Entry>, SharedError> {
    signal.throw_if_aborted()?;
    match read_entries(absolute).await {
        Ok(entries) => {
            signal.throw_if_aborted()?;
            Ok(entries)
        }
        Err(_) => {
            signal.throw_if_aborted()?;
            // An unreadable/missing subtree contributes no candidates; other readable
            // branches remain useful and autocomplete is advisory.
            Ok(Vec::new())
        }
    }
}

fn visible_for_global_query(path: &str, query: &str) -> bool {
    if query.starts_with('.') || query.contains("/.") {
        return true;
    }
    !path.split('/').any(|segment| segment.starts_with('.'))
}

fn rank_candidates(
    candidates: Vec<&FileReferenceCandidate>,
    query: &str,
    limit: usize,
) -> Vec<FileReferenceCandidate> {
    let mut ranked: Vec<RankedPath> = candidates
        .into_iter()
        .filter_map(|candidate| {
            score_candidate(candidate, query).map(|score| RankedPath {
                candidate: candidate.clone(),
                score,
            })
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| kind_rank(left.candidate.kind).cmp(&kind_rank(right.candidate.kind)))
            .then_with(|| {
                if query.is_empty() {
                    Ordering::Equal
                } else {
                    utf16_length(&left.candidate.path).cmp(&utf16_length(&right.candidate.path))
                }
            })
            .then_with(|| compare_text(&left.candidate.path, &right.candidate.path))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|entry| entry.candidate)
        .collect()
}

fn score_candidate(candidate: &FileReferenceCandidate, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let path = candidate.path.to_lowercase();
    let name = &path[path.rfind('/').map_or(0, |slash| slash + 1)..];
    let needle = query.to_lowercase();
    let directory_bonus = if candidate.kind == FileReferenceKind::Directory {
        25
    } else {
        0
    };
    if name == needle {
        return Some(1_000 + directory_bonus);
    }
    if name.starts_with(&needle) {
        return Some(900 + directory_bonus);
    }
    if name.contains(&needle) {
        return Some(700 + directory_bonus);
    }
    if path.contains(&needle) {
        return Some(500 + directory_bonus);
    }
    subsequence_score(&path, &needle).map(|subsequence| 300 + subsequence + directory_bonus)
}

/// `indexOf` runs over UTF-16 code units, so the gap is counted in them.
fn subsequence_score(target: &str, query: &str) -> Option<i64> {
    let target: Vec<u16> = target.encode_utf16().collect();
    let mut target_index = 0;
    let mut gap: i64 = 0;
    for character in query.chars() {
        let mut buffer = [0u16; 2];
        let needle: &[u16] = character.encode_utf16(&mut buffer);
        let found = target
            .get(target_index..)?
            .windows(needle.len())
            .position(|window| window == needle)?
            + target_index;
        gap += i64::try_from(found - target_index).unwrap_or(i64::MAX);
        target_index = found + 1;
    }
    Some((100 - gap).max(0))
}

fn kind_rank(kind: FileReferenceKind) -> u8 {
    match kind {
        FileReferenceKind::Directory => 0,
        FileReferenceKind::File => 1,
    }
}

/// JavaScript's `<` on strings: UTF-16 code-unit order.
fn compare_text(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn utf16_length(text: &str) -> usize {
    text.encode_utf16().count()
}

async fn wait_for_index(
    mut result: watch::Receiver<Option<IndexResult>>,
    signal: &AbortSignal,
) -> Result<Arc<Vec<FileReferenceCandidate>>, SharedError> {
    signal.throw_if_aborted()?;
    let ready = async {
        match result.wait_for(Option::is_some).await {
            Ok(settled) => settled
                .clone()
                .unwrap_or_else(|| Err(SharedError::new("file search index failed"))),
            Err(_) => Err(SharedError::new("file search index failed")),
        }
    };
    tokio::select! {
        biased;
        reason = signal.cancelled() => Err(reason),
        outcome = ready => outcome,
    }
}
