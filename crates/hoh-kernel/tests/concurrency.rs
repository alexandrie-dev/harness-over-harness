mod support;
use hoh_kernel::*;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use support::*;
#[tokio::test(start_paused = true)]
async fn commands_are_sent_before_their_futures_are_polled() {
    let (k, r, t) = harness();
    drop(r.provide::<A>(Arc::new(1)));
    let f = r.plugin::<Recorded>(json!({"value":1})).unwrap();
    drop(f.update(json!({"value":2})));
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["load 2"]);
    assert!(r.has("foo"));
    drop(f.restart());
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["load 2", "unload 2", "load 2"]);
    drop(f.dispose());
    turn().await;
    assert!(f.is_disposed());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn notifications_allow_reentrant_commands_without_published_table_locks() {
    let (k, r, t) = harness();
    let rr = r.clone();
    r.on::<InternalService>(move |e| {
        let _ = rr.has(&e.name);
        drop(rr.root().fiber().settled());
    })
    .unwrap();
    let t2 = t.clone();
    r.on::<InternalPlugin>(move |e| {
        if !e.fiber.is_disposed() {
            t2.push("created");
            drop(e.fiber.dispose());
        }
    })
    .unwrap();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["created"]);
    assert!(f.is_disposed());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn failed_kernel_observers_do_not_starve_other_observers() {
    let (k, r, t) = harness();
    r.on::<InternalPlugin>(|_| panic!("observer failed"))
        .unwrap();
    let tt = t.clone();
    r.on::<InternalPlugin>(move |_| tt.push("observed"))
        .unwrap();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    f.dispose().await;
    assert_eq!(t.list(), ["observed", "load 0", "observed", "unload 0"]);
    assert_eq!(
        r.logger()
            .buffer()
            .iter()
            .filter(|r| r.level == Level::Error)
            .count(),
        2
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn self_and_ancestor_waits_return_self_wait_after_sending_commands() {
    let (k, r, t) = harness();
    let x = t.clone();
    let f = r
        .inject(move |parent, (): ()| {
            let t = x.clone();
            async move {
                let ancestor = parent.fiber();
                let child = parent.inject(move |ctx, (): ()| {
                    let t = t.clone();
                    let ancestor = ancestor.clone();
                    async move {
                        assert!(matches!(
                            ctx.fiber().settled().await,
                            Err(KernelError::SelfWait)
                        ));
                        assert!(matches!(
                            ancestor.settled().await,
                            Err(KernelError::SelfWait)
                        ));
                        ctx.root().fiber().settled().await.unwrap();
                        t.push("done");
                        Ok(())
                    }
                })?;
                child.settled().await?;
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["done"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn an_old_activation_cannot_register_after_a_restart() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    let old = t.context();
    let token = old.token();
    f.restart().await.unwrap();
    assert!(token.is_cancelled());
    assert!(matches!(old.on::<Ping>(|_| {}), Err(KernelError::Inactive)));
    let called = AtomicBool::new(false);
    assert!(
        old.effect("old", || {
            called.store(true, Ordering::Relaxed);
            Undo::none()
        })
        .is_err()
    );
    assert!(!called.load(Ordering::Relaxed));
    assert!(t.context().ensure_active().is_ok());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn setup_is_registered_before_it_requests_unload() {
    let (k, r, t) = harness();
    let f = r
        .inject(|ctx, (): ()| async move {
            let fiber = ctx.fiber();
            let t = ctx.tag::<Arc<Track>>().unwrap();
            ctx.effect_async::<Error, _>("setup", move |u| async move {
                drop(fiber.dispose());
                turn().await;
                let tt = t.clone();
                assert!(matches!(
                    u.push(Undo::sync(move || tt.push("rollback"))),
                    Err(KernelError::Inactive)
                ));
                Ok(())
            })
            .await?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["rollback"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn a_leaked_service_reference_is_reported_after_unload() {
    let (k, r, _) = harness();
    let f = r
        .inject(|ctx, (): ()| async move {
            ctx.provide::<A>(Arc::new(7)).await?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    let leaked = r.get::<A>().unwrap();
    f.dispose().await;
    assert_eq!(*leaked, 7);
    assert!(r.logger().buffer().iter().any(|r| r.level == Level::Warn
        && r.message == "service \"foo\" is still referenced after its provider unloaded"));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn shutdown_closes_all_future_operations() {
    let (k, r, _) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    k.shutdown().await;
    assert!(matches!(
        r.provide::<A>(Arc::new(1)).await,
        Err(KernelError::Closed)
    ));
    assert!(matches!(
        f.update(json!({})).await,
        Err(KernelError::Closed)
    ));
    assert!(matches!(f.restart().await, Err(KernelError::Closed)));
    assert!(r.get::<A>().is_none());
}
#[tokio::test(start_paused = true)]
async fn service_type_and_duplicate_checks_span_all_realms() {
    let (k, r, _) = harness();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    assert!(matches!(
        r.provide::<A>(Arc::new(2)).await,
        Err(KernelError::DuplicateService { .. })
    ));
    service!(Other:String="foo");
    assert!(matches!(
        r.isolate("foo", Realm::new())
            .provide::<Other>(Arc::new("wrong".into()))
            .await,
        Err(KernelError::ServiceType { .. })
    ));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn configuration_hooks_reject_the_wrong_config_type() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    let ctx = t.context();
    assert!(matches!(
        ctx.on_candidate::<NoConfig>(|_| Ok(())),
        Err(KernelError::ConfigType)
    ));
    assert!(matches!(
        ctx.on_update::<NoConfig, _>(|_, _| async { Ok(()) }),
        Err(KernelError::ConfigType)
    ));
    ctx.on_candidate::<Value>(|v| {
        if v["bad"] == true {
            Err("candidate denied".into())
        } else {
            Ok(())
        }
    })
    .unwrap();
    let raw = f.raw_config();
    assert!(f.resolve_config(&json!({"bad":true})).is_err());
    assert!(f.update(json!({"bad":true})).await.is_err());
    assert!(Arc::ptr_eq(&raw, &f.raw_config()));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn cancellation_stops_background_work_and_pending_sleep() {
    let (k, r, t) = harness();
    let f = r
        .inject(|ctx, (): ()| async move {
            let t = ctx.tag::<Arc<Track>>().unwrap();
            ctx.spawn("worker", move |token| async move {
                token.cancelled().await;
                t.push("stopped");
                Ok(())
            })?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    f.dispose().await;
    assert_eq!(t.list(), ["stopped"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn dispatch_observers_filters_tags_prepend_and_global_are_composable() {
    let (k, r, t) = harness();
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    r.on_dispatch(move |mode, name, event| {
        assert!(event.is::<Ping>());
        lock(&s).push((mode, name));
    })
    .unwrap();
    let tt = t.clone();
    r.with_tag(1_u32)
        .on::<Ping>(move |_| tt.push("one"))
        .unwrap();
    let tt = t.clone();
    r.with_tag(2_u32)
        .listen()
        .prepend()
        .on::<Ping>(move |_| tt.push("two"))
        .unwrap();
    let tt = t.clone();
    r.listen()
        .global()
        .on::<Ping>(move |_| tt.push("global"))
        .unwrap();
    r.with_filter(|c| c.tag::<u32>().is_some_and(|n| *n == 2))
        .emit(&Ping);
    assert_eq!(t.list(), ["two", "global"]);
    assert_eq!(*lock(&seen), [(Dispatch::Emit, "custom-event")]);
    k.shutdown().await;
}
