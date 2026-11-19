mod common;
use common::*;
use hoh_error::{BoxError, SharedError};
use hoh_fs::*;
use hoh_fs_local::watch::*;
use hoh_timeout::AbortController;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Notify, oneshot};
type ReadyResult = Result<(), BoxError>;
#[derive(Default)]
struct ControlledWatcher {
    closed: AtomicBool,
    close_started: Notify,
    release_close: Mutex<Option<Arc<Notify>>>,
}
impl FsWatcher for ControlledWatcher {
    fn close(&self) -> FsFuture<'_, ()> {
        Box::pin(async move {
            self.close_started.notify_one();
            let release = self.release_close.lock().unwrap().clone();
            if let Some(release) = release {
                release.notified().await;
            }
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}
#[derive(Default)]
struct Factory {
    request: Mutex<Option<WatchRequest>>,
    sender: Mutex<Option<oneshot::Sender<ReadyResult>>>,
    started: Notify,
    watcher: Arc<ControlledWatcher>,
}
impl WatchFactory for Factory {
    fn start(&self, request: WatchRequest) -> FsFuture<'_, WatchRegistration> {
        Box::pin(async move {
            *self.request.lock().unwrap() = Some(request);
            let (tx, rx) = oneshot::channel();
            *self.sender.lock().unwrap() = Some(tx);
            self.started.notify_one();
            Ok(WatchRegistration {
                watcher: self.watcher.clone(),
                ready: Box::pin(async move {
                    rx.await
                        .map_err(|_| std::io::Error::other("watch readiness channel closed"))?
                }),
            })
        })
    }
}
impl Factory {
    fn ready(&self) {
        self.sender
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(Ok(()))
            .unwrap();
    }
    fn emit(&self, path: &str) {
        if !self.watcher.closed.load(Ordering::SeqCst) {
            self.request
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .notify_path(path);
        }
    }
    fn fail(&self, error: BoxError) -> SharedError {
        let error = self
            .request
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .report_error(error);
        if let Some(tx) = self.sender.lock().unwrap().take() {
            let _ = tx.send(Err(error.clone().into()));
        }
        error
    }
}
struct Harness {
    e: Env,
    t: FsTarget,
    factory: Arc<Factory>,
    changes: Arc<Mutex<Vec<Option<BoxError>>>>,
    signal: Arc<AbortController>,
}
impl Harness {
    async fn new(kind: &str) -> Self {
        let e = Env::new();
        let t = match kind {
            "file" => e.file("file.txt", "a").await,
            "directory" => {
                tokio::fs::create_dir(e.path("file.txt")).await.unwrap();
                e.target("file.txt").await
            }
            _ => e.target("file.txt").await,
        };
        let factory = Arc::new(Factory::default());
        *e.fs.watch_factory.lock().unwrap() = Some(factory.clone());
        Self {
            e,
            t,
            factory,
            changes: Arc::default(),
            signal: Arc::new(AbortController::new()),
        }
    }
    fn changed(&self) -> FsChanged {
        let changes = self.changes.clone();
        Arc::new(move |e| changes.lock().unwrap().push(e))
    }
}
#[tokio::test]
async fn waits_for_the_parent_watch_of_a_target_and_awaits_closure() {
    for kind in ["file", "missing"] {
        let h = Harness::new(kind).await;
        let pending = h.e.fs.watch(&h.t, h.changed(), h.signal.signal());
        tokio::pin!(pending);
        tokio::select! {_=&mut pending=>panic!("watch resolved before ready"),_=h.factory.started.notified()=>{}}
        {
            let r = h.factory.request.lock().unwrap();
            let r = r.as_ref().unwrap();
            assert_eq!(r.root, h.e.path(""));
            assert!(!r.directory);
            assert!(!r.ignored(&r.root));
            assert!(!r.ignored(&r.path));
            assert!(r.ignored(&h.e.path("unrelated.txt")));
        }
        assert!(poll_once(pending.as_mut()).await.is_pending());
        h.factory.ready();
        let watcher = pending.await.unwrap();
        h.factory.emit(&h.e.path(""));
        h.factory.emit(&h.e.path("unrelated.txt"));
        assert!(h.changes.lock().unwrap().is_empty());
        h.factory.emit(h.t.target_key.as_str());
        assert_eq!(h.changes.lock().unwrap().len(), 1);
        assert!(h.changes.lock().unwrap()[0].is_none());
        let release = Arc::new(Notify::new());
        *h.factory.watcher.release_close.lock().unwrap() = Some(release.clone());
        let closing = watcher.close();
        tokio::pin!(closing);
        tokio::select! {_=&mut closing=>panic!("close resolved before native release"),_=h.factory.watcher.close_started.notified()=>{}}
        assert!(!h.factory.watcher.closed.load(Ordering::SeqCst));
        release.notify_one();
        closing.await.unwrap();
        assert!(h.factory.watcher.closed.load(Ordering::SeqCst));
    }
}
#[tokio::test]
async fn watches_a_directory_itself_without_filtering_its_direct_entries() {
    let h = Harness::new("directory").await;
    let pending = h.e.fs.watch(&h.t, h.changed(), h.signal.signal());
    tokio::pin!(pending);
    tokio::select! {_=&mut pending=>panic!(),_=h.factory.started.notified()=>{}}
    {
        let r = h.factory.request.lock().unwrap();
        let r = r.as_ref().unwrap();
        assert_eq!(r.root, h.t.target_key.as_str());
        assert!(r.directory);
        assert!(!r.ignored(&h.e.path("file.txt/child.txt")));
    }
    h.factory.ready();
    let watcher = pending.await.unwrap();
    h.factory.emit(&h.e.path("file.txt/child.txt"));
    assert_eq!(h.changes.lock().unwrap().len(), 1);
    watcher.close().await.unwrap();
}
#[tokio::test]
async fn does_not_acquire_a_watcher_when_cancelled_during_metadata_lookup() {
    let h = Harness::new("file").await;
    let c = h.signal.clone();
    h.e.hooks().inspect_watch_metadata = Some(Arc::new(move |_| {
        c.abort();
        Box::pin(async { Ok(()) })
    }));
    assert!(
        h.e.fs
            .watch(&h.t, h.changed(), h.signal.signal())
            .await
            .is_err()
    );
    assert!(h.factory.request.lock().unwrap().is_none());
}
#[tokio::test]
async fn rejects_an_already_cancelled_initialization_without_acquiring_a_watcher() {
    let h = Harness::new("file").await;
    h.signal.abort();
    assert!(
        h.e.fs
            .watch(&h.t, h.changed(), h.signal.signal())
            .await
            .is_err()
    );
    assert!(h.factory.request.lock().unwrap().is_none());
}
#[tokio::test]
async fn closes_the_acquired_watcher_when_initialization_is_cancelled() {
    let h = Harness::new("file").await;
    let pending = h.e.fs.watch(&h.t, h.changed(), h.signal.signal());
    tokio::pin!(pending);
    tokio::select! {_=&mut pending=>panic!(),_=h.factory.started.notified()=>{}}
    let reason = SharedError::new(std::io::Error::other("cancel initialization"));
    h.signal.abort_with(reason.clone());
    let error = pending.await.err().unwrap();
    let error = error.downcast_ref::<hoh_timeout::AbortError>().unwrap();
    assert_eq!(error.to_string(), "The operation was aborted");
    assert!(
        std::error::Error::source(error)
            .unwrap()
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&reason)
    );
    assert!(h.factory.watcher.closed.load(Ordering::SeqCst));
    assert!(h.changes.lock().unwrap().is_empty());
}
#[tokio::test]
async fn reports_initialization_errors_and_closes_before_rejecting() {
    let h = Harness::new("file").await;
    let pending = h.e.fs.watch(&h.t, h.changed(), h.signal.signal());
    tokio::pin!(pending);
    tokio::select! {_=&mut pending=>panic!(),_=h.factory.started.notified()=>{}}
    let sentinel = h.factory.fail(std::io::Error::other("watch failed").into());
    let error = pending.await.err().unwrap();
    assert!(
        error
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&sentinel)
    );
    let changes = h.changes.lock().unwrap();
    assert_eq!(changes.len(), 1);
    assert!(
        changes[0]
            .as_ref()
            .unwrap()
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&sentinel)
    );
    assert!(h.factory.watcher.closed.load(Ordering::SeqCst));
}
#[tokio::test]
async fn normalizes_watcher_errors_after_initialization() {
    let h = Harness::new("file").await;
    let pending = h.e.fs.watch(&h.t, h.changed(), h.signal.signal());
    tokio::pin!(pending);
    tokio::select! {_=&mut pending=>panic!(),_=h.factory.started.notified()=>{}}
    h.factory.ready();
    let watcher = pending.await.unwrap();
    let sentinel = h.factory.fail(std::io::Error::other("watch failed").into());
    h.factory
        .fail(std::io::Error::other("watch unavailable").into());
    {
        let changes = h.changes.lock().unwrap();
        assert_eq!(changes.len(), 2);
        assert!(
            changes[0]
                .as_ref()
                .unwrap()
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&sentinel)
        );
        assert_eq!(
            changes[1].as_ref().unwrap().to_string(),
            "watch unavailable"
        );
    }
    watcher.close().await.unwrap();
    assert!(h.factory.watcher.closed.load(Ordering::SeqCst));
}
#[tokio::test]
async fn notify_observes_in_place_write_atomic_replacement_delete_and_recreation() {
    let e = Env::new();
    let t = e.file("file.txt", "initial").await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let changed: FsChanged = Arc::new(move |error| {
        let _ = tx.send(error);
    });
    let watcher =
        e.fs.watch(&t, changed, AbortController::new().signal())
            .await
            .unwrap();
    for operation in ["in-place", "atomic", "delete", "recreate"] {
        while rx.try_recv().is_ok() {}
        match operation {
            "in-place" => {
                tokio::fs::write(e.path("file.txt"), "changed")
                    .await
                    .unwrap();
            }
            "atomic" => {
                e.fs.write_text(&t, "replaced", None, None, None)
                    .await
                    .unwrap();
            }
            "delete" => tokio::fs::remove_file(e.path("file.txt")).await.unwrap(),
            _ => tokio::fs::write(e.path("file.txt"), "recreated")
                .await
                .unwrap(),
        };
        let event = tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(event.is_none(), "watch failed: {event:?}");
    }
    watcher.close().await.unwrap();
}
#[tokio::test]
async fn notify_readiness_covers_initially_missing_file_creation() {
    let e = Env::new();
    let t = e.target("new.txt").await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let watcher =
        e.fs.watch(
            &t,
            Arc::new(move |error| {
                let _ = tx.send(error);
            }),
            AbortController::new().signal(),
        )
        .await
        .unwrap();
    e.fs.write_text(
        &t,
        "created",
        Some(FsWriteIntent::CreateIfAbsent),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    watcher.close().await.unwrap();
}
