use hoh_kernel::*;
use std::sync::{Arc, Mutex};
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
struct E;
event!(E = "ext2", Contained);
#[tokio::test(start_paused = true)]
async fn non_root_factory_and_future_panics_preserve_payloads() {
    let k = Kernel::start();
    let child = k.root().child("listener").unwrap();
    child.on_contained::<E>(|_| panic!("sync text")).unwrap();
    child
        .on_contained_async::<E, _>(|_| {
            std::panic::panic_any("factory text".to_owned());
            #[allow(unreachable_code)]
            async {
                Ok(())
            }
        })
        .unwrap();
    child
        .on_contained_async::<E, _>(|_| async {
            panic!("future text");
            #[allow(unreachable_code)]
            Ok(())
        })
        .unwrap();
    child
        .on_contained_async::<E, _>(|_| async {
            tokio::task::yield_now().await;
            std::panic::panic_any("pending future text".to_owned());
            #[allow(unreachable_code)]
            Ok(())
        })
        .unwrap();
    child
        .on_contained_async::<E, _>(|_| async {
            std::panic::panic_any(7_u8);
            #[allow(unreachable_code)]
            Ok(())
        })
        .unwrap();
    let reports = Arc::new(Mutex::new(vec![]));
    let out = reports.clone();
    k.root().dispatch(E).invoke(move |e| {
        lock(&out).push(match e {
            ListenerFailure::Threw(e) => ("threw", e.to_string()),
            ListenerFailure::Rejected(e) => ("rejected", e.to_string()),
        })
    });
    assert_eq!(
        *lock(&reports),
        [
            ("threw", "event listener panicked: sync text".into()),
            ("rejected", "event listener panicked: factory text".into()),
            ("rejected", "event listener panicked: future text".into()),
            ("rejected", "event listener panicked".into())
        ]
    );
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        lock(&reports).last().unwrap(),
        &(
            "rejected",
            "event listener panicked: pending future text".into()
        )
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn async_prefix_and_ready_rejection_run_inside_invoke() {
    let k = Kernel::start();
    let log = Arc::new(Mutex::new(vec![]));
    let a = log.clone();
    k.root()
        .on_contained::<E>(move |_| {
            lock(&a).push("A");
            Ok(())
        })
        .unwrap();
    let b = log.clone();
    k.root()
        .on_contained_async::<E, _>(move |_| {
            let b = b.clone();
            async move {
                lock(&b).push("B");
                Err("ready".into())
            }
        })
        .unwrap();
    let c = log.clone();
    k.root()
        .on_contained::<E>(move |_| {
            lock(&c).push("C");
            Ok(())
        })
        .unwrap();
    let r = log.clone();
    k.root().dispatch(E).invoke(move |e| {
        assert!(matches!(e, ListenerFailure::Rejected(_)));
        lock(&r).push("rejected");
    });
    assert_eq!(*lock(&log), ["A", "B", "C", "rejected"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn disposed_snapshot_still_polls_async_prefix() {
    let k = Kernel::start();
    let ctx = k.root().child("listener").unwrap();
    let log = Arc::new(Mutex::new(vec![]));
    let a = log.clone();
    ctx.on_contained::<E>(move |_| {
        lock(&a).push("sync");
        Ok(())
    })
    .unwrap();
    let b = log.clone();
    ctx.on_contained_async::<E, _>(move |_| {
        let b = b.clone();
        async move {
            lock(&b).push("async");
            std::future::pending::<()>().await;
            Ok(())
        }
    })
    .unwrap();
    let ready = log.clone();
    ctx.on_contained_async::<E, _>(move |_| {
        let ready = ready.clone();
        async move {
            lock(&ready).push("ready");
            Err("closed ready".into())
        }
    })
    .unwrap();
    let snapshot = k.root().dispatch(E);
    ctx.fiber().dispose().await;
    let out = log.clone();
    snapshot.invoke(move |e| match e {
        ListenerFailure::Rejected(e) => {
            assert_eq!(e.to_string(), "closed ready");
            lock(&out).push("rejected");
        }
        ListenerFailure::Threw(e) => panic!("unexpected {e:?}"),
    });
    assert_eq!(*lock(&log), ["sync", "async", "ready", "rejected"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn pending_child_uses_source_unloading_status_and_drains_early_effects() {
    let k = Kernel::start();
    let log = Arc::new(Mutex::new(vec![]));
    let s = log.clone();
    k.root()
        .on::<InternalStatus>(move |e| {
            if e.fiber.name().as_ref() == "early" {
                lock(&s).push(format!("{:?}->{:?}", e.old, e.fiber.state()));
            }
        })
        .unwrap();
    let parent = k.root().child("parent").unwrap();
    parent.fiber().settled().await.unwrap();
    let disposed = parent.fiber().dispose();
    let child = parent.child("early").unwrap();
    let c = log.clone();
    child
        .effect("early", move || {
            Undo::sync(move || lock(&c).push("cleanup".into()))
        })
        .unwrap();
    assert_eq!(child.fiber().state(), FiberState::Pending);
    let child_disposed = child.fiber().dispose();
    disposed.await;
    child_disposed.await;
    assert_eq!(
        *lock(&log),
        ["Pending->Unloading", "cleanup", "Unloading->Disposed"]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn dynamic_label_is_preserved_in_cleanup_diagnostics() {
    let k = Kernel::start();
    let label = format!("register({:?})", "field");
    k.root()
        .try_effect::<KernelError>(label.clone(), |u| {
            u.push(Undo::future(|| async { Err("failure".into()) }))
        })
        .unwrap()
        .dispose()
        .await;
    assert!(
        k.root()
            .logger()
            .buffer()
            .iter()
            .any(|r| r.message.contains(&label))
    );
    k.shutdown().await;
}
