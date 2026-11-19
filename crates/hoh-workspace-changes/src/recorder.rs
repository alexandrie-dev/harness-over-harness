//! Per-Session serialized snapshots, captures, summaries, and lazy comparisons.

use crate::{
    capture::{Capture, capture_file, same_capture},
    compare::compare_text,
    git::{
        GitRunner, GitWorkspace, blob_text, diff_trees, gitlink_paths, ignored_paths,
        locate_git_workspace, snapshot_tree, tree_blob,
    },
    paths::{
        canonical_path, compare_display, display_path_of, durable_path_of, is_inside,
        is_temporary_path, temporary_roots, to_posix,
    },
    types::{WorkspaceChangedFile, WorkspaceChangesSummary, WorkspaceFileDiff, WorkspaceSnapshot},
};
use hoh_error::BoxError;
use hoh_session::{Session, SessionEvent};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot};

pub type GitResolver = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Option<GitRunner>, BoxError>> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct RecorderEnvironment {
    pub git: GitResolver,
    pub temp_root: PathBuf,
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub diff_timeout_ms: u64,
    pub warn: Arc<dyn Fn(String) + Send + Sync>,
}

#[derive(Clone)]
struct Repository {
    git: GitRunner,
    workspace: GitWorkspace,
}

#[derive(Clone)]
struct Baseline {
    repository: Repository,
    tree: String,
}

enum BaselineState {
    None,
    Ready(Box<Baseline>),
    Failed,
}

struct TurnState {
    turn: u64,
    baseline: BaselineState,
    captures: BTreeMap<PathBuf, Capture>,
    last_tool_result_seq: Option<u64>,
    attempted_after_seq: Option<u64>,
    recorded_after_seq: Option<u64>,
}

impl TurnState {
    fn fresh(turn: u64) -> Self {
        Self {
            turn,
            baseline: BaselineState::None,
            captures: BTreeMap::new(),
            last_tool_result_seq: None,
            attempted_after_seq: None,
            recorded_after_seq: None,
        }
    }
}

#[derive(Clone)]
enum ContentSource {
    Snapshot {
        repository: Box<Repository>,
        tree: String,
        path: String,
    },
    Absent,
    File(PathBuf),
}

#[derive(Clone)]
enum FileSources {
    Refusal(Refusal),
    Compare {
        before: Box<ContentSource>,
        after: Box<ContentSource>,
    },
}

#[derive(Clone, Copy)]
enum Refusal {
    Binary,
    Oversized,
}

#[derive(Clone)]
struct TurnRecord {
    summary: WorkspaceChangesSummary,
    sources: Vec<FileSources>,
}

struct Listed {
    file: WorkspaceChangedFile,
    sources: FileSources,
}

#[derive(Clone)]
struct Paths {
    cwd: PathBuf,
    home: Option<PathBuf>,
    temporary_roots: Vec<PathBuf>,
}

enum Work {
    Start(u64),
    Capture(PathBuf),
    Observe {
        turn: u64,
        seq: u64,
    },
    Stopping {
        turn: u64,
        done: oneshot::Sender<()>,
    },
    End(u64),
    Barrier(oneshot::Sender<()>),
    Dispose(oneshot::Sender<()>),
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct TurnRecorder {
    sender: mpsc::UnboundedSender<Work>,
    records: Arc<Mutex<HashMap<u64, TurnRecord>>>,
    lifetime: Arc<AbortController>,
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
    max_file_bytes: usize,
    diff_timeout_ms: u64,
}

impl TurnRecorder {
    pub fn new(session: Session, cwd: String, environment: RecorderEnvironment) -> Arc<Self> {
        let max_file_bytes = environment.max_file_bytes;
        let diff_timeout_ms = environment.diff_timeout_ms;
        let (sender, receiver) = mpsc::unbounded_channel();
        let records = Arc::new(Mutex::new(HashMap::new()));
        let lifetime = Arc::new(AbortController::new());
        let worker = Worker {
            session,
            cwd,
            environment,
            records: records.clone(),
            lifetime: lifetime.clone(),
            state: TurnState::fresh(0),
            paths: None,
            repository: None,
            scratch: None,
        };
        let join = tokio::spawn(worker.run(receiver));
        Arc::new(Self {
            sender,
            records,
            lifetime,
            join: Mutex::new(Some(join)),
            max_file_bytes,
            diff_timeout_ms,
        })
    }

