mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::*;
#[tokio::test(start_paused = true)]
async fn apply_functional_component() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"foo":"bar"})).unwrap();
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["load 0"]);
    assert_eq!(lock(&t.configs)[0].get()["foo"], "bar");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn inactive_context() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    let old = t.context();
    f.dispose().await;
    assert!(matches!(
        old.effect("bad", || panic!("setup must not run")),
        Err(KernelError::Inactive)
    ));
    assert!(old.plugin::<Recorded>(json!({})).is_err());
    assert!(old.on::<Ping>(|_| {}).is_err());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn nested_components() {
    let (k, r, _) = harness();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    r.on::<Ping>(move |_| {
        c.fetch_add(1, Ordering::Relaxed);
    })
    .unwrap();
    let c = calls.clone();
    let f = r
        .inject(move |ctx, (): ()| {
            let c = c.clone();
            async move {
                let cc = c.clone();
                ctx.on::<Ping>(move |_| {
                    cc.fetch_add(1, Ordering::Relaxed);
                })?;
                let child = ctx.inject(move |ctx, (): ()| {
                    let c = c.clone();
                    async move {
                        let cc = c.clone();
                        ctx.on::<Ping>(move |_| {
                            cc.fetch_add(1, Ordering::Relaxed);
                        })?;
                        let child = ctx.inject(move |ctx, (): ()| {
                            let c = c.clone();
                            async move {
                                ctx.on::<Ping>(move |_| {
                                    c.fetch_add(1, Ordering::Relaxed);
                                })?;
                                Ok(())
                            }
                        })?;
                        child.settled().await?;
                        Ok(())
                    }
                })?;
                child.settled().await?;
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    r.emit(&Ping);
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    f.dispose().await;
    r.emit(&Ping);
    assert_eq!(calls.load(Ordering::Relaxed), 5);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn compare_snapshot() {
    let (k, r, _) = harness();
    let before = k.snapshot().await.fibers.len();
    for _ in 0..2 {
        let f = r
            .inject(|ctx, (): ()| async move {
                ctx.on::<Ping>(|_| {})?;
                Ok(())
            })
            .unwrap();
        f.settled().await.unwrap();
        assert_eq!(k.snapshot().await.fibers.len(), before + 1);
        f.dispose().await;
        assert_eq!(k.snapshot().await.fibers.len(), before);
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn root_dispose() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    assert_eq!(r.fiber().uid(), 0);
    assert_eq!(f.uid(), 1);
    k.shutdown().await;
    assert!(f.is_disposed());
    assert_eq!(t.list(), ["load 0", "unload 0"]);
    assert!(matches!(r.ensure_active(), Err(KernelError::Closed)));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn service_init() {
    let (k, r, t) = harness();
    let f = r
        .inject(|ctx, (): ()| async move {
            let t = ctx.tag::<Arc<Track>>().unwrap();
            t.push("start");
            ctx.effect("stop", move || Undo::sync(move || t.push("stop")))?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["start"]);
    f.dispose().await;
    assert_eq!(t.list(), ["start", "stop"]);
    k.shutdown().await;
}
