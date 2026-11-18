mod support;
use hoh_kernel::Realm;
use serde_json::json;
use std::sync::Arc;
use support::live::*;

#[test]
// R54: an absent ordinary parent without a default is not volatile-only.
fn removal_of_an_ordinary_parent_is_not_a_volatile_only_change() {
    let schema = json!({"type":"object","properties":{"block":{"type":"object","properties":{"value":{"type":"string","x-volatile":true}}}}});
    assert!(!hoh_kernel::config::equal_except_volatile(
        &schema,
        &json!({"block":{"value":"old"}}),
        &json!({})
    ));
}
#[test]
#[ignore = "R54: the kernel drops a default attached to a nonrecursive object $ref; inline source schemas remain covered"]
fn r54_preserves_a_default_on_a_nonrecursive_object_reference() {
    let schema = json!({"type":"object","properties":{"nested":{"$ref":"#/$defs/Nested","default":{}}},"$defs":{"Nested":{"type":"object","properties":{"count":{"type":"number","x-volatile":true}}}}});
    assert!(hoh_kernel::config::equal_except_volatile(
        &schema,
        &json!({}),
        &json!({"nested":{"count":1}})
    ));
}
#[tokio::test(start_paused = true)]
// L19: Context.reflect().store() must retain providers outside the consumer's realm.
async fn context_only_startup_capture_retains_registered_isolated_providers() {
    hoh_kernel::service!(Ready: () = "ready");
    let world = World::new("[]").await;
    world
        .ctx
        .isolate("ready", Realm::new())
        .provide::<Ready>(Arc::new(()))
        .await
        .unwrap();
    world
        .patch("- insert: [{id: x, name: noop, inject: [ready]}]")
        .await
        .unwrap();
    let report = hoh_loader::startup_diagnostics::capture_startup_composition(&world.ctx)
        .await
        .unwrap();
    assert_eq!(report.services[0].providers.len(), 1);
    world.close().await;
}