    pub fn start(&self, turn: u64) {
        let _ = self.sender.send(Work::Start(turn));
    }

    pub fn capture(&self, path: PathBuf) {
        let _ = self.sender.send(Work::Capture(path));
    }

    pub fn observe(&self, event: &SessionEvent) {
        if let Some(turn) = event.data().get("turn").and_then(serde_json::Value::as_u64) {
            let _ = self.sender.send(Work::Observe {
                turn,
                seq: event.seq().get(),
            });
        }
    }

    pub async fn stopping(&self, turn: u64) {
        let (done, wait) = oneshot::channel();
        if self.sender.send(Work::Stopping { turn, done }).is_ok() {
            let _ = wait.await;
        }
    }

    pub fn end(&self, turn: u64) {
        let _ = self.sender.send(Work::End(turn));
    }

    pub async fn settled(&self) {
        let (done, wait) = oneshot::channel();
        if self.sender.send(Work::Barrier(done)).is_ok() {
            let _ = wait.await;
        }
    }

    pub fn summary(&self, seq: u64) -> Option<WorkspaceChangesSummary> {
        lock(&self.records)
            .get(&seq)
            .map(|record| record.summary.clone())
    }

    pub async fn diff(
        &self,
        seq: u64,
        index: usize,
        signal: AbortSignal,
    ) -> Result<Option<WorkspaceFileDiff>, BoxError> {
        self.diff_inner(seq, index, signal, false).await
    }

