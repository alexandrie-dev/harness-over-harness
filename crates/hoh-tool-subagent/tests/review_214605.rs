//! Review 214605, items 1-3: the route preflight branches that the earlier matrix wrongly deferred.
//! Every case ends in the source's error text and no `start` call.

mod support;
use hoh_agent::{AgentOptions, AgentSessionMeta, Live};
use hoh_llm::LlmRuntime;
use hoh_session::SessionId;
use hoh_timeout::AbortController;
use hoh_tool_subagent::model_selection::preflight_child_llm_route;
use serde_json::json;
use std::sync::{Arc, atomic::Ordering};
use support::*;

const NO_ROUTE: &str = "cannot select child LLM values without an effective provider and model";
const NO_LLM: &str =
    "cannot resolve the selected child LLM route because the `llm` service is unavailable";

fn effort(value: &str) -> AgentOptions {
    AgentOptions {
        reasoning_effort: Some(Live::Value(value.into())),
        ..Default::default()
    }
}

fn info(result: &hoh_tools::ToolExecutionResult) -> (String, String) {
    let info = result.error().unwrap().info.as_ref().unwrap();
    (info.name.clone(), info.code.clone())
}

#[tokio::test]
async fn preflight_without_an_effective_provider_and_model_is_rejected() {
    let llm = LlmRuntime::new();
    let signal = AbortController::new().signal();
    let error = preflight_child_llm_route(
        &llm,
        &AgentOptions::default(),
        Some(&effort("high")),
        &signal,
        true,
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), NO_ROUTE);
}

#[tokio::test]
async fn a_configured_effort_with_a_parent_without_a_route_fails_the_call_before_start() {
    let env = Env::new().await;
    let provider = Scripted::new("test");
    let started = provider.started.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({
        "provider": "test", "maxDepth": "provider-managed",
        "agentOptions": { "reasoningEffort": "high" }
    }))
    .await;
    let parent = env
        .harness
        .create(SessionId::from("no-route"))
        .await
        .unwrap();
    let result = env
        .call(
            "subagent",
            json!({ "description": "d", "prompt": "p" }),
            Some(parent),
        )
        .await;
    assert!(result.is_error());
    assert!(text(&result).contains(NO_ROUTE), "{}", text(&result));
    assert_eq!(
        info(&result),
        (
            "ToolContextUnavailableError".into(),
            "TOOL_CONTEXT_UNAVAILABLE".into()
        )
    );
    assert_eq!(started.load(Ordering::SeqCst), 0);
    env.close().await;
}

async fn parent_with_route() -> (Env, Arc<dyn hoh_agent::Agent>) {
    let env = Env::new().await;
    let agent = env
        .harness
        .create_with(
            SessionId::from("route-parent"),
            AgentOptions {
                provider: Some(Live::Value("alpha".into())),
                model: Some(Live::Value("fast".into())),
                ..Default::default()
            },
            AgentSessionMeta::default(),
        )
        .await
        .unwrap();
    (env, agent)
}

#[tokio::test]
async fn selected_routes_are_rejected_without_the_llm_service() {
    let (with_llm, parent) = parent_with_route().await;
    let env = NoLlmEnv::new_with(json!({
        "provider": "test", "maxDepth": "provider-managed", "modelSelection": true
    }))
    .await;
    let provider = Scripted::new("test");
    let started = provider.started.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    let result = env
        .call_tool(
            "subagent",
            json!({
                "description": "d", "prompt": "p", "provider": "alpha", "model": "fast"
            }),
            Some(parent),
        )
        .await;
    assert!(result.is_error());
    assert!(text(&result).contains(NO_LLM), "{}", text(&result));
    assert_eq!(
        info(&result),
        (
            "ToolContextUnavailableError".into(),
            "TOOL_CONTEXT_UNAVAILABLE".into()
        )
    );
    assert_eq!(started.load(Ordering::SeqCst), 0);
    env.kernel.shutdown().await;
    with_llm.close().await;
}

#[tokio::test]
async fn a_configured_effort_is_rejected_without_the_llm_service() {
    let (with_llm, parent) = parent_with_route().await;
    let env = NoLlmEnv::new_with(json!({
        "provider": "test", "maxDepth": "provider-managed",
        "agentOptions": { "reasoningEffort": "high" }
    }))
    .await;
    let provider = Scripted::new("test");
    let started = provider.started.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    let result = env
        .call_tool(
            "subagent",
            json!({ "description": "d", "prompt": "p" }),
            Some(parent),
        )
        .await;
    assert!(result.is_error());
    assert!(text(&result).contains(NO_LLM), "{}", text(&result));
    assert_eq!(started.load(Ordering::SeqCst), 0);
    env.kernel.shutdown().await;
    with_llm.close().await;
}

#[tokio::test]
async fn a_configured_unknown_effort_is_validated_before_child_creation() {
    let env = Env::new().await;
    let provider = Scripted::new("test");
    let started = provider.started.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({
        "provider": "test", "maxDepth": "provider-managed",
        "agentOptions": { "provider": "alpha", "model": "fast", "reasoningEffort": "bogus" }
    }))
    .await;
    let _alpha = env.catalog(&["alpha"], CatalogAdapter::new());
    let parent = env.harness.create(SessionId::from("effort")).await.unwrap();
    let result = env
        .call(
            "subagent",
            json!({ "description": "d", "prompt": "p" }),
            Some(parent),
        )
        .await;
    assert!(result.is_error());
    assert!(text(&result).contains("bogus"), "{}", text(&result));
    assert_eq!(started.load(Ordering::SeqCst), 0);
    env.close().await;
}
