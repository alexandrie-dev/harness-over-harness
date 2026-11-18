mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use support::*;
async fn inertia(early: bool, provider: bool) {
    let (k, r, t) = harness();
    let p = r.provide::<A>(Arc::new(1)).await.unwrap();
    let t2 = t.clone();
    let f = r
        .inject(move |ctx, deps: ADeps| {
            let t = t2.clone();
            async move {
                t.push(format!("load {}", *deps.a));
                lock(&t.contexts).push(ctx.clone());
                ctx.effect_async::<Error, _>("delayed", move |u| async move {
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    let _ = u.push(Undo::future(move || async move {
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                        drop(deps);
                        Ok(())
                    }));
                    Ok(())
                })
                .await?;
                Ok(())
            }
        })
        .unwrap();
    turn().await;
    tokio::time::advance(Duration::from_millis(400)).await;
    assert_eq!(f.state(), FiberState::Loading);
    if provider {
        tokio::time::advance(Duration::from_millis(600)).await;
        turn().await;
        assert_eq!(f.state(), FiberState::Active);
        assert_eq!(*t.context().get::<A>().unwrap(), 1);
        p.dispose().await;
        assert_eq!(f.state(), FiberState::Pending);
    } else {
        let withdrawn = p.dispose();
        turn().await;
        tokio::time::advance(Duration::from_millis(400)).await;
        assert_eq!(f.state(), FiberState::Loading);
        if early {
            r.provide::<A>(Arc::new(2)).await.unwrap();
        }
        tokio::time::advance(Duration::from_millis(400)).await;
        turn().await;
        assert_eq!(f.state(), FiberState::Unloading);
        if !early {
            r.provide::<A>(Arc::new(2)).await.unwrap();
        }
        tokio::time::advance(Duration::from_millis(1000)).await;
        turn().await;
        assert_eq!(f.state(), FiberState::Loading);
        tokio::time::advance(Duration::from_millis(1000)).await;
        f.settled().await.unwrap();
        withdrawn.await;
        assert_eq!(f.state(), FiberState::Active);
        assert_eq!(*t.context().get::<A>().unwrap(), 2);
        assert_eq!(t.list(), ["load 1", "load 2"]);
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn inertia_lock_1() {
    inertia(false, false).await;
}
#[tokio::test(start_paused = true)]
async fn inertia_lock_2() {
    inertia(true, false).await;
}
#[tokio::test(start_paused = true)]
async fn inertia_lock_3() {
    inertia(false, true).await;
}
#[tokio::test(start_paused = true)]
async fn component_error() {
    let (k, r, t) = harness();
    let a = r
        .plugin::<Recorded>(json!({"fail":true,"listen":true}))
        .unwrap();
    let b = r
        .plugin::<Recorded>(json!({"value":1,"listen":true}))
        .unwrap();
    assert!(a.settled().await.is_err());
    b.settled().await.unwrap();
    assert_eq!(a.state(), FiberState::Failed);
    assert_eq!(b.state(), FiberState::Active);
    assert_eq!(
        r.logger()
            .buffer()
            .iter()
            .filter(|r| r.level == Level::Error)
            .count(),
        1
    );
    assert!(t.list().contains(&"unload 0".into()));
    r.emit(&Ping);
    assert_eq!(t.list().iter().filter(|s| s.as_str() == "event").count(), 1);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn failed_fiber_does_not_re_enter_on_dependency_refresh() {
    let (k, r, t) = harness();
    let p = r.provide::<A>(Arc::new(1)).await.unwrap();
    let f = r
        .plugin_dyn(
            &PluginDef::of::<Recorded>(),
            json!({"fail":true}),
            PluginOptions {
                inject: vec!["foo".into()],
                ..Default::default()
            },
        )
        .unwrap();
    assert!(f.settled().await.is_err());
    p.dispose().await;
    r.provide::<A>(Arc::new(2)).await.unwrap();
    assert!(f.restart().await.is_err());
    assert_eq!(t.list(), ["load 0", "unload 0"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn update_recovers_a_failed_fiber() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"fail":true})).unwrap();
    assert!(f.settled().await.is_err());
    f.update(json!({})).await.unwrap();
    assert_eq!(f.state(), FiberState::Active);
    assert_eq!(t.list(), ["load 0", "unload 0", "load 0"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn update_waits_for_asynchronous_recovery() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"fail":true})).unwrap();
    assert!(f.settled().await.is_err());
    let update = f.update(json!({"wait":true}));
    turn().await;
    assert_eq!(f.state(), FiberState::Loading);
    t.gate.add_permits(1);
    update.await.unwrap();
    assert_eq!(f.state(), FiberState::Active);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn update_surfaces_a_failed_recovery_to_its_caller() {
    let (k, r, _) = harness();
    let f = r.plugin::<Recorded>(json!({"fail":true})).unwrap();
    assert!(f.settled().await.is_err());
    assert_eq!(
        f.update(json!({"fail":true,"value":1}))
            .await
            .unwrap_err()
            .to_string(),
        "failure 1"
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn update_surfaces_a_failed_reload_to_its_caller() {
    let (k, r, _) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    assert!(f.update(json!({"fail":true})).await.is_err());
    assert_eq!(f.state(), FiberState::Failed);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn update_does_not_leak_a_dropped_failure() {
    let (k, r, _) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    drop(f.update(json!({"fail":true})));
    turn().await;
    assert_eq!(f.state(), FiberState::Failed);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn dispose_error() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"cleanupError":true})).unwrap();
    f.settled().await.unwrap();
    f.dispose().await;
    assert_eq!(t.list(), ["load 0", "unload 0"]);
    assert_eq!(
        r.logger()
            .buffer()
            .iter()
            .filter(|r| r.level == Level::Error)
            .count(),
        1
    );
    k.shutdown().await;
}
async fn disposal_state(failed: bool, pending: bool) {
    let (k, r, _) = harness();
    let transitions = Arc::new(Mutex::new(vec![]));
    let f = if pending {
        r.inject(|_, _: MissingDeps| async { Ok(()) }).unwrap()
    } else {
        r.plugin::<Recorded>(json!({"fail":failed})).unwrap()
    };
    let _ = f.settled().await;
    let c = transitions.clone();
    let uid = f.uid();
    r.on::<InternalStatus>(move |e| {
        if e.fiber.uid() == uid {
            lock(&c).push((e.old, e.fiber.state()));
        }
    })
    .unwrap();
    let old = f.state();
    f.dispose().await;
    let expected = vec![
        (old, FiberState::Unloading),
        (FiberState::Unloading, FiberState::Disposed),
    ];
    assert_eq!(*lock(&transitions), expected);
    assert!(f.is_disposed());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn settles_a_pending_fiber_to_disposed_on_dispose() {
    disposal_state(false, true).await;
}
#[tokio::test(start_paused = true)]
async fn settles_a_failed_fiber_to_disposed_on_dispose() {
    disposal_state(true, false).await;
}
#[tokio::test(start_paused = true)]
async fn keeps_the_active_dispose_sequence_stable() {
    disposal_state(false, false).await;
}
#[tokio::test(start_paused = true)]
async fn update_config_on_wrapped_fiber() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"value":1})).unwrap();
    let wrapper = f.clone();
    f.settled().await.unwrap();
    wrapper.update(json!({"value":2})).await.unwrap();
    f.update(json!({"value":3})).await.unwrap();
    assert_eq!(wrapper.raw_config()["value"], 3);
    assert_eq!(
        t.list(),
        ["load 1", "unload 1", "load 2", "unload 2", "load 3"]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn restart_wrapped_fiber() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    f.clone().restart().await.unwrap();
    assert_eq!(t.list(), ["load 0", "unload 0", "load 0"]);
    assert_eq!(f.state(), FiberState::Active);
    k.shutdown().await;
}
#[plugin(Provider, name = "provider")]
async fn provider(ctx: Context, config: Cfg<serde_json::Value>) -> Result<(), Error> {
    ctx.provide::<A>(Arc::new(config.get()["value"].as_i64().unwrap() as i32))
        .await?;
    Ok(())
}
#[plugin(Consumer, name = "consumer")]
async fn consumer(ctx: Context, deps: ADeps, config: Cfg<serde_json::Value>) -> Result<(), Error> {
    ctx.tag::<Arc<Track>>().unwrap().push(format!(
        "{} {}",
        *deps.a,
        config.get()["mode"].as_str().unwrap()
    ));
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn update_config_while_injected_service_reloads() {
    let (k, r, t) = harness();
    let p = r.plugin::<Provider>(json!({"value":1})).unwrap();
    let c = r.plugin::<Consumer>(json!({"mode":"old"})).unwrap();
    p.settled().await.unwrap();
    c.settled().await.unwrap();
    let a = p.update(json!({"value":2}));
    let b = c.update(json!({"mode":"new"}));
    a.await.unwrap();
    b.await.unwrap();
    c.settled().await.unwrap();
    assert_eq!(t.list(), ["1 old", "2 new"]);
    k.shutdown().await;
}
fn checked(r: &Context, raw: serde_json::Value) -> Fiber {
    r.plugin_dyn(
        &PluginDef::of::<Recorded>(),
        raw,
        PluginOptions {
            resolver: Some(Arc::new(ResolveOk)),
            ..Default::default()
        },
    )
    .unwrap()
}
#[tokio::test(start_paused = true)]
async fn fail_on_first_load() {
    let (k, r, t) = harness();
    let f = checked(&r, json!({"ok":false}));
    assert!(f.settled().await.is_err());
    assert!(t.list().is_empty());
    assert_eq!(f.state(), FiberState::Failed);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn keep_a_running_component_on_update() {
    let (k, r, t) = harness();
    let f = checked(&r, json!({"ok":true}));
    f.settled().await.unwrap();
    assert!(matches!(
        f.update(json!({"ok":false})).await,
        Err(KernelError::Validation(_))
    ));
    assert_eq!(t.list(), ["load 0"]);
    assert_eq!(*f.raw_config(), json!({"ok":true}));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn keep_the_issues_and_their_paths() {
    let issues = vec![
        Issue {
            message: "not a number".into(),
            path: vec![PathSeg::Key("items".into()), PathSeg::Index(0)],
        },
        Issue {
            message: "required".into(),
            path: vec![],
        },
    ];
    let error = ValidationError {
        issues: issues.clone(),
    };
    assert_eq!(error.issues, issues);
    assert_eq!(
        error.to_string(),
        "invalid config:\n  - not a number (at items.0)\n  - required"
    );
}
#[tokio::test(start_paused = true)]
async fn pass_the_resolved_config_to_listeners() {
    let (k, r, t) = harness();
    let f = checked(&r, json!({"ok":true,"hook":true}));
    f.settled().await.unwrap();
    f.update(json!({"ok":true,"extra":1,"delegate":true}))
        .await
        .unwrap();
    assert_eq!(
        lock(&t.updates)[0],
        json!({"ok":true,"extra":1,"delegate":true,"resolved":true})
    );
    assert_eq!(
        *f.raw_config(),
        json!({"ok":true,"extra":1,"delegate":true})
    );
    assert_eq!(t.list(), ["load 0", "unload 0", "load 0"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn keep_the_source_config_when_a_listener_handles_the_update() {
    let (k, r, t) = harness();
    let f = checked(&r, json!({"ok":true,"hook":true}));
    f.settled().await.unwrap();
    f.update(json!({"ok":true,"extra":1})).await.unwrap();
    assert_eq!(lock(&t.updates)[0]["resolved"], true);
    assert!(f.raw_config().get("resolved").is_none());
    assert_eq!(t.list(), ["load 0"]);
    f.restart().await.unwrap();
    assert_eq!(t.list(), ["load 0", "unload 0", "load 0"]);
    assert_eq!(lock(&t.configs).last().unwrap().get()["extra"], 1);
    k.shutdown().await;
}