    async fn diff_inner(
        &self,
        seq: u64,
        index: usize,
        signal: AbortSignal,
        abort_lifetime_after_read: bool,
    ) -> Result<Option<WorkspaceFileDiff>, BoxError> {
        let Some((file, sources)) = lock(&self.records).get(&seq).and_then(|record| {
            Some((
                record.summary.files.get(index)?.clone(),
                record.sources.get(index)?.clone(),
            ))
        }) else {
            return Ok(None);
        };
        match sources {
            FileSources::Refusal(Refusal::Binary) => Ok(Some(WorkspaceFileDiff::Binary {
                path: file.path,
                display: file.display,
            })),
            FileSources::Refusal(Refusal::Oversized) => Ok(Some(WorkspaceFileDiff::Oversized {
                path: file.path,
                display: file.display,
            })),
            FileSources::Compare { before, after } => {
                let combined = AbortSignal::any([signal, self.lifetime.signal()]);
                let read = async {
                    let before = read_side(*before, combined.clone(), self.max_file_bytes).await?;
                    let after = read_side(*after, combined.clone(), self.max_file_bytes).await?;
                    Ok::<_, BoxError>((before, after))
                };
                match read.await {
                    Ok((before, after)) => {
                        #[cfg(test)]
                        if abort_lifetime_after_read {
                            self.lifetime.abort();
                        }
                        #[cfg(not(test))]
                        let _ = abort_lifetime_after_read;
                        if let Err(error) = combined.throw_if_aborted() {
                            if self.lifetime.signal().aborted() {
                                return Ok(None);
                            }
                            return Err(Box::new(error));
                        }
                        if matches!(before, SideRead::Oversized)
                            || matches!(after, SideRead::Oversized)
                        {
                            return Ok(Some(WorkspaceFileDiff::Oversized {
                                path: file.path,
                                display: file.display,
                            }));
                        }
                        let before_text = before.text().map(str::to_owned);
                        let after_text = after.text().map(str::to_owned);
                        let timeout_ms = self.diff_timeout_ms;
                        let comparison = tokio::task::spawn_blocking(move || {
                            compare_text(before_text.as_deref(), after_text.as_deref(), timeout_ms)
                        })
                        .await
                        .map_err(|error| Box::new(error) as BoxError)?;
                        Ok(Some(WorkspaceFileDiff::Text {
                            path: file.path,
                            display: file.display,
                            before: !matches!(before, SideRead::Absent),
                            after: !matches!(after, SideRead::Absent),
                            hunks: comparison.hunks,
                            coarse: comparison.coarse,
                        }))
                    }
                    Err(_) if self.lifetime.signal().aborted() => Ok(None),
                    Err(error) => Err(error),
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn diff_aborting_after_read_for_test(
        &self,
        seq: u64,
        index: usize,
        signal: AbortSignal,
    ) -> Result<Option<WorkspaceFileDiff>, BoxError> {
        self.diff_inner(seq, index, signal, true).await
    }

    pub async fn dispose(&self) {
        self.lifetime.abort();
        lock(&self.records).clear();
        let (done, wait) = oneshot::channel();
        if self.sender.send(Work::Dispose(done)).is_ok() {
            let _ = wait.await;
        }
        let join = lock(&self.join).take();
        if let Some(join) = join {
            let _ = join.await;
        }
    }
}

/// Construct a recorder and register its immutable line-diff bound.
pub fn create_recorder(
    session: Session,
    cwd: String,
    environment: RecorderEnvironment,
) -> Arc<TurnRecorder> {
    TurnRecorder::new(session, cwd, environment)
}

async fn abortable_read(path: &Path, signal: AbortSignal) -> Result<String, BoxError> {
    signal.throw_if_aborted()?;
    tokio::select! {
        result = tokio::fs::read(path) => Ok(String::from_utf8_lossy(&result?).into_owned()),
        reason = signal.cancelled() => Err(Box::new(reason)),
    }
}

enum SideRead {
    Absent,
    Text(String),
    Oversized,
}

impl SideRead {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Absent | Self::Oversized => None,
        }
    }
}

async fn read_side(
    source: ContentSource,
    signal: AbortSignal,
    max_file_bytes: usize,
) -> Result<SideRead, BoxError> {
    match source {
        ContentSource::Absent => Ok(SideRead::Absent),
        ContentSource::File(path) => abortable_read(&path, signal).await.map(SideRead::Text),
        ContentSource::Snapshot {
            repository,
            tree,
            path,
        } => {
            let Some(blob) = tree_blob(
                &repository.git,
                &repository.workspace,
                &tree,
                &path,
                signal.clone(),
            )
            .await?
            else {
                return Ok(SideRead::Absent);
            };
            if blob.size > max_file_bytes as u64 {
                return Ok(SideRead::Oversized);
            }
            Ok(SideRead::Text(
                blob_text(
                    &repository.git,
                    &repository.workspace,
                    &blob.oid,
                    max_file_bytes as u64,
                    signal,
                )
                .await?,
            ))
        }
    }
}

struct Worker {
    session: Session,
    cwd: String,
    environment: RecorderEnvironment,
    records: Arc<Mutex<HashMap<u64, TurnRecord>>>,
    lifetime: Arc<AbortController>,
    state: TurnState,
    paths: Option<Paths>,
    repository: Option<Repository>,
    scratch: Option<PathBuf>,
}

impl Worker {
    async fn run(mut self, mut receiver: mpsc::UnboundedReceiver<Work>) {
        while let Some(work) = receiver.recv().await {
            if self.lifetime.signal().aborted() {
                match work {
                    Work::Stopping { done, .. } | Work::Barrier(done) => {
                        let _ = done.send(());
                    }
                    Work::Dispose(done) => {
                        if let Some(scratch) = self.scratch.take()
                            && let Err(error) = tokio::fs::remove_dir_all(&scratch).await
                        {
                            (self.environment.warn)(format!(
                                "workspace-changes: failed to remove {}: {error}",
                                scratch.display()
                            ));
                        }
                        let _ = done.send(());
                        break;
                    }
                    Work::Start(_) | Work::Capture(_) | Work::Observe { .. } | Work::End(_) => {}
                }
                continue;
            }
            match work {
                Work::Start(turn) => {
                    self.state = TurnState::fresh(turn);
                    if let Err(error) = self.start().await {
                        self.state.baseline = BaselineState::Failed;
                        self.warn(error);
                    }
                }
                Work::Capture(path) => {
                    if let Err(error) = self.capture(path).await {
                        self.warn(error);
                    }
                }
                Work::Observe { turn, seq } => {
                    if turn == self.state.turn {
                        self.state.last_tool_result_seq = Some(seq);
                    }
                }
                Work::Stopping { turn, done } => {
                    if turn == self.state.turn
                        && let Err(error) = self.record().await
                    {
                        self.warn(error);
                    }
                    let _ = done.send(());
                }
                Work::End(turn) => {
                    let should_record = turn == self.state.turn
                        && self.state.last_tool_result_seq.is_some_and(|last| {
                            self.state
                                .attempted_after_seq
                                .is_none_or(|attempted| attempted < last)
                        });
                    if should_record && let Err(error) = self.record().await {
                        self.warn(error);
                    }
                }
                Work::Barrier(done) => {
                    let _ = done.send(());
                }
                Work::Dispose(done) => {
                    if let Some(scratch) = self.scratch.take()
                        && let Err(error) = tokio::fs::remove_dir_all(&scratch).await
                    {
                        (self.environment.warn)(format!(
                            "workspace-changes: failed to remove {}: {error}",
                            scratch.display()
                        ));
                    }
                    let _ = done.send(());
                    break;
                }
            }
        }
    }

    fn warn(&self, error: BoxError) {
        if !self.lifetime.signal().aborted() {
            (self.environment.warn)(format!("workspace-changes: {error}"));
        }
    }

    async fn start(&mut self) -> Result<(), BoxError> {
        if self.paths.is_none() {
            let cwd = tokio::fs::canonicalize(&self.cwd).await?;
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| async move { canonical_path(&path).await });
            let home = match home {
                Some(home) => Some(home.await),
                None => None,
            };
            self.paths = Some(Paths {
                cwd,
                home,
                temporary_roots: temporary_roots(&[PathBuf::from("/tmp"), std::env::temp_dir()])
                    .await,
            });
        }
        let cwd = self.paths.as_ref().expect("initialized paths").cwd.clone();
        let Some(repository) = self.locate(&cwd).await? else {
            return Ok(());
        };
        let tree = snapshot_tree(
            &repository.git,
            &repository.workspace,
            self.lifetime.signal(),
        )
        .await?;
        self.state.baseline = BaselineState::Ready(Box::new(Baseline { repository, tree }));
        Ok(())
    }

    async fn capture(&mut self, path: PathBuf) -> Result<(), BoxError> {
        if self.paths.is_none() {
            return Ok(());
        }
        let absolute = canonical_path(&path).await;
        if self.state.captures.contains_key(&absolute) {
            return Ok(());
        }
        let captures = self.scratch_dir().await?.join("captures");
        if let Some(capture) =
            capture_file(&absolute, &captures, self.environment.max_file_bytes).await?
        {
            self.state.captures.insert(absolute, capture);
        }
        Ok(())
    }

    async fn locate(&mut self, cwd: &Path) -> Result<Option<Repository>, BoxError> {
        if let Some(repository) = &self.repository {
            return Ok(Some(repository.clone()));
        }
        let Some(git) = (self.environment.git)().await? else {
            return Ok(None);
        };
        let temp_root = self.environment.temp_root.clone();
        let current = self.scratch.clone();
        let workspace = locate_git_workspace(
            &git,
            cwd,
            || async move {
                if let Some(path) = current {
                    Ok(path)
                } else {
                    unique_scratch(&temp_root).await.map_err(Into::into)
                }
            },
            self.lifetime.signal(),
        )
        .await?;
        let Some(workspace) = workspace else {
            return Ok(None);
        };
        self.scratch = Some(workspace.scratch.clone());
        let repository = Repository { git, workspace };
        self.repository = Some(repository.clone());
        Ok(Some(repository))
    }

    async fn scratch_dir(&mut self) -> Result<PathBuf, BoxError> {
        if let Some(path) = &self.scratch {
            return Ok(path.clone());
        }
        let path = unique_scratch(&self.environment.temp_root).await?;
        self.scratch = Some(path.clone());
        Ok(path)
    }

    async fn record(&mut self) -> Result<(), BoxError> {
        let Some(paths) = self.paths.clone() else {
            return Ok(());
        };
        if matches!(self.state.baseline, BaselineState::Failed)
            || self.state.last_tool_result_seq.is_none()
        {
            return Ok(());
        }
        self.state.attempted_after_seq = self.state.last_tool_result_seq;
        let baseline = match &self.state.baseline {
            BaselineState::Ready(baseline) => Some(baseline.clone()),
            BaselineState::None => None,
            BaselineState::Failed => return Ok(()),
        };
        let root = baseline.as_ref().map_or_else(
            || paths.cwd.clone(),
            |baseline| baseline.repository.workspace.root.clone(),
        );
        let mut listed: BTreeMap<PathBuf, Listed> = BTreeMap::new();
        let mut snapshot = None;
        if let Some(baseline) = &baseline {
            let after = snapshot_tree(
                &baseline.repository.git,
                &baseline.repository.workspace,
                self.lifetime.signal(),
            )
            .await?;
            snapshot = Some(WorkspaceSnapshot {
                before: baseline.tree.clone(),
                after: after.clone(),
            });
            for entry in diff_trees(
                &baseline.repository.git,
                &baseline.repository.workspace,
                &baseline.tree,
                &after,
                self.lifetime.signal(),
            )
            .await?
            {
                let absolute = root.join(&entry.path);
                let sources = if entry.binary {
                    FileSources::Refusal(Refusal::Binary)
                } else {
                    FileSources::Compare {
                        before: Box::new(ContentSource::Snapshot {
                            repository: Box::new(baseline.repository.clone()),
                            tree: baseline.tree.clone(),
                            path: entry.old_path.clone().unwrap_or_else(|| entry.path.clone()),
                        }),
                        after: Box::new(ContentSource::Snapshot {
                            repository: Box::new(baseline.repository.clone()),
                            tree: after.clone(),
                            path: entry.path.clone(),
                        }),
                    }
                };
                listed.insert(
                    absolute.clone(),
                    Listed {
                        file: changed_file(
                            &paths,
                            &root,
                            &absolute,
                            entry.added,
                            entry.deleted,
                            entry.binary,
                            false,
                        ),
                        sources,
                    },
                );
            }
        }
        let captured: Vec<PathBuf> = self
            .state
            .captures
            .keys()
            .filter(|path| !listed.contains_key(*path))
            .cloned()
            .collect();
        let mut in_workspace: Vec<PathBuf> = captured
            .iter()
            .filter(|path| is_inside(&root, path))
            .cloned()
            .collect();
        if let Some(baseline) = &baseline
            && !in_workspace.is_empty()
        {
            let links = gitlink_paths(
                &baseline.repository.git,
                &baseline.repository.workspace,
                self.lifetime.signal(),
            )
            .await?;
            in_workspace.retain(|absolute| {
                !links
                    .iter()
                    .any(|link| is_inside(&root.join(link), absolute))
            });
        }
        let relative = |absolute: &Path| {
            to_posix(
                absolute
                    .strip_prefix(&root)
                    .expect("path classified inside work tree"),
            )
        };
        let uncovered: HashSet<String> = if let Some(baseline) = &baseline {
            let candidates: Vec<String> = in_workspace.iter().map(|path| relative(path)).collect();
            ignored_paths(
                &baseline.repository.git,
                &baseline.repository.workspace,
                &candidates,
                self.lifetime.signal(),
            )
            .await?
        } else {
            in_workspace.iter().map(|path| relative(path)).collect()
        };
        for absolute in captured {
            let include = if is_inside(&root, &absolute) {
                uncovered.contains(&relative(&absolute))
            } else {
                !is_temporary_path(&absolute, &paths.temporary_roots)
            };
            if !include {
                continue;
            }
            let before = self
                .state
                .captures
                .get(&absolute)
                .expect("captured key")
                .clone();
            let captures = self.scratch_dir().await?.join("captures");
            let Some(after) =
                capture_file(&absolute, &captures, self.environment.max_file_bytes).await?
            else {
                continue;
            };
            if same_capture(&before, &after) {
                continue;
            }
            listed.insert(
                absolute.clone(),
                compared(
                    &paths,
                    &root,
                    &absolute,
                    before,
                    after,
                    self.environment.diff_timeout_ms,
                )
                .await?,
            );
        }
        let mut sorted: Vec<Listed> = listed.into_values().collect();
        sorted.sort_by(|left, right| compare_display(&left.file.display, &right.file.display));
        if sorted.is_empty() && self.state.recorded_after_seq.is_none() {
            return Ok(());
        }
        let total = sorted.len() as u64;
        let added = sorted.iter().map(|entry| entry.file.added).sum();
        let deleted = sorted.iter().map(|entry| entry.file.deleted).sum();
        let event = self.session.append(
            "workspace/changes",
            json!({ "turn": self.state.turn }),
            None,
        )?;
        sorted.truncate(self.environment.max_files);
        lock(&self.records).insert(
            event.seq().get(),
            TurnRecord {
                summary: WorkspaceChangesSummary {
                    turn: self.state.turn,
                    cwd: self.cwd.clone(),
                    files: sorted.iter().map(|entry| entry.file.clone()).collect(),
                    total,
                    added,
                    deleted,
                    snapshot,
                },
                sources: sorted.into_iter().map(|entry| entry.sources).collect(),
            },
        );
        self.state.recorded_after_seq = Some(event.seq().get());
        Ok(())
    }
}

static SCRATCH_NUMBER: AtomicU64 = AtomicU64::new(1);

async fn unique_scratch(parent: &Path) -> std::io::Result<PathBuf> {
    for _ in 0..1_000 {
        let number = SCRATCH_NUMBER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            "dsh-workspace-changes-{}-{number}",
            std::process::id()
        ));
        let mut builder = tokio::fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        match builder.create(&path).await {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate workspace-changes scratch directory",
    ))
}

