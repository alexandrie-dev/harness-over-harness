use super::*;
#[tokio::test(start_paused = true)]
async fn completed_contained_tasks_release_their_owner_records() {
    struct Notice;
    event!(Notice = "contained/records", Contained);
    let k = Kernel::start();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n = count.clone();
    k.root()
        .on_contained_async::<Notice, _>(move |_| {
            let n = n.clone();
            async move {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .unwrap();
    for _ in 0..1000 {
        k.root()
            .dispatch(Notice)
            .invoke(|failure| panic!("unexpected {failure:?}"));
    }
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(count.load(Ordering::SeqCst), 1000);
    assert_eq!(lock(&k.root().activation.effects).effects.len(), 1);
    k.shutdown().await;
}
use std::sync::atomic::AtomicUsize;
struct RaceEvent;
event!(RaceEvent = "review/race", Emit);
async fn bounded(f: impl Future<Output = ()>) {
    tokio::time::timeout(std::time::Duration::from_secs(10), f)
        .await
        .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e1_create_child_against_parent_dispose() {
    bounded(async {
        let k = Kernel::start();
        for _ in 0..64 {
            let slot = Arc::new(Mutex::new(None));
            let s = slot.clone();
            let p = k
                .root()
                .inject(move |ctx, (): ()| {
                    let s = s.clone();
                    async move {
                        *lock(&s) = Some(ctx);
                        Ok(())
                    }
                })
                .unwrap();
            p.settled().await.unwrap();
            let ctx = lock(&slot).clone().unwrap();
            let sentinel = Arc::new(());
            let weak = Arc::downgrade(&sentinel);
            let create = tokio::spawn(async move {
                ctx.inject(move |ctx, (): ()| {
                    let sentinel = sentinel.clone();
                    async move {
                        ctx.on::<RaceEvent>(move |_| {
                            let _ = &sentinel;
                        })?;
                        Ok(())
                    }
                })
            });
            let dispose = tokio::spawn(p.dispose());
            let child = create.await.unwrap();
            dispose.await.unwrap();
            if let Ok(child) = child {
                let _ = child.settled().await;
                child.dispose().await;
            }
            assert!(weak.upgrade().is_none());
            assert_eq!(k.snapshot().await.fibers.len(), 1);
        }
        assert!(lock(&k.root().activation.effects).effects.is_empty());
        k.shutdown().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e1_begin_against_finish_setup() {
    bounded(async {
        let k = Kernel::start();
        for _ in 0..256 {
            let count = Arc::new(AtomicUsize::new(0));
            let c = count.clone();
            let d = k.root().reserve_effect("race").unwrap();
            let effect = d.effect.clone();
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            let b = barrier.clone();
            let setup = tokio::spawn(async move {
                b.wait().await;
                effect.add(Undo::sync(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                }));
                effect.finish_setup();
            });
            let cleanup = tokio::spawn(async move {
                barrier.wait().await;
                d.dispose().await;
            });
            setup.await.unwrap();
            cleanup.await.unwrap();
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
        assert!(lock(&k.root().activation.effects).effects.is_empty());
        k.shutdown().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e1_reserve_against_close() {
    bounded(async {
        let k = Kernel::start();
        for _ in 0..128 {
            let slot = Arc::new(Mutex::new(None));
            let s = slot.clone();
            let p = k
                .root()
                .inject(move |ctx, (): ()| {
                    let s = s.clone();
                    async move {
                        *lock(&s) = Some(ctx);
                        Ok(())
                    }
                })
                .unwrap();
            p.settled().await.unwrap();
            let ctx = lock(&slot).clone().unwrap();
            let setup = Arc::new(AtomicUsize::new(0));
            let clean = Arc::new(AtomicUsize::new(0));
            let a = setup.clone();
            let b = clean.clone();
            let reserve = tokio::spawn(async move {
                ctx.effect("race", move || {
                    a.fetch_add(1, Ordering::SeqCst);
                    Undo::sync(move || {
                        b.fetch_add(1, Ordering::SeqCst);
                    })
                })
            });
            let dispose = tokio::spawn(p.dispose());
            drop(reserve.await.unwrap());
            dispose.await.unwrap();
            assert_eq!(setup.load(Ordering::SeqCst), clean.load(Ordering::SeqCst));
            assert_eq!(k.snapshot().await.fibers.len(), 1);
        }
        k.shutdown().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e1_concurrent_updates() {
    bounded(async {
        let k = Kernel::start();
        let sentinel = Arc::new(());
        let weak = Arc::downgrade(&sentinel);
        let f = k
            .root()
            .inject(move |ctx, (): ()| {
                let s = sentinel.clone();
                async move {
                    ctx.on::<RaceEvent>(move |_| {
                        let _ = &s;
                    })?;
                    Ok(())
                }
            })
            .unwrap();
        f.settled().await.unwrap();
        let mut tasks = vec![];
        for worker in 0..4 {
            let f = f.clone();
            tasks.push(tokio::spawn(async move {
                for n in 0..25 {
                    f.update(serde_json::json!({"n":worker*25+n}))
                        .await
                        .unwrap();
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        f.settled().await.unwrap();
        assert_eq!(f.state(), FiberState::Active);
        f.dispose().await;
        drop(f);
        assert!(weak.upgrade().is_none());
        assert_eq!(k.snapshot().await.fibers.len(), 1);
        assert!(lock(&k.root().activation.effects).effects.is_empty());
        k.shutdown().await;
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn c1_manual_effect_records_are_released() {
    let k = Kernel::start();
    let ctx = k.root();
    for _ in 0..10000 {
        ctx.effect("short", Undo::none).unwrap().dispose().await;
    }
    assert!(lock(&ctx.activation.effects).effects.is_empty());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c1_disposed_children_release_parent_records() {
    let k = Kernel::start();
    let root = k.root();
    struct R(Arc<()>);
    impl ConfigResolver for R {
        fn resolve(&self, _: &Context, v: &serde_json::Value) -> Result<serde_json::Value, Error> {
            let _ = &self.0;
            Ok(v.clone())
        }
    }
    struct P;
    impl Plugin for P {
        const NAME: &'static str = "short";
        type Config = NoConfig;
        type Deps = ();
        async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
            Ok(())
        }
    }
    let mut probes = Vec::new();
    for _ in 0..10000 {
        let value = Arc::new(());
        probes.push(Arc::downgrade(&value));
        let f = root
            .plugin_dyn(
                &PluginDef::of::<P>(),
                serde_json::Value::Null,
                PluginOptions {
                    resolver: Some(Arc::new(R(value))),
                    ..Default::default()
                },
            )
            .unwrap();
        f.settled().await.unwrap();
        f.dispose().await;
    }
    assert!(probes.iter().all(|p| p.upgrade().is_none()));
    assert!(lock(&root.activation.effects).effects.is_empty());
    k.shutdown().await;
}
#[test]
fn c8_dispose_without_runtime_does_not_panic() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let c = called.clone();
    let (k, d) = runtime.block_on(async {
        let k = Kernel::start();
        let d = k
            .root()
            .try_effect::<KernelError>("async", |u| {
                u.push(Undo::future(|| async { Ok(()) }))?;
                u.push(Undo::sync(move || {
                    c.store(true, Ordering::Release);
                }))
            })
            .unwrap();
        (k, d)
    });
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| drop(d.dispose())));
    assert!(result.is_ok());
    assert!(called.load(Ordering::Acquire));
    assert!(k.root().logger().buffer().iter().any(|record| {
        record
            .message
            .contains("asynchronous disposal requires a tokio runtime")
    }));
    runtime.block_on(k.shutdown());
}
