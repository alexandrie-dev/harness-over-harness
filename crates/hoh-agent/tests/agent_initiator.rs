mod common;

use common::stub_agent;
use hoh_agent::{AgentRegistry, AgentRegistryPlugin, Agents, spawn_inherited};
use hoh_kernel::{Context, Fiber, FiberState, InternalStatus, Kernel};
use hoh_session_projection::SessionProjectionRegistry;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

const DISPOSED: &str = "agent initiator scope is disposed";
const NO_INITIATOR: &str = "no initiating agent is active";

struct Harness {
    ctx: Context,
    kernel: Kernel,
    service: AgentRegistry,
    fiber: Fiber,
}

async fn harness() -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = ctx.plugin::<AgentRegistryPlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    let service = (*ctx.get::<Agents>().unwrap()).clone();
    Harness {
        ctx,
        kernel,
        service,
        fiber,
    }
}

fn err_text<T>(result: Result<T, hoh_error::BoxError>) -> String {
    result.err().expect("expected an error").to_string()
}

#[tokio::test]
async fn reports_an_absent_initiator_and_requires_an_active_boundary() {
    let h = harness().await;
    assert!(h.service.current_initiator().unwrap().is_none());
    assert_eq!(err_text(h.service.require_initiator()), NO_INITIATOR);
    h.fiber.dispose().await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_exact_return_values_across_await() {
    let h = harness().await;
    let initiator = stub_agent(&h.ctx, "identity");
    let sync_value = h
        .service
        .with_initiator_sync(initiator.clone(), || {
            assert!(Arc::ptr_eq(
                &h.service.require_initiator().unwrap(),
                &initiator
            ));
            7
        })
        .unwrap();
    assert_eq!(sync_value, 7);

    let service = h.service.clone();
    let expected = initiator.clone();
    let result = h
        .service
        .with_initiator(initiator.clone(), async move {
            assert!(Arc::ptr_eq(
                &service.require_initiator().unwrap(),
                &expected
            ));
            tokio::task::yield_now().await;
            assert!(Arc::ptr_eq(
                &service.require_initiator().unwrap(),
                &expected
            ));
            // A nested task started with `spawn_inherited` continues the chain; a plain
            // `tokio::spawn` starts a new one (the rule in PORT_NOTES).
            let nested = {
                let (service, expected) = (service.clone(), expected.clone());
                spawn_inherited(async move {
                    tokio::task::yield_now().await;
                    Arc::ptr_eq(&service.require_initiator().unwrap(), &expected)
                })
            };
            assert!(nested.await.unwrap());
            let plain = {
                let service = service.clone();
                tokio::spawn(async move { service.current_initiator().unwrap().is_none() })
            };
            assert!(plain.await.unwrap());
            "value"
        })
        .unwrap()
        .await;
    assert_eq!(result, "value");
    assert!(h.service.current_initiator().unwrap().is_none());
    h.fiber.dispose().await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn isolates_overlapping_initiators() {
    let h = harness().await;
    let (a, b) = (stub_agent(&h.ctx, "a"), stub_agent(&h.ctx, "b"));
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let started = Arc::new(tokio::sync::Barrier::new(3));
    let run = |initiator: Arc<dyn hoh_agent::Agent>| {
        let service = h.service.clone();
        let (mut release, started) = (release_rx.clone(), started.clone());
        h.service
            .with_initiator(initiator.clone(), async move {
                assert!(Arc::ptr_eq(
                    &service.require_initiator().unwrap(),
                    &initiator
                ));
                started.wait().await;
                release.wait_for(|released| *released).await.unwrap();
                assert!(Arc::ptr_eq(
                    &service.require_initiator().unwrap(),
                    &initiator
                ));
            })
            .unwrap()
    };
    let pending = [tokio::spawn(run(a)), tokio::spawn(run(b))];
    started.wait().await;
    assert!(h.service.current_initiator().unwrap().is_none());
    release_tx.send(true).unwrap();
    for task in pending {
        task.await.unwrap();
    }
    h.fiber.dispose().await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn restores_nested_and_explicitly_cleared_boundaries() {
    let h = harness().await;
    let (parent, child) = (stub_agent(&h.ctx, "parent"), stub_agent(&h.ctx, "child"));
    let service = h.service.clone();
    h.service
        .with_initiator_sync(parent.clone(), || {
            assert!(Arc::ptr_eq(&service.require_initiator().unwrap(), &parent));
            service
                .with_initiator_sync(child.clone(), || {
                    assert!(Arc::ptr_eq(&service.require_initiator().unwrap(), &child));
                })
                .unwrap();
            assert!(Arc::ptr_eq(&service.require_initiator().unwrap(), &parent));
            service
                .without_initiator_sync(|| {
                    assert!(service.current_initiator().unwrap().is_none());
                    assert_eq!(err_text(service.require_initiator()), NO_INITIATOR);
                })
                .unwrap();
            assert!(Arc::ptr_eq(&service.require_initiator().unwrap(), &parent));
        })
        .unwrap();
    assert!(h.service.current_initiator().unwrap().is_none());
    h.fiber.dispose().await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn restores_the_parent_after_panics_and_failed_operations() {
    let h = harness().await;
    let (parent, child) = (stub_agent(&h.ctx, "parent"), stub_agent(&h.ctx, "child"));
    let service = h.service.clone();
    h.service
        .with_initiator_sync(parent.clone(), || {
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = service.with_initiator_sync(child.clone(), || panic!("sync failure"));
            }));
            assert!(panicked.is_err());
            assert!(Arc::ptr_eq(&service.require_initiator().unwrap(), &parent));
        })
        .unwrap();
    let failed: Result<(), &str> = h
        .service
        .with_initiator(child, async {
            tokio::task::yield_now().await;
            Err("async failure")
        })
        .unwrap()
        .await;
    assert_eq!(failed, Err("async failure"));
    assert!(h.service.current_initiator().unwrap().is_none());
    h.fiber.dispose().await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn stops_new_boundaries_drains_active_futures_and_invalidates_retained_references() {
    let h = harness().await;
    let initiator = stub_agent(&h.ctx, "draining");
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let service = h.service.clone();
    let expected = initiator.clone();
    let pending = h
        .service
        .with_initiator(initiator.clone(), async move {
            release_rx.await.unwrap();
            assert!(Arc::ptr_eq(
                &service.require_initiator().unwrap(),
                &expected
            ));
        })
        .unwrap();
    let pending = tokio::spawn(pending);
    let disposal = tokio::spawn(h.fiber.dispose());
    for _ in 0..50 {
        if h.service
            .with_initiator_sync(initiator.clone(), || ())
            .is_err()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        err_text(h.service.with_initiator_sync(initiator.clone(), || 1)),
        DISPOSED
    );
    assert_eq!(err_text(h.service.without_initiator_sync(|| 1)), DISPOSED);
    assert!(!disposal.is_finished());
    assert!(h.ctx.get::<Agents>().is_none());
    release_tx.send(()).unwrap();
    pending.await.unwrap();
    disposal.await.unwrap();
    assert_eq!(err_text(h.service.current_initiator()), DISPOSED);
    assert_eq!(err_text(h.service.require_initiator()), DISPOSED);
    h.kernel.shutdown().await;
}

/// The kernel runs teardown in its own task; `Fiber::dispose()` hands it the caller's initiator
/// chain, so the boundary that waits for the disposal is excluded from the drain by itself.
#[tokio::test]
async fn does_not_self_deadlock_when_a_boundary_waits_for_service_disposal() {
    let h = harness().await;
    let initiator = stub_agent(&h.ctx, "service-disposer");
    let fiber = h.fiber.clone();
    let boundary = h
        .service
        .with_initiator(initiator, async move {
            fiber.dispose().await;
        })
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), boundary)
        .await
        .expect("initiator teardown did not settle promptly");
    assert_eq!(err_text(h.service.current_initiator()), DISPOSED);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn does_not_self_deadlock_when_nested_boundaries_wait_for_ancestor_disposal() {
    let h = harness().await;
    let (parent, child) = (
        stub_agent(&h.ctx, "parent-disposer"),
        stub_agent(&h.ctx, "child-disposer"),
    );
    let fiber = h.fiber.clone();
    let inner = h.service.clone();
    let boundary = h
        .service
        .with_initiator(parent, async move {
            inner
                .with_initiator(child, async move {
                    fiber.dispose().await;
                })
                .unwrap()
                .await
        })
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), boundary)
        .await
        .expect("initiator teardown did not settle promptly");
    assert_eq!(err_text(h.service.current_initiator()), DISPOSED);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn excludes_an_asynchronous_teardown_initiator_while_draining_unrelated_boundaries() {
    let h = harness().await;
    let (initiator, unrelated) = (
        stub_agent(&h.ctx, "async-disposer"),
        stub_agent(&h.ctx, "unrelated"),
    );
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let service = h.service.clone();
    let expected = unrelated.clone();
    let pending = tokio::spawn(
        h.service
            .with_initiator(unrelated, async move {
                release_rx.await.unwrap();
                assert!(Arc::ptr_eq(
                    &service.require_initiator().unwrap(),
                    &expected
                ));
            })
            .unwrap(),
    );
    let fiber = h.fiber.clone();
    let returned = tokio::spawn(
        h.service
            .with_initiator(initiator, async move {
                tokio::task::yield_now().await;
                fiber.dispose().await;
            })
            .unwrap(),
    );
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert!(!returned.is_finished());
    release_tx.send(()).unwrap();
    pending.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), returned)
        .await
        .expect("initiator teardown did not settle promptly")
        .unwrap();
    assert_eq!(err_text(h.service.current_initiator()), DISPOSED);
    h.kernel.shutdown().await;
}

