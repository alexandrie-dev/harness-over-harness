use hoh_kernel::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test(start_paused = true)]
async fn removal_of_an_ordinary_parent_is_not_a_volatile_only_change() {
    let schema = json!({"type":"object","properties":{"block":{"type":"object","properties":{"value":{"type":"string","x-volatile":true}}}}});
    assert!(!config::equal_except_volatile(
        &schema,
        &json!({"block":{"value":"old"}}),
        &json!({})
    ));
}

#[tokio::test(start_paused = true)]
async fn comparison_checks_raw_shapes_before_object_defaults() {
    for schema in [
        json!({"type":"string","default":"default"}),
        json!({"type":"array","default":[],"items":{"x-volatile":true}}),
        json!({"type":"object","default":{},"additionalProperties":{"x-volatile":true}}),
        json!({"type":"object","properties":{"value":{"x-volatile":true}},"$ref":"#","default":{}}),
    ] {
        let default = schema["default"].clone();
        assert!(
            !config::equal_except_volatile(&schema, &json!(null), &default),
            "{schema}"
        );
        let parent = json!({"type":"object","properties":{"item":schema}});
        assert!(
            !config::equal_except_volatile(&parent, &json!({}), &json!({"item":default})),
            "{parent}"
        );
    }
    for schema in [
        json!({"type":"array","items":{"x-volatile":true}}),
        json!({"anyOf":[{"type":"object","properties":{"value":{"x-volatile":true}}}]}),
    ] {
        assert!(!config::equal_except_volatile(
            &schema,
            &json!(["old"]),
            &json!(["new"])
        ));
    }
    let object = json!({"type":"object","properties":{"value":{"x-volatile":true}}});
    assert!(!config::equal_except_volatile(
        &object,
        &json!(null),
        &json!({})
    ));
    assert!(!config::equal_except_volatile(
        &object,
        &json!({"__jsExpr":"before","value":"old"}),
        &json!({"__jsExpr":"after","value":"new"})
    ));
    let expression = json!({"type":"object","properties":{"__jsExpr":{"x-volatile":true},"extra":{"type":"integer"}}});
    assert!(!config::equal_except_volatile(
        &expression,
        &json!({"__jsExpr":"before","extra":1}),
        &json!({"__jsExpr":"after","extra":1})
    ));
}

