mod common;
use common::*;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{
    format::session_dir,
    lease::*,
    testing::{IoHook, IoOperation, NodeFileError},
    *,
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
async fn second(f: &Fixture) -> JsonlSessionPersistence {
    let mut config = Config::new(f.root.path().to_string_lossy());
    config.compression = JsonlCompression::None;
    JsonlSessionPersistence::new(&f.kernel.root(), config)
        .await
        .unwrap()
}
fn lock_path(f: &Fixture, id: &str) -> String {
    format!(
        "{}/{}",
        session_dir(&f.root.path().to_string_lossy(), None, &id.into()).unwrap(),
        LEASE_FILENAME
    )
}
fn events() -> Vec<serde_json::Value> {
    vec![
        event("turn/start", 0, json!({"turn":1})),
        event(
            "turn/end",
            1,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ]
}
#[tokio::test]
async fn excludes_a_second_instance_while_the_holder_is_live_and_admits_it_after_close() {
    let f = Fixture::new(JsonlCompression::None).await;
    let other = second(&f).await;
    let h = f.backend.create(&meta("excluded"), None).await.unwrap();
    h.append(&arcs(&events()), None).await.unwrap();
    assert!(has_class::<SessionAlreadyExistsError>(
        fail(other.create(&meta("excluded"), None).await, "exists").as_ref()
    ));
    assert!(has_class::<SessionAlreadyOwnedError>(
        fail(
            other
                .open(&"excluded".into(), SessionAccess::Write, None)
                .await,
            "owned"
        )
        .as_ref()
    ));
    let a = f.backend.create(&meta("pending"), None).await.unwrap();
    let b = other.create(&meta("pending"), None).await.unwrap();
    a.append(&arcs(&events()), None).await.unwrap();
    assert!(has_class::<SessionAlreadyOwnedError>(
        fail(b.append(&arcs(&events()), None).await, "owned").as_ref()
    ));
    b.close().await.unwrap();
    a.close().await.unwrap();
    let r = other
        .open(&"excluded".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await, events());
    r.close().await.unwrap();
    h.close().await.unwrap();
    assert!(std::path::Path::new(&lock_path(&f, "excluded")).exists());
    let w = other
        .open(&"excluded".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    w.append(&arcs(&[event("turn/start", 2, json!({"turn":2}))]), None)
        .await
        .unwrap();
    w.close().await.unwrap();
    other.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn removing_the_lock_file_forfeits_a_wedged_holder() {
    let f = Fixture::new(JsonlCompression::None).await;
    let other = second(&f).await;
    let w = f.backend.create(&meta("wedged"), None).await.unwrap();
    w.append(&arcs(&events()), None).await.unwrap();
    tokio::fs::remove_file(lock_path(&f, "wedged"))
        .await
        .unwrap();
    let successor = other
        .open(&"wedged".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    successor
        .append(&arcs(&[event("turn/start", 2, json!({"turn":2}))]), None)
        .await
        .unwrap();
    successor.close().await.unwrap();
    w.close().await.unwrap();
    other.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn write_opening_an_absent_session_leaves_no_lock_residue() {
    let f = Fixture::new(JsonlCompression::None).await;
    assert!(has_class::<SessionPersistenceNotFoundError>(
        fail(
            f.backend
                .open(&"absent".into(), SessionAccess::Write, None)
                .await,
            "not found"
        )
        .as_ref()
    ));
    assert!(!std::path::Path::new(&lock_path(&f, "absent")).exists());
    f.finish().await;
}
#[tokio::test]
async fn write_opening_an_absent_id_under_an_existing_project_reports_not_found() {
    let f = Fixture::new(JsonlCompression::None).await;
    let w = f.backend.create(&meta("sibling"), None).await.unwrap();
    w.append(&arcs(&events()), None).await.unwrap();
    w.close().await.unwrap();
    assert!(has_class::<SessionPersistenceNotFoundError>(
        fail(
            f.backend
                .open(&"absent".into(), SessionAccess::Write, None)
                .await,
            "not found"
        )
        .as_ref()
    ));
    f.finish().await;
}
#[tokio::test]
async fn a_never_materialized_create_leaves_no_filesystem_footprint() {
    let f = Fixture::new(JsonlCompression::None).await;
    let w = f.backend.create(&meta("erased"), None).await.unwrap();
    assert!(
        !std::path::Path::new(&lock_path(&f, "erased"))
            .parent()
            .unwrap()
            .exists()
    );
    w.close().await.unwrap();
    assert!(
        f.backend
            .stat(&"erased".into(), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!std::path::Path::new(&lock_path(&f, "erased")).exists());
    f.finish().await;
}
#[tokio::test]
async fn materialization_publishes_the_lock_before_first_log_bytes_and_reuses_it() {
    let n = Arc::new(AtomicUsize::new(0));
    let calls = n.clone();
    let hook: IoHook = Arc::new(move |r| {
        let calls = calls.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Flock && !r.after {
                calls.fetch_add(1, Ordering::SeqCst);
            }
            if r.operation == IoOperation::Write && !r.after {
                assert!(
                    std::path::Path::new(&r.path)
                        .parent()
                        .unwrap()
                        .join(LEASE_FILENAME)
                        .exists()
                );
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let w = f.backend.create(&meta("lazy-lock"), None).await.unwrap();
    w.append(&arcs(&events()), None).await.unwrap();
    w.append(&arcs(&[event("turn/start", 2, json!({"turn":2}))]), None)
        .await
        .unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn an_explicitly_flushed_empty_session_takes_the_lock_with_its_header() {
    let f = Fixture::new(JsonlCompression::None).await;
    let other = second(&f).await;
    let w = f.backend.create(&meta("empty"), None).await.unwrap();
    w.flush(None).await.unwrap();
    assert!(has_class::<SessionAlreadyOwnedError>(
        fail(
            other
                .open(&"empty".into(), SessionAccess::Write, None)
                .await,
            "owned"
        )
        .as_ref()
    ));
    w.close().await.unwrap();
    other.close().await.unwrap();
    f.finish().await;
}
async fn refused_operation(operation: IoOperation, code: i32, expected: &str, owned: bool) {
    let armed = Arc::new(AtomicBool::new(false));
    let enabled = armed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let enabled = enabled.clone();
        Box::pin(async move {
            if !r.after
                && r.path.ends_with(LEASE_FILENAME)
                && r.operation == operation
                && enabled.swap(false, Ordering::SeqCst)
            {
                return Err(Box::new(NodeFileError::new(
                    std::io::Error::from_raw_os_error(code),
                    "injected",
                    &r.path,
                    None,
                )) as hoh_error::BoxError);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let w = f.backend.create(&meta("refusal"), None).await.unwrap();
    w.append(&arcs(&events()), None).await.unwrap();
    w.close().await.unwrap();
    armed.store(true, Ordering::SeqCst);
    let e = fail(
        f.backend
            .open(&"refusal".into(), SessionAccess::Write, None)
            .await,
        expected,
    );
    if owned {
        assert!(has_class::<SessionAlreadyOwnedError>(e.as_ref()));
    }
    let w = f
        .backend
        .open(&"refusal".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn surfaces_a_filesystem_refusal_opening_the_lock_file() {
    refused_operation(IoOperation::Open, 13, "EACCES", false).await;
}
#[tokio::test]
async fn surfaces_a_non_contention_flock_failure() {
    refused_operation(IoOperation::Flock, 13, "EACCES", false).await;
}
#[tokio::test]
async fn maps_the_ewouldblock_contention_spelling_to_already_owned() {
    refused_operation(IoOperation::Flock, 11, "owned", true).await;
}
#[tokio::test]
async fn surfaces_a_lock_path_stat_refusal_from_inode_verification() {
    refused_operation(IoOperation::Stat, 13, "EACCES", false).await;
}
async fn churn(times: usize, drop_only: bool) {
    let remaining = Arc::new(AtomicUsize::new(times));
    let count = remaining.clone();
    let hook: IoHook = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            if !r.after
                && r.path.ends_with(LEASE_FILENAME)
                && r.operation == IoOperation::Stat
                && count
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
            {
                tokio::fs::remove_file(&r.path).await.unwrap();
                if !drop_only {
                    tokio::fs::write(&r.path, b"").await.unwrap();
                }
            }
            Ok(())
        })
    });
    let root = tempfile::tempdir().unwrap();
    let result = SessionWriteLease::acquire_with_io_hook(
        &root.path().to_string_lossy(),
        &"churn".into(),
        Some(hook),
    )
    .await;
    if times == 3 {
        assert!(has_class::<SessionAlreadyOwnedError>(
            fail(result, "owned").as_ref()
        ));
    } else {
        result.unwrap().release().await.unwrap();
    }
    assert_eq!(remaining.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn retries_when_the_locked_inode_is_no_longer_the_lock_path() {
    churn(1, false).await;
}
#[tokio::test]
async fn retries_when_the_lock_path_vanishes_under_the_verify_read() {
    churn(1, true).await;
}
#[tokio::test]
async fn gives_up_as_already_owned_when_the_lock_path_never_stabilizes() {
    churn(3, false).await;
}
#[tokio::test]
async fn a_failing_lock_release_still_frees_the_in_process_claim_on_close() {
    let armed = Arc::new(AtomicBool::new(false));
    let flag = armed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Close
                && r.after
                && r.path.ends_with(LEASE_FILENAME)
                && flag.swap(false, Ordering::SeqCst)
            {
                return Err(Box::new(std::io::Error::other("injected release failure"))
                    as hoh_error::BoxError);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let w = f.backend.create(&meta("release"), None).await.unwrap();
    w.append(&arcs(&events()), None).await.unwrap();
    armed.store(true, Ordering::SeqCst);
    fail(w.close().await, "injected release failure");
    let w = f
        .backend
        .open(&"release".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn release_is_idempotent_and_never_removes_the_lock_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_string_lossy();
    let lease = SessionWriteLease::acquire(&path, &"solo".into())
        .await
        .unwrap();
    lease.release().await.unwrap();
    lease.release().await.unwrap();
    assert!(root.path().join(LEASE_FILENAME).exists());
    SessionWriteLease::acquire(&path, &"solo".into())
        .await
        .unwrap()
        .release()
        .await
        .unwrap();
    assert!(root.path().join(LEASE_FILENAME).exists());
}
#[tokio::test]
async fn keeps_distinct_sessions_independently_lockable() {
    let f = Fixture::new(JsonlCompression::None).await;
    let a = f.backend.create(&meta("a"), None).await.unwrap();
    let b = f.backend.create(&meta("b"), None).await.unwrap();
    a.append(&arcs(&events()), None).await.unwrap();
    b.append(&arcs(&events()), None).await.unwrap();
    a.close().await.unwrap();
    b.close().await.unwrap();
    f.finish().await;
}

#[tokio::test]
async fn a_write_open_failure_with_a_failing_release_aggregates_both_and_frees_the_claim() {
    let armed = Arc::new(AtomicBool::new(false));
    let flag = armed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Close
                && r.after
                && r.path.ends_with(LEASE_FILENAME)
                && flag.swap(false, Ordering::SeqCst)
            {
                return Err(Box::new(std::io::Error::other("injected release failure"))
                    as hoh_error::BoxError);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let (path, mut bytes) = f.store(&meta("open-and-release"), &events()).await;
    bytes[0] = b'#';
    tokio::fs::write(path, bytes).await.unwrap();
    armed.store(true, Ordering::SeqCst);
    let e = fail(
        f.backend
            .open(&"open-and-release".into(), SessionAccess::Write, None)
            .await,
        "write open failed and its lock release failed",
    );
    let JsonlError::Aggregate { errors, .. } = e.downcast_ref::<JsonlError>().unwrap() else {
        panic!("aggregate")
    };
    assert_eq!(errors.len(), 2);
    assert!(errors[0].to_string().contains("corrupt"));
    assert!(errors[1].to_string().contains("injected release failure"));
    fail(
        f.backend
            .open(&"open-and-release".into(), SessionAccess::Write, None)
            .await,
        "corrupt",
    );
    f.finish().await;
}
#[tokio::test]
async fn a_drain_failure_and_release_failure_reject_close_as_one_aggregate_error() {
    let armed = Arc::new(AtomicBool::new(false));
    let flag = armed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if flag.load(Ordering::SeqCst)
                && !r.after
                && r.operation == IoOperation::Write
                && r.path.ends_with(".jsonl")
            {
                return Err(Box::new(std::io::Error::other("injected drain refusal"))
                    as hoh_error::BoxError);
            }
            if r.operation == IoOperation::Close
                && r.after
                && r.path.ends_with(LEASE_FILENAME)
                && flag.swap(false, Ordering::SeqCst)
            {
                return Err(Box::new(std::io::Error::other("injected release failure"))
                    as hoh_error::BoxError);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let h = f
        .backend
        .create(&meta("drain-and-release"), None)
        .await
        .unwrap();
    h.append(&arcs(&events()), None).await.unwrap();
    armed.store(true, Ordering::SeqCst);
    let live = testing::writer(&f.backend, &"drain-and-release".into()).unwrap();
    live.enqueue_live(
        &hoh_session::SessionEvent::from_value(event("turn/start", 2, json!({"turn":2}))),
        no_op_report(),
    );
    let e = fail(
        h.close().await,
        "close failed to drain and to release its write lock",
    );
    let JsonlError::Aggregate { errors, .. } =
        underlying(e.as_ref()).downcast_ref::<JsonlError>().unwrap()
    else {
        panic!("aggregate")
    };
    assert_eq!(errors.len(), 2);
    assert!(errors[0].to_string().contains("drain refusal"));
    assert!(errors[1].to_string().contains("release failure"));
    let w = f
        .backend
        .open(&"drain-and-release".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    w.close().await.unwrap();
    f.finish().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn del2_lease_does_not_follow_a_session_directory_symlink() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = root.path().join("session");
    symlink(outside.path(), &link).unwrap();
    assert!(
        SessionWriteLease::acquire(link.to_str().unwrap(), &"session".into())
            .await
            .is_err()
    );
    assert!(!outside.path().join("session.lock").exists());
}

#[tokio::test]
async fn del3_open_flags_are_architecture_independent_and_ordinary_sessions_open() {
    let lease = include_str!("../src/lease.rs");
    let backend = include_str!("../src/backend.rs");
    assert!(!lease.contains("custom_flags(0x"));
    assert!(!backend.contains("custom_flags(0x"));
    let f = Fixture::new(JsonlCompression::None).await;
    let h = f.backend.create(&meta("del3-normal"), None).await.unwrap();
    h.append(&arcs(&events()), None).await.unwrap();
    h.close().await.unwrap();
    let h = f
        .backend
        .open(&"del3-normal".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    h.close().await.unwrap();
    f.backend.trash(&"del3-normal".into()).await.unwrap();
    f.finish().await;
}

#[tokio::test]
async fn del3_failed_final_append_keeps_the_writer_and_blocks_repeated_trash() {
    let armed = Arc::new(AtomicBool::new(false));
    let flag = armed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let fail = flag.load(Ordering::SeqCst)
            && !r.after
            && r.operation == IoOperation::Write
            && r.path.ends_with(".jsonl");
        Box::pin(async move {
            if fail {
                Err(std::io::Error::other("tail write failed").into())
            } else {
                Ok(())
            }
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let id = "del3-tail".into();
    let h = f.backend.create(&meta("del3-tail"), None).await.unwrap();
    h.append(&arcs(&events()), None).await.unwrap();
    f.backend.prepare_delete(&id).await.unwrap();
    let live = testing::writer(&f.backend, &id).unwrap();
    live.enqueue_live(
        &hoh_session::SessionEvent::from_value(event("turn/start", 2, json!({"turn":2}))),
        Arc::new(|_| {}),
    );
    armed.store(true, Ordering::SeqCst);
    assert!(h.close().await.is_err());
    assert!(
        f.backend.trash(&id).await.is_err(),
        "first Delete moved a truncated log"
    );
    assert!(
        f.backend.trash(&id).await.is_err(),
        "repeated Delete moved a truncated log"
    );
    armed.store(false, Ordering::SeqCst);
    h.close().await.unwrap();
    f.backend.trash(&id).await.unwrap();
    f.finish().await;
}
