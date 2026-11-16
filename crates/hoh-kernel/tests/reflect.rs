mod support;
use hoh_kernel::*;
use std::sync::Arc;
use support::*;
#[tokio::test(start_paused = true)]
async fn service_injection() {
    let (k, r, _) = harness();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    let c = r
        .with_tag(2_u32)
        .inject(|ctx, d: ADeps| async move {
            assert_eq!(*d.a, 1);
            assert_eq!(*ctx.tag::<u32>().unwrap(), 2);
            assert!(ctx.get::<B>().is_none());
            Ok(())
        })
        .unwrap();
    c.settled().await.unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn service_inject_leak() {
    let (k, r, t) = harness();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    let c = r
        .inject(|ctx, _: ADeps| async move {
            let t = ctx.tag::<Arc<Track>>().unwrap();
            lock(&t.contexts).push(ctx);
            Ok(())
        })
        .unwrap();
    c.settled().await.unwrap();
    let old = t.context();
    assert!(old.get::<A>().is_some());
    c.dispose().await;
    assert!(old.get::<A>().is_none());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_load_a_fiber_disposed_during_service_notification() {
    let (k, r, t) = harness();
    let t1 = t.clone();
    let first = r
        .inject(move |_, _: ADeps| {
            let t = t1.clone();
            async move {
                t.push("1");
                Ok(())
            }
        })
        .unwrap();
    let t2 = t.clone();
    let second = r
        .inject(move |_, _: ADeps| {
            let t = t2.clone();
            async move {
                t.push("2");
                Ok(())
            }
        })
        .unwrap();
    let uid = first.uid();
    let second2 = second.clone();
    r.on::<InternalStatus>(move |e| {
        if e.fiber.uid() == uid && e.fiber.state() == FiberState::Loading {
            drop(second2.dispose());
        }
    })
    .unwrap();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    first.settled().await.unwrap();
    second.settled().await.unwrap();
    assert_eq!(t.list(), ["1"]);
    assert_eq!(second.state(), FiberState::Disposed);
    k.shutdown().await;
}
