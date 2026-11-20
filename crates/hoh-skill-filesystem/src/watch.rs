use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_skill::SkillProviderControl;
use indexmap::IndexMap;
use notify::{
    Config, Event, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind, RemoveKind},
};

use crate::{ResolvedWatchConfig, SkillRoot, is_potential_skill_path};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Invalidator {
    runtime: tokio::runtime::Handle,
    control: SkillProviderControl,
    pending: Mutex<Option<tokio::task::JoinHandle<()>>>,
    closing: AtomicBool,
    stability_threshold: Duration,
}

impl Invalidator {
    fn queue(self: &Arc<Self>) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let this = self.clone();
        let task = self.runtime.spawn(async move {
            tokio::time::sleep(this.stability_threshold).await;
            if !this.closing.load(Ordering::Acquire) {
                this.control.invalidate();
            }
        });
        let mut pending = lock(&self.pending);
        if self.closing.load(Ordering::Acquire) {
            task.abort();
            return;
        }
        if let Some(previous) = pending.replace(task) {
            previous.abort();
        }
    }

    fn close(&self) {
        self.closing.store(true, Ordering::Release);
        if let Some(task) = lock(&self.pending).take() {
            task.abort();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WatchMode {
    Root {
        anchor: PathBuf,
        filter_root: PathBuf,
    },
    Ancestor {
        next_path: PathBuf,
        stamp: Option<PathStamp>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PathStamp {
    directory: bool,
    len: u64,
    modified: Option<SystemTime>,
}

enum WatchHandle {
    Native(Box<dyn Watcher + Send>),
    Ancestor {
        stop: Arc<AtomicBool>,
        task: tokio::task::JoinHandle<()>,
    },
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        match self {
            Self::Native(watcher) => {
                let _ = watcher;
            }
            Self::Ancestor { stop, task } => {
                stop.store(true, Ordering::Release);
                task.abort();
            }
        }
    }
}

struct RootWatch {
    owners: HashSet<String>,
    mode: WatchMode,
    unhealthy: Arc<AtomicBool>,
    _handle: WatchHandle,
}

#[derive(Default)]
struct State {
    roots: HashMap<PathBuf, RootWatch>,
    host_roots: HashMap<PathBuf, SkillRoot>,
    projects: IndexMap<PathBuf, Vec<PathBuf>>,
}

pub struct WatchManager {
    state: Arc<Mutex<State>>,
    config: ResolvedWatchConfig,
    invalidator: Arc<Invalidator>,
    lifecycle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    ctx: Context,
}

impl WatchManager {
    pub fn new(ctx: Context, control: SkillProviderControl, config: ResolvedWatchConfig) -> Self {
        let runtime = tokio::runtime::Handle::current();
        let signal = control.signal.clone();
        let state = Arc::new(Mutex::new(State::default()));
        let invalidator = Arc::new(Invalidator {
            runtime: runtime.clone(),
            control,
            pending: Mutex::new(None),
            closing: AtomicBool::new(false),
            stability_threshold: Duration::from_millis(config.stability_threshold_ms),
        });
        let lifecycle_state = state.clone();
        let lifecycle_invalidator = invalidator.clone();
        let lifecycle = runtime.spawn(async move {
            let _ = signal.cancelled().await;
            lifecycle_invalidator.close();
            let mut state = lock(&lifecycle_state);
            state.projects.clear();
            state.roots.clear();
            state.host_roots.clear();
        });
        Self {
            state,
            config,
            invalidator,
            lifecycle: Mutex::new(Some(lifecycle)),
            ctx,
        }
    }

    pub async fn observe_roots(&self, roots: &[SkillRoot]) -> Result<(), BoxError> {
        if self.invalidator.closing.load(Ordering::Acquire) {
            return Ok(());
        }
        {
            let mut state = lock(&self.state);
            for root in roots {
                state.host_roots.insert(root.path.clone(), root.clone());
            }
        }
        let mut failed = false;
        let mut project_groups: IndexMap<PathBuf, Vec<SkillRoot>> = IndexMap::new();
        for root in roots {
            if let Some(project) = &root.project_root {
                project_groups
                    .entry(project.clone())
                    .or_default()
                    .push(root.clone());
            } else {
                if self.config.enabled
                    && let Err(error) = self
                        .retain(root.clone(), format!("shared:{}", root.path.display()))
                        .await
                {
                    failed = true;
                    self.ctx.logger().warn(format_args!(
                        "skill-filesystem: failed to watch {}: {error}",
                        root.path.display()
                    ));
                }
            }
        }
        for (project, grouped) in project_groups {
            let owner = format!("project:{}", project.display());
            {
                let mut state = lock(&self.state);
                if let Some(previous) = state.projects.shift_remove(&project) {
                    release_project_paths(&mut state, &owner, previous);
                }
                for root in &grouped {
                    state.host_roots.insert(root.path.clone(), root.clone());
                }
                state.projects.insert(
                    project.clone(),
                    grouped.iter().map(|root| root.path.clone()).collect(),
                );
            }
            for root in grouped {
                if !self.config.enabled {
                    continue;
                }
                if let Err(error) = self.retain(root.clone(), owner.clone()).await {
                    failed = true;
                    self.ctx.logger().warn(format_args!(
                        "skill-filesystem: failed to watch {}: {error}",
                        root.path.display()
                    ));
                }
            }
        }
        let mut evicted = false;
        loop {
            let oldest = {
                let mut state = lock(&self.state);
                if state.projects.len() <= self.config.max_projects {
                    None
                } else {
                    state.projects.shift_remove_index(0)
                }
            };
            let Some((project, paths)) = oldest else {
                break;
            };
            let owner = format!("project:{}", project.display());
            let mut state = lock(&self.state);
            release_project_paths(&mut state, &owner, paths);
            evicted = true;
        }
        if evicted {
            self.invalidator.control.invalidate();
        }
        if failed {
            Err(io::Error::other("one or more skill roots could not be watched").into())
        } else {
            Ok(())
        }
    }

    async fn retain(&self, root: SkillRoot, owner: String) -> Result<(), BoxError> {
        let mode = resolve_watch_mode(&root.path, self.config.follow_symlinks).await?;
        {
            let mut state = lock(&self.state);
            if let Some(current) = state.roots.get_mut(&root.path)
                && watch_is_reusable(&current.mode, &mode, &current.unhealthy)
            {
                current.owners.insert(owner);
                return Ok(());
            }
            state.roots.remove(&root.path);
        }
        let unhealthy = Arc::new(AtomicBool::new(false));
        let handle = self.open(&root, &mode, unhealthy.clone())?;
        let mut owners = HashSet::new();
        owners.insert(owner);
        lock(&self.state).roots.insert(
            root.path.clone(),
            RootWatch {
                owners,
                mode,
                unhealthy,
                _handle: handle,
            },
        );
        Ok(())
    }

    fn open(
        &self,
        root: &SkillRoot,
        mode: &WatchMode,
        unhealthy: Arc<AtomicBool>,
    ) -> Result<WatchHandle, BoxError> {
        match mode {
            WatchMode::Root {
                anchor,
                filter_root,
            } => self.open_root(root, anchor, filter_root, unhealthy),
            WatchMode::Ancestor { next_path, stamp } => {
                Ok(self.open_ancestor(root, next_path.clone(), stamp.clone(), unhealthy))
            }
        }
    }

    fn open_root(
        &self,
        root: &SkillRoot,
        anchor: &Path,
        filter_root: &Path,
        unhealthy: Arc<AtomicBool>,
    ) -> Result<WatchHandle, BoxError> {
        let mut event_root = root.clone();
        event_root.path = filter_root.to_path_buf();
        let invalidator = self.invalidator.clone();
        let root_path = root.path.clone();
        let logger = self.ctx.logger();
        let known_directories = Arc::new(Mutex::new(HashSet::<PathBuf>::new()));
        let event_directories = known_directories.clone();
        let follow_symlinks = self.config.follow_symlinks;
        let handler = move |result: notify::Result<Event>| match result {
            Ok(event) => {
                let directory_entry_change = event.paths.iter().any(|path| {
                    is_directory_entry_change(
                        &event_root,
                        follow_symlinks,
                        &event.kind,
                        path,
                        &event_directories,
                    )
                });
                if directory_entry_change
                    || event
                        .paths
                        .iter()
                        .any(|path| is_relevant_watch_event(&event_root, &event.kind, path))
                {
                    if directory_entry_change
                        || matches!(
                            event.kind,
                            EventKind::Create(CreateKind::Folder | CreateKind::Any)
                                | EventKind::Remove(RemoveKind::Folder | RemoveKind::Any)
                                | EventKind::Modify(ModifyKind::Name(_))
                        )
                    {
                        unhealthy.store(true, Ordering::Release);
                    }
                    invalidator.queue();
                }
            }
            Err(error) => {
                unhealthy.store(true, Ordering::Release);
                logger.warn(format_args!(
                    "skill-filesystem: watcher for {} failed: {error}",
                    root_path.display()
                ));
                invalidator.queue();
            }
        };
        let config = Config::default()
            .with_poll_interval(Duration::from_millis(self.config.poll_interval_ms))
            .with_follow_symlinks(self.config.follow_symlinks);
        let mut watcher: Box<dyn Watcher + Send> = if self.config.use_polling {
            Box::new(PollWatcher::new(handler, config)?)
        } else {
            Box::new(RecommendedWatcher::new(handler, config)?)
        };
        watcher.watch(anchor, RecursiveMode::NonRecursive)?;
        for entry in std::fs::read_dir(anchor)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let followed_directory = self.config.follow_symlinks
                && file_type.is_symlink()
                && std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir());
            if file_type.is_dir()
                || (self.config.follow_symlinks
                    && file_type.is_symlink()
                    && std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir()))
            {
                lock(&known_directories).insert(entry.path());
            }
            if file_type.is_dir() || followed_directory {
                watcher.watch(&entry.path(), RecursiveMode::NonRecursive)?;
            }
        }
        Ok(WatchHandle::Native(watcher))
    }

    fn open_ancestor(
        &self,
        root: &SkillRoot,
        next_path: PathBuf,
        initial: Option<PathStamp>,
        unhealthy: Arc<AtomicBool>,
    ) -> WatchHandle {
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        let invalidator = self.invalidator.clone();
        let interval = Duration::from_millis(self.config.poll_interval_ms);
        let root_path = root.path.clone();
        let logger = self.ctx.logger();
        let task = self.invalidator.runtime.spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            while !task_stop.load(Ordering::Acquire) {
                ticker.tick().await;
                match path_stamp(&next_path).await {
                    Ok(current) if current == initial => {}
                    Ok(_) => {
                        unhealthy.store(true, Ordering::Release);
                        invalidator.queue();
                        break;
                    }
                    Err(error) => {
                        unhealthy.store(true, Ordering::Release);
                        logger.warn(format_args!(
                            "skill-filesystem: watcher for {} failed: {error}",
                            root_path.display()
                        ));
                        invalidator.queue();
                        break;
                    }
                }
            }
        });
        WatchHandle::Ancestor { stop, task }
    }

    pub fn observe_host_mutation(&self, path: &Path) {
        if self.invalidator.closing.load(Ordering::Acquire) {
            return;
        }
        if lock(&self.state)
            .host_roots
            .values()
            .any(|root| is_potential_skill_path(root, path))
        {
            self.invalidator.control.invalidate();
        }
    }

    pub fn dispose(&self) {
        self.invalidator.close();
        if let Some(task) = lock(&self.lifecycle).take() {
            task.abort();
        }
        let mut state = lock(&self.state);
        state.projects.clear();
        state.roots.clear();
        state.host_roots.clear();
    }
}

