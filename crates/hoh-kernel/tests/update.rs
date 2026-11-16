mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use support::*;
#[tokio::test(start_paused = true)]
async fn settles_concurrent_updates_on_the_latest_config() {
    for inject in [false, true] {
        let (k, r, t) = harness();
        r.provide::<A>(Arc::new(1)).await.unwrap();
        let def = PluginDef::of::<Recorded>();
        let f = r
            .plugin_dyn(
                &def,
                json!({"value":0}),
                PluginOptions {
                    inject: if inject { vec!["foo".into()] } else { vec![] },
                    ..Default::default()
                },
            )
            .unwrap();
        f.settled().await.unwrap();
        let settled = Arc::new(AtomicUsize::new(0));
        let first = f.update(json!({"value":1,"wait":true}));
        let count = settled.clone();
        let first = tokio::spawn(async move {
            let result = first.await;
            count.fetch_add(1, Ordering::Relaxed);
            result
        });
        turn().await;
        assert_eq!(f.state(), FiberState::Loading);
        let second = f.update(json!({"value":2}));
        let third = f.update(json!({"value":3,"wait":true}));
        let count = settled.clone();
        let second = tokio::spawn(async move {
            let result = second.await;
            count.fetch_add(1, Ordering::Relaxed);
            result
        });
        let count = settled.clone();
        let third = tokio::spawn(async move {
            let result = third.await;
            count.fetch_add(1, Ordering::Relaxed);
            result
        });
        turn().await;
        t.gate.add_permits(1);
        turn().await;
        assert_eq!(
            t.list(),
            ["load 0", "unload 0", "load 1", "unload 1", "load 3"]
        );
        assert_eq!(f.state(), FiberState::Loading);
        assert_eq!(settled.load(Ordering::Relaxed), 0);
        t.gate.add_permits(1);
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        third.await.unwrap().unwrap();
        assert_eq!(settled.load(Ordering::Relaxed), 3);
        assert_eq!(f.raw_config()["value"], 3);
        k.shutdown().await;
        assert_eq!(t.list().last().unwrap(), "unload 3");
    }
}
#[tokio::test(start_paused = true)]
async fn skips_superseded_loads_before_executing_the_component() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"value":1})).unwrap();
    let first = f.update(json!({"value":2}));
    let second = f.update(json!({"value":3}));
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(t.list(), ["load 3"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn coalesces_updates_during_unloading() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"cleanup":true})).unwrap();
    f.settled().await.unwrap();
    let a = f.update(json!({"value":1}));
    turn().await;
    let b = f.update(json!({"value":2}));
    turn().await;
    assert_eq!(f.state(), FiberState::Unloading);
    assert_eq!(t.list(), ["load 0", "unload 0"]);
    t.cleanup.add_permits(1);
    a.await.unwrap();
    b.await.unwrap();
    assert_eq!(t.list(), ["load 0", "unload 0", "load 2"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn honors_restart_config_aba_while_loading_even_when_config_is_unchanged() {
    for aba in [false, true] {
        let (k, r, t) = harness();
        let original = json!({"value":1,"wait":true});
        let f = r.plugin::<Recorded>(original.clone()).unwrap();
        let original_raw = f.raw_config();
        turn().await;
        let mut updates = vec![];
        if aba {
            updates.push(Box::pin(f.update(json!({"value":2})))
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<(), KernelError>> + Send>,
                >);
            updates.push(Box::pin(f.update(original.clone())));
        } else {
            updates.push(Box::pin(f.restart()));
        }
        turn().await;
        t.gate.add_permits(2);
        for u in updates {
            u.await.unwrap();
        }
        assert_eq!(t.list(), ["load 1", "unload 1", "load 1"]);
        assert_eq!(f.state(), FiberState::Active);
        assert_eq!(*f.raw_config(), original);
        assert!(Arc::ptr_eq(&f.raw_config(), &f.clone().raw_config()));
        if !aba {
            assert!(Arc::ptr_eq(&original_raw, &f.raw_config()));
        }
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn cleans_up_a_superseded_failure_without_failing_the_latest_update() {
    let (k, r, t) = harness();
    let f = r
        .plugin::<Recorded>(json!({"value":1,"wait":true,"fail":true}))
        .unwrap();
    turn().await;
    let update = f.update(json!({"value":2}));
    turn().await;
    t.gate.add_permits(1);
    update.await.unwrap();
    assert_eq!(t.list(), ["load 1", "unload 1", "load 2"]);
    assert_eq!(f.state(), FiberState::Active);
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
#[tokio::test(start_paused = true)]
async fn rejects_all_waiting_updates_when_the_latest_load_fails() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    let a = f.update(json!({"value":1,"wait":true}));
    turn().await;
    let b = f.update(json!({"value":2,"fail":true,"message":"latest load failed"}));
    turn().await;
    t.gate.add_permits(1);
    let a = a.await.unwrap_err();
    let b = b.await.unwrap_err();
    assert_eq!(a.to_string(), b.to_string());
    assert_eq!(a.to_string(), "latest load failed");
    assert_eq!(b.to_string(), "latest load failed");
    assert_eq!(f.state(), FiberState::Failed);
    assert_eq!(
        t.list(),
        [
            "load 0", "unload 0", "load 1", "unload 1", "load 2", "unload 2"
        ]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn stops_advancing_an_outdated_async_generator_and_collects_its_final_yield() {
    let (k, r, t) = harness();
    let f = r
        .plugin::<Recorded>(json!({"value":1,"wait":true,"marker":true}))
        .unwrap();
    turn().await;
    let u = f.update(json!({"value":2}));
    turn().await;
    t.gate.add_permits(1);
    u.await.unwrap();
    assert_eq!(t.list(), ["load 1", "unload 1", "load 2"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn waits_for_dependencies_and_loads_only_the_latest_config() {
    let (k, r, t) = harness();
    let f = r
        .plugin_dyn(
            &PluginDef::of::<Recorded>(),
            json!({"value":0}),
            PluginOptions {
                inject: vec!["foo".into()],
                ..Default::default()
            },
        )
        .unwrap();
    let a = f.update(json!({"value":1}));
    let b = f.update(json!({"value":2}));
    a.await.unwrap();
    b.await.unwrap();
    assert_eq!(f.state(), FiberState::Pending);
    assert!(t.list().is_empty());
    r.provide::<A>(Arc::new(1)).await.unwrap();
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["load 2"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_restart_when_a_listener_accepts_an_update_during_loading() {
    let (k, r, t) = harness();
    let f = r
        .plugin::<Recorded>(json!({"value":1,"wait":true,"hook":true}))
        .unwrap();
    turn().await;
    f.update(json!({"value":2})).await.unwrap();
    assert_eq!(t.list(), ["load 1"]);
    assert_eq!(lock(&t.updates)[0]["value"], 2);
    t.gate.add_permits(1);
    f.settled().await.unwrap();
    assert_eq!(f.effective_config().unwrap()["value"], 2);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_load_a_queued_config_after_disposal() {
    let (k, r, t) = harness();
    let f = r
        .plugin::<Recorded>(json!({"value":1,"wait":true}))
        .unwrap();
    turn().await;
    let update = f.update(json!({"value":2}));
    let dispose = f.dispose();
    turn().await;
    t.gate.add_permits(1);
    update.await.unwrap();
    dispose.await;
    assert_eq!(t.list(), ["load 1", "unload 1"]);
    assert_eq!(f.state(), FiberState::Disposed);
    k.shutdown().await;
}
