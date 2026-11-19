mod support;

use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_terminal::*;
use std::error::Error;
use std::future::{Future, poll_fn};
use std::sync::{
    Arc, Barrier, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::task::Poll;
use support::*;
use tokio::sync::oneshot;

fn named(kind: &str, name: &str) -> TerminalSpawnRequest {
    TerminalSpawnRequest {
        kind: kind.into(),
        name: Some(name.into()),
        cwd: None,
    }
}

fn boxed_session(session: Arc<StubSession>) -> Arc<dyn TerminalBackendSession> {
    session
}

fn assert_same_error(error: &BoxError, expected: &SharedError) {
    let actual = SharedError::of(error.as_ref()).unwrap();
    assert!(actual.ptr_eq(expected));
}

#[tokio::test]
async fn preserves_id_brand_backend_order_and_exact_contributions() {
    assert_eq!(terminal_session_id("pty-1").as_ref(), "pty-1");
    let h = harness().await;
    let (first, _) = backend("first");
    let (second, _) = backend("second");
    let dispose_first = h.terminals.register_backend(&h.ctx, first).unwrap();
    h.terminals.register_backend(&h.ctx, second).unwrap();
    assert_eq!(h.terminals.list_backends(), ["first", "second"]);
    let duplicate = h
        .terminals
        .register_backend(&h.ctx, backend("first").0)
        .err()
        .unwrap();
    assert_eq!(code(duplicate.as_ref()), Some("DUPLICATE_BACKEND"));
    dispose_first.dispose().await;
    assert_eq!(h.terminals.list_backends(), ["second"]);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_empty_backend_types() {
    let h = harness().await;
    let error = h
        .terminals
        .register_backend(&h.ctx, backend("").0)
        .err()
        .unwrap();
    assert!(error.to_string().contains("must be non-empty"));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn publishes_after_spawn_and_fences_every_operation_to_exact_owner() {
    let h = harness().await;
    h.terminals
        .register_backend(&h.ctx, backend("stub").0)
        .unwrap();
    let (_owner_fiber, session_owner) = owner(&h, "owner").await;
    let (_foreign_fiber, foreign) = owner(&h, "foreign").await;
    let created = h
        .terminals
        .spawn(session_owner.clone(), named("stub", "main"), None)
        .await
        .unwrap();
    assert_eq!(created.session_id.as_ref(), "pty-1");
    assert_eq!(
        (created.name.as_deref(), created.pid, created.motd.as_str()),
        (Some("main"), Some(123), "stub ready")
    );
    assert!(h.terminals.has_owner_activity(&session_owner));
    assert_eq!(h.terminals.list(&session_owner).len(), 1);
    assert!(h.terminals.list(&foreign).is_empty());
    let read = h
        .terminals
        .read(
            &foreign,
            &created.session_id,
            TerminalReadRequest::default(),
        )
        .err()
        .unwrap();
    assert_eq!(code(read.as_ref()), Some("FOREIGN_SESSION"));
    let signal = h
        .terminals
        .signal(&foreign, &created.session_id, TerminalSignal::Sigint)
        .await
        .unwrap_err();
    assert_eq!(code(signal.as_ref()), Some("FOREIGN_SESSION"));
    let kill = h
        .terminals
        .kill(&foreign, &created.session_id, None)
        .await
        .unwrap_err();
    assert_eq!(code(kill.as_ref()), Some("FOREIGN_SESSION"));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_unknown_backend_non_live_owner_duplicate_names_and_active_sends() {
    let h = harness().await;
    let not_live = stub_agent(&h.ctx, "not-live");
    let error = h
        .terminals
        .spawn(not_live, spawn_request("missing"), None)
        .await
        .unwrap_err();
    assert_eq!(code(error.as_ref()), Some("OWNER_NOT_LIVE"));
    let (_owner_fiber, owner) = owner(&h, "owner").await;
    let missing = h
        .terminals
        .spawn(owner.clone(), spawn_request("missing"), None)
        .await
        .unwrap_err();
    assert_eq!(code(missing.as_ref()), Some("NO_BACKEND"));
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let created = h
        .terminals
        .spawn(owner.clone(), named("stub", "main"), None)
        .await
        .unwrap();
    let empty = h
        .terminals
        .spawn(owner.clone(), named("stub", ""), None)
        .await
        .unwrap_err();
    assert!(empty.to_string().contains("must be non-empty"));
    let controller = hoh_timeout::AbortController::new();
    controller.abort_with("spawn aborted");
    let aborted = h
        .terminals
        .spawn(
            owner.clone(),
            spawn_request("stub"),
            Some(controller.signal()),
        )
        .await
        .unwrap_err();
    assert_eq!(aborted.to_string(), "spawn aborted");
    let duplicate = h
        .terminals
        .spawn(owner.clone(), named("stub", "main"), None)
        .await
        .unwrap_err();
    assert_eq!(code(duplicate.as_ref()), Some("DUPLICATE_NAME"));
    let operation = h
        .terminals
        .start_send(&owner, &created.session_id, send_request())
        .unwrap();
    let active = h
        .terminals
        .start_send(&owner, &created.session_id, send_request())
        .err()
        .unwrap();
    assert_eq!(code(active.as_ref()), Some("SEND_ACTIVE"));
    assert_eq!(operation.read_output().delta, "delta");
    assert!(operation.cancel());
    operation.done().await.unwrap();
    let next = h
        .terminals
        .start_send(&owner, &created.session_id, send_request())
        .unwrap();
    assert!(next.cancel());
    next.done().await.unwrap();
    lock(&sessions)[0]
        .reject_send
        .store(true, Ordering::Release);
    let rejected = h
        .terminals
        .start_send(&owner, &created.session_id, send_request())
        .unwrap()
        .done()
        .await
        .unwrap_err();
    assert_eq!(rejected.to_string(), "send failed");
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn reserves_concurrent_names_and_rolls_back_when_owner_disappears() {
    let h = harness().await;
    let (send, receive) = oneshot::channel();
    let receive = Arc::new(Mutex::new(Some(receive)));
    let observed_signal = Arc::new(Mutex::new(None));
    let backend_signal = observed_signal.clone();
    let slow = custom_backend("slow", move |spec| {
        *lock(&backend_signal) = spec.signal;
        let receive = lock(&receive).take().unwrap();
        Box::pin(async move { Ok(boxed_session(receive.await.unwrap())) })
    });
    h.terminals.register_backend(&h.ctx, slow).unwrap();
    let (owner_fiber, owner) = owner(&h, "owner").await;
    let pending = h
        .terminals
        .spawn(owner.clone(), named("slow", "main"), None);
    let duplicate = h
        .terminals
        .spawn(owner.clone(), named("slow", "main"), None)
        .await
        .unwrap_err();
    assert_eq!(code(duplicate.as_ref()), Some("DUPLICATE_NAME"));
    let disposal = owner_fiber.disposer().unwrap().dispose();
    let signal = lock(&observed_signal).clone().unwrap();
    let abort_reason = signal.cancelled().await;
    assert_eq!(code(abort_reason.get_ref()), Some("OWNER_NOT_LIVE"));
    let session = Arc::new(StubSession::default());
    assert!(send.send(session.clone()).is_ok());
    let (failure, ()) = tokio::join!(pending, disposal);
    assert_eq!(code(failure.unwrap_err().as_ref()), Some("OWNER_NOT_LIVE"));
    assert_eq!(lock(&session.closed).as_slice(), ["PTY spawn rolled back"]);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_caller_cancellation_when_pending_spawn_completes() {
    let h = harness().await;
    let (send, receive) = oneshot::channel();
    let receive = Arc::new(Mutex::new(Some(receive)));
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("slow", move |_| {
                let receive = lock(&receive).take().unwrap();
                Box::pin(async move { Ok(boxed_session(receive.await.unwrap())) })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let controller = hoh_timeout::AbortController::new();
    let reason = SharedError::new("cancelled by caller");
    let pending = h
        .terminals
        .spawn(owner, spawn_request("slow"), Some(controller.signal()));
    controller.abort_with(reason.clone());
    let session = Arc::new(StubSession::default());
    assert!(send.send(session.clone()).is_ok());
    let error = pending.await.unwrap_err();
    assert_same_error(&error, &reason);
    assert_eq!(lock(&session.closed).as_slice(), ["PTY spawn rolled back"]);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn caller_cancellation_wins_even_when_unpublished_rollback_fails() {
    let h = harness().await;
    let (send, receive) = oneshot::channel();
    let receive = Arc::new(Mutex::new(Some(receive)));
    let session = Arc::new(StubSession::default());
    session.reject_close.store(true, Ordering::Release);
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("slow", move |_| {
                let receive = lock(&receive).take().unwrap();
                Box::pin(async move { Ok(boxed_session(receive.await.unwrap())) })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let controller = hoh_timeout::AbortController::new();
    let reason = SharedError::new("cancelled by caller");
    let pending = h.terminals.spawn(
        owner.clone(),
        spawn_request("slow"),
        Some(controller.signal()),
    );
    controller.abort_with(reason.clone());
    assert!(send.send(session).is_ok());
    let error = pending.await.unwrap_err();
    assert_same_error(&error, &reason);
    assert!(h.terminals.has_owner_activity(&owner));
    assert!(h.terminals.dispose_all().await.is_err());
    assert!(!h.terminals.has_owner_activity(&owner));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn caller_cancellation_wins_over_backend_rejection() {
    let h = harness().await;
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("abortable", |spec| {
                Box::pin(async move {
                    let reason = spec.signal.unwrap().cancelled().await;
                    Err(Box::new(reason) as _)
                })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let controller = hoh_timeout::AbortController::new();
    let reason = SharedError::new("cancelled by caller");
    let pending = h
        .terminals
        .spawn(owner, spawn_request("abortable"), Some(controller.signal()));
    controller.abort_with(reason.clone());
    let error = pending.await.unwrap_err();
    assert_same_error(&error, &reason);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn uncancelled_spawn_preserves_backend_cleanup_error_shape() {
    let h = harness().await;
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("cleanup", |_| {
                Box::pin(async {
                    Err(Box::new(TerminalBackendCleanupError::new(
                        "startup failed",
                        "backend cleanup failed",
                    )) as _)
                })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let error = h
        .terminals
        .spawn(owner.clone(), spawn_request("cleanup"), None)
        .await
        .unwrap_err();
    assert_eq!(code(error.as_ref()), Some("CLEANUP_FAILED"));
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().name(),
        "TerminalBackendCleanupError"
    );
    assert!(h.terminals.has_owner_activity(&owner));
    h.kernel.shutdown().await;
}

async fn retained_cleanup_failure(owner_scope: bool) {
    let h = harness().await;
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("cleanup", |spec| {
                Box::pin(async move {
                    let reason = spec.signal.unwrap().cancelled().await;
                    Err(Box::new(TerminalBackendCleanupError::new(
                        reason,
                        "backend cleanup failed",
                    )) as _)
                })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let controller = hoh_timeout::AbortController::new();
    let reason = SharedError::new("cancelled by caller");
    let pending = h.terminals.spawn(
        owner.clone(),
        spawn_request("cleanup"),
        Some(controller.signal()),
    );
    controller.abort_with(reason.clone());
    let error = pending.await.unwrap_err();
    assert_same_error(&error, &reason);
    assert!(h.terminals.has_owner_activity(&owner));
    let disposal = if owner_scope {
        h.terminals.dispose_owned(owner.clone())
    } else {
        h.terminals.dispose_all()
    };
    assert!(
        disposal
            .await
            .unwrap_err()
            .to_string()
            .contains("failed to clean up PTY lifecycle")
    );
    assert!(!h.terminals.has_owner_activity(&owner));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn owner_disposal_retains_caller_triggered_backend_cleanup_failure() {
    retained_cleanup_failure(true).await;
}

#[tokio::test]
async fn service_disposal_retains_caller_triggered_backend_cleanup_failure() {
    retained_cleanup_failure(false).await;
}

async fn disposal_aborts_and_awaits_spawn(owner_scope: bool) {
    let h = harness().await;
    let (send, receive) = oneshot::channel();
    let receive = Arc::new(Mutex::new(Some(receive)));
    let observed = Arc::new(Mutex::new(None));
    let observed_spawn = observed.clone();
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("slow", move |spec| {
                *lock(&observed_spawn) = spec.signal;
                let receive = lock(&receive).take().unwrap();
                Box::pin(async move { Ok(boxed_session(receive.await.unwrap())) })
            }),
        )
        .unwrap();
    let (owner_fiber, owner) = owner(&h, "owner").await;
    let pending = h
        .terminals
        .spawn(owner.clone(), spawn_request("slow"), None);
    let settled = Arc::new(AtomicBool::new(false));
    let marked = settled.clone();
    let disposal: TerminalFuture<()> = if owner_scope {
        let owner_disposal = owner_fiber.disposer().unwrap().dispose();
        Box::pin(async move {
            owner_disposal.await;
            Ok(())
        })
    } else {
        h.terminals.dispose_all()
    };
    let disposal = async move {
        let result = disposal.await;
        marked.store(true, Ordering::Release);
        result
    };
    tokio::task::yield_now().await;
    let signal = lock(&observed).clone().unwrap();
    assert!(signal.aborted());
    let expected = if owner_scope {
        "OWNER_NOT_LIVE"
    } else {
        "SERVICE_DISPOSING"
    };
    assert_eq!(code(signal.reason().unwrap().get_ref()), Some(expected));
    assert!(!settled.load(Ordering::Acquire));
    let session = Arc::new(StubSession::default());
    assert!(send.send(session.clone()).is_ok());
    let (spawn_result, disposal_result) = tokio::join!(pending, disposal);
    assert_eq!(code(spawn_result.unwrap_err().as_ref()), Some(expected));
    disposal_result.unwrap();
    assert_eq!(lock(&session.closed).as_slice(), ["PTY spawn rolled back"]);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn owner_disposal_aborts_and_awaits_unpublished_backend_setup() {
    disposal_aborts_and_awaits_spawn(true).await;
}

#[tokio::test]
async fn service_disposal_aborts_and_awaits_unpublished_backend_setup() {
    disposal_aborts_and_awaits_spawn(false).await;
}

#[tokio::test]
async fn service_disposal_reports_unpublished_rollback_failure() {
    let h = harness().await;
    let (send, receive) = oneshot::channel();
    let receive = Arc::new(Mutex::new(Some(receive)));
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("slow", move |_| {
                let receive = lock(&receive).take().unwrap();
                Box::pin(async move { Ok(boxed_session(receive.await.unwrap())) })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let pending = h.terminals.spawn(owner, spawn_request("slow"), None);
    let disposal = h.terminals.dispose_all();
    let session = Arc::new(StubSession::default());
    session.reject_close.store(true, Ordering::Release);
    assert!(send.send(session.clone()).is_ok());
    let (spawn_result, disposal_result) = tokio::join!(pending, disposal);
    assert!(
        spawn_result
            .unwrap_err()
            .to_string()
            .contains("spawn and rollback both failed")
    );
    assert!(
        disposal_result
            .unwrap_err()
            .to_string()
            .contains("failed to clean up PTY lifecycle")
    );
    h.kernel.shutdown().await;
}

async fn disposal_retains_backend_side_cleanup_failure(owner_scope: bool) {
    let h = harness().await;
    let cleanup_failure = SharedError::new("backend cleanup failed");
    let backend_cleanup_failure = cleanup_failure.clone();
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("cleanup", move |spec| {
                let cleanup_failure = backend_cleanup_failure.clone();
                Box::pin(async move {
                    let reason = spec.signal.unwrap().cancelled().await;
                    Err(Box::new(TerminalBackendCleanupError::new(reason, cleanup_failure)) as _)
                })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let pending = h
        .terminals
        .spawn(owner.clone(), spawn_request("cleanup"), None);
    let disposal = if owner_scope {
        h.terminals.dispose_owned(owner)
    } else {
        h.terminals.dispose_all()
    };
    let (spawn_result, disposal_result) = tokio::join!(pending, disposal);
    let expected = if owner_scope {
        "OWNER_NOT_LIVE"
    } else {
        "SERVICE_DISPOSING"
    };
    assert_eq!(code(spawn_result.unwrap_err().as_ref()), Some(expected));
    let disposal_error = disposal_result.unwrap_err();
    let lifecycle = SharedError::of(disposal_error.as_ref())
        .and_then(|error| error.downcast_ref::<AggregateError>())
        .unwrap();
    assert_eq!(lifecycle.message, "failed to clean up PTY lifecycle");
    let rollback = lifecycle.errors[0]
        .downcast_ref::<AggregateError>()
        .unwrap();
    assert_eq!(
        rollback.message,
        "failed to roll back unpublished PTY setup"
    );
    assert_eq!(rollback.errors.len(), 1);
    assert!(rollback.errors[0].ptr_eq(&cleanup_failure));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn owner_disposal_retains_backend_side_startup_cleanup_failure() {
    disposal_retains_backend_side_cleanup_failure(true).await;
}

#[tokio::test]
async fn service_disposal_retains_backend_side_startup_cleanup_failure() {
    disposal_retains_backend_side_cleanup_failure(false).await;
}

#[tokio::test]
async fn independent_reservations_provider_failure_and_dropped_caller_are_driven() {
    let h = harness().await;
    let calls = Arc::new(Mutex::new(Vec::<oneshot::Receiver<Arc<StubSession>>>::new()));
    let (first_send, first_receive) = oneshot::channel();
    let (second_send, second_receive) = oneshot::channel();
    lock(&calls).extend([first_receive, second_receive]);
    let spawn_calls = calls.clone();
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("slow", move |_| {
                let receive = lock(&spawn_calls).remove(0);
                Box::pin(async move { Ok(boxed_session(receive.await.unwrap())) })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let first = h.terminals.spawn(owner.clone(), named("slow", "one"), None);
    let second = h.terminals.spawn(owner.clone(), named("slow", "two"), None);
    drop(first);
    assert!(first_send.send(Arc::new(StubSession::default())).is_ok());
    assert!(second_send.send(Arc::new(StubSession::default())).is_ok());
    second.await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(h.terminals.list(&owner).len(), 2);
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("throwing", |_| {
                Box::pin(async { Err("provider failed".into()) })
            }),
        )
        .unwrap();
    assert_eq!(
        h.terminals
            .spawn(owner, spawn_request("throwing"), None)
            .await
            .unwrap_err()
            .to_string(),
        "provider failed"
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn omits_optional_pid_metadata() {
    let h = harness().await;
    let session = Arc::new(StubSession {
        pid: None,
        ..StubSession::default()
    });
    let output = session.clone();
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("virtual", move |_| {
                let output = output.clone();
                Box::pin(async move { Ok(boxed_session(output)) })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner, spawn_request("virtual"), None)
        .await
        .unwrap();
    assert_eq!(created.pid, None);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn rollback_and_close_failures_never_publish_false_success() {
    let h = harness().await;
    let session = Arc::new(StubSession::default());
    session.reject_close.store(true, Ordering::Release);
    let output = session.clone();
    h.terminals
        .register_backend(
            &h.ctx,
            custom_backend("bad", move |spec| {
                let output = output.clone();
                Box::pin(async move {
                    spec.signal.unwrap().cancelled().await;
                    Ok(boxed_session(output))
                })
            }),
        )
        .unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let pending = h.terminals.spawn(owner.clone(), spawn_request("bad"), None);
    let disposal = h.terminals.dispose_owned(owner.clone());
    let (spawn_result, disposal_result) = tokio::join!(pending, disposal);
    assert!(
        spawn_result
            .unwrap_err()
            .to_string()
            .contains("spawn and rollback both failed")
    );
    assert!(disposal_result.is_err());
    assert!(h.terminals.list(&owner).is_empty());

    let (_next_fiber, next_owner) = support::owner(&h, "next").await;
    let (provider, sessions) = backend("bad-close");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let created = h
        .terminals
        .spawn(next_owner.clone(), spawn_request("bad-close"), None)
        .await
        .unwrap();
    lock(&sessions)[0]
        .reject_close
        .store(true, Ordering::Release);
    assert!(
        h.terminals
            .kill(&next_owner, &created.session_id, None)
            .await
            .is_err()
    );
    assert_eq!(h.terminals.list(&next_owner).len(), 1);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn joins_running_close_and_refuses_new_sends_while_closing() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    let gate = Arc::new(Gate::default());
    *lock(&lock(&sessions)[0].close_gate) = Some(gate.clone());
    let first = h.terminals.kill(&owner, &created.session_id, None);
    let closing = h
        .terminals
        .start_send(&owner, &created.session_id, send_request())
        .err()
        .unwrap();
    assert_eq!(code(closing.as_ref()), Some("SESSION_CLOSING"));
    let second = h.terminals.kill(&owner, &created.session_id, None);
    gate.release();
    let (first, second) = tokio::join!(first, second);
    assert!(first.unwrap());
    assert!(!second.unwrap());
    assert!(
        h.terminals
            .read(&owner, &created.session_id, TerminalReadRequest::default())
            .is_err()
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn kill_waits_for_a_synchronously_starting_send_before_backend_close() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    let session = lock(&sessions)[0].clone();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    *lock(&session.start_entered) = Some(entered.clone());
    *lock(&session.start_release) = Some(release.clone());

    let service = h.terminals.clone();
    let send_owner = owner.clone();
    let session_id = created.session_id.clone();
    let send =
        std::thread::spawn(move || service.start_send(&send_owner, &session_id, send_request()));
    entered.wait();
    let kill = h
        .terminals
        .kill(&owner, &created.session_id, Some("race close"));
    assert!(lock(&session.closed).is_empty());
    release.wait();
    let operation = send.join().unwrap().unwrap();

    assert!(kill.await.unwrap());
    assert_eq!(lock(&session.closed).as_slice(), ["race close"]);
    operation.done().await.unwrap();
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn owner_cleanup_removes_sessions_after_backend_reload() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    let provider_disposer = h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    provider_disposer.dispose().await;
    assert!(h.terminals.list_backends().is_empty());
    assert_eq!(
        h.terminals
            .read(&owner, &created.session_id, TerminalReadRequest::default())
            .unwrap()
            .text,
        "0:0"
    );
    dispose(&fiber).await;
    assert_eq!(
        lock(&lock(&sessions)[0].closed).as_slice(),
        ["PTY owner disposed"]
    );
    assert!(h.terminals.list(&owner).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn kill_is_idempotent_and_service_disposal_closes_all_owners() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_first_fiber, first) = owner(&h, "first").await;
    let (_second_fiber, second) = owner(&h, "second").await;
    let first_session = h
        .terminals
        .spawn(first.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    h.terminals
        .spawn(second, spawn_request("stub"), None)
        .await
        .unwrap();
    assert!(
        h.terminals
            .kill(&first, &first_session.session_id, None)
            .await
            .unwrap()
    );
    assert_eq!(
        lock(&lock(&sessions)[0].closed).as_slice(),
        ["model request"]
    );
    h.terminals.dispose_all().await.unwrap();
    assert_eq!(
        lock(&lock(&sessions)[1].closed).as_slice(),
        ["PTY service disposed"]
    );
    let error = h
        .terminals
        .spawn(first, spawn_request("stub"), None)
        .await
        .unwrap_err();
    assert_eq!(code(error.as_ref()), Some("SERVICE_DISPOSING"));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn joined_close_failures_retry_once_and_keep_the_record() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    let session = lock(&sessions)[0].clone();
    let gate = Arc::new(Gate::default());
    *lock(&session.close_gate) = Some(gate.clone());
    session.reject_close.store(true, Ordering::Release);
    let first = h
        .terminals
        .kill(&owner, &created.session_id, Some("test failure"));
    let second = h
        .terminals
        .kill(&owner, &created.session_id, Some("joined failure"));
    gate.release();
    let (first, second) = tokio::join!(first, second);
    assert!(first.is_err() && second.is_err());
    assert_eq!(lock(&session.closed).as_slice(), ["test failure"]);
    assert_eq!(h.terminals.list(&owner).len(), 1);
    session.reject_close.store(false, Ordering::Release);
    *lock(&session.close_gate) = None;
    assert!(
        h.terminals
            .kill(&owner, &created.session_id, Some("retry"))
            .await
            .unwrap()
    );
    assert_eq!(lock(&session.closed).as_slice(), ["test failure", "retry"]);
    assert!(h.terminals.list(&owner).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn owner_disposal_preserves_the_close_aggregate_and_failed_record_for_retry() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    let session = lock(&sessions)[0].clone();
    session.reject_close.store(true, Ordering::Release);

    let error = h.terminals.dispose_owned(owner.clone()).await.unwrap_err();
    let lifecycle = SharedError::of(error.as_ref())
        .and_then(|error| error.downcast_ref::<AggregateError>())
        .unwrap();
    assert_eq!(lifecycle.message, "failed to clean up PTY lifecycle");
    let close = lifecycle.errors[0]
        .downcast_ref::<AggregateError>()
        .unwrap();
    assert_eq!(close.message, "failed to close 1 PTY session(s)");
    assert_eq!(h.terminals.list(&owner).len(), 1);

    session.reject_close.store(false, Ordering::Release);
    assert!(
        h.terminals
            .kill(&owner, &created.session_id, Some("retry"))
            .await
            .unwrap()
    );
    assert!(h.terminals.list(&owner).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn dropped_successful_kill_is_completed_by_the_service_actor() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    let session = lock(&sessions)[0].clone();
    let gate = Arc::new(Gate::default());
    *lock(&session.close_gate) = Some(gate.clone());

    drop(
        h.terminals
            .kill(&owner, &created.session_id, Some("dropped caller")),
    );
    gate.release();
    for _ in 0..100 {
        if h.terminals.list(&owner).is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert!(h.terminals.list(&owner).is_empty());
    let error = h
        .terminals
        .read(&owner, &created.session_id, TerminalReadRequest::default())
        .unwrap_err();
    assert_eq!(code(error.as_ref()), Some("NO_SESSION"));
    assert!(!h.terminals.has_owner_activity(&owner));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn dropped_failed_kill_clears_closing_and_the_next_kill_retries() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    let session = lock(&sessions)[0].clone();
    let gate = Arc::new(Gate::default());
    *lock(&session.close_gate) = Some(gate.clone());
    session.reject_close.store(true, Ordering::Release);

    drop(
        h.terminals
            .kill(&owner, &created.session_id, Some("dropped failure")),
    );
    gate.release();
    tokio::task::yield_now().await;
    session.reject_close.store(false, Ordering::Release);
    *lock(&session.close_gate) = None;

    assert!(
        h.terminals
            .kill(&owner, &created.session_id, Some("retry"))
            .await
            .unwrap()
    );
    assert_eq!(
        lock(&session.closed).as_slice(),
        ["dropped failure", "retry"]
    );
    assert!(h.terminals.list(&owner).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn concurrent_service_disposals_share_the_pending_failure() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    h.terminals
        .spawn(owner, spawn_request("stub"), None)
        .await
        .unwrap();
    let session = lock(&sessions)[0].clone();
    let gate = Arc::new(Gate::default());
    *lock(&session.close_gate) = Some(gate.clone());
    session.reject_close.store(true, Ordering::Release);

    let first = h.terminals.dispose_all();
    for _ in 0..100 {
        if !lock(&session.closed).is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(lock(&session.closed).as_slice(), ["PTY service disposed"]);
    let second = h.terminals.dispose_all();
    tokio::pin!(second);
    poll_fn(|context| match second.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("joined disposal settled before close completed"),
    })
    .await;
    gate.release();
    let (first, second) = tokio::join!(first, &mut second);
    let first = first.unwrap_err();
    let second = second.unwrap_err();
    assert!(
        SharedError::of(first.as_ref())
            .unwrap()
            .ptr_eq(SharedError::of(second.as_ref()).unwrap())
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn owner_disposal_during_service_disposal_joins_the_same_failure() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    h.terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    let session = lock(&sessions)[0].clone();
    let gate = Arc::new(Gate::default());
    *lock(&session.close_gate) = Some(gate.clone());
    session.reject_close.store(true, Ordering::Release);

    let service_disposal = h.terminals.dispose_all();
    for _ in 0..100 {
        if !lock(&session.closed).is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(lock(&session.closed).as_slice(), ["PTY service disposed"]);
    let owner_disposal = h.terminals.dispose_owned(owner);
    tokio::pin!(owner_disposal);
    poll_fn(|context| match owner_disposal.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("owner disposal settled before service close completed"),
    })
    .await;
    gate.release();
    let (service_disposal, owner_disposal) = tokio::join!(service_disposal, &mut owner_disposal);
    let service_error = service_disposal.unwrap_err();
    let owner_error = owner_disposal.unwrap_err();
    assert!(
        SharedError::of(service_error.as_ref())
            .unwrap()
            .ptr_eq(SharedError::of(owner_error.as_ref()).unwrap())
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn send_after_service_disposal_starts_never_reaches_the_backend() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    let created = h
        .terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    let session = lock(&sessions)[0].clone();
    let gate = Arc::new(Gate::default());
    *lock(&session.close_gate) = Some(gate.clone());

    let disposal = h.terminals.dispose_all();
    let error = match h
        .terminals
        .start_send(&owner, &created.session_id, send_request())
    {
        Ok(_) => panic!("send reached backend after disposal started"),
        Err(error) => error,
    };
    assert_eq!(code(error.as_ref()), Some("SERVICE_DISPOSING"));
    assert!(lock(&session.operation).is_none());
    gate.release();
    disposal.await.unwrap();
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn disposal_clears_registries_and_owner_cleanups_after_close_failure() {
    let h = harness().await;
    let (provider, sessions) = backend("stub");
    h.terminals.register_backend(&h.ctx, provider).unwrap();
    let (_fiber, owner) = owner(&h, "owner").await;
    h.terminals
        .spawn(owner.clone(), spawn_request("stub"), None)
        .await
        .unwrap();
    lock(&sessions)[0]
        .reject_close
        .store(true, Ordering::Release);
    let error = h.terminals.dispose_all().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failed to clean up PTY lifecycle")
    );
    assert_eq!(
        lock(&lock(&sessions)[0].closed).as_slice(),
        ["PTY service disposed", "PTY owner disposed"]
    );
    assert_eq!(h.terminals.list(&owner).len(), 1);
    assert!(h.terminals.list_backends().is_empty());
    h.kernel.shutdown().await;
}

#[test]
fn terminal_debug_output_redacts_input_and_terminal_content() {
    let request = TerminalSendRequest {
        text: "sudo-password".into(),
        submit: true,
        signal: None,
    };
    let send_read = TerminalSendRead {
        delta: "send-secret".into(),
        truncated: false,
    };
    let send_result = TerminalSendResult {
        viewport: "viewport-secret".into(),
        wait_reason: TerminalWaitReason::StdinRead,
        session_status: TerminalSessionStatus::Running,
        truncated: false,
    };
    let read_result = TerminalReadResult {
        text: "read-secret".into(),
        total_lines: 1,
        line_begin: 0,
        line_end: 1,
        truncated: false,
    };
    let spawn_result = TerminalSpawnResult {
        session_id: terminal_session_id("pty-secret"),
        name: None,
        kind: "stub".into(),
        pid: None,
        status: TerminalSessionStatus::Running,
        motd: "motd-secret".into(),
    };

    for (debug, secret) in [
        (format!("{request:?}"), "sudo-password"),
        (format!("{send_read:?}"), "send-secret"),
        (format!("{send_result:?}"), "viewport-secret"),
        (format!("{read_result:?}"), "read-secret"),
        (format!("{spawn_result:?}"), "motd-secret"),
    ] {
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains(secret));
    }
}

#[test]
fn cleanup_error_preserves_ordered_spawn_and_cleanup_causes() {
    let error = TerminalBackendCleanupError::new("setup", "cleanup");
    let detail = TerminalBackendCleanupError::detail(&error).unwrap();
    assert_eq!(detail.errors[0].to_string(), "setup");
    assert_eq!(detail.errors[1].to_string(), "cleanup");
    let cause = error
        .source()
        .and_then(|cause| cause.downcast_ref::<AggregateError>())
        .unwrap();
    assert_eq!(
        cause
            .errors
            .iter()
            .map(SharedError::to_string)
            .collect::<Vec<_>>(),
        ["setup", "cleanup"]
    );
}
