//! Chokidar 5 event coalescing (`atomic: true`, `change` throttle) and followed symlinks.
mod common;
use common::*;
use hoh_fs::*;
use hoh_fs_local::watch::*;
use hoh_timeout::AbortController;
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

mod coalescer {
    use super::*;
    fn emits(c: &mut Coalescer, kind: WatchEventKind, at: u64) -> bool {
        c.event(kind, "/w/f", ms(at)).emit
    }
    #[test]
    fn an_unlink_waits_for_the_atomic_window() {
        let mut c = Coalescer::default();
        let outcome = c.event(WatchEventKind::Unlink, "/w/f", ms(0));
        assert!(!outcome.emit);
        assert_eq!(outcome.flush_at, Some(ms(100)));
        assert!(c.flush(ms(99)).is_empty());
        assert_eq!(c.flush(ms(100)), ["/w/f"]);
        assert!(c.flush(ms(500)).is_empty());
    }
    #[test]
    fn an_add_inside_the_window_replaces_the_unlink_with_one_change() {
        let mut c = Coalescer::default();
        c.event(WatchEventKind::Unlink, "/w/f", ms(0));
        assert!(emits(&mut c, WatchEventKind::Add, 50));
        assert!(c.flush(ms(100)).is_empty());
    }
    #[test]
    fn an_add_after_the_window_is_a_plain_add() {
        let mut c = Coalescer::default();
        c.event(WatchEventKind::Unlink, "/w/f", ms(0));
        assert_eq!(c.flush(ms(100)), ["/w/f"]);
        assert!(emits(&mut c, WatchEventKind::Add, 101));
        assert!(emits(&mut c, WatchEventKind::Add, 102));
    }
    #[test]
    fn a_flush_reports_every_unlink_pending_at_that_moment() {
        let mut c = Coalescer::default();
        c.event(WatchEventKind::Unlink, "/w/a", ms(0));
        c.event(WatchEventKind::Unlink, "/w/b", ms(60));
        assert_eq!(c.flush(ms(100)), ["/w/a", "/w/b"]);
        assert!(c.flush(ms(160)).is_empty());
    }
    #[test]
    fn changes_to_one_path_are_throttled_from_the_first_one() {
        let mut c = Coalescer::default();
        assert!(emits(&mut c, WatchEventKind::Change, 0));
        assert!(!emits(&mut c, WatchEventKind::Change, 10));
        assert!(!emits(&mut c, WatchEventKind::Change, 49));
        assert!(emits(&mut c, WatchEventKind::Change, 50));
        assert!(c.event(WatchEventKind::Change, "/w/other", ms(51)).emit);
    }
    #[test]
    fn an_add_that_becomes_a_change_is_throttled_too() {
        let mut c = Coalescer::default();
        assert!(emits(&mut c, WatchEventKind::Change, 0));
        c.event(WatchEventKind::Unlink, "/w/f", ms(10));
        assert!(!emits(&mut c, WatchEventKind::Add, 20));
    }
    #[test]
    fn other_events_are_never_delayed_or_throttled() {
        let mut c = Coalescer::default();
        assert!(emits(&mut c, WatchEventKind::Other, 0));
        assert!(emits(&mut c, WatchEventKind::Other, 1));
    }
}

