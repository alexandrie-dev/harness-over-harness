mod common;
use common::*;
use hoh_session_persistence::SessionAccess;
use hoh_session_persistence::{SessionPersistence, SessionPersistenceStatOptions};
use hoh_session_persistence_jsonl::{
    JsonlCompression,
    testing::{IoHook, IoOperation, NodeFileError},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[tokio::test]
async fn stat_preserves_cancellation_over_a_failed_uncancellable_header_open_or_identity_stat() {
    for operation in [IoOperation::Open, IoOperation::Stat] {
        for errno in [2, 13] {
            let controller = Arc::new(hoh_timeout::AbortController::new());
            let c = controller.clone();
            let reason = hoh_error::SharedError::new(std::io::Error::other(
                "stat cancelled after discovery",
            ));
            let cancelled = reason.clone();
            let hook: IoHook = Arc::new(move |r| {
                let c = c.clone();
                let cancelled = cancelled.clone();
                Box::pin(async move {
                    if !r.after
                        && r.operation == operation
                        && r.path.ends_with("/cancel-stat/session.v4.jsonl")
                    {
                        c.abort_with(cancelled);
                        return Err(Box::new(NodeFileError::new(
                            std::io::Error::from_raw_os_error(errno),
                            "injected",
                            &r.path,
                            None,
                        )) as hoh_error::BoxError);
                    }
                    Ok(())
                })
            });
            let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
            f.store(&meta("cancel-stat"), &log()).await;
            let error = fail(
                f.backend
                    .stat(
                        &"cancel-stat".into(),
                        Some(&SessionPersistenceStatOptions {
                            signal: Some(controller.signal()),
                        }),
                    )
                    .await,
                "stat cancelled after discovery",
            );
            assert!(
                hoh_error::SharedError::of(error.as_ref())
                    .unwrap()
                    .ptr_eq(&reason)
            );
            f.finish().await;
        }
    }
}
#[tokio::test]
async fn dropping_an_inflight_write_open_releases_its_claim_and_kernel_lease() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let armed = Arc::new(AtomicBool::new(true));
    let (notice, wait, flag) = (entered.clone(), gate, armed);
    let hook: IoHook = Arc::new(move |r| {
        let (notice, wait, flag) = (notice.clone(), wait.clone(), flag.clone());
        Box::pin(async move {
            if r.operation == IoOperation::ReadFile
                && !r.after
                && flag.swap(false, Ordering::SeqCst)
            {
                notice.notify_one();
                wait.acquire().await.unwrap().forget();
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("cancel-open"), &log()).await;
    let backend = f.backend.clone();
    let opening = tokio::spawn(async move {
        backend
            .open(&"cancel-open".into(), SessionAccess::Write, None)
            .await
    });
    entered.notified().await;
    opening.abort();
    match opening.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("open did not cancel"),
    };
    let next = f
        .backend
        .open(&"cancel-open".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    assert_eq!(read(next.as_ref()).await, log());
    next.close().await.unwrap();
    f.finish().await;
}
