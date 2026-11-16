use hoh_kernel::*;
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
async fn turn() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}
struct Ping;
event!(Ping = "scope/ping", Emit);
#[tokio::test(start_paused = true)]
async fn a_carrier_can_read_and_preserve_a_separate_base_filter() {
    let k = Kernel::start();
    let base = k
        .root()
        .with_filter(|listener| listener.tag::<Tag>().is_some_and(|tag| tag.0 != 2));
    assert!(base.matches_filter(&k.root().with_tag(Tag(1))));
    assert!(!base.matches_filter(&k.root().with_tag(Tag(2))));
    let carrier = k.root().with_filter(move |listener| {
        base.matches_filter(listener) && listener.tag::<Tag>().is_some_and(|tag| tag.0 == 1)
    });
    assert!(carrier.matches_filter(&k.root().with_tag(Tag(1))));
    assert!(!carrier.matches_filter(&k.root().with_tag(Tag(2))));
    assert!(!carrier.matches_filter(&k.root().with_tag(Tag(3))));
    assert!(k.root().matches_filter(&k.root()));
    k.shutdown().await;
}
#[derive(Clone)]
struct Tag(u32);

#[tokio::test(start_paused = true)]
async fn is_usable_synchronously_before_the_backing_fiber_activates() {
    let k = Kernel::start();
    let scope = k
        .root()
        .with_tag(Tag(1))
        .child("scope")
        .unwrap()
        .with_tag(Tag(2));
    assert_eq!(scope.fiber().state(), FiberState::Pending);
    assert_eq!(scope.fiber().name().as_ref(), "scope");
    assert_eq!(scope.tag::<Tag>().unwrap().0, 2);
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    scope
        .on::<Ping>(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    k.root()
        .with_filter(|ctx| ctx.tag::<Tag>().is_some_and(|tag| tag.0 == 2))
        .emit(&Ping);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    scope.fiber().settled().await.unwrap();
    assert!(scope.ensure_active().is_ok());
    k.root().emit(&Ping);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    scope.fiber().dispose().await;
    k.root().emit(&Ping);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        scope.on::<Ping>(|_| {}),
        Err(KernelError::Inactive)
    ));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn preactivation_effects_are_disposed_when_creation_loses_to_parent_shutdown() {
    let k = Kernel::start();
    let shutdown = k.shutdown();
    let child = k.root().child("scope").unwrap();
    let disposed = Arc::new(AtomicBool::new(false));
    let d = disposed.clone();
    child
        .effect("early", move || {
            Undo::sync(move || d.store(true, Ordering::SeqCst))
        })
        .unwrap();
    shutdown.await;
    assert!(disposed.load(Ordering::SeqCst));
    assert_eq!(child.fiber().state(), FiberState::Disposed);
}
#[tokio::test(start_paused = true)]
async fn disposing_before_first_activation_runs_every_effect() {
    let k = Kernel::start();
    let child = k.root().child("scope").unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    for _ in 0..3 {
        let count = count.clone();
        child
            .effect("early", move || {
                Undo::sync(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                })
            })
            .unwrap();
    }
    child.fiber().dispose().await;
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert_eq!(k.snapshot().await.fibers.len(), 1);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn raw_child_disposer_is_adopted_by_ordered_parent_effect() {
    let k = Kernel::start();
    let trace = Arc::new(Mutex::new(vec![]));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let inner_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let t = trace.clone();
    let g = gate.clone();
    let i = inner_gate.clone();
    let parent = k
        .root()
        .inject(move |ctx, (): ()| {
            let t = t.clone();
            let g = g.clone();
            let i = i.clone();
            async move {
                let owner = ctx.clone();
                ctx.effect_async::<Error, _>("ordered", move |u| async move {
                    let outer = t.clone();
                    u.push(Undo::future(move || async move {
                        lock(&outer).push("outer");
                        Ok(())
                    }))?;
                    let scope = owner.child("scope")?;
                    let middle = t.clone();
                    scope.effect("scope", move || {
                        Undo::future(move || async move {
                            lock(&middle).push("scope begin");
                            g.acquire().await.unwrap().forget();
                            lock(&middle).push("scope end");
                            Ok(())
                        })
                    })?;
                    u.push(scope.fiber().disposer().ok_or(KernelError::Inactive)?)?;
                    u.push(Undo::future(move || async move {
                        lock(&t).push("inner begin");
                        i.acquire().await.unwrap().forget();
                        lock(&t).push("inner end");
                        Ok(())
                    }))?;
                    Ok(())
                })
                .await?;
                Ok(())
            }
        })
        .unwrap();
    parent.settled().await.unwrap();
    let dispose = parent.dispose();
    turn().await;
    assert_eq!(*lock(&trace), ["inner begin"]);
    inner_gate.add_permits(1);
    turn().await;
    assert_eq!(*lock(&trace), ["inner begin", "inner end", "scope begin"]);
    gate.add_permits(1);
    dispose.await;
    assert_eq!(
        *lock(&trace),
        [
            "inner begin",
            "inner end",
            "scope begin",
            "scope end",
            "outer"
        ]
    );
    assert!(k.root().ensure_active().is_ok());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn raw_disposal_and_repeated_fiber_disposal_join_full_teardown() {
    let k = Kernel::start();
    let child = k.root().child("scope").unwrap();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let g = gate.clone();
    let cleaned = Arc::new(AtomicBool::new(false));
    let c = cleaned.clone();
    child
        .effect("slow", move || {
            Undo::future(move || async move {
                g.acquire().await.unwrap().forget();
                c.store(true, Ordering::SeqCst);
                Ok(())
            })
        })
        .unwrap();
    child.fiber().settled().await.unwrap();
    let raw = tokio::spawn(child.fiber().disposer().unwrap().dispose());
    let wait = tokio::spawn(child.fiber().dispose());
    turn().await;
    assert!(!raw.is_finished());
    assert!(!wait.is_finished());
    assert!(!cleaned.load(Ordering::SeqCst));
    gate.add_permits(1);
    raw.await.unwrap();
    wait.await.unwrap();
    assert!(cleaned.load(Ordering::SeqCst));
    child.fiber().dispose().await;
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn composed_filter_calls_base_first_and_reads_listener_scope_tag() {
    let k = Kernel::start();
    let order = Arc::new(Mutex::new(vec![]));
    let first = order.clone();
    let second = order.clone();
    let target = k
        .root()
        .with_filter(move |ctx| {
            lock(&first).push(("base", ctx.tag::<Tag>().unwrap().0));
            ctx.tag::<Tag>().unwrap().0 != 2
        })
        .and_filter(move |ctx| {
            lock(&second).push(("scope", ctx.tag::<Tag>().unwrap().0));
            ctx.tag::<Tag>().unwrap().0 == 1
        });
    let calls = Arc::new(AtomicUsize::new(0));
    for tag in [1, 2, 3] {
        let scope = k.root().child("scope").unwrap().with_tag(Tag(tag));
        let c = calls.clone();
        scope
            .on::<Ping>(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
    }
    target.emit(&Ping);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *lock(&order),
        [
            ("base", 1),
            ("scope", 1),
            ("base", 2),
            ("base", 3),
            ("scope", 3)
        ]
    );
    k.shutdown().await;
}
service!(Resource:String="resource");
#[derive(Inject)]
struct Needs {
    value: Service<Resource>,
}
#[tokio::test(start_paused = true)]
async fn scoped_context_inherits_creator_committed_services_through_teardown() {
    let k = Kernel::start();
    let value = Arc::new("original".to_owned());
    let weak = Arc::downgrade(&value);
    let provision = k.root().provide::<Resource>(value).await.unwrap();
    let held = Arc::new(Mutex::new(None::<Context>));
    let out = held.clone();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let g = gate.clone();
    let read = Arc::new(AtomicBool::new(false));
    let seen = read.clone();
    let owner = k
        .root()
        .inject(move |ctx, deps: Needs| {
            let out = out.clone();
            let g = g.clone();
            let seen = seen.clone();
            async move {
                assert_eq!(deps.value.as_str(), "original");
                let scope = ctx.child("scope")?;
                assert_eq!(scope.get::<Resource>().unwrap().as_str(), "original");
                *lock(&out) = Some(scope.clone());
                let reading = scope.clone();
                scope.effect("read on cleanup", move || {
                    Undo::future(move || async move {
                        g.acquire().await.unwrap().forget();
                        assert!(
                            reading
                                .isolate("resource", Realm::ROOT)
                                .get::<Resource>()
                                .is_none()
                        );
                        assert_eq!(
                            reading
                                .isolate("other", Realm::new())
                                .get::<Resource>()
                                .unwrap()
                                .as_str(),
                            "original"
                        );
                        assert_eq!(reading.get::<Resource>().unwrap().as_str(), "original");
                        seen.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    owner.settled().await.unwrap();
    let scope = lock(&held).clone().unwrap();
    scope.fiber().settled().await.unwrap();
    let withdrawal = provision.dispose();
    turn().await;
    assert_eq!(owner.state(), FiberState::Unloading);
    assert!(!read.load(Ordering::SeqCst));
    assert!(weak.upgrade().is_some());
    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(1), withdrawal)
        .await
        .unwrap();
    owner.settled().await.unwrap();
    assert!(read.load(Ordering::SeqCst));
    assert!(scope.get::<Resource>().is_none());
    assert!(weak.upgrade().is_none());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn scoped_context_inherits_tags_realms_and_intercepts() {
    let k = Kernel::start();
    let realm = Realm::new();
    let parent = k
        .root()
        .with_tag(Tag(7))
        .isolate("resource", realm)
        .intercept("resource", serde_json::json!({"secret":true}));
    let scope = parent.child("scope").unwrap();
    assert_eq!(scope.tag::<Tag>().unwrap().0, 7);
    assert_eq!(scope.realm_of("resource"), realm);
    assert_eq!(scope.intercepted::<Value>("resource")["secret"], true);
    k.shutdown().await;
}
