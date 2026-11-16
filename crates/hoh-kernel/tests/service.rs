mod support;
use hoh_kernel::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use support::*;
#[tokio::test(start_paused = true)]
async fn pending_inject() {
    let (k, r, t) = harness();
    let t2 = t.clone();
    let c = r
        .inject(move |_, _: ADeps| {
            let t = t2.clone();
            async move {
                t.push("consumer");
                Ok(())
            }
        })
        .unwrap();
    let t2 = t.clone();
    let p = r
        .inject(move |ctx, (): ()| {
            let t = t2.clone();
            async move {
                ctx.provide::<A>(Arc::new(1)).await?;
                t.gate.acquire().await.unwrap().forget();
                Ok(())
            }
        })
        .unwrap();
    turn().await;
    assert_eq!(p.state(), FiberState::Loading);
    assert_eq!(c.state(), FiberState::Pending);
    assert!(!r.has("foo"));
    t.gate.add_permits(1);
    p.settled().await.unwrap();
    c.settled().await.unwrap();
    assert_eq!(t.list(), ["consumer"]);
    k.shutdown().await;
}
struct Counter(Arc<AtomicUsize>);
impl Counter {
    fn increase(&self, ctx: &Context) -> Result<(), KernelError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        ctx.effect("counter", Undo::none)?;
        Ok(())
    }
}
service!(Count:Counter="counter");
#[derive(Inject)]
struct CountDeps {
    counter: Service<Count>,
}
async fn traceable(inject: bool) {
    let (k, r, _) = harness();
    let value = Arc::new(AtomicUsize::new(0));
    r.provide::<Count>(Arc::new(Counter(value.clone())))
        .await
        .unwrap();
    r.get::<Count>().unwrap().increase(&r).unwrap();
    let f = if inject {
        r.inject(|ctx, d: CountDeps| async move {
            d.counter.increase(&ctx)?;
            Ok(())
        })
        .unwrap()
    } else {
        r.inject(|ctx, (): ()| async move {
            ctx.get::<Count>().unwrap().increase(&ctx)?;
            Ok(())
        })
        .unwrap()
    };
    f.settled().await.unwrap();
    assert_eq!(value.load(Ordering::Relaxed), 2);
    f.dispose().await;
    r.get::<Count>().unwrap().increase(&r).unwrap();
    assert_eq!(value.load(Ordering::Relaxed), 3);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn traceable_effect_with_inject() {
    traceable(true).await;
}
#[tokio::test(start_paused = true)]
async fn traceable_effect_without_inject() {
    traceable(false).await;
}
#[tokio::test(start_paused = true)]
async fn compare_snapshot() {
    let (k, r, _) = harness();
    for _ in 0..2 {
        let f = r
            .inject(|ctx, (): ()| async move {
                ctx.provide::<A>(Arc::new(1)).await?;
                ctx.inject(|_, _: ADeps| async { Ok(()) })?;
                Ok(())
            })
            .unwrap();
        f.settled().await.unwrap();
        turn().await;
        assert_eq!(k.snapshot().await.provisions.len(), 1);
        f.dispose().await;
        assert!(k.snapshot().await.provisions.is_empty());
        assert_eq!(k.snapshot().await.fibers.len(), 1);
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn multiple_injects() {
    let (k, r, t) = harness();
    let x = t.clone();
    let a = r
        .inject(move |ctx, d: ADeps| {
            let t = x.clone();
            async move {
                assert_eq!(*d.a, 1);
                t.push("foo");
                ctx.provide::<B>(Arc::new(2)).await?;
                Ok(())
            }
        })
        .unwrap();
    let x = t.clone();
    let b = r
        .inject(move |_, d: ABDeps| {
            let t = x.clone();
            async move {
                assert_eq!((*d.a, *d.b), (1, 2));
                t.push("bar");
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(a.state(), FiberState::Pending);
    assert_eq!(b.state(), FiberState::Pending);
    r.provide::<A>(Arc::new(1)).await.unwrap();
    a.settled().await.unwrap();
    b.settled().await.unwrap();
    assert_eq!(t.list(), ["foo", "bar"]);
    k.shutdown().await;
}
pub async fn dependents(restart: bool, isolated: bool) {
    let (k, r, t) = harness();
    let r = if isolated {
        r.isolate("foo", Realm::new())
    } else {
        r
    };
    let alive = Arc::new(AtomicBool::new(true));
    let a = alive.clone();
    let p = r
        .inject(move |ctx, (): ()| {
            let alive = a.clone();
            async move {
                alive.store(true, Ordering::Release);
                ctx.provide::<A>(Arc::new(1)).await?;
                let t = ctx.tag::<Arc<Track>>().unwrap();
                ctx.effect("provider", move || {
                    Undo::sync(move || {
                        alive.store(false, Ordering::Release);
                        t.push("foo");
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    p.settled().await.unwrap();
    let a = alive.clone();
    let c = r
        .inject(move |ctx, d: ADeps| {
            let alive = a.clone();
            async move {
                let t = ctx.tag::<Arc<Track>>().unwrap();
                ctx.effect("consumer", move || {
                    Undo::future(move || async move {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        assert!(alive.load(Ordering::Acquire));
                        assert_eq!(*d.a, 1);
                        t.push("dependent");
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    c.settled().await.unwrap();
    if restart {
        p.restart().await.unwrap();
        c.settled().await.unwrap();
    } else {
        p.dispose().await;
    }
    assert_eq!(t.list(), ["dependent", "foo"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn dispose_dependents_before_provider() {
    dependents(false, false).await;
}
#[tokio::test(start_paused = true)]
async fn dispose_dependents_before_provider_restarts() {
    dependents(true, false).await;
}
#[tokio::test(start_paused = true)]
async fn dispose_dependents_before_isolated_provider() {
    dependents(false, true).await;
}