mod pipeline {
    use super::*;
    struct Probe {
        e: Env,
        path: String,
        seen: Arc<Mutex<Vec<bool>>>,
        pipeline: Arc<EventPipeline>,
    }
    /// Each callback records whether the file exists at that moment (a re-read after `changed`).
    fn probe() -> Probe {
        let e = Env::new();
        let path = e.path("file.txt");
        std::fs::write(&path, "initial").unwrap();
        let seen = Arc::new(Mutex::new(vec![]));
        let (record, watched) = (seen.clone(), path.clone());
        let pipeline = EventPipeline::new(WatchRequest {
            path: path.clone(),
            root: e.path(""),
            directory: false,
            changed: Arc::new(move |error| {
                assert!(error.is_none());
                record
                    .lock()
                    .unwrap()
                    .push(std::path::Path::new(&watched).exists());
            }),
        });
        Probe {
            e,
            path,
            seen,
            pipeline,
        }
    }
    async fn advance(by: u64) {
        tokio::time::advance(ms(by)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }
    #[tokio::test(start_paused = true)]
    async fn a_file_recreated_inside_the_window_is_one_change_never_a_deletion() {
        let p = probe();
        std::fs::remove_file(&p.path).unwrap();
        p.pipeline.dispatch(WatchEventKind::Unlink, &p.path);
        advance(50).await;
        assert!(p.seen.lock().unwrap().is_empty());
        std::fs::write(&p.path, "again").unwrap();
        p.pipeline.dispatch(WatchEventKind::Add, &p.path);
        assert_eq!(*p.seen.lock().unwrap(), [true]);
        advance(500).await;
        assert_eq!(*p.seen.lock().unwrap(), [true]);
        drop(p.e);
    }
    #[tokio::test(start_paused = true)]
    async fn a_deletion_is_reported_once_after_the_window() {
        let p = probe();
        std::fs::remove_file(&p.path).unwrap();
        p.pipeline.dispatch(WatchEventKind::Unlink, &p.path);
        advance(99).await;
        assert!(p.seen.lock().unwrap().is_empty());
        advance(1).await;
        assert_eq!(*p.seen.lock().unwrap(), [false]);
        advance(500).await;
        assert_eq!(p.seen.lock().unwrap().len(), 1);
    }
    #[tokio::test(start_paused = true)]
    async fn a_burst_of_changes_notifies_once_per_throttle_window() {
        let p = probe();
        for _ in 0..5 {
            p.pipeline.dispatch(WatchEventKind::Change, &p.path);
            advance(10).await;
        }
        assert_eq!(p.seen.lock().unwrap().len(), 1);
        advance(10).await;
        p.pipeline.dispatch(WatchEventKind::Change, &p.path);
        assert_eq!(p.seen.lock().unwrap().len(), 2);
    }
    #[tokio::test(start_paused = true)]
    async fn events_of_other_paths_are_ignored() {
        let p = probe();
        p.pipeline
            .dispatch(WatchEventKind::Change, &p.e.path("unrelated.txt"));
        p.pipeline
            .dispatch(WatchEventKind::Unlink, &p.e.path("unrelated.txt"));
        advance(500).await;
        assert!(p.seen.lock().unwrap().is_empty());
    }
}

mod native {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;
    fn listener() -> (FsChanged, UnboundedReceiver<Option<hoh_error::BoxError>>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(move |error| {
                let _ = tx.send(error);
            }),
            rx,
        )
    }
    async fn next(rx: &mut UnboundedReceiver<Option<hoh_error::BoxError>>, wait: u64) -> bool {
        tokio::time::timeout(Duration::from_millis(wait), rx.recv())
            .await
            .is_ok_and(|event| event.is_some_and(|e| e.is_none()))
    }
    #[tokio::test]
    async fn a_file_recreated_within_the_window_never_looks_deleted() {
        let e = Env::new();
        let t = e.file("file.txt", "initial").await;
        let (changed, mut rx) = listener();
        let watched = e.path("file.txt");
        let exists = Arc::new(Mutex::new(vec![]));
        let record = exists.clone();
        let watcher =
            e.fs.watch(
                &t,
                Arc::new(move |error| {
                    changed(error);
                    record
                        .lock()
                        .unwrap()
                        .push(std::path::Path::new(&watched).exists());
                }),
                AbortController::new().signal(),
            )
            .await
            .unwrap();
        tokio::fs::remove_file(e.path("file.txt")).await.unwrap();
        tokio::time::sleep(ms(30)).await;
        tokio::fs::write(e.path("file.txt"), "again").await.unwrap();
        assert!(next(&mut rx, 20_000).await);
        tokio::time::sleep(ms(300)).await;
        assert!(
            exists.lock().unwrap().iter().all(|present| *present),
            "a callback ran while the file was absent: {:?}",
            exists.lock().unwrap()
        );
        watcher.close().await.unwrap();
    }
    async fn assert_target_change_reaches(
        e: &Env,
        outside: &str,
        rx: &mut UnboundedReceiver<Option<hoh_error::BoxError>>,
    ) {
        // Link watches of children created after readiness are added asynchronously: retry.
        for round in 0..40 {
            while rx.try_recv().is_ok() {}
            tokio::fs::write(e.path(outside), format!("changed {round}"))
                .await
                .unwrap();
            if next(rx, 500).await {
                return;
            }
        }
        panic!("no notification for a change of the link target");
    }
    #[tokio::test]
    async fn a_change_of_a_followed_symlink_target_notifies_the_directory_watch() {
        let e = Env::new();
        e.file("outside.txt", "initial").await;
        tokio::fs::create_dir(e.path("d")).await.unwrap();
        std::os::unix::fs::symlink("../outside.txt", e.path("d/link.txt")).unwrap();
        let dir = e.target("d").await;
        let (changed, mut rx) = listener();
        let watcher =
            e.fs.watch(&dir, changed, AbortController::new().signal())
                .await
                .unwrap();
        while rx.try_recv().is_ok() {}
        tokio::fs::write(e.path("outside.txt"), "changed")
            .await
            .unwrap();
        assert!(next(&mut rx, 20_000).await);
        watcher.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_symlink_created_after_readiness_is_followed_too() {
        let e = Env::new();
        e.file("outside.txt", "initial").await;
        tokio::fs::create_dir(e.path("d")).await.unwrap();
        let dir = e.target("d").await;
        let (changed, mut rx) = listener();
        let watcher =
            e.fs.watch(&dir, changed, AbortController::new().signal())
                .await
                .unwrap();
        std::os::unix::fs::symlink("../outside.txt", e.path("d/late.txt")).unwrap();
        assert!(next(&mut rx, 20_000).await);
        assert_target_change_reaches(&e, "outside.txt", &mut rx).await;
        watcher.close().await.unwrap();
    }
    #[tokio::test]
    async fn a_late_symlink_is_followed_before_its_add_callback_changes_the_target() {
        let e = Env::new();
        e.file("outside.txt", "initial").await;
        tokio::fs::create_dir(e.path("d")).await.unwrap();
        let link = e.path("d/link.txt");
        let installed = Arc::new(AtomicBool::new(false));
        let (installed_tx, installed_rx) = tokio::sync::oneshot::channel();
        let installed_tx = Arc::new(Mutex::new(Some(installed_tx)));
        {
            let installed = installed.clone();
            let installed_tx = installed_tx.clone();
            let expected = link.clone();
            e.hooks().inspect_watch_link = Some(Arc::new(move |path| {
                if path == expected {
                    installed.store(true, Ordering::SeqCst);
                    if let Some(tx) = installed_tx.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                }
            }));
        }
        let dir = e.target("d").await;
        let calls = Arc::new(AtomicUsize::new(0));
        let installed_at_add = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let changed: FsChanged = {
            let calls = calls.clone();
            let installed = installed.clone();
            let installed_at_add = installed_at_add.clone();
            let outside = e.path("outside.txt");
            Arc::new(move |error| {
                assert!(error.is_none());
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    installed_at_add.store(installed.load(Ordering::SeqCst), Ordering::SeqCst);
                    std::fs::write(&outside, "changed in add callback").unwrap();
                }
                let _ = tx.send(call);
            })
        };
        let watcher =
            e.fs.watch(&dir, changed, AbortController::new().signal())
                .await
                .unwrap();
        std::os::unix::fs::symlink("../outside.txt", &link).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(20), rx.recv())
                .await
                .unwrap(),
            Some(0)
        );
        tokio::time::timeout(Duration::from_secs(20), installed_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            installed_at_add.load(Ordering::SeqCst),
            "the add callback ran before the followed-link watch was installed"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(20), rx.recv())
                .await
                .unwrap(),
            Some(1)
        );
        assert_eq!(e.bytes("outside.txt").await, "changed in add callback");
        watcher.close().await.unwrap();
    }
    #[tokio::test]
    async fn add_dispatch_does_not_wait_for_the_native_mutex_during_link_watch_installation() {
        const FILES: usize = 128;
        let e = Env::new();
        e.file("outside.txt", "initial").await;
        tokio::fs::create_dir(e.path("d")).await.unwrap();
        let link = e.path("d/link.txt");
        let ordinary = (0..FILES)
            .map(|index| e.path(&format!("d/x-{index:03}.txt")))
            .collect::<Vec<_>>();
        let expected = std::iter::once(link.clone())
            .chain(ordinary.iter().cloned())
            .collect::<HashSet<_>>();

        let (held_tx, held_rx) = tokio::sync::oneshot::channel();
        let held_tx = Arc::new(Mutex::new(Some(held_tx)));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(Mutex::new(Some(release_rx)));
        let continue_watch = Arc::new(AtomicBool::new(false));
        let gated = Arc::new(AtomicBool::new(false));
        {
            let expected_link = link.clone();
            let held_tx = held_tx.clone();
            let release_rx = release_rx.clone();
            let continue_watch = continue_watch.clone();
            let gated = gated.clone();
            e.hooks().before_watch_link = Some(Arc::new(move |path| {
                if path == expected_link && !gated.swap(true, Ordering::SeqCst) {
                    if let Some(tx) = held_tx.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    if let Some(rx) = release_rx.lock().unwrap().take() {
                        let _ = rx.recv();
                    }
                    return continue_watch.load(Ordering::SeqCst);
                }
                true
            }));
        }

        let probe = ordinary[0].clone();
        let (progressed_tx, mut progressed_rx) = tokio::sync::oneshot::channel();
        let progressed_tx = Arc::new(Mutex::new(Some(progressed_tx)));
        let (emitted_tx, emitted_rx) = tokio::sync::oneshot::channel();
        let emitted_tx = Arc::new(Mutex::new(Some(emitted_tx)));
        let emitted = Arc::new(Mutex::new(HashSet::new()));
        {
            let progressed_tx = progressed_tx.clone();
            let emitted_tx = emitted_tx.clone();
            let emitted = emitted.clone();
            e.hooks().inspect_watch_add = Some(Arc::new(move |path, after_callback| {
                if !after_callback
                    && path == probe
                    && let Some(tx) = progressed_tx.lock().unwrap().take()
                {
                    let _ = tx.send(());
                }
                if after_callback && expected.contains(&path) {
                    let mut seen = emitted.lock().unwrap();
                    seen.insert(path);
                    if seen.len() == expected.len()
                        && let Some(tx) = emitted_tx.lock().unwrap().take()
                    {
                        let _ = tx.send(());
                    }
                }
            }));
        }

        let (changed, mut rx) = listener();
        let dir = e.target("d").await;
        let watcher =
            e.fs.watch(&dir, changed, AbortController::new().signal())
                .await
                .unwrap();
        std::os::unix::fs::symlink("../outside.txt", &link).unwrap();
        tokio::time::timeout(Duration::from_secs(20), held_rx)
            .await
            .unwrap()
            .unwrap();
        let files = ordinary.clone();
        let creator = tokio::task::spawn_blocking(move || {
            for path in files {
                std::fs::File::create(path).unwrap();
            }
        });
        let progressed = tokio::time::timeout(Duration::from_secs(2), &mut progressed_rx)
            .await
            .is_ok();
        continue_watch.store(progressed, Ordering::SeqCst);
        release_tx.send(()).unwrap();
        creator.await.unwrap();
        if !progressed {
            tokio::time::timeout(Duration::from_secs(20), watcher.close())
                .await
                .unwrap()
                .unwrap();
            panic!("a second Add waited for the native mutex held by watch(link)");
        }
        tokio::time::timeout(Duration::from_secs(20), emitted_rx)
            .await
            .unwrap()
            .unwrap();
        while rx.try_recv().is_ok() {}
        tokio::fs::write(e.path("outside.txt"), "changed once")
            .await
            .unwrap();
        assert!(next(&mut rx, 20_000).await);
        tokio::time::timeout(Duration::from_secs(20), watcher.close())
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn a_change_outside_the_watched_directory_without_a_link_is_not_reported() {
        let e = Env::new();
        e.file("outside.txt", "initial").await;
        e.file("plain.txt", "initial").await;
        tokio::fs::create_dir(e.path("d")).await.unwrap();
        let dir = e.target("d").await;
        let (changed, mut rx) = listener();
        let watcher =
            e.fs.watch(&dir, changed, AbortController::new().signal())
                .await
                .unwrap();
        tokio::fs::write(e.path("outside.txt"), "changed")
            .await
            .unwrap();
        assert!(!next(&mut rx, 600).await);
        watcher.close().await.unwrap();
    }
}