fn release_project_paths(state: &mut State, owner: &str, paths: Vec<PathBuf>) {
    for path in paths {
        if let Some(root) = state.roots.get_mut(&path) {
            root.owners.remove(owner);
            if root.owners.is_empty() {
                state.roots.remove(&path);
            }
        }
        let retained_by_project = state.projects.values().any(|paths| paths.contains(&path));
        let retained_as_shared = state
            .roots
            .get(&path)
            .is_some_and(|root| root.owners.iter().any(|owner| owner.starts_with("shared:")));
        if !retained_by_project && !retained_as_shared {
            state.host_roots.remove(&path);
        }
    }
}

fn watch_is_reusable(current: &WatchMode, desired: &WatchMode, unhealthy: &AtomicBool) -> bool {
    current == desired && !unhealthy.load(Ordering::Acquire)
}

/// `notify` reports symlinked directories as `File` on Linux. Remember directory-like root
/// entries so both creation and the later unlink are treated like chokidar's addDir/unlinkDir.
fn is_directory_entry_change(
    root: &SkillRoot,
    follow_symlinks: bool,
    kind: &EventKind,
    path: &Path,
    known_directories: &Mutex<HashSet<PathBuf>>,
) -> bool {
    let Ok(relative) = path.strip_prefix(&root.path) else {
        return false;
    };
    let segments = relative.components().collect::<Vec<_>>();
    if segments.len() != 1 {
        return false;
    }
    if root.skip_system && segments[0].as_os_str() == ".system" {
        return false;
    }
    match kind {
        EventKind::Create(CreateKind::Folder | CreateKind::File | CreateKind::Any) => {
            let directory = is_watched_directory(path, follow_symlinks);
            if directory {
                lock(known_directories).insert(path.to_path_buf());
            }
            directory
        }
        EventKind::Remove(RemoveKind::Folder | RemoveKind::File | RemoveKind::Any) => {
            lock(known_directories).remove(path)
        }
        EventKind::Modify(ModifyKind::Name(_)) => {
            let mut known = lock(known_directories);
            if is_watched_directory(path, follow_symlinks) {
                known.insert(path.to_path_buf());
                true
            } else {
                known.remove(path)
            }
        }
        _ => false,
    }
}

