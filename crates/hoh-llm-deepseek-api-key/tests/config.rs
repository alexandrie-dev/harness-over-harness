use hoh_launch_environment::{
    LaunchEnvironmentLayerInput, LaunchEnvironmentSource, create_launch_environment_snapshot,
};
use hoh_llm_deepseek_api_key::{
    Config, DeepSeekCatalogModel, Options, plain_options, resolve_adapter_options,
};
use indexmap::IndexMap;

fn model(id: &str) -> DeepSeekCatalogModel {
    DeepSeekCatalogModel {
        id: id.into(),
        name: None,
        description: None,
        context_window: None,
        max_tokens: None,
        input_modalities: None,
        image_pixel_budget: None,
        image_max_bytes: None,
        system_prompt_update: None,
        tool_update: None,
    }
}

#[test]
fn follows_the_official_listing_for_the_built_in_catalog() {
    let resolved = resolve_adapter_options(&Options::default(), None).unwrap();
    let resolved_models = resolved.models.clone();
    let listing = resolved.listing.unwrap();
    assert_eq!(listing.url, "https://api.deepseek.com/models");
    assert_eq!(listing.timeout_ms, 5_000.0);
    assert_eq!(listing.max_age_ms, 600_000.0);

    let resolved = resolve_adapter_options(&plain_options(&Config::default()), None).unwrap();
    assert!(resolved.listing.is_some());
    let explicit = resolve_adapter_options(
        &Options {
            models: Some(resolved_models),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(explicit.listing.is_some());
}

#[test]
fn custom_listing_timing_is_preserved_on_the_official_origin() {
    let resolved = resolve_adapter_options(
        &Options {
            base_url: Some("https://api.deepseek.com/anthropic/v1".into()),
            models_timeout_ms: Some(7.0),
            models_max_age_ms: Some(0.0),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let listing = resolved.listing.unwrap();
    assert_eq!(listing.url, "https://api.deepseek.com/models");
    assert_eq!(listing.timeout_ms, 7.0);
    assert_eq!(listing.max_age_ms, 0.0);
}

#[test]
fn listing_timing_errors_precede_protocol_errors_like_the_source_resolver() {
    let error = resolve_adapter_options(
        &Options {
            thinking: Some(hoh_llm_deepseek_api_key::Thinking::Disabled),
            reasoning_effort: Some(hoh_llm_deepseek_api_key::ReasoningEffort::High),
            models_timeout_ms: Some(0.0),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("llm-deepseek-api-key: modelsTimeoutMs must be a positive finite number")
    );
}

#[test]
fn custom_catalog_does_not_follow_the_listing() {
    let resolved = resolve_adapter_options(
        &Options {
            models: Some(vec![model("custom")]),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(resolved.listing.is_none());
}

#[test]
fn empty_catalog_does_not_follow_the_listing() {
    let resolved = resolve_adapter_options(
        &Options {
            models: Some(vec![]),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(resolved.listing.is_none());
}

#[test]
fn gateway_does_not_follow_the_listing() {
    let resolved = resolve_adapter_options(
        &Options {
            base_url: Some("https://gateway.example/anthropic".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(resolved.listing.is_none());
}

#[test]
fn official_host_over_http_does_not_follow_the_listing() {
    let resolved = resolve_adapter_options(
        &Options {
            base_url: Some("http://api.deepseek.com/anthropic".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(resolved.listing.is_none());
}

#[test]
fn rejects_zero_listing_timeout_even_without_a_listing() {
    let error = resolve_adapter_options(
        &Options {
            models: Some(vec![model("custom")]),
            models_timeout_ms: Some(0.0),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("llm-deepseek-api-key: modelsTimeoutMs must be a positive finite number")
    );
}

#[test]
fn rejects_fractional_listing_age_even_without_a_listing() {
    let error = resolve_adapter_options(
        &Options {
            models: Some(vec![model("custom")]),
            models_max_age_ms: Some(1.5),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert!(
        error.to_string().starts_with(
            "llm-deepseek-api-key: modelsMaxAgeMs must be a non-negative safe integer"
        )
    );
}

#[test]
fn launch_environment_supplies_the_protocol_endpoint() {
    let environment = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
        source: LaunchEnvironmentSource::ProjectEnv,
        path: Some("fixture.env".into()),
        values: IndexMap::from_iter([(
            "DEEPSEEK_BASE_URL".into(),
            "https://gateway.example/messages".into(),
        )]),
    }]);
    let resolved = resolve_adapter_options(&Options::default(), Some(&environment)).unwrap();
    assert_eq!(resolved.base_url, "https://gateway.example/messages");
    assert!(resolved.listing.is_none());
}

#[test]
fn validates_the_credential_reference() {
    let error = resolve_adapter_options(
        &Options {
            api_key_env: Some("bad-ref".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "credential ref \"bad-ref\" must match /^[A-Za-z_][A-Za-z0-9_]*$/"
    );
}

#[test]
fn config_schema_marks_api_key_as_volatile_credential_reference() {
    let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
    assert_eq!(schema["properties"]["apiKeyEnv"]["x-volatile"], true);
    assert_eq!(
        schema["properties"]["apiKeyEnv"]["x-role"],
        "credential-ref"
    );
    assert_eq!(
        schema["properties"]["apiKeyEnv"]["default"],
        "DEEPSEEK_API_KEY"
    );
}
