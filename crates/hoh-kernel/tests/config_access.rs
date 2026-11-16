mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::Arc;
use support::*;
#[tokio::test(start_paused = true)]
async fn keeps_raw_and_running_references_unchanged_while_resolving_valid_or_invalid_candidates() {
    let (k, r, t) = harness();
    let f = r
        .plugin_dyn(
            &PluginDef::of::<Recorded>(),
            json!({"ok":true,"value":2}),
            PluginOptions {
                resolver: Some(Arc::new(ResolveOk)),
                ..Default::default()
            },
        )
        .unwrap();
    f.settled().await.unwrap();
    let raw = f.raw_config();
    let running = f.effective_config().unwrap();
    let candidate = f.resolve_config(&json!({"ok":true,"value":4})).unwrap();
    assert_eq!(candidate["value"], 4);
    assert!(f.resolve_config(&json!({"ok":false})).is_err());
    assert!(Arc::ptr_eq(&raw, &f.raw_config()));
    assert!(Arc::ptr_eq(&running, &f.effective_config().unwrap()));
    assert_eq!(running["value"], 2);
    assert_eq!(t.list(), ["load 2"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn uses_the_canonical_fiber_through_registry_wrappers_and_retains_raw_input_without_updating()
{
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"value":1})).unwrap();
    f.settled().await.unwrap();
    let wrapper = f.clone();
    let initial = f.effective_config().unwrap();
    wrapper.retain_raw_config(json!({"value":2})).await.unwrap();
    assert_eq!(f.raw_config()["value"], 2);
    assert!(Arc::ptr_eq(&initial, &wrapper.effective_config().unwrap()));
    assert_eq!(wrapper.resolve_config(&f.raw_config()).unwrap()["value"], 2);
    assert_eq!(t.list(), ["load 1"]);
    wrapper.restart().await.unwrap();
    assert_eq!(f.effective_config().unwrap()["value"], 2);
    assert_eq!(t.list(), ["load 1", "unload 1", "load 2"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn publishes_updates_through_registry_wrappers_to_the_canonical_fiber() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"value":1})).unwrap();
    let wrapper = f.clone();
    wrapper.settled().await.unwrap();
    wrapper.update(json!({"value":2})).await.unwrap();
    assert!(Arc::ptr_eq(&f.raw_config(), &wrapper.raw_config()));
    assert!(Arc::ptr_eq(
        &f.effective_config().unwrap(),
        &wrapper.effective_config().unwrap()
    ));
    assert_eq!(t.list(), ["load 1", "unload 1", "load 2"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retains_a_pending_raw_update_until_its_injection_is_available() {
    let (k, r, t) = harness();
    let f = r
        .plugin_dyn(
            &PluginDef::of::<Recorded>(),
            json!({"value":1}),
            PluginOptions {
                inject: vec!["foo".into()],
                ..Default::default()
            },
        )
        .unwrap();
    f.update(json!({"value":2})).await.unwrap();
    assert_eq!(f.raw_config()["value"], 2);
    assert!(f.effective_config().is_none());
    r.provide::<A>(Arc::new(1)).await.unwrap();
    f.settled().await.unwrap();
    assert_eq!(t.list(), ["load 2"]);
    k.shutdown().await;
}
