//! The file watcher seam and its `notify` implementation.
//!
//! The source watches the document with `chokidar` (`ignoreInitial`, `awaitWriteFinish`) and
//! reacts to its `all`, `ready` and `error` events. The provider here reacts to the same three
//! events through a [`WatchSink`], so tests drive the pipeline with a fake watcher exactly as the
//! source's tests drive a fake `chokidar`.

use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use hoh_error::BoxError;
use notify::{RecursiveMode, Watcher as _};

type Callback = Arc<dyn Fn() + Send + Sync>;
type ErrorCallback = Arc<dyn Fn(BoxError) + Send + Sync>;

/// Where a watcher reports: the three events the provider listens to.
#[derive(Clone)]
pub struct WatchSink {
    pub(crate) all: Callback,
    pub(crate) ready: Callback,
    pub(crate) error: ErrorCallback,
}

impl WatchSink {
    /// `watcher.on('all')`: the document changed, appeared or went away.
    pub fn all(&self) {
        (self.all)();
    }

    /// `watcher.on('ready')`: the watcher is active.
    pub fn ready(&self) {
        (self.ready)();
    }

    /// `watcher.on('error')`: the watcher backend failed.
    pub fn error(&self, error: BoxError) {
        (self.error)(error);
    }
}

/// What the provider asks of a watcher (the `chokidar.watch(path, options)` call).
pub struct WatchRequest {
    /// The canonical path of the document.
    pub path: PathBuf,
    /// `awaitWriteFinish.stabilityThreshold`: the write-settle window in milliseconds.
    pub stability_threshold_ms: u64,
    /// `awaitWriteFinish.pollInterval`: `max(1, min(debounceMs, 10))`.
    pub poll_interval_ms: u64,
    /// Where events go.
    pub sink: WatchSink,
}