/// The source closes the initiator scope when the fiber of any ancestor starts unloading, before
/// the registry's own fiber is touched.
#[tokio::test]
async fn rejects_new_boundaries_once_an_ancestor_fiber_starts_unloading() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let parent = root.child("parent").unwrap();
    let fiber = parent.plugin::<AgentRegistryPlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    let service = (*parent.get::<Agents>().unwrap()).clone();
    let initiator = stub_agent(&root, "ancestor-unload");
    let before = service
        .with_initiator_sync(initiator.clone(), || 1)
        .unwrap();
    assert_eq!(before, 1);

    // Registered after the registry's own listener, so it runs after the registry has reacted.
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (sink, svc, subject, parent_uid, own_fiber) = (
        seen.clone(),
        service.clone(),
        initiator.clone(),
        parent.fiber().uid(),
        fiber.clone(),
    );
    root.on::<InternalStatus>(move |event| {
        if event.fiber.uid() == parent_uid && event.fiber.state() == FiberState::Unloading {
            let attempt = svc.with_initiator_sync(subject.clone(), || ());
            sink.lock().unwrap().push((
                attempt.err().map(|error| error.to_string()),
                own_fiber.state(),
            ));
        }
    })
    .unwrap();

    parent.fiber().dispose().await;
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0.as_deref(), Some(DISPOSED));
    }
    kernel.shutdown().await;
}

