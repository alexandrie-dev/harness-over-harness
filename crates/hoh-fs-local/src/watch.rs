//! Notify adapter: caller-owned initialization and asynchronous close.
use crate::{LocalFileSystem, fsio, lock};
use hoh_error::BoxError;
use hoh_fs::{FileSystem, FsChanged, FsFuture, FsTarget, FsType, FsWatcher};
use hoh_timeout::AbortSignal;
use hoh_util_workspace_path::node_path;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub struct WatchRequest {
    pub path: String,
    pub root: String,
    pub directory: bool,
    pub changed: FsChanged,
}
impl WatchRequest {
    pub fn ignored(&self, path: &str) -> bool {
        let path = node_path::resolve("/", &[path]);
        !self.directory && path != self.root && path != self.path
    }
    pub fn notify_path(&self, path: &str) {
        let path = node_path::resolve("/", &[path]);
        let relevant = if self.directory {
            path == self.path || node_path::dirname(&path) == self.path
        } else {
            path == self.path
        };
        if relevant {
            (self.changed)(None);
        }
    }
    pub fn report_error(&self, error: BoxError) -> hoh_error::SharedError {
        let error = hoh_error::SharedError::new(error);
        (self.changed)(Some(error.clone().into()));
        error
    }
}
pub struct WatchRegistration {
    pub watcher: Arc<dyn FsWatcher>,
    pub ready: FsFuture<'static, ()>,
}
pub trait WatchFactory: Send + Sync {
    fn start(&self, request: WatchRequest) -> FsFuture<'_, WatchRegistration>;
}
struct NotifyFactory {
    link_watch_hook: Option<fsio::WatchLinkHook>,
    before_watch_link: Option<fsio::WatchLinkGateHook>,
    add_hook: Option<fsio::WatchAddHook>,
}
enum WatchPhase {
    Initializing(Option<hoh_error::SharedError>),
    Ready,
}
/// What chokidar 5 distinguishes before its `atomic` and `change` throttling (`index.ts` `_emit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchEventKind {
    Add,
    Change,
    Unlink,
    /// `addDir`, `unlinkDir` and anything else reported at once.
    Other,
}
/// Delay of a pending `unlink` (chokidar `atomic: true`, 100 ms on Linux).
pub const ATOMIC_WINDOW: Duration = Duration::from_millis(100);
/// Leading-edge throttle of `change` per path (chokidar `_throttle(EV.CHANGE, path, 50)`).
pub const CHANGE_THROTTLE: Duration = Duration::from_millis(50);
/// Result of one event: whether the listener runs now, and the instant a flush is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub emit: bool,
    pub flush_at: Option<Duration>,
}
/// Chokidar's per-path event coalescing as a pure state machine over an explicit clock
/// (`Duration` since an origin chosen by the caller).
#[derive(Default)]
pub struct Coalescer {
    pending: Vec<(String, Duration)>,
    flushes: VecDeque<Duration>,
    throttled: HashMap<String, Duration>,
}
impl Coalescer {
    pub fn event(&mut self, kind: WatchEventKind, path: &str, now: Duration) -> Outcome {
        let mut kind = kind;
        match kind {
            WatchEventKind::Unlink => {
                match self.pending.iter_mut().find(|(p, _)| p == path) {
                    Some(entry) => entry.1 = now,
                    None => self.pending.push((path.to_owned(), now)),
                }
                let at = now + ATOMIC_WINDOW;
                self.flushes.push_back(at);
                return Outcome {
                    emit: false,
                    flush_at: Some(at),
                };
            }
            WatchEventKind::Add => {
                if let Some(index) = self.pending.iter().position(|(p, _)| p == path) {
                    self.pending.remove(index);
                    kind = WatchEventKind::Change;
                }
            }
            _ => {}
        }
        if kind == WatchEventKind::Change {
            if let Some(first) = self.throttled.get(path)
                && now < *first + CHANGE_THROTTLE
            {
                return Outcome {
                    emit: false,
                    flush_at: None,
                };
            }
            self.throttled.insert(path.to_owned(), now);
        }
        Outcome {
            emit: true,
            flush_at: None,
        }
    }
    /// Every scheduled flush due by `now` reports all unlinks still pending at that flush.
    pub fn flush(&mut self, now: Duration) -> Vec<String> {
        let mut emitted = Vec::new();
        while self.flushes.front().is_some_and(|due| *due <= now) {
            let due = self.flushes.pop_front().unwrap_or_default();
            let mut index = 0;
            while index < self.pending.len() {
                if self.pending[index].1 <= due {
                    emitted.push(self.pending.remove(index).0);
                } else {
                    index += 1;
                }
            }
        }
        self.throttled.retain(|_, at| now < *at + CHANGE_THROTTLE);
        emitted
    }
}
struct Native {
    watcher: Option<RecommendedWatcher>,
    links: HashSet<String>,
}
/// Filters native events for one request and reports them like chokidar's `all` listener.
pub struct EventPipeline {
    request: WatchRequest,
    state: Mutex<Coalescer>,
    origin: tokio::time::Instant,
    runtime: tokio::runtime::Handle,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    changed_lock: Mutex<()>,
    closed: AtomicBool,
    native_ready: AtomicBool,
    native: Mutex<Native>,
    link_watch_hook: Option<fsio::WatchLinkHook>,
    before_watch_link: Option<fsio::WatchLinkGateHook>,
    add_hook: Option<fsio::WatchAddHook>,
}
impl EventPipeline {
    /// Must be called inside a Tokio runtime: flush timers run on it.
    pub fn new(request: WatchRequest) -> Arc<Self> {
        Self::new_with_hooks(request, None, None, None)
    }
    fn new_with_hooks(
        request: WatchRequest,
        link_watch_hook: Option<fsio::WatchLinkHook>,
        before_watch_link: Option<fsio::WatchLinkGateHook>,
        add_hook: Option<fsio::WatchAddHook>,
    ) -> Arc<Self> {
        Arc::new(Self {
            request,
            state: Mutex::new(Coalescer::default()),
            origin: tokio::time::Instant::now(),
            runtime: tokio::runtime::Handle::current(),
            tasks: Mutex::new(Vec::new()),
            changed_lock: Mutex::new(()),
            closed: AtomicBool::new(false),
            native_ready: AtomicBool::new(false),
            native: Mutex::new(Native {
                watcher: None,
                links: HashSet::new(),
            }),
            link_watch_hook,
            before_watch_link,
            add_hook,
        })
    }
    fn relevant(&self, path: &str) -> bool {
        if self.request.directory {
            path == self.request.path || node_path::dirname(path) == self.request.path
        } else {
            path == self.request.path
        }
    }
    fn elapsed(&self) -> Duration {
        tokio::time::Instant::now().saturating_duration_since(self.origin)
    }
    fn track(&self, task: tokio::task::JoinHandle<()>) {
        let mut tasks = lock(&self.tasks);
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
    }
    fn emit(&self) {
        let _guard = lock(&self.changed_lock);
        if !self.closed.load(Ordering::SeqCst) {
            (self.request.changed)(None);
        }
    }
    /// Feed one classified event for `path`; irrelevant paths are dropped.
    pub fn dispatch(self: &Arc<Self>, kind: WatchEventKind, path: &str) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let path = node_path::resolve("/", &[path]);
        if !self.relevant(&path) {
            return;
        }
        let outcome = lock(&self.state).event(kind, &path, self.elapsed());
        if let Some(at) = outcome.flush_at {
            let this = Arc::clone(self);
            let deadline = self.origin + at;
            let task = self.runtime.spawn(async move {
                tokio::time::sleep_until(deadline).await;
                this.flush();
            });
            self.track(task);
        }
        if kind == WatchEventKind::Add && self.native_ready.load(Ordering::SeqCst) {
            let this = Arc::clone(self);
            let emitted_path = path.clone();
            let task = self.runtime.spawn(async move {
                this.refresh_links().await;
                if outcome.emit {
                    this.emit();
                }
                if let Some(hook) = &this.add_hook {
                    hook(emitted_path, true);
                }
            });
            self.track(task);
            if let Some(hook) = &self.add_hook {
                hook(path, false);
            }
            return;
        }
        if kind == WatchEventKind::Add
            && let Some(hook) = &self.add_hook
        {
            hook(path.clone(), false);
        }
        if outcome.emit {
            self.emit();
        }
        if kind == WatchEventKind::Add
            && let Some(hook) = &self.add_hook
        {
            hook(path.clone(), true);
        }
        if matches!(kind, WatchEventKind::Add | WatchEventKind::Unlink) {
            let this = Arc::clone(self);
            let task = self.runtime.spawn(async move {
                this.refresh_links().await;
            });
            self.track(task);
        }
    }
    fn flush(&self) {
        let due = lock(&self.state).flush(self.elapsed());
        for _ in due {
            self.emit();
        }
    }
    /// Map a native event to chokidar-style kinds and dispatch every relevant path.
    pub fn handle_native(self: &Arc<Self>, event: &notify::Event) {
        use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
        let paths: Vec<String> = event
            .paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let single = |kind: WatchEventKind| {
            for path in &paths {
                self.dispatch(kind, path);
            }
        };
        match event.kind {
            EventKind::Access(_) => {}
            EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder) => {
                single(WatchEventKind::Other);
            }
            EventKind::Create(_) => single(WatchEventKind::Add),
            EventKind::Remove(_) => single(WatchEventKind::Unlink),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)) => single(WatchEventKind::Unlink),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)) => single(WatchEventKind::Add),
            // The paired `Both` event repeats `From` and `To`, which inotify reports separately.
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {}
            EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                single(WatchEventKind::Change);
            }
        }
    }
    /// Keep one native watch per followed symlink to a file among the direct children
    /// (chokidar `followSymlinks`: it watches the link target through `fs.watch(link)`).
    async fn refresh_links(self: &Arc<Self>) {
        if !self.native_ready.load(Ordering::SeqCst) {
            return;
        }
        let this = Arc::clone(self);
        let _ = tokio::task::spawn_blocking(move || {
            let wanted = link_candidates(&this.request);
            let hook = this.link_watch_hook.clone();
            let before = this.before_watch_link.clone();
            let mut native = lock(&this.native);
            let Native { watcher, links } = &mut *native;
            let Some(watcher) = watcher.as_mut() else {
                return;
            };
            for stale in links.clone().difference(&wanted) {
                let _ = watcher.unwatch(Path::new(stale));
                links.remove(stale);
            }
            for link in wanted.difference(&links.clone()) {
                if before.as_ref().is_some_and(|hook| !hook(link.clone())) {
                    continue;
                }
                if watcher
                    .watch(Path::new(link), RecursiveMode::NonRecursive)
                    .is_ok()
                {
                    links.insert(link.clone());
                    if let Some(hook) = &hook {
                        hook(link.clone());
                    }
                }
            }
        })
        .await;
    }
    fn close(&self) -> Option<RecommendedWatcher> {
        self.closed.store(true, Ordering::SeqCst);
        self.native_ready.store(false, Ordering::SeqCst);
        for task in lock(&self.tasks).drain(..) {
            task.abort();
        }
        lock(&self.native).watcher.take()
    }
}
/// Direct children of a watched directory that are symlinks to regular files. A watched
/// file is addressed by its realpath, so it never is a link itself.
fn link_candidates(request: &WatchRequest) -> HashSet<String> {
    if !request.directory {
        return HashSet::new();
    }
    let Ok(entries) = std::fs::read_dir(&request.path) else {
        return HashSet::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| node_path::join(&[&request.path, &entry.file_name().to_string_lossy()]))
        .filter(|path| {
            std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
                && std::fs::metadata(path).is_ok_and(|m| m.is_file())
        })
        .collect()
}
struct LocalWatcher {
    pipeline: Arc<EventPipeline>,
}
impl FsWatcher for LocalWatcher {
    fn close(&self) -> FsFuture<'_, ()> {
        let native = self.pipeline.close();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || drop(native))
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Ok(())
        })
    }
}
impl WatchFactory for NotifyFactory {
    fn start(&self, request: WatchRequest) -> FsFuture<'_, WatchRegistration> {
        let link_watch_hook = self.link_watch_hook.clone();
        let before_watch_link = self.before_watch_link.clone();
        let add_hook = self.add_hook.clone();
        Box::pin(async move {
            let errors = request.changed.clone();
            let root = request.root.clone();
            let pipeline = EventPipeline::new_with_hooks(
                request,
                link_watch_hook,
                before_watch_link,
                add_hook,
            );
            let phase = Arc::new(Mutex::new(WatchPhase::Initializing(None)));
            let callback_pipeline = pipeline.clone();
            let native =
                tokio::task::spawn_blocking(move || -> Result<RecommendedWatcher, BoxError> {
                    let mut root = root;
                    while !Path::new(&root).exists() {
                        let parent = node_path::dirname(&root);
                        if root == parent {
                            break;
                        }
                        root = parent;
                    }
                    let recursive = root != callback_pipeline.request.root;
                    let root_for_error = callback_pipeline.request.root.clone();
                    let callback_phase = phase.clone();
                    let events = callback_pipeline.clone();
                    let mut watcher = RecommendedWatcher::new(
                        move |event: notify::Result<notify::Event>| {
                            if events.closed.load(Ordering::SeqCst) {
                                return;
                            }
                            match event {
                                Ok(event) => events.handle_native(&event),
                                Err(error) => {
                                    let error = events
                                        .request
                                        .report_error(notify_error(error, &root_for_error));
                                    if let WatchPhase::Initializing(pending) =
                                        &mut *lock(&callback_phase)
                                    {
                                        pending.get_or_insert(error);
                                    }
                                }
                            }
                        },
                        notify::Config::default(),
                    )
                    .map_err(|e| notify_error(e, &root))?;
                    watcher
                        .watch(
                            Path::new(&root),
                            if recursive {
                                RecursiveMode::Recursive
                            } else {
                                RecursiveMode::NonRecursive
                            },
                        )
                        .map_err(|e| notify_error(e, &root))?;
                    let mut links = HashSet::new();
                    if !recursive {
                        for link in link_candidates(&callback_pipeline.request) {
                            if watcher
                                .watch(Path::new(&link), RecursiveMode::NonRecursive)
                                .is_ok()
                            {
                                links.insert(link.clone());
                                if let Some(hook) = &callback_pipeline.link_watch_hook {
                                    hook(link.clone());
                                }
                            }
                        }
                    }
                    let previous_phase = {
                        let mut phase = lock(&phase);
                        std::mem::replace(&mut *phase, WatchPhase::Ready)
                    };
                    if let WatchPhase::Initializing(Some(error)) = previous_phase {
                        return Err(error.into());
                    }
                    lock(&callback_pipeline.native).links = links;
                    Ok(watcher)
                })
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            match native {
                Ok(native) => {
                    lock(&pipeline.native).watcher = Some(native);
                    pipeline.native_ready.store(true, Ordering::SeqCst);
                    Ok(WatchRegistration {
                        watcher: Arc::new(LocalWatcher { pipeline }),
                        ready: Box::pin(std::future::ready(Ok(()))),
                    })
                }
                Err(error) => {
                    if error.is::<hoh_error::SharedError>() {
                        return Err(error);
                    }
                    // As with chokidar's error event, initialization reports the error
                    // to the listener as well as rejecting the readiness promise.
                    let shared = hoh_error::SharedError::new(error);
                    errors(Some(Box::new(shared.clone())));
                    Err(shared.into())
                }
            }
        })
    }
}
fn notify_error(error: notify::Error, path: &str) -> BoxError {
    match error.kind {
        notify::ErrorKind::Io(e) => fsio::errno(e, "watch", Some(path), None),
        _ => std::io::Error::other(error.to_string()).into(),
    }
}
pub(crate) async fn watch_target(
    fs: &LocalFileSystem,
    target: FsTarget,
    changed: FsChanged,
    signal: AbortSignal,
) -> Result<Arc<dyn FsWatcher>, BoxError> {
    signal.throw_if_aborted()?;
    let path = node_path::resolve("/", &[&fs.process_path(&target)]);
    let directory = fs
        .stat(&target, Some(signal.clone()))
        .await?
        .is_some_and(|s| s.r#type == FsType::Directory);
    let hooks = lock(&fs.internals).clone();
    if let Some(h) = hooks.inspect_watch_metadata {
        h(target).await?;
    }
    signal.throw_if_aborted()?;
    let root = if directory {
        path.clone()
    } else {
        node_path::dirname(&path)
    };
    let factory = lock(&fs.watch_factory).clone().unwrap_or_else(|| {
        Arc::new(NotifyFactory {
            link_watch_hook: hooks.inspect_watch_link,
            before_watch_link: hooks.before_watch_link,
            add_hook: hooks.inspect_watch_add,
        })
    });
    let registration = factory
        .start(WatchRequest {
            path,
            root,
            directory,
            changed,
        })
        .await?;
    let WatchRegistration { watcher, mut ready } = registration;
    let cancellation = || -> BoxError {
        // Called only for an aborted signal: its stable reason is present.
        hoh_timeout::AbortError::new("The operation was aborted")
            .with_cause(
                signal
                    .reason()
                    .expect("completed cancellation has a reason"),
            )
            .into()
    };
    let result = if signal.aborted() {
        Err(cancellation())
    } else {
        tokio::select! {
            result=&mut ready=>result,
            _=signal.cancelled()=>Err(cancellation()),
        }
    };
    if let Err(error) = result {
        watcher.close().await?;
        return Err(error);
    }
    Ok(watcher)
}