/// A live watcher; closing stops its events.
pub trait Watcher: Send {
    /// Stop watching. Events a backend still has in flight are dropped.
    fn close(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// Starts a watcher; tests replace it with a fake.
pub type WatcherFactory =
    Arc<dyn Fn(WatchRequest) -> Result<Box<dyn Watcher>, BoxError> + Send + Sync>;

/// The default watcher: `notify` on the document's directory, with the write-settle wait done here.
pub(crate) fn notify_factory() -> WatcherFactory {
    Arc::new(start_notify)
}

struct NotifyWatcher {
    watcher: Arc<Mutex<notify::RecommendedWatcher>>,
    pump: tokio::task::JoinHandle<()>,
}

impl Watcher for NotifyWatcher {
    fn close(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let NotifyWatcher { watcher, pump } = *self;
        pump.abort();
        Box::pin(async move {
            let _ = pump.await;
            drop(watcher);
        })
    }
}

/// The nearest existing directory at or above `start`.
fn nearest_existing_from(start: &Path) -> Option<PathBuf> {
    let mut directory = start;
    loop {
        if directory.is_dir() {
            return Some(directory.to_path_buf());
        }
        directory = directory.parent()?;
    }
}

/// The nearest directory that exists among the document's parent and its ancestors. A directory
/// that has not been created yet cannot be watched, so its closest existing ancestor stands in.
fn nearest_existing_directory(path: &Path) -> Option<PathBuf> {
    nearest_existing_from(path.parent()?)
}

/// Move the watch to the nearest existing directory above the document. A directory can vanish
/// between the search and the watch; the search then runs again from the failed directory's
/// parent, so the document is never left unobserved while some ancestor can still be watched.
/// `watch(old, new)` releases `old` and watches `new`.
fn retarget<E>(
    path: &Path,
    watched: &mut PathBuf,
    mut watch: impl FnMut(&Path, &Path) -> Result<(), E>,
) -> Result<(), E> {
    let mut start = path.parent().map(Path::to_path_buf);
    let mut failure = None;
    while let Some(from) = start.take() {
        let Some(nearest) = nearest_existing_from(&from) else {
            break;
        };
        match watch(watched, &nearest) {
            Ok(()) => {
                *watched = nearest;
                return Ok(());
            }
            Err(error) => {
                start = nearest.parent().map(Path::to_path_buf);
                failure = Some(error);
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

fn start_notify(request: WatchRequest) -> Result<Box<dyn Watcher>, BoxError> {
    let WatchRequest {
        path,
        stability_threshold_ms,
        sink,
        ..
    } = request;
    let mut watched = nearest_existing_directory(&path).ok_or_else(|| -> BoxError {
        "credentials-local: the document has no parent directory".into()
    })?;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<()>();
    let target = path.clone();
    let failures = sink.clone();
    let mut watcher = notify::RecommendedWatcher::new(
        move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                // The document itself, or any directory on the way to it appearing or vanishing.
                if event
                    .paths
                    .iter()
                    .any(|touched| target.starts_with(touched))
                {
                    let _ = sender.send(());
                }
            }
            Err(error) => failures.error(Box::new(error)),
        },
        notify::Config::default(),
    )?;
    watcher.watch(&watched, RecursiveMode::NonRecursive)?;
    let watcher = Arc::new(Mutex::new(watcher));
    // `awaitWriteFinish`: an event is reported once the document stayed quiet for the window.
    let settle = Duration::from_millis(stability_threshold_ms);
    let reporter = sink.clone();
    let rearmed = watcher.clone();
    let pump = tokio::spawn(async move {
        while receiver.recv().await.is_some() {
            loop {
                match tokio::time::timeout(settle, receiver.recv()).await {
                    Ok(Some(())) => {}
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            // Follow the document into a directory that appeared (or out of one that went away),
            // like chokidar, which keeps watching a path whose parent does not exist yet.
            {
                let mut guard = rearmed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Always re-armed, even onto the same directory: one removed and created again
                // has lost its watch.
                let outcome = retarget(&path, &mut watched, |old, new| {
                    let _ = guard.unwatch(old);
                    guard.watch(new, RecursiveMode::NonRecursive)
                });
                if let Err(error) = outcome {
                    reporter.error(Box::new(error));
                }
            }
            // Anything written while no watch covered the document fired no event of its own.
            reporter.all();
        }
    });
    sink.ready();
    Ok(Box::new(NotifyWatcher { watcher, pump }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        let file = dir.path().join("a/b/c/.credentials.yaml");
        (dir, file)
    }

    #[test]
    fn retargets_to_the_nearest_existing_directory() {
        let (dir, file) = tree();
        let mut watched = dir.path().to_path_buf();
        let mut seen = Vec::new();
        retarget(&file, &mut watched, |_, new| {
            seen.push(new.to_path_buf());
            Ok::<(), ()>(())
        })
        .unwrap();
        assert_eq!(watched, dir.path().join("a/b"));
        assert_eq!(seen, vec![dir.path().join("a/b")]);
    }

    #[test]
    fn falls_back_to_an_ancestor_when_the_watch_fails_on_a_vanished_directory() {
        let (dir, file) = tree();
        let mut watched = dir.path().to_path_buf();
        let mut attempts = Vec::new();
        let vanished = dir.path().join("a/b");
        retarget(&file, &mut watched, |_, new| {
            attempts.push(new.to_path_buf());
            if new == vanished { Err("gone") } else { Ok(()) }
        })
        .unwrap();
        assert_eq!(attempts, vec![vanished, dir.path().join("a")]);
        assert_eq!(watched, dir.path().join("a"));
    }

    #[test]
    fn reports_the_failure_when_no_ancestor_can_be_watched() {
        let (dir, file) = tree();
        let mut watched = dir.path().to_path_buf();
        let failure = retarget(&file, &mut watched, |_, _| Err("denied"));
        assert_eq!(failure, Err("denied"));
        assert_eq!(watched, dir.path());
    }
}