#[tokio::test(start_paused = true)]
async fn context_only_startup_capture_retains_registered_isolated_providers() {
    service!(Ready: () = "ready");
    #[derive(Inject)]
    struct NeedsReady {
        _ready: Service<Ready>,
    }
    let k = Kernel::start();
    let ctx = k.root();
    let provider = ctx.isolate(Ready::NAME, Realm::new());
    provider.provide::<Ready>(Arc::new(())).await.unwrap();
    let consumer = ctx.inject(|_, _: NeedsReady| async { Ok(()) }).unwrap();
    consumer.settled().await.unwrap();
    // The capture helper receives only a Context, without a Kernel handle.
    async fn capture(ctx: &Context) -> Snapshot {
        ctx.reflect().store().await
    }
    let snapshot = capture(&ctx.with_filter(|_| false)).await;
    let row = snapshot
        .fibers
        .iter()
        .find(|f| f.uid == consumer.uid())
        .unwrap();
    assert_eq!(row.missing, [Arc::<str>::from("ready")]);
    let provisions: Vec<_> = snapshot
        .provisions
        .iter()
        .filter(|p| p.name.as_ref() == "ready")
        .collect();
    assert_eq!(provisions.len(), 1);
    assert_eq!(provisions[0].realm, provider.realm_of("ready"));
    assert_ne!(provisions[0].realm, ctx.realm_of("ready"));
    assert!(provisions[0].active);
    let owner = snapshot
        .fibers
        .iter()
        .find(|f| f.uid == provisions[0].owner)
        .unwrap();
    assert_eq!(owner.uid, provider.fiber().uid());
    assert_eq!(owner.state, FiberState::Active);
    k.shutdown().await;
    assert!(ctx.reflect().store().await.provisions.is_empty());
    assert_eq!(snapshot.provisions.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn reflection_keeps_loading_owners_and_their_registered_provisions_in_one_capture() {
    service!(Registered: () = "registered");
    let k = Kernel::start();
    let root = k.root();
    let scope = root.isolate(Registered::NAME, Realm::new());
    let (entered, ready) = tokio::sync::oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let g = gate.clone();
    let owner = scope
        .inject(move |ctx, (): ()| {
            let entered = entered.clone();
            let g = g.clone();
            async move {
                ctx.provide::<Registered>(Arc::new(())).await?;
                if let Some(entered) = entered.lock().unwrap().take() {
                    let _ = entered.send(());
                }
                let _ = g.acquire().await;
                Ok(())
            }
        })
        .unwrap();
    ready.await.unwrap();
    let reflection = root.reflect();
    let snapshot = reflection.store().await;
    let p = snapshot
        .provisions
        .iter()
        .find(|p| p.name.as_ref() == "registered")
        .unwrap();
    assert_eq!(p.owner, owner.uid());
    assert_eq!(p.realm, scope.realm_of("registered"));
    assert!(!p.active);
    assert_eq!(
        snapshot
            .fibers
            .iter()
            .find(|f| f.uid == p.owner)
            .unwrap()
            .state,
        FiberState::Loading
    );
    gate.add_permits(1);
    owner.settled().await.unwrap();
    assert!(
        reflection
            .store()
            .await
            .provisions
            .iter()
            .any(|p| p.name.as_ref() == "registered" && p.active)
    );
    k.shutdown().await;
}

service!(Missing: () = "fix3-missing");
#[derive(Inject)]
struct MissingDeps {
    _missing: Service<Missing>,
}
fn record_status(root: &Context, fiber: &Fiber) -> Arc<Mutex<Vec<(FiberState, FiberState)>>> {
    let log = Arc::new(Mutex::new(vec![]));
    let out = log.clone();
    let uid = fiber.uid();
    root.on::<InternalStatus>(move |e| {
        if e.fiber.uid() == uid {
            out.lock().unwrap().push((e.old, e.fiber.state()));
        }
    })
    .unwrap();
    log
}

#[tokio::test(start_paused = true)]
async fn an_ordinary_pending_fiber_unloads_once_before_disposal() {
    let k = Kernel::start();
    let root = k.root();
    let f = root.inject(|_, _: MissingDeps| async { Ok(()) }).unwrap();
    f.settled().await.unwrap();
    let log = record_status(&root, &f);
    f.dispose().await;
    f.dispose().await;
    assert_eq!(
        *log.lock().unwrap(),
        [
            (FiberState::Pending, FiberState::Unloading),
            (FiberState::Unloading, FiberState::Disposed),
        ]
    );
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_idle_failed_fiber_also_enters_unloading_before_disposal() {
    let k = Kernel::start();
    let root = k.root();
    let f = root
        .inject(|_, (): ()| async { Err("failed".into()) })
        .unwrap();
    assert!(f.settled().await.is_err());
    assert_eq!(f.state(), FiberState::Failed);
    let log = record_status(&root, &f);
    f.dispose().await;
    assert_eq!(
        *log.lock().unwrap(),
        [
            (FiberState::Failed, FiberState::Unloading),
            (FiberState::Unloading, FiberState::Disposed),
        ]
    );
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_pending_fiber_after_dependency_loss_starts_a_new_unload_for_disposal() {
    let k = Kernel::start();
    let root = k.root();
    let provision = root.provide::<Missing>(Arc::new(())).await.unwrap();
    let f = root.inject(|_, _: MissingDeps| async { Ok(()) }).unwrap();
    f.settled().await.unwrap();
    provision.dispose().await;
    f.settled().await.unwrap();
    assert_eq!(f.state(), FiberState::Pending);
    let log = record_status(&root, &f);
    f.dispose().await;
    assert_eq!(
        *log.lock().unwrap(),
        [
            (FiberState::Pending, FiberState::Unloading),
            (FiberState::Unloading, FiberState::Disposed),
        ]
    );
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn disposal_during_an_existing_unload_does_not_start_a_second_one() {
    let k = Kernel::start();
    let root = k.root();
    let provider = root.provide::<Missing>(Arc::new(())).await.unwrap();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let g = gate.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let f = root
        .inject(move |ctx, _: MissingDeps| {
            let g = g.clone();
            let entered = entered.clone();
            async move {
                ctx.effect("hold", move || {
                    Undo::future(move || async move {
                        if let Some(entered) = entered.lock().unwrap().take() {
                            let _ = entered.send(());
                        }
                        let _ = g.acquire().await;
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    let log = record_status(&root, &f);
    let withdrawal = provider.dispose();
    ready.await.unwrap();
    assert_eq!(f.state(), FiberState::Unloading);
    let disposed = f.dispose();
    gate.add_permits(1);
    disposed.await;
    withdrawal.await;
    assert_eq!(
        *log.lock().unwrap(),
        [
            (FiberState::Active, FiberState::Unloading),
            (FiberState::Unloading, FiberState::Disposed),
        ]
    );
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn disposal_during_loading_waits_for_that_transition_before_unloading() {
    let k = Kernel::start();
    let root = k.root();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let g = gate.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let f = root
        .inject(move |_, (): ()| {
            let entered = entered.clone();
            let g = g.clone();
            async move {
                if let Some(entered) = entered.lock().unwrap().take() {
                    let _ = entered.send(());
                }
                let _ = g.acquire().await;
                Ok(())
            }
        })
        .unwrap();
    ready.await.unwrap();
    let log = record_status(&root, &f);
    let disposed = f.dispose();
    let _ = root.reflect().store().await;
    assert_eq!(f.state(), FiberState::Loading);
    assert!(log.lock().unwrap().is_empty());
    gate.add_permits(1);
    disposed.await;
    assert_eq!(
        *log.lock().unwrap(),
        [
            (FiberState::Loading, FiberState::Unloading),
            (FiberState::Unloading, FiberState::Disposed),
        ]
    );
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn synchronous_cleanup_failure_replaces_the_setup_failure() {
    let k = Kernel::start();
    let result = k.root().try_effect::<Error>("rollback", |u| {
        u.push(Undo::sync(|| panic!("#2")))?;
        Err("#1".into())
    });
    let Err(error) = result else {
        panic!("setup succeeded")
    };
    assert_eq!(error.to_string(), "#2");
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn fallible_synchronous_cleanup_preserves_its_error_and_stops_rollback() {
    #[derive(Debug, thiserror::Error)]
    #[error("#2")]
    struct Cleanup(Arc<()>);
    let k = Kernel::start();
    let root = k.root();
    let marker = Arc::new(());
    let identity = marker.clone();
    let reached = Arc::new(Mutex::new(false));
    let out = reached.clone();
    let result = root.try_effect::<KernelError>("rollback", |u| {
        u.push(Undo::sync(move || *out.lock().unwrap() = true))?;
        u.push(Undo::try_sync(move || Err(Box::new(Cleanup(marker)))))?;
        Err(KernelError::Failed(Arc::new("#1".into())))
    });
    let Err(KernelError::Failed(error)) = result else {
        panic!("missing cleanup failure")
    };
    assert!(Arc::ptr_eq(
        &error.downcast_ref::<Cleanup>().unwrap().0,
        &identity
    ));
    assert_eq!(error.to_string(), "#2");
    assert!(!*reached.lock().unwrap());
    assert!(root.logger().buffer().is_empty());
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn synchronous_rollback_observes_an_adopted_cleanup_error() {
    let k = Kernel::start();
    let root = k.root();
    let inner = root
        .effect("inner", || Undo::try_sync(|| Err("#2".into())))
        .unwrap();
    let result = root.try_effect::<Error>("outer", |u| {
        u.push(inner)?;
        Err("#1".into())
    });
    let Err(error) = result else {
        panic!("missing cleanup failure")
    };
    assert_eq!(error.to_string(), "#2");
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn promise_cleanup_failure_is_logged_and_keeps_the_setup_error() {
    let k = Kernel::start();
    let root = k.root();
    let result = root.try_effect::<Error>("promise rollback", |u| {
        u.push(Undo::future(|| async { Err("#2".into()) }))?;
        Err("#1".into())
    });
    let Err(error) = result else {
        panic!("missing setup failure")
    };
    assert_eq!(error.to_string(), "#1");
    k.shutdown().await;
    assert!(
        root.logger()
            .buffer()
            .iter()
            .any(|r| r.message == "effect \"promise rollback\": #2")
    );
}

#[tokio::test(start_paused = true)]
async fn direct_disposal_and_unloading_log_cleanup_errors_with_the_existing_unit_api() {
    let k = Kernel::start();
    let root = k.root();
    root.effect("direct", || Undo::try_sync(|| Err("direct error".into())))
        .unwrap()
        .dispose()
        .await;
    root.effect("unload", || Undo::try_sync(|| Err("unload error".into())))
        .unwrap();
    k.shutdown().await;
    let log = root.logger().buffer();
    assert!(
        log.iter()
            .any(|r| r.message == "effect \"direct\": direct error")
    );
    assert!(
        log.iter()
            .any(|r| r.message == "effect \"unload\": unload error")
    );
}
