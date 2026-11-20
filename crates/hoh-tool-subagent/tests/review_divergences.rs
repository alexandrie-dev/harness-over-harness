//! Review 212524, items 3-5: configuration validation, policy projection texts and the
//! `origin: "subagent"` fallback of the settings owner, each against the TypeScript source.

mod support;
use hoh_agent::AgentSessionMeta;
use hoh_session::SessionId;
use hoh_tool_subagent::{
    Config, model_selection_settings::Config as SettingsConfig,
    model_selection_state::subagent_model_selection_projection_definition,
};
use serde_json::{Value, json};
use std::sync::Arc;
use support::*;

fn apply_error(data: Value) -> String {
    let definition = subagent_model_selection_projection_definition();
    let event = hoh_session::SessionEvent::from_value(json!({
        "seq": 0, "time": 0, "type": "subagent/model-selection-policy", "data": data
    }));
    (definition.apply)(&Arc::new(None), &event)
        .unwrap_err()
        .to_string()
}

fn state_error(state: Value) -> bool {
    let definition = subagent_model_selection_projection_definition();
    (definition.state_schema)(Some(&state)).is_err()
}

#[tokio::test]
async fn max_tokens_above_safe_integer_is_rejected() {
    let env = Env::new().await;
    let plugin = env
        .ctx
        .plugin::<hoh_tool_subagent::ToolSubagent>(json!({
            "provider": "test",
            "agentOptions": { "maxTokens": 9007199254740993u64 }
        }))
        .unwrap();
    assert!(plugin.settled().await.is_err());
    env.close().await;
}

#[test]
fn settings_route_ids_are_validated_when_the_config_loads() {
    for route in [
        json!({ "provider": "", "model": "m" }),
        json!({ "provider": "p", "model": "" }),
    ] {
        let error = serde_json::from_value::<SettingsConfig>(json!({ "allowedModels": [route] }));
        assert!(error.is_err(), "{route}");
    }
    assert!(
        serde_json::from_value::<SettingsConfig>(
            json!({ "allowedModels": [{ "provider": "p", "model": "m" }] })
        )
        .is_ok()
    );
}

#[test]
fn an_empty_provider_string_satisfies_required_like_the_source() {
    // schemastery `required()` rejects only a missing (null/undefined) value, never "".
    let config: Config = serde_json::from_value(json!({ "provider": "" })).unwrap();
    assert_eq!(config.provider, "");
    assert!(serde_json::from_value::<Config>(json!({})).is_err());
}

#[tokio::test]
async fn an_empty_provider_string_mounts_without_error_and_waits() {
    let env = Env::new().await;
    env.mount(json!({ "provider": "" })).await;
    assert!(env.tools().get("subagent", None, None).is_none());
    env.close().await;
}

#[test]
fn projection_apply_error_texts_match_source() {
    assert_eq!(
        apply_error(json!({})),
        "subagent model selection requires an array of routes"
    );
    assert_eq!(
        apply_error(json!({ "allowedModels": "x" })),
        "subagent model selection requires an array of routes"
    );
    for routes in [
        json!(["x"]),
        json!([null]),
        json!([[]]),
        json!([{ "provider": "p" }]),
        json!([{ "provider": 1, "model": "m" }]),
        json!([{ "provider": "", "model": "m" }]),
    ] {
        assert_eq!(
            apply_error(json!({ "allowedModels": routes })),
            "subagent model selection requires non-empty provider and model ids",
            "{routes}"
        );
    }
}

#[test]
fn projection_state_schema_is_strict_without_checking_duplicates() {
    let route = json!({ "provider": "p", "model": "m" });
    assert!(state_error(
        json!([{ "provider": "p", "model": "m", "extra": 1 }])
    ));
    assert!(state_error(json!([])));
    assert!(state_error(json!([{ "provider": "", "model": "m" }])));
    assert!(!state_error(json!([route.clone(), route])));
    assert!(!state_error(Value::Null));
}

#[tokio::test]
async fn subagent_header_without_parent_falls_back_to_root_sampling() {
    let env = Env::new().await;
    env.with_selection_policy().await;
    let agent = env
        .harness
        .create_with(
            SessionId::from("orphan-child"),
            Default::default(),
            AgentSessionMeta {
                origin: Some("subagent".into()),
                delegation_depth: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let policies = agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "subagent/model-selection-policy")
        .count();
    assert_eq!(policies, 1);
    env.close().await;
}
