use hoh_kernel::Kernel;
use hoh_launch_environment::{LaunchEnvironment, create_launch_environment_snapshot};
use hoh_web::{Web, WebRuntime, WebSearchRequest};
use hoh_web_search_deepseek::{
    Config, DEEPSEEK_DEFAULT_API_VERSION, DEEPSEEK_DEFAULT_MAX_TOKENS, DEEPSEEK_DEFAULT_MAX_USES,
    DEEPSEEK_DEFAULT_MODEL, DEEPSEEK_PROVIDER_ID, WebSearchDeepSeek,
};

#[test]
fn config_defaults_and_positive_integer_validation_match_source() {
    let config: Config = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(config.api_key.0, None);
    assert_eq!(config.api_key_env.0, "DEEPSEEK_API_KEY");
    assert_eq!(config.base_url.0, None);
    assert_eq!(config.model.0, DEEPSEEK_DEFAULT_MODEL);
    assert_eq!(config.api_version.0, DEEPSEEK_DEFAULT_API_VERSION);
    assert_eq!(config.max_tokens.0, DEEPSEEK_DEFAULT_MAX_TOKENS);
    assert_eq!(config.max_uses.0, DEEPSEEK_DEFAULT_MAX_USES);
    for value in [
        serde_json::json!({"maxTokens":0}),
        serde_json::json!({"maxUses":0}),
        serde_json::json!({"maxUses":1.5}),
        serde_json::json!({"maxTokens":1.5}),
        serde_json::json!({"apiKey":null}),
        serde_json::json!({"baseURL":null}),
    ] {
        assert!(serde_json::from_value::<Config>(value).is_err());
    }
    for value in [
        serde_json::json!({"maxTokens":2.0}),
        serde_json::from_str(r#"{"maxTokens":1e3}"#).unwrap(),
    ] {
        assert!(serde_json::from_value::<Config>(value).is_ok());
    }
    let too_large = serde_json::from_str(r#"{"maxTokens":18446744073709551616.0}"#).unwrap();
    assert!(serde_json::from_value::<Config>(too_large).is_err());
}

#[tokio::test]
async fn plugin_registers_metadata_and_disposal_unregisters_provider() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let web = ctx
        .plugin::<WebRuntime>(serde_json::json!({
            "searchProvider": DEEPSEEK_PROVIDER_ID
        }))
        .unwrap();
    web.settled().await.unwrap();
    let plugin = ctx
        .plugin::<WebSearchDeepSeek>(serde_json::json!({"apiKey":"ds-key"}))
        .unwrap();
    plugin.settled().await.unwrap();
    let listed = ctx.get::<Web>().unwrap().list_providers();
    assert_eq!(listed.search.len(), 1);
    assert_eq!(listed.search[0].id, DEEPSEEK_PROVIDER_ID);
    assert_eq!(listed.search[0].title, None);
    assert!(listed.search[0].description.is_some());
    plugin.dispose().await;
    assert!(ctx.get::<Web>().unwrap().list_providers().search.is_empty());
    let error = ctx
        .get::<Web>()
        .unwrap()
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "WEB_PROVIDER_CONFIGURED_MISSING"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn plugin_reports_the_actionable_missing_credential_error() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.provide::<LaunchEnvironment>(std::sync::Arc::new(create_launch_environment_snapshot(&[])))
        .await
        .unwrap();
    let web = ctx
        .plugin::<WebRuntime>(serde_json::json!({
            "searchProvider": DEEPSEEK_PROVIDER_ID
        }))
        .unwrap();
    web.settled().await.unwrap();
    let plugin = ctx
        .plugin::<WebSearchDeepSeek>(serde_json::json!({
            "apiKeyEnv":"HOH_WEB_SEARCH_DEEPSEEK_TEST_UNSET"
        }))
        .unwrap();
    plugin.settled().await.unwrap();
    let error = ctx
        .get::<Web>()
        .unwrap()
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "WEB_PROVIDER_CREDENTIAL_MISSING");
    assert!(
        error
            .message()
            .contains("store it through the credentials service")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn volatile_endpoint_update_is_used_by_the_next_search_without_reregistering() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let web = ctx
        .plugin::<WebRuntime>(serde_json::json!({
            "searchProvider": DEEPSEEK_PROVIDER_ID
        }))
        .unwrap();
    web.settled().await.unwrap();
    let plugin = ctx
        .plugin::<WebSearchDeepSeek>(serde_json::json!({
            "apiKey":"ds-key",
            "baseURL":"https://before.test/v1",
            "apiVersion":"invalid\nversion"
        }))
        .unwrap();
    plugin.settled().await.unwrap();

    let first = ctx
        .get::<Web>()
        .unwrap()
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(
        first
            .to_string()
            .contains("https://before.test/v1/messages")
    );

    plugin
        .update(serde_json::json!({
            "apiKey":"ds-key",
            "baseURL":"https://after.test/v1",
            "apiVersion":"invalid\nversion"
        }))
        .await
        .unwrap();
    let second = ctx
        .get::<Web>()
        .unwrap()
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(
        second
            .to_string()
            .contains("https://after.test/v1/messages")
    );
    assert_eq!(ctx.get::<Web>().unwrap().list_providers().search.len(), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_credential_reference_fails_provider_operations_without_panicking() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let web = ctx
        .plugin::<WebRuntime>(serde_json::json!({
            "searchProvider": DEEPSEEK_PROVIDER_ID
        }))
        .unwrap();
    web.settled().await.unwrap();
    let plugin = ctx
        .plugin::<WebSearchDeepSeek>(serde_json::json!({"apiKeyEnv":"bad key"}))
        .unwrap();
    plugin.settled().await.unwrap();
    let error = ctx
        .get::<Web>()
        .unwrap()
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("must match"));
    kernel.shutdown().await;
}