fn is_watched_directory(path: &Path, follow_symlinks: bool) -> bool {
    if !follow_symlinks
        && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return false;
    }
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

fn is_relevant_watch_event(root: &SkillRoot, kind: &EventKind, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(&root.path) else {
        return false;
    };
    let segments = relative.components().collect::<Vec<_>>();
    if segments.is_empty() {
        return matches!(kind, EventKind::Create(_) | EventKind::Remove(_));
    }
    if segments.len() > 2 {
        return false;
    }
    let first = segments[0].as_os_str().to_string_lossy();
    if root.skip_system && first == ".system" {
        return false;
    }
    if segments.len() == 1 {
        if first.ends_with(".md") {
            return true;
        }
        return false;
    }
    segments[1].as_os_str() == "SKILL.md"
}

async fn resolve_watch_mode(root: &Path, follow_symlinks: bool) -> io::Result<WatchMode> {
    let mut candidate = root.to_path_buf();
    loop {
        match tokio::fs::metadata(&candidate).await {
            Ok(metadata) if metadata.is_dir() => {
                if candidate == root {
                    let preserve_link = !follow_symlinks
                        && tokio::fs::symlink_metadata(&candidate)
                            .await
                            .is_ok_and(|metadata| metadata.file_type().is_symlink());
                    let anchor = if preserve_link {
                        std::path::absolute(&candidate)?
                    } else {
                        tokio::fs::canonicalize(&candidate).await?
                    };
                    return Ok(WatchMode::Root {
                        filter_root: anchor.clone(),
                        anchor,
                    });
                }
                let anchor = tokio::fs::canonicalize(&candidate).await?;
                let first = root
                    .strip_prefix(&candidate)
                    .ok()
                    .and_then(|relative| relative.components().next())
                    .map(|component| component.as_os_str().to_owned());
                let Some(first) = first else {
                    return Ok(WatchMode::Root {
                        filter_root: anchor.clone(),
                        anchor,
                    });
                };
                let next_path = anchor.join(first);
                let stamp = path_stamp(&next_path).await?;
                return Ok(WatchMode::Ancestor { next_path, stamp });
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => return Err(error),
        }
        let parent = candidate.parent().map(Path::to_path_buf);
        let Some(parent) = parent else {
            return Ok(WatchMode::Ancestor {
                next_path: root.to_path_buf(),
                stamp: path_stamp(root).await?,
            });
        };
        if parent == candidate {
            return Ok(WatchMode::Ancestor {
                next_path: root.to_path_buf(),
                stamp: path_stamp(root).await?,
            });
        }
        candidate = parent;
    }
}

async fn path_stamp(path: &Path) -> io::Result<Option<PathStamp>> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(PathStamp {
            directory: metadata.is_dir(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: PathBuf) -> SkillRoot {
        SkillRoot {
            path,
            source: "test".into(),
            rank: 1,
            skip_system: false,
            project_root: None,
            trusted_host: false,
        }
    }

    #[tokio::test]
    async fn missing_root_polls_only_the_next_path_component() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".agents/skills");
        let mode = resolve_watch_mode(&path, true).await.unwrap();
        assert_eq!(
            mode,
            WatchMode::Ancestor {
                next_path: home.path().join(".agents"),
                stamp: None,
            }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn followed_symlink_root_uses_the_canonical_path_for_filtering() {
        let home = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let path = home.path().join("skills");
        std::os::unix::fs::symlink(external.path(), &path).unwrap();
        let canonical = std::fs::canonicalize(external.path()).unwrap();
        assert_eq!(
            resolve_watch_mode(&path, true).await.unwrap(),
            WatchMode::Root {
                anchor: canonical.clone(),
                filter_root: canonical,
            }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unfollowed_symlink_root_preserves_the_link_path() {
        let home = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let path = home.path().join("skills");
        std::os::unix::fs::symlink(external.path(), &path).unwrap();
        let absolute = std::path::absolute(&path).unwrap();
        assert_eq!(
            resolve_watch_mode(&path, false).await.unwrap(),
            WatchMode::Root {
                anchor: absolute.clone(),
                filter_root: absolute,
            }
        );
    }

    #[test]
    fn native_filter_accepts_skill_files_but_not_unrelated_deep_paths() {
        let path = PathBuf::from("/skills");
        let root = root(path.clone());
        assert!(is_relevant_watch_event(
            &root,
            &EventKind::Modify(ModifyKind::Any),
            &path.join("one/SKILL.md"),
        ));
        assert!(!is_relevant_watch_event(
            &root,
            &EventKind::Modify(ModifyKind::Any),
            &path.join("one/assets/data.json"),
        ));
    }

    #[cfg(unix)]
    #[test]
    fn file_kind_events_for_directory_symlinks_are_directory_changes() {
        let directory = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let link = directory.path().join("linked");
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        let known = Mutex::new(HashSet::new());
        let skill_root = root(directory.path().to_path_buf());

        assert!(!is_directory_entry_change(
            &skill_root,
            false,
            &EventKind::Create(CreateKind::File),
            &link,
            &known,
        ));
        assert!(!is_directory_entry_change(
            &skill_root,
            false,
            &EventKind::Create(CreateKind::Any),
            &link,
            &known,
        ));
        assert!(!is_relevant_watch_event(
            &skill_root,
            &EventKind::Create(CreateKind::Any),
            &link,
        ));
        assert!(!is_directory_entry_change(
            &skill_root,
            false,
            &EventKind::Create(CreateKind::Folder),
            &link,
            &known,
        ));
        assert!(!is_directory_entry_change(
            &skill_root,
            false,
            &EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Any)),
            &link,
            &known,
        ));
        assert!(!is_relevant_watch_event(
            &skill_root,
            &EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Any)),
            &link,
        ));
        assert!(is_directory_entry_change(
            &skill_root,
            true,
            &EventKind::Create(CreateKind::File),
            &link,
            &known,
        ));
        assert!(lock(&known).contains(&link));
        std::fs::remove_file(&link).unwrap();
        assert!(is_directory_entry_change(
            &skill_root,
            true,
            &EventKind::Remove(RemoveKind::File),
            &link,
            &known,
        ));
        assert!(!lock(&known).contains(&link));

        let nested = directory.path().join("skill/assets");
        std::fs::create_dir_all(directory.path().join("skill")).unwrap();
        std::os::unix::fs::symlink(target.path(), &nested).unwrap();
        assert!(!is_directory_entry_change(
            &skill_root,
            true,
            &EventKind::Create(CreateKind::File),
            &nested,
            &known,
        ));

        let system_link = directory.path().join(".system");
        std::os::unix::fs::symlink(target.path(), &system_link).unwrap();
        let mut system_root = skill_root;
        system_root.skip_system = true;
        assert!(!is_directory_entry_change(
            &system_root,
            true,
            &EventKind::Create(CreateKind::File),
            &system_link,
            &known,
        ));
    }

    #[test]
    fn runtime_error_forces_replacement_even_when_the_watch_mode_is_unchanged() {
        let mode = WatchMode::Root {
            anchor: PathBuf::from("/skills"),
            filter_root: PathBuf::from("/skills"),
        };
        let unhealthy = AtomicBool::new(false);
        assert!(watch_is_reusable(&mode, &mode, &unhealthy));
        unhealthy.store(true, Ordering::Release);
        assert!(!watch_is_reusable(&mode, &mode, &unhealthy));
    }

    #[test]
    fn project_eviction_releases_host_mutation_roots_without_a_watcher() {
        let skill_path = PathBuf::from("/workspace/.dsh/skills");
        let mut state = State::default();
        state
            .host_roots
            .insert(skill_path.clone(), root(skill_path.clone()));

        release_project_paths(&mut state, "project:/workspace", vec![skill_path.clone()]);

        assert!(!state.host_roots.contains_key(&skill_path));
    }

    #[test]
    fn project_eviction_keeps_roots_retained_by_another_project() {
        let skill_path = PathBuf::from("/workspace/.dsh/skills");
        let mut state = State::default();
        state
            .host_roots
            .insert(skill_path.clone(), root(skill_path.clone()));
        state
            .projects
            .insert(PathBuf::from("/other-workspace"), vec![skill_path.clone()]);

        release_project_paths(&mut state, "project:/workspace", vec![skill_path.clone()]);

        assert!(state.host_roots.contains_key(&skill_path));
    }
}