fn capture_source(capture: Capture) -> ContentSource {
    match capture {
        Capture::Absent => ContentSource::Absent,
        Capture::File { file, .. } => ContentSource::File(file),
        Capture::Oversized => unreachable!("oversized capture is refused before source creation"),
    }
}

async fn capture_text(capture: &Capture) -> Result<Option<String>, BoxError> {
    match capture {
        Capture::Absent => Ok(None),
        Capture::File { file, .. } => Ok(Some(
            String::from_utf8_lossy(&tokio::fs::read(file).await?).into_owned(),
        )),
        Capture::Oversized => unreachable!("oversized capture is refused before text comparison"),
    }
}

async fn compared(
    paths: &Paths,
    root: &Path,
    absolute: &Path,
    before: Capture,
    after: Capture,
    timeout_ms: u64,
) -> Result<Listed, BoxError> {
    let listed = |added, deleted, binary, oversized, sources| Listed {
        file: changed_file(paths, root, absolute, added, deleted, binary, oversized),
        sources,
    };
    if matches!(before, Capture::Oversized) || matches!(after, Capture::Oversized) {
        return Ok(listed(
            0,
            0,
            false,
            true,
            FileSources::Refusal(Refusal::Oversized),
        ));
    }
    let binary = matches!(before, Capture::File { binary: true, .. })
        || matches!(after, Capture::File { binary: true, .. });
    if binary {
        return Ok(listed(
            0,
            0,
            true,
            false,
            FileSources::Refusal(Refusal::Binary),
        ));
    }
    let before_text = capture_text(&before).await?;
    let after_text = capture_text(&after).await?;
    let comparison = tokio::task::spawn_blocking(move || {
        compare_text(before_text.as_deref(), after_text.as_deref(), timeout_ms)
    })
    .await
    .map_err(|error| Box::new(error) as BoxError)?;
    Ok(listed(
        comparison.added,
        comparison.deleted,
        false,
        false,
        FileSources::Compare {
            before: Box::new(capture_source(before)),
            after: Box::new(capture_source(after)),
        },
    ))
}

fn changed_file(
    paths: &Paths,
    root: &Path,
    absolute: &Path,
    added: u64,
    deleted: u64,
    binary: bool,
    oversized: bool,
) -> WorkspaceChangedFile {
    WorkspaceChangedFile {
        path: durable_path_of(absolute, &paths.cwd),
        display: display_path_of(absolute, &paths.cwd, root, paths.home.as_deref()),
        added,
        deleted,
        binary,
        oversized,
    }
}
