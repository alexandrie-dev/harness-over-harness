mod support;
use hoh_kernel::*;
use serde_json::{Value, json};
use std::sync::Arc;
use support::resource;
use support::*;
#[tokio::test(start_paused = true)]
async fn awaits_the_latest_update_while_loading() {
    for inject in [false, true] {
        let (k, r, t) = harness();
        r.provide::<A>(Arc::new(1)).await.unwrap();
        let f = r
            .plugin_dyn(
                &PluginDef::of::<Recorded>(),
                json!({"value":0}),
                PluginOptions {
                    inject: if inject { vec!["foo".into()] } else { vec![] },
                    ..Default::default()
                },
            )
            .unwrap();
        f.settled().await.unwrap();
        let a = f.update(json!({"value":1,"wait":true}));
        turn().await;
        let b = f.update(json!({"value":2}));
        let c = f.update(json!({"value":3}));
        turn().await;
        t.gate.add_permits(1);
        a.await.unwrap();
        b.await.unwrap();
        c.await.unwrap();
        assert_eq!(
            t.list(),
            ["load 0", "unload 0", "load 1", "unload 1", "load 3"]
        );
        assert_eq!(f.effective_config().unwrap()["value"], 3);
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn cleans_up_a_superseded_activation_failure_and_accepts_the_latest_update() {
    let (k, r, t) = harness();
    let f = r
        .plugin::<Recorded>(json!({"value":1,"fail":true,"wait":true}))
        .unwrap();
    turn().await;
    let update = f.update(json!({"value":2}));
    turn().await;
    t.gate.add_permits(1);
    update.await.unwrap();
    assert_eq!(t.list(), ["load 1", "unload 1", "load 2"]);
    assert_eq!(f.state(), FiberState::Active);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retains_isolated_providers_through_dependent_teardown() {
    for restart in [false, true] {
        let (k, r, t) = harness();
        let ctx = r.isolate("foo", Realm::new());
        let p = resource::provider(&ctx, false).await;
        let value = ctx.get::<resource::Foo>().unwrap();
        let c = resource::consumer(&ctx, true, false).await;
        let dispose = if restart {
            let task = p.restart();
            Box::pin(async move {
                task.await.unwrap();
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        } else {
            Box::pin(p.dispose())
        };
        turn().await;
        assert!(value.live.load(std::sync::atomic::Ordering::Acquire));
        t.gate.add_permits(10);
        dispose.await;
        assert_eq!(t.list(), ["consumer read: open", "provider closed"]);
        let _ = c.settled().await;
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn separates_own_isolated_provisions_from_the_committed_injection() {
    let (k, r, _) = harness();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    let f = r
        .inject(|ctx, d: ADeps| async move {
            let isolated = ctx.isolate("foo", Realm::new());
            let d2 = isolated.provide::<A>(Arc::new(2)).await?;
            assert_eq!(*d.a, 1);
            assert_eq!(*isolated.get::<A>().unwrap(), 2);
            d2.dispose().await;
            assert_eq!(*d.a, 1);
            assert!(isolated.get::<A>().is_none());
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn publishes_successful_retained_instance_updates_and_ignores_stale_or_rejected_completions()
{
    let (k, r, t) = harness();
    let f = r
        .plugin::<Recorded>(json!({"hook":true,"value":0}))
        .unwrap();
    f.settled().await.unwrap();
    let ctx = t.context();
    let tt = t.clone();
    let old = ctx
        .on_update::<Value, _>(move |c, _| {
            let t = tt.clone();
            async move {
                if c["value"] == 1 {
                    t.gate.acquire().await.unwrap().forget();
                }
                if c["value"] == 3 {
                    return Err("update rejected".into());
                }
                Ok(())
            }
        })
        .unwrap();
    let initial = lock(&t.configs)[0].clone();
    let a = f.update(json!({"value":1,"delegate":true}));
    turn().await;
    assert_eq!(f.effective_config().unwrap()["value"], 0);
    f.update(json!({"value":2,"delegate":true})).await.unwrap();
    t.gate.add_permits(1);
    a.await.unwrap();
    assert_eq!(f.effective_config().unwrap()["value"], 2);
    assert_eq!(initial.get()["value"], 2);
    assert!(f.update(json!({"value":3,"delegate":true})).await.is_err());
    assert_eq!(f.raw_config()["value"], 3);
    assert_eq!(f.effective_config().unwrap()["value"], 2);
    assert_eq!(t.list(), ["load 0"]);
    old.dispose().await;
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_publish_an_old_update_after_raw_retention_restart_dispose() {
    for action in 0..3 {
        let (k, r, t) = harness();
        let f = r.plugin::<Recorded>(json!({"value":0})).unwrap();
        f.settled().await.unwrap();
        let tt = t.clone();
        t.context()
            .on_update::<Value, _>(move |_, _| {
                let t = tt.clone();
                async move {
                    t.gate.acquire().await.unwrap().forget();
                    Ok(())
                }
            })
            .unwrap();
        let old = f.update(json!({"value":1}));
        turn().await;
        match action {
            0 => f.retain_raw_config(json!({"value":2})).await.unwrap(),
            1 => f.restart().await.unwrap(),
            _ => f.dispose().await,
        }
        let effective = f.effective_config().unwrap();
        t.gate.add_permits(1);
        old.await.unwrap();
        assert!(Arc::ptr_eq(&effective, &f.effective_config().unwrap()));
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn keeps_the_restarted_config_identity_when_a_listener_delegates() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"hook":true})).unwrap();
    f.settled().await.unwrap();
    f.update(json!({"value":1,"delegate":true})).await.unwrap();
    assert_eq!(f.effective_config().unwrap()["value"], 1);
    assert_eq!(lock(&t.configs).last().unwrap().get()["value"], 1);
    assert_eq!(t.list(), ["load 0", "unload 0", "load 1"]);
    k.shutdown().await;
}
