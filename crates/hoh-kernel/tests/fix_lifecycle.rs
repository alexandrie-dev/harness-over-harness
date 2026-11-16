mod support;
use futures_util::StreamExt;
use hoh_kernel::*;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use support::*;
#[tokio::test(start_paused = true)]
async fn c2_late_undo_runs() {
    let k = Kernel::start();
    let slot = Mutex::new(None);
    let d = k
        .root()
        .try_effect::<KernelError>("late", |u| {
            *lock(&slot) = Some(u.clone());
            Ok(())
        })
        .unwrap();
    d.dispose().await;
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    assert!(matches!(
        lock(&slot).as_ref().unwrap().push(Undo::sync(move || {
            c.fetch_add(1, Ordering::Relaxed);
        })),
        Err(KernelError::Inactive)
    ));
    assert_eq!(count.load(Ordering::Relaxed), 1);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c2_dropped_adoption_does_not_escape() {
    let (k, r, t) = harness();
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let f = r
        .inject(move |ctx, (): ()| {
            let c = c.clone();
            async move {
                drop(Undo::from(ctx.on::<Ping>(move |_| {
                    c.fetch_add(1, Ordering::Relaxed);
                })?));
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    f.restart().await.unwrap();
    r.emit(&Ping);
    assert_eq!(count.load(Ordering::Relaxed), 1);
    f.dispose().await;
    r.emit(&Ping);
    assert_eq!(count.load(Ordering::Relaxed), 1);
    drop(t);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c3_source_retains_rejected_raw() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"value":0})).unwrap();
    f.settled().await.unwrap();
    let gate = t.clone();
    t.context()
        .on_update::<Value, _>(move |v, _| {
            let gate = gate.clone();
            async move {
                if v["value"] == 1 {
                    gate.gate.acquire().await.unwrap().forget();
                    Ok(())
                } else {
                    Err("rejected B".into())
                }
            }
        })
        .unwrap();
    let a = f.update(json!({"value":1}));
    turn().await;
    assert!(f.update(json!({"value":2})).await.is_err());
    t.gate.add_permits(1);
    a.await.unwrap();
    assert_eq!(f.raw_config()["value"], 2);
    assert_eq!(f.effective_config().unwrap()["value"], 0);
    f.restart().await.unwrap();
    assert_eq!(f.effective_config().unwrap()["value"], 2);
    k.shutdown().await;
}
struct RegisterResolver {
    count: Arc<AtomicUsize>,
}
impl ConfigResolver for RegisterResolver {
    fn resolve(&self, ctx: &Context, raw: &Value) -> Result<Value, Error> {
        let count = self.count.clone();
        let _ = ctx.on::<Ping>(move |_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        Ok(raw.clone())
    }
}
#[tokio::test(start_paused = true)]
async fn c4_pending_resolver_has_no_live_activation() {
    let (k, r, _) = harness();
    let count = Arc::new(AtomicUsize::new(0));
    let f = r
        .plugin_dyn(
            &PluginDef::of::<Recorded>(),
            json!({}),
            PluginOptions {
                inject: vec!["foo".into()],
                resolver: Some(Arc::new(RegisterResolver {
                    count: count.clone(),
                })),
                ..Default::default()
            },
        )
        .unwrap();
    f.settled().await.unwrap();
    f.resolve_config(&json!({})).unwrap();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    f.settled().await.unwrap();
    f.dispose().await;
    r.emit(&Ping);
    assert_eq!(count.load(Ordering::Relaxed), 0);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c5_cleanup_self_dispose_finishes() {
    let k = Kernel::start();
    let f = k
        .root()
        .inject(|ctx, (): ()| async move {
            let own = ctx.fiber();
            ctx.effect("self", move || {
                Undo::future(move || async move {
                    own.dispose().await;
                    Ok(())
                })
            })?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), f.dispose())
            .await
            .is_ok()
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c5_spawn_self_dispose_finishes() {
    let (k, r, t) = harness();
    let f = r
        .inject(|ctx, (): ()| async move {
            let own = ctx.fiber();
            ctx.spawn("self", move |_| async move {
                own.dispose().await;
                Ok(())
            })?;
            Ok(())
        })
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), async {
            f.settled().await.unwrap();
            f.dispose().await;
        })
        .await
        .is_ok()
    );
    drop(t);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c5_indirect_event_wait_is_guarded() {
    let k = Kernel::start();
    let f = k
        .root()
        .inject(|ctx, (): ()| async move {
            let own = ctx.fiber();
            ctx.on_serial::<Ser, i32, _>(move |_| {
                let own = own.clone();
                async move {
                    assert!(matches!(own.settled().await, Err(KernelError::SelfWait)));
                    Ok(None)
                }
            })?;
            ctx.serial(Ser).await?;
            Ok(())
        })
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), f.settled())
            .await
            .unwrap()
            .is_ok()
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c6_filter_panic_does_not_close_actor() {
    let k = Kernel::start();
    k.root().on::<InternalStatus>(|_| {}).unwrap();
    let f = k
        .root()
        .with_filter(|_| panic!("filter"))
        .inject(|_, (): ()| async { Ok(()) })
        .unwrap();
    f.settled().await.unwrap();
    assert!(k.root().ensure_active().is_ok());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c7_zero_is_one_millisecond() {
    let k = Kernel::start();
    let start = tokio::time::Instant::now();
    let mut stream = Box::pin(k.root().interval(Duration::ZERO));
    assert_eq!(stream.next().await, Some(()));
    assert_eq!(start.elapsed(), Duration::from_millis(1));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c7_idle_ticks_are_discarded() {
    let k = Kernel::start();
    let start = tokio::time::Instant::now();
    let mut stream = Box::pin(k.root().interval(Duration::from_millis(100)));
    tokio::time::advance(Duration::from_millis(350)).await;
    stream.next().await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_millis(400));
    stream.next().await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_millis(500));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c8_service_name_type_can_be_reused() {
    let k = Kernel::start();
    service!(First:u32="reused");
    service!(Second:String="reused");
    let d = k.root().provide::<First>(Arc::new(1)).await.unwrap();
    d.dispose().await;
    k.root()
        .provide::<Second>(Arc::new("next".into()))
        .await
        .unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c9_old_context_releases_committed_value() {
    let k = Kernel::start();
    service!(Key:String="held");
    #[derive(Inject)]
    struct D {
        _value: Service<Key>,
    }
    let value = Arc::new("resource".to_owned());
    let weak = Arc::downgrade(&value);
    let d = k.root().provide::<Key>(value).await.unwrap();
    let slot = Arc::new(Mutex::new(None::<Context>));
    let s = slot.clone();
    let f = k
        .root()
        .inject(move |ctx, _: D| {
            let s = s.clone();
            async move {
                *lock(&s) = Some(ctx);
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    f.dispose().await;
    d.dispose().await;
    assert!(weak.upgrade().is_none());
    assert!(lock(&slot).is_some());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c10_source_stale_delegation_still_restarts() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"value":0})).unwrap();
    f.settled().await.unwrap();
    let gate = t.clone();
    t.context()
        .on_update::<Value, _>(move |v, next| {
            let gate = gate.clone();
            async move {
                if v["value"] == 1 {
                    gate.gate.acquire().await.unwrap().forget();
                    next.run().await?;
                }
                Ok(())
            }
        })
        .unwrap();
    let a = f.update(json!({"value":1}));
    turn().await;
    f.update(json!({"value":2})).await.unwrap();
    t.gate.add_permits(1);
    a.await.unwrap();
    assert_eq!(t.list(), ["load 0", "unload 0", "load 2"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c11_loading_withdrawal_notifies() {
    let (k, r, t) = harness();
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    r.on::<InternalService>(move |e| {
        if e.name.as_ref() == "foo" {
            lock(&s).push(e.available);
        }
    })
    .unwrap();
    let observations = seen.clone();
    let f = r
        .inject(move |ctx, (): ()| {
            let observations = observations.clone();
            async move {
                let d = ctx.provide::<A>(Arc::new(1)).await?;
                lock(&observations).clear();
                d.dispose().await;
                let t = ctx.tag::<Arc<Track>>().unwrap();
                t.gate.acquire().await.unwrap().forget();
                Ok(())
            }
        })
        .unwrap();
    turn().await;
    assert_eq!(f.state(), FiberState::Loading);
    assert_eq!(*lock(&seen), [false]);
    t.gate.add_permits(1);
    f.settled().await.unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c13_nested_aggregate_is_flattened() {
    let k = Kernel::start();
    let f = k
        .root()
        .inject(|_, (): ()| async {
            let inner = KernelError::Listeners {
                event: "inner",
                errors: vec![Arc::new("leaf one".into()), Arc::new("leaf two".into())],
            };
            Err(Box::new(KernelError::Listeners {
                event: "outer",
                errors: vec![Arc::new(Box::new(inner) as Error)],
            }) as Error)
        })
        .unwrap();
    assert!(f.settled().await.is_err());
    assert_eq!(
        k.root()
            .logger()
            .buffer()
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>(),
        ["leaf one", "leaf two"]
    );
    k.shutdown().await;
}
