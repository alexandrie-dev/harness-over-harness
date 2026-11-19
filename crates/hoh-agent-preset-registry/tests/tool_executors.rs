mod support;

use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use serde_json::json;
use std::sync::Arc;
use support::{MissingService, definition, fresh};

#[tokio::test]
async fn loads_scoped_plugin_objects_and_retains_them_with_the_preset_revision() {
    let (kernel, ctx, registry) = fresh();
    let first_registration = registry
        .register(
            &ctx,
            definition("custom", json!([{"id":"executor","name":"fixture:active"}])),
        )
        .await
        .unwrap();
    let first = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    registry.mount(&first.ctx, Some("custom")).await.unwrap();
    first_registration.dispose().await;
    registry
        .register(
            &ctx,
            definition("custom", json!([{"id":"executor","name":"fixture:active"}])),
        )
        .await
        .unwrap();
    let second = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.mount(&second.ctx, Some("custom")).await.unwrap();
    let old = registry.inspect_compositions(Some(&first.ctx)).await;
    let new = registry.inspect_compositions(Some(&second.ctx)).await;
    assert_eq!(old.len(), 1);
    assert_eq!(new.len(), 1);
    assert_eq!(old[0].modules.len(), 1);
    assert_eq!(new[0].modules.len(), 1);
    assert_eq!(registry.inspect_compositions(None).await.len(), 2);
    first.dispose().await;
    assert_eq!(registry.inspect_compositions(None).await.len(), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn loads_executor_presets_when_the_required_runtime_becomes_available() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition(
                "ptc",
                json!([{"id":"executor","name":"fixture:needs-missing"}]),
            ),
        )
        .await
        .unwrap();
    let unavailable = registry.resolve(Some("ptc")).await.unwrap();
    assert!(
        unavailable
            .broken
            .unwrap()
            .contains("serviceThatDoesNotExist")
    );
    let _provider = ctx.provide::<MissingService>(Arc::new(())).await.unwrap();
    let available = registry.resolve(Some("ptc")).await.unwrap();
    assert_eq!(available.broken, None);
    let agent = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.mount(&agent.ctx, Some("ptc")).await.unwrap();
    kernel.shutdown().await;
}
