//! Runtime cases of `model-selection.spec.ts`: route fields, their rejection, and preflight
//! failures observed through the mounted tool.

mod support;
use hoh_session::SessionId;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use support::*;

fn error_info(result: &hoh_tools::ToolExecutionResult) -> (String, String) {
    let info = result.error().unwrap().info.as_ref().unwrap();
    (info.name.clone(), info.code.clone())
}

fn property_names(schema: &hoh_llm::ToolSchema) -> Vec<String> {
    let mut names: Vec<_> = schema.parameters["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

fn subagent_schema(env: &Env) -> hoh_llm::ToolSchema {
    env.tools()
        .schemas(None, None)
        .unwrap()
        .into_iter()
        .find(|schema| schema.name == "subagent")
        .unwrap()
}

#[tokio::test]
async fn exposes_route_fields_and_discovery_when_selection_is_enabled() {
    let env = Env::new().await;
    env.with_selection_policy().await;
    let _provider = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("test")))
        .unwrap();
    env.mount(json!({
        "provider": "test", "maxDepth": "provider-managed",
        "modelSelection": true, "modelDiscovery": true
    }))
    .await;
    let schema = subagent_schema(&env);
    assert_eq!(
        property_names(&schema),
        [
            "cwd",
            "description",
            "model",
            "name",
            "prompt",
            "provider",
            "reasoning_effort"
        ]
    );
    assert!(!schema.description.contains("list_subagent_models"));
    assert!(
        !schema
            .description
            .contains("Changing the route can prevent provider-side reuse")
    );
    assert!(
        env.tools()
            .get("list_subagent_models", None, None)
            .is_some()
    );
    env.close().await;
}

#[tokio::test]
async fn hides_and_rejects_route_fields_without_model_selection() {
    let env = Env::new().await;
    let _provider = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("test")))
        .unwrap();
    env.mount(json!({ "provider": "test", "maxDepth": "provider-managed" }))
        .await;
    let schema = subagent_schema(&env);
    assert_eq!(
        property_names(&schema),
        ["cwd", "description", "name", "prompt"]
    );
    assert!(!schema.description.contains("list_subagent_models"));
    assert!(
        env.tools()
            .get("list_subagent_models", None, None)
            .is_none()
    );
    let parent = env.harness.create(SessionId::from("hidden")).await.unwrap();
    for selection in [
        json!({ "provider": "alpha", "model": "parent-model" }),
        json!({ "reasoning_effort": "low" }),
    ] {
        let mut args = json!({ "description": "forced route", "prompt": "do it" });
        args.as_object_mut()
            .unwrap()
            .extend(selection.as_object().unwrap().clone());
        let result = env.call("subagent", args, Some(parent.clone())).await;
        assert!(result.is_error());
        assert!(text(&result).contains("child model selection is disabled for this tool instance"));
        assert_eq!(
            error_info(&result),
            ("SubagentError".into(), "MODEL_SELECTION_DISABLED".into())
        );
    }
    env.close().await;
}

#[tokio::test]
async fn rejects_route_arguments_for_a_backend_without_agent_options_support() {
    let env = Env::new().await;
    let mut provider = Scripted::new("test");
    provider.capabilities.agent_options = false;
    let _provider = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({ "provider": "test", "maxDepth": "provider-managed" }))
        .await;
    let parent = env.harness.create(SessionId::from("noopts")).await.unwrap();
    let result = env
        .call(
            "subagent",
            json!({
                "description": "unsupported route", "prompt": "do it",
                "provider": "alpha", "model": "parent-model"
            }),
            Some(parent),
        )
        .await;
    assert!(result.is_error());
    assert!(text(&result).contains("model selection is disabled for this tool instance"));
    assert_eq!(
        error_info(&result),
        ("SubagentError".into(), "MODEL_SELECTION_DISABLED".into())
    );
    env.close().await;
}

#[tokio::test]
async fn rejects_enabled_model_selection_when_the_provider_cannot_apply_agent_options() {
    let env = Env::new().await;
    let mut provider = Scripted::new("test");
    provider.capabilities.agent_options = false;
    let _provider = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    let error = env
        .ctx
        .plugin::<hoh_tool_subagent::ToolSubagent>(json!({
            "provider": "test", "maxDepth": "provider-managed", "modelSelection": true
        }))
        .unwrap()
        .settled()
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("provider \"test\" does not support child model selection"),
        "{error}"
    );
    env.close().await;
}

#[tokio::test]
async fn pure_inherited_routing_does_not_resolve_an_llm_route() {
    let env = Env::new().await;
    let provider = Scripted::new("test");
    let started = provider.started.clone();
    let _provider = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({ "provider": "test", "maxDepth": "provider-managed" }))
        .await;
    let adapter = GateAdapter::new();
    env.adapter(adapter.clone());
    let parent = env
        .harness
        .create(SessionId::from("inherit"))
        .await
        .unwrap();
    let result = env
        .call(
            "subagent",
            json!({ "description": "inherit route", "prompt": "do it" }),
            Some(parent),
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.resolving.load(Ordering::SeqCst), 0);
    env.close().await;
}

#[tokio::test]
async fn propagates_an_exact_route_resolver_failure_before_child_creation() {
    let env = Env::new().await;
    env.with_selection_policy().await;
    let provider = Scripted::new("test");
    let started = provider.started.clone();
    let _provider = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({
        "provider": "test", "maxDepth": "provider-managed", "modelSelection": true
    }))
    .await;
    let adapter = GateAdapter::new();
    adapter.fail.store(true, Ordering::SeqCst);
    adapter.release();
    env.adapter(adapter);
    let parent = env.harness.create(SessionId::from("fail")).await.unwrap();
    let result = env
        .call(
            "subagent",
            json!({
                "description": "route work", "prompt": "do it",
                "provider": "alpha", "model": "child-model"
            }),
            Some(parent),
        )
        .await;
    assert!(result.is_error());
    assert!(text(&result).contains("selected route unavailable"));
    assert_eq!(started.load(Ordering::SeqCst), 0);
    let _: Option<Value> = None;
    env.close().await;
}
