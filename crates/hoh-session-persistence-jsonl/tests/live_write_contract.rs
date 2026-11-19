mod common;
use common::*;
use hoh_session::{CreateSessionOptions, Session, SessionStore};
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{testing::*, *};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
async fn fixture(hook: Option<IoHook>) -> (Fixture, SessionStore, Session) {
    let root = tempfile::tempdir().unwrap();
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let mut config = Config::new(root.path().to_string_lossy());
    config.compression = JsonlCompression::None;
    let backend = create_jsonl_test_backend(&ctx, config, hook).await.unwrap();
    let store = SessionStore::new();
    let s = store
        .create(&ctx, Some("live".into()), CreateSessionOptions::default())
        .unwrap();
    (
        Fixture {
            kernel,
            root,
            backend,
        },
        store,
        s,
    )
}
async fn writer(f: &Fixture, s: &Session) -> Arc<dyn SessionHandle> {
    let header = serde_json::from_value((*s.header()).clone()).unwrap();
    f.backend.create(&header, None).await.unwrap()
}
fn append_turn(s: &Session) {
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
}
#[tokio::test]
async fn a_session_without_an_active_write_handle_persists_nothing() {
    let (f, store, s) = fixture(None).await;
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    store.flush(&s).await.unwrap();
    assert!(f.backend.stat(s.id(), None).await.unwrap().is_none());
    f.finish().await;
}
#[tokio::test]
async fn persists_a_plugin_record_that_a_restarted_process_reads_back() {
    let (f, store, s) = fixture(None).await;
    let w = writer(&f, &s).await;
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    hoh_session::append_plugin_record(&s, "plugin:persistence-test/state", json!({"count":1}))
        .unwrap();
    assert!(store.flush(&s).await.unwrap());
    w.close().await.unwrap();
    let mut config = Config::new(f.root.path().to_string_lossy());
    config.compression = JsonlCompression::None;
    let other = JsonlSessionPersistence::new(&f.kernel.root(), config)
        .await
        .unwrap();
    let r = other.open(s.id(), SessionAccess::Read, None).await.unwrap();
    let rows = read(r.as_ref()).await;
    assert_eq!(rows.len(), 2);
    assert!(rows[0].get("ignorable").is_none());
    assert_eq!(rows[1]["type"], "plugin:persistence-test/state");
    assert_eq!(rows[1]["ignorable"], true);
    assert_eq!(rows[1]["data"], json!({"count":1}));
    r.close().await.unwrap();
    other.close().await.unwrap();
    f.finish().await;
}
#[tokio::test(start_paused = true)]
async fn session_flush_drains_immediately_and_surfaces_a_retained_background_failure() {
    let failing = Arc::new(AtomicBool::new(false));
    let flag = failing.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if flag.load(Ordering::SeqCst) && r.operation == IoOperation::Write && !r.after {
                return Err(
                    Box::new(std::io::Error::other("backend write refused")) as hoh_error::BoxError
                );
            }
            Ok(())
        })
    });
    let (f, store, s) = fixture(Some(hook)).await;
    let w = writer(&f, &s).await;
    w.flush(None).await.unwrap();
    let warned = Arc::new(tokio::sync::Notify::new());
    let notice = warned.clone();
    let _export = f
        .kernel
        .root()
        .logger()
        .exporter(Default::default(), move |record| {
            if record
                .message
                .contains("background write for session \"live\" failed")
            {
                notice.notify_one();
            }
        })
        .unwrap();
    failing.store(true, Ordering::SeqCst);
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_millis(200)).await;
    warned.notified().await;
    fail(store.flush(&s).await, "backend write refused");
    failing.store(false, Ordering::SeqCst);
    assert!(store.flush(&s).await.unwrap());
    assert_eq!(read(w.as_ref()).await.len(), 1);
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn service_level_flush_drains_every_active_handle_and_aggregates_the_failures() {
    let fail_on = Arc::new(AtomicBool::new(true));
    let flag = fail_on.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if flag.load(Ordering::SeqCst)
                && r.operation == IoOperation::Write
                && !r.after
                && r.path.contains("/failing/")
            {
                return Err(
                    Box::new(std::io::Error::other("backend write refused")) as hoh_error::BoxError
                );
            }
            Ok(())
        })
    });
    let (f, store, healthy) = fixture(Some(hook)).await;
    let failing = store
        .create(
            &f.kernel.root(),
            Some("failing".into()),
            CreateSessionOptions::default(),
        )
        .unwrap();
    let a = writer(&f, &healthy).await;
    let b = writer(&f, &failing).await;
    healthy
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    failing
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    let e = fail(
        f.backend.flush().await,
        "session-persistence-jsonl flush failed",
    );
    let JsonlError::Aggregate { errors, .. } = e.downcast_ref::<JsonlError>().unwrap() else {
        panic!("aggregate")
    };
    assert_eq!(errors.len(), 1);
    assert!(errors[0].to_string().contains("backend write refused"));
    assert_eq!(read(a.as_ref()).await.len(), 1);
    fail_on.store(false, Ordering::SeqCst);
    f.backend.flush().await.unwrap();
    assert_eq!(read(b.as_ref()).await.len(), 1);
    a.close().await.unwrap();
    b.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn backend_teardown_drains_buffered_events_through_the_close_sweep() {
    let (f, _, s) = fixture(None).await;
    let w = writer(&f, &s).await;
    append_turn(&s);
    f.backend.close().await.unwrap();
    fail(w.append(&[], None).await, "closed handle");
    let r = f
        .backend
        .open(s.id(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await.len(), 2);
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn close_itself_surfaces_a_failing_drain_and_still_releases_write_ownership() {
    let flag = Arc::new(AtomicBool::new(true));
    let enabled = flag.clone();
    let hook: IoHook = Arc::new(move |r| {
        let enabled = enabled.clone();
        Box::pin(async move {
            if enabled.load(Ordering::SeqCst) && r.operation == IoOperation::Write && !r.after {
                return Err(Box::new(std::io::Error::other("storage refused the drain"))
                    as hoh_error::BoxError);
            }
            Ok(())
        })
    });
    let (f, _, s) = fixture(Some(hook)).await;
    let w = writer(&f, &s).await;
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    fail(w.close().await, "storage refused the drain");
    flag.store(false, Ordering::SeqCst);
    writer(&f, &s).await.close().await.unwrap();
    f.finish().await;
}
#[tokio::test(start_paused = true)]
async fn a_failed_drain_retains_order_quiets_the_timer_and_recovers_exactly_once() {
    let flag = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let (enabled, count) = (flag.clone(), calls.clone());
    let hook: IoHook = Arc::new(move |r| {
        let (enabled, count) = (enabled.clone(), count.clone());
        Box::pin(async move {
            if enabled.load(Ordering::SeqCst) && r.operation == IoOperation::Write && !r.after {
                count.fetch_add(1, Ordering::SeqCst);
                return Err(
                    Box::new(std::io::Error::other("first drain refused")) as hoh_error::BoxError
                );
            }
            Ok(())
        })
    });
    let (f, store, s) = fixture(Some(hook)).await;
    let w = writer(&f, &s).await;
    w.flush(None).await.unwrap();
    flag.store(true, Ordering::SeqCst);
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    s.append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    let live = hoh_session_persistence_jsonl::testing::writer(&f.backend, s.id()).unwrap();
    fail(live.drain_live().await, "first drain refused");
    s.append("step/end", json!({"turn":1,"step":1}), None)
        .unwrap();
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    tokio::time::advance(std::time::Duration::from_millis(1000)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    flag.store(false, Ordering::SeqCst);
    assert!(store.flush(&s).await.unwrap());
    assert_eq!(
        read(w.as_ref())
            .await
            .iter()
            .map(|e| e["seq"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn a_second_write_handle_after_close_routes_subsequent_events() {
    let (f, store, s) = fixture(None).await;
    let first = writer(&f, &s).await;
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    store.flush(&s).await.unwrap();
    first.close().await.unwrap();
    let second = f
        .backend
        .open(s.id(), SessionAccess::Write, None)
        .await
        .unwrap();
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    store.flush(&s).await.unwrap();
    assert_eq!(read(second.as_ref()).await.len(), 2);
    second.close().await.unwrap();
    f.finish().await;
}

async fn disposed(fails: bool) {
    let hook: IoHook = Arc::new(move |r| {
        Box::pin(async move {
            if fails && r.operation == IoOperation::Write && !r.after {
                return Err(
                    Box::new(std::io::Error::other("drain exploded")) as hoh_error::BoxError
                );
            }
            Ok(())
        })
    });
    let (f, store, _) = fixture(Some(hook)).await;
    let s = store
        .prepare(Some("disposed".into()), Default::default())
        .unwrap();
    let detach = store.enter(&f.kernel.root(), s.clone()).unwrap();
    store.announce(&s).unwrap();
    let h = writer(&f, &s).await;
    let notified = Arc::new(tokio::sync::Notify::new());
    let notice = notified.clone();
    let _export = f
        .kernel
        .root()
        .logger()
        .exporter(Default::default(), move |record| {
            if record
                .message
                .contains("final drain for session \"disposed\" failed")
            {
                notice.notify_one();
            }
        })
        .unwrap();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    detach.detach();
    let mut closed = false;
    for _ in 0..10000 {
        match h.append(&[], None).await {
            Err(error) => {
                assert!(has_class::<SessionHandleClosedError>(error.as_ref()));
                closed = true;
                break;
            }
            Ok(()) => tokio::task::yield_now().await,
        }
    }
    assert!(closed, "session/disposed did not close its routed handle");
    if fails {
        fail(h.close().await, "drain exploded");
        notified.notified().await;
    } else {
        h.close().await.unwrap();
        let r = f
            .backend
            .open(s.id(), SessionAccess::Read, None)
            .await
            .unwrap();
        assert_eq!(read(r.as_ref()).await.len(), 1);
        r.close().await.unwrap();
    }
    f.finish().await;
}
#[tokio::test]
async fn session_disposed_drains_buffered_events_and_closes_the_handle() {
    disposed(false).await;
}
#[tokio::test]
async fn a_failing_final_drain_on_disposal_is_warned_not_thrown() {
    disposed(true).await;
}