/// The source excludes the calling chain from its own drain with no help from the caller: a
/// boundary that awaits the disposal of the registry's fiber finishes (the kernel carries the
/// caller's chain into the teardown, `capture_on_dispose`).
#[tokio::test]
async fn does_not_self_deadlock_without_the_manual_release() {
    let h = harness().await;
    let initiator = stub_agent(&h.ctx, "implicit-disposer");
    let fiber = h.fiber.clone();
    let boundary = h
        .service
        .with_initiator(initiator, async move {
            fiber.dispose().await;
        })
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), boundary)
        .await
        .expect("initiator teardown did not settle promptly");
    assert_eq!(err_text(h.service.current_initiator()), DISPOSED);
    h.kernel.shutdown().await;
}

/// A second boundary whose code calls `Fiber::dispose()` after the teardown has started is
/// excluded from the drain too, though the first call already passed its chain on.
#[tokio::test]
async fn excludes_a_second_boundary_that_disposes_after_the_teardown_started() {
    let h = harness().await;
    let (first, second, unrelated) = (
        stub_agent(&h.ctx, "first-disposer"),
        stub_agent(&h.ctx, "second-disposer"),
        stub_agent(&h.ctx, "still-running"),
    );
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (go_tx, go_rx) = oneshot::channel::<()>();
    // All three boundaries open before the teardown starts (a closing scope accepts no more).
    let pending = tokio::spawn(
        h.service
            .with_initiator(unrelated, async move {
                release_rx.await.unwrap();
            })
            .unwrap(),
    );
    let fiber = h.fiber.clone();
    let first = tokio::spawn(
        h.service
            .with_initiator(first, async move {
                fiber.dispose().await;
            })
            .unwrap(),
    );
    let fiber = h.fiber.clone();
    let second = tokio::spawn(
        h.service
            .with_initiator(second, async move {
                go_rx.await.unwrap();
                fiber.dispose().await;
            })
            .unwrap(),
    );
    // The teardown is running: it drains `unrelated` and has released `first`.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(!first.is_finished());
    go_tx.send(()).unwrap();
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(!pending.is_finished() && !second.is_finished());
    release_tx.send(()).unwrap();
    pending.await.unwrap();
    for task in [first, second] {
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("initiator teardown did not settle promptly")
            .unwrap();
    }
    assert_eq!(err_text(h.service.current_initiator()), DISPOSED);
    h.kernel.shutdown().await;
}

/// `Kernel::shutdown()` called from a boundary hands the chain to the teardown like `dispose()`.
#[tokio::test]
async fn does_not_self_deadlock_when_a_boundary_shuts_the_kernel_down() {
    let h = harness().await;
    let initiator = stub_agent(&h.ctx, "kernel-shutdown");
    let kernel = h.kernel.clone();
    let boundary = h
        .service
        .with_initiator(initiator, async move {
            kernel.shutdown().await;
        })
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), boundary)
        .await
        .expect("initiator teardown did not settle promptly");
    assert_eq!(err_text(h.service.current_initiator()), DISPOSED);
}

/// `fiber.disposer().dispose()` runs `child.dispose()` in a cleanup task of the kernel: the
/// chain of whoever called the disposer is passed on to it.
#[tokio::test]
async fn does_not_self_deadlock_when_a_boundary_disposes_through_the_fiber_disposer() {
    let h = harness().await;
    let initiator = stub_agent(&h.ctx, "raw-disposer");
    let disposer = h.fiber.disposer().unwrap();
    let boundary = h
        .service
        .with_initiator(initiator, async move {
            disposer.dispose().await;
        })
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), boundary)
        .await
        .expect("initiator teardown did not settle promptly");
    assert_eq!(err_text(h.service.current_initiator()), DISPOSED);
    h.kernel.shutdown().await;
}
