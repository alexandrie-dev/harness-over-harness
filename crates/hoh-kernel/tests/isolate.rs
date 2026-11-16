mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::Arc;
use support::*;
#[tokio::test(start_paused = true)]
async fn does_not_expose_own_provisions_across_isolation_boundaries() {
    let (k, r, _) = harness();
    let isolated = r.isolate("foo", Realm::new());
    isolated.provide::<A>(Arc::new(1)).await.unwrap();
    assert!(r.get::<A>().is_none());
    assert_eq!(*isolated.get::<A>().unwrap(), 1);
    let c = isolated
        .inject(|ctx, d: ADeps| async move {
            assert_eq!(*d.a, 1);
            assert_eq!(*ctx.get::<A>().unwrap(), 1);
            Ok(())
        })
        .unwrap();
    c.settled().await.unwrap();
    let c = r
        .inject(|ctx, (): ()| async move {
            assert!(ctx.get::<A>().is_none());
            Ok(())
        })
        .unwrap();
    c.settled().await.unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_inherit_the_injected_view_across_isolation_boundaries() {
    let (k, r, _) = harness();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    let c = r
        .inject(|ctx, d: ADeps| async move {
            assert_eq!(*d.a, 1);
            assert!(ctx.isolate("foo", Realm::new()).get::<A>().is_none());
            let child = ctx
                .isolate("foo", Realm::new())
                .inject(|ctx, (): ()| async move {
                    assert!(ctx.get::<A>().is_none());
                    Ok(())
                })?;
            child.settled().await?;
            Ok(())
        })
        .unwrap();
    c.settled().await.unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn isolated_context() {
    let (k, r, t) = harness();
    let first = r.isolate("foo", Realm::new());
    let second = r.isolate("foo", Realm::new());
    let mut fibers = vec![];
    for ctx in [&r, &first, &second] {
        let t = t.clone();
        fibers.push(
            ctx.inject(move |ctx, d: ADeps| {
                let t = t.clone();
                async move {
                    t.push(format!("load {}", *d.a));
                    ctx.effect("cleanup", move || Undo::sync(move || t.push("cleanup")))?;
                    Ok(())
                }
            })
            .unwrap(),
        );
    }
    let d = r.provide::<A>(Arc::new(100)).await.unwrap();
    fibers[0].settled().await.unwrap();
    assert_eq!(t.list(), ["load 100"]);
    first.provide::<A>(Arc::new(200)).await.unwrap();
    fibers[1].settled().await.unwrap();
    d.dispose().await;
    assert!(r.get::<A>().is_none());
    assert!(second.get::<A>().is_none());
    second.provide::<A>(Arc::new(300)).await.unwrap();
    fibers[2].settled().await.unwrap();
    assert_eq!(t.list(), ["load 100", "load 200", "cleanup", "load 300"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn shared_label() {
    let (k, r, t) = harness();
    let realm = Realm::new();
    let first = r.isolate("foo", realm);
    let second = r.isolate("foo", realm);
    let mut fibers = vec![];
    for ctx in [&r, &first, &second] {
        let t = t.clone();
        fibers.push(
            ctx.inject(move |_, d: ADeps| {
                let t = t.clone();
                async move {
                    t.push(d.a.to_string());
                    Ok(())
                }
            })
            .unwrap(),
        );
    }
    r.provide::<A>(Arc::new(100)).await.unwrap();
    fibers[0].settled().await.unwrap();
    let d = first.provide::<A>(Arc::new(200)).await.unwrap();
    fibers[1].settled().await.unwrap();
    fibers[2].settled().await.unwrap();
    assert_eq!(t.list(), ["100", "200", "200"]);
    d.dispose().await;
    assert!(first.get::<A>().is_none());
    assert!(second.get::<A>().is_none());
    assert_eq!(*r.get::<A>().unwrap(), 100);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn isolated_event() {
    let (k, r, t) = harness();
    let realm = Realm::new();
    let inner = r.isolate("foo", realm);
    let a = t.clone();
    r.on::<Ping>(move |_| a.push("outer")).unwrap();
    let a = t.clone();
    inner.on::<Ping>(move |_| a.push("inner")).unwrap();
    inner
        .with_filter(move |ctx| ctx.realm_of("foo") == realm)
        .emit(&Ping);
    assert_eq!(t.list(), ["inner"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_own_provisions_separate_from_the_injected_view() {
    let (k, r, t) = harness();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    let f = r
        .inject(|ctx, d: ADeps| async move {
            let first = ctx.isolate("foo", Realm::new());
            let second = ctx.isolate("foo", Realm::new());
            let withdraw = first.provide::<A>(Arc::new(2)).await?;
            second.provide::<A>(Arc::new(3)).await?;
            assert_eq!(*d.a, 1);
            assert_eq!(*first.get::<A>().unwrap(), 2);
            let t = ctx.tag::<Arc<Track>>().unwrap();
            lock(&t.contexts).push(first);
            lock(&t.contexts).push(second);
            ctx.effect("withdraw", || Undo::from(withdraw))?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    let contexts = lock(&t.contexts).clone();
    assert_eq!(*contexts[0].get::<A>().unwrap(), 2);
    assert_eq!(*contexts[1].get::<A>().unwrap(), 3);
    assert_eq!(*r.get::<A>().unwrap(), 1);
    f.update(json!({})).await.unwrap();
    k.shutdown().await;
}
