use hoh_kernel::Kernel;
use hoh_launch_environment::{
    LaunchEnvironment, LaunchEnvironmentLayerInput, LaunchEnvironmentSource,
    create_launch_environment_snapshot,
};
use hoh_llm::{Llm, LlmCallConfig, LlmPlugin, ResolvedRetryPolicy};
use hoh_llm_deepseek_api_key::DeepSeekApiKey;
use indexmap::IndexMap;
use serde_json::json;
use std::sync::Arc;

async fn host(key: Option<&str>) -> (Kernel, hoh_kernel::Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let values = key
        .map(|key| IndexMap::from_iter([("DEEPSEEK_API_KEY".into(), key.into())]))
        .unwrap_or_default();
    ctx.provide::<LaunchEnvironment>(Arc::new(create_launch_environment_snapshot(&[
        LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::Process,
            path: None,
            values,
        },
    ])))
    .await
    .unwrap();
    let llm = ctx.plugin::<LlmPlugin>(json!({})).unwrap();
    llm.settled().await.unwrap();
    (kernel, ctx)
}

#[tokio::test]
async fn hides_default_and_custom_models_without_an_api_key() {
    for key in [None, Some("")] {
        for config in [json!({}), json!({"models":[{"id":"configured-model"}]})] {
            let (kernel, ctx) = host(key).await;
            let plugin = ctx.plugin::<DeepSeekApiKey>(config).unwrap();
            plugin.settled().await.unwrap();
            let llm = ctx.get::<Llm>().unwrap();
            assert_eq!(llm.list_models("deepseek-official").await.unwrap(), []);
            kernel.shutdown().await;
        }
    }
}

#[tokio::test]
async fn advertises_configured_models_with_an_api_key() {
    let (kernel, ctx) = host(Some("fixture-api-key")).await;
    let plugin = ctx
        .plugin::<DeepSeekApiKey>(json!({"models":[{"id":"configured-model","name":"Configured"}]}))
        .unwrap();
    plugin.settled().await.unwrap();
    let llm = ctx.get::<Llm>().unwrap();
    let models = llm.list_models("deepseek-official").await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].provider, "deepseek-official");
    assert_eq!(models[0].id, "configured-model");
    assert_eq!(models[0].name, "Configured");
    kernel.shutdown().await;
}

#[tokio::test]
async fn reports_a_malformed_credential_during_discovery() {
    let (kernel, ctx) = host(Some("invalid\nheader")).await;
    let plugin = ctx
        .plugin::<DeepSeekApiKey>(json!({"models":[{"id":"configured-model"}]}))
        .unwrap();
    plugin.settled().await.unwrap();
    let error = ctx
        .get::<Llm>()
        .unwrap()
        .list_models("deepseek-official")
        .await
        .unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "INVALID_CREDENTIAL"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn registers_directory_and_resolves_model_capabilities() {
    let (kernel, ctx) = host(Some("fixture-api-key")).await;
    let plugin = ctx
        .plugin::<DeepSeekApiKey>(json!({
            "models":[{
                "id":"configured-model",
                "contextWindow":12345,
                "maxTokens":321,
                "systemPromptUpdate":"in-history",
                "toolUpdate":"addition-only"
            }]
        }))
        .unwrap();
    plugin.settled().await.unwrap();
    let llm = ctx.get::<Llm>().unwrap();
    assert_eq!(
        llm.list_configurable_providers()[0].provider,
        "deepseek-official"
    );
    let info = llm
        .resolve_model_info("deepseek-official", "configured-model", None)
        .await
        .unwrap();
    assert_eq!(info.context.unwrap().context_window, 12_345);
    assert_eq!(info.default_max_tokens, Some(321));
    assert_eq!(info.system_prompt_update.as_deref(), Some("in-history"));
    assert_eq!(info.tool_update.as_deref(), Some("addition-only"));
    let call = llm
        .prepare_call(
            &ctx,
            LlmCallConfig {
                provider: "deepseek-official".into(),
                model: "uncatalogued-pass-through".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(call.config.model, "uncatalogued-pass-through");
    kernel.shutdown().await;
}

#[tokio::test]
async fn volatile_retry_policy_replaces_the_registered_adapter_policy() {
    let (kernel, ctx) = host(Some("fixture-api-key")).await;
    let plugin = ctx
        .plugin::<DeepSeekApiKey>(json!({"models":[{"id":"configured-model"}]}))
        .unwrap();
    plugin.settled().await.unwrap();
    let llm = ctx.get::<Llm>().unwrap();
    let ResolvedRetryPolicy::Normal { max_retries, .. } =
        llm.provider_retry_policy("deepseek-official").unwrap()
    else {
        panic!("normal retry policy")
    };
    assert_eq!(max_retries, 5);
    assert!(
        plugin
            .commit_volatile(json!({
                "models":[{"id":"configured-model"}],
                "retryPolicy":{"mode":"normal","maxRetries":1}
            }))
            .await
            .unwrap()
    );
    let ResolvedRetryPolicy::Normal { max_retries, .. } =
        llm.provider_retry_policy("deepseek-official").unwrap()
    else {
        panic!("normal retry policy")
    };
    assert_eq!(max_retries, 1);
    kernel.shutdown().await;
}
