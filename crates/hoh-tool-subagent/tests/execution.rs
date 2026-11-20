//! Execution of the mounted delegation tool through the real tool pipeline and subagent runtime
//! (cases of `tool-subagent.spec.ts` that need a provider able to start a child).

mod support;
use hoh_session::SessionId;
use hoh_subagent::{SubagentCapabilities, SubagentProvider};
use hoh_timeout::AbortController;
use hoh_tool_subagent::ToolSubagent;
use serde_json::json;
use std::sync::{Arc, atomic::Ordering};
use support::*;
use tokio::sync::Notify;

#[tokio::test]
async fn starts_a_managed_subagent_and_returns_its_name_without_collecting_output() {
    let env = Env::new().await;
    let _provider = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("test")))
        .unwrap();
    env.mount(json!({ "provider": "test", "maxDepth": "provider-managed" }))
        .await;
    let parent = env.harness.create(SessionId::from("parent")).await.unwrap();
    let result = env
        .call(
            "subagent",
            json!({ "description": "do a thing", "prompt": "go research X" }),
            Some(parent),
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    assert_eq!(
        result.value().unwrap(),
        &json!({ "kind": "activation", "name": "agent-1" })
    );
    assert_eq!(result.meta(), result.value());
    assert!(text(&result).starts_with("started subagent "));
    env.close().await;
}

#[tokio::test]
async fn resolves_an_explicit_child_directory_against_the_parent_directory() {
    let env = Env::new().await;
    let provider = Scripted::new("test");
    let seen = provider.seen.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({ "provider": "test", "maxDepth": "provider-managed" }))
        .await;
    let parent = env
        .harness
        .create(SessionId::from("cwd-parent"))
        .await
        .unwrap();
    let parent_cwd = env
        .ctx
        .get::<hoh_working_directory::WorkingDirectoryKey>()
        .unwrap()
        .get(parent.session());
    let result = env
        .call(
            "subagent",
            json!({ "description": "inspect packages", "prompt": "list", "cwd": "packages" }),
            Some(parent.clone()),
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    let child_cwd = seen.lock().unwrap()[0].cwd.clone();
    assert_eq!(
        child_cwd,
        std::path::Path::new(&parent_cwd)
            .join("packages")
            .to_string_lossy()
    );
    let after = env
        .ctx
        .get::<hoh_working_directory::WorkingDirectoryKey>()
        .unwrap()
        .get(parent.session());
    assert_eq!(after, parent_cwd);
    env.close().await;
}

#[tokio::test]
async fn overlaps_sibling_delegations_dispatched_concurrently() {
    let env = Env::new().await;
    let mut provider = Scripted::new("test");
    provider.gate = Some(Arc::new(Notify::new()));
    provider.expect_started = 2;
    let seen = provider.seen.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({ "provider": "test", "maxDepth": "provider-managed" }))
        .await;
    let parent = env
        .harness
        .create(SessionId::from("overlap-parent"))
        .await
        .unwrap();
    // Hidden serialisation would deadlock here: each child blocks until both have started.
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            env.call(
                "subagent",
                json!({ "description": "first", "prompt": "p1" }),
                Some(parent.clone())
            ),
            env.call(
                "subagent",
                json!({ "description": "second", "prompt": "p2" }),
                Some(parent.clone())
            )
        )
    })
    .await
    .expect("sibling delegations were serialised");
    assert!(!first.is_error() && !second.is_error());
    let mut labels: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|seen| seen.label.clone().unwrap_or_default())
        .collect();
    labels.sort();
    assert_eq!(labels, ["first", "second"]);
    env.close().await;
}

#[tokio::test]
async fn registers_under_a_configurable_tool_name_so_providers_coexist() {
    let env = Env::new().await;
    let _spawn = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("spawn")))
        .unwrap();
    let _acp = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("acp")))
        .unwrap();
    env.mount(
        json!({ "provider": "spawn", "toolName": "subagent", "maxDepth": "provider-managed" }),
    )
    .await;
    env.mount(
        json!({ "provider": "acp", "toolName": "subagent_acp", "maxDepth": "provider-managed" }),
    )
    .await;
    let mut names: Vec<_> = env
        .tools()
        .schemas(None, None)
        .unwrap()
        .into_iter()
        .map(|schema| schema.name)
        .filter(|name| name.starts_with("subagent"))
        .collect();
    names.sort();
    assert_eq!(names, ["subagent", "subagent_acp"]);
    let parent = env.harness.create(SessionId::from("multi")).await.unwrap();
    for tool in ["subagent", "subagent_acp"] {
        let result = env
            .call(
                tool,
                json!({ "description": "d", "prompt": "p" }),
                Some(parent.clone()),
            )
            .await;
        assert!(text(&result).starts_with("started subagent "), "{tool}");
    }
    env.close().await;
}

#[tokio::test]
async fn passes_max_depth_through_to_the_start_request_and_omits_agent_options() {
    let env = Env::new().await;
    let provider = Scripted::new("test");
    let seen = provider.seen.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({ "provider": "test", "maxDepth": 3 }))
        .await;
    let parent = env.harness.create(SessionId::from("depth")).await.unwrap();
    let result = env
        .call(
            "subagent",
            json!({ "description": "d", "prompt": "p" }),
            Some(parent),
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].max_depth, Some(3));
    assert!(!seen[0].has_agent_options);
    env.close().await;
}

#[tokio::test]
async fn registers_when_the_provider_appears_later_and_ignores_other_providers() {
    let env = Env::new().await;
    env.mount(json!({ "provider": "late", "maxDepth": "provider-managed" }))
        .await;
    assert!(env.tools().get("subagent", None, None).is_none());
    let _other = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("other")))
        .unwrap();
    assert!(env.tools().get("subagent", None, None).is_none());
    let late = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("late")))
        .unwrap();
    assert!(env.tools().get("subagent", None, None).is_some());
    drop(late);
    assert!(env.tools().get("subagent", None, None).is_none());
    // Re-registration derives the tool again.
    let _again = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("late")))
        .unwrap();
    assert!(env.tools().get("subagent", None, None).is_some());
    env.close().await;
}

#[tokio::test]
async fn rejects_configured_agent_options_when_the_provider_cannot_apply_them() {
    let env = Env::new().await;
    let mut provider = Scripted::new("test");
    provider.capabilities.agent_options = false;
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    let fiber = env
        .ctx
        .plugin::<ToolSubagent>(json!({
            "provider": "test",
            "maxDepth": "provider-managed",
            "agentOptions": { "model": "configured-model" }
        }))
        .unwrap();
    let error = fiber.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not support child agentOptions"),
        "{error}"
    );
    env.close().await;
}

#[tokio::test]
async fn disposing_the_plugin_fiber_unmounts_the_tool() {
    let env = Env::new().await;
    let _registration = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("test")))
        .unwrap();
    let fiber = env
        .ctx
        .plugin::<ToolSubagent>(json!({ "provider": "test", "maxDepth": "provider-managed" }))
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(env.tools().get("subagent", None, None).is_some());
    fiber.dispose().await;
    assert!(env.tools().get("subagent", None, None).is_none());
    env.close().await;
}

#[tokio::test]
async fn skips_startup_when_cancellation_wins_asynchronous_route_preflight() {
    let env = Env::new().await;
    let provider = Scripted::new("test");
    let started = provider.started.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({
        "provider": "test",
        "maxDepth": "provider-managed",
        "agentOptions": { "provider": "alpha", "model": "selected-model" }
    }))
    .await;
    let adapter = GateAdapter::new();
    env.adapter(adapter.clone());
    let parent = env.harness.create(SessionId::from("cancel")).await.unwrap();
    let controller = AbortController::new();
    let pending = env.call_with(
        "subagent",
        json!({ "description": "cancelled selection", "prompt": "do it" }),
        Some(parent),
        controller.signal(),
    );
    let released = async {
        adapter.until_resolving().await;
        controller.abort();
        adapter.release();
    };
    let (result, ()) = tokio::join!(pending, released);
    assert!(result.is_error());
    assert_eq!(started.load(Ordering::SeqCst), 0);
    env.close().await;
}

#[tokio::test]
async fn rejects_startup_when_the_provider_changes_during_route_preflight() {
    let env = Env::new().await;
    env.with_selection_policy().await;
    let mut old = Scripted::new("test");
    old.defaults = Some(("alpha".into(), "selected-model".into()));
    let old_started = old.started.clone();
    let registration = env.subagents().register_provider(Arc::new(old)).unwrap();
    env.mount(json!({
        "provider": "test", "maxDepth": "provider-managed", "modelSelection": true
    }))
    .await;
    let adapter = GateAdapter::new();
    env.adapter(adapter.clone());
    let parent = env.harness.create(SessionId::from("swap")).await.unwrap();
    let pending = env.call(
        "subagent",
        json!({
            "description": "swapped provider", "prompt": "do it",
            "provider": "alpha", "model": "selected-model"
        }),
        Some(parent),
    );
    let swapped = async {
        adapter.until_resolving().await;
        drop(registration);
        let mut replacement = Scripted::new("test");
        replacement.capabilities.depth_limit = false;
        replacement.capabilities.output_schema = false;
        replacement.defaults = Some(("beta".into(), "replacement-model".into()));
        let replacement_started = replacement.started.clone();
        let registration = env
            .subagents()
            .register_provider(Arc::new(replacement))
            .unwrap();
        adapter.release();
        (registration, replacement_started)
    };
    let (result, (_registration, replacement_started)) = tokio::join!(pending, swapped);
    assert!(result.is_error());
    assert!(text(&result).contains("changed while resolving the child LLM route"));
    let info = result.error().unwrap().info.as_ref().unwrap();
    assert_eq!(
        (info.name.as_str(), info.code.as_str()),
        ("SubagentError", "PROVIDER_CHANGED")
    );
    assert_eq!(old_started.load(Ordering::SeqCst), 0);
    assert_eq!(replacement_started.load(Ordering::SeqCst), 0);
    env.close().await;
}

#[tokio::test]
async fn merges_model_overrides_over_provider_route_defaults_before_preflight() {
    let env = Env::new().await;
    env.with_selection_policy().await;
    let mut provider = Scripted::new("test");
    provider.defaults = Some(("alpha".into(), "child-model".into()));
    let seen = provider.seen.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({
        "provider": "test", "maxDepth": "provider-managed", "modelSelection": true,
        "agentOptions": { "reasoningEffort": "high", "maxTokens": 321 }
    }))
    .await;
    let adapter = GateAdapter::new();
    adapter.release();
    env.adapter(adapter);
    let parent = env.harness.create(SessionId::from("merge")).await.unwrap();
    let result = env
        .call(
            "subagent",
            json!({
                "description": "d", "prompt": "p",
                "provider": "alpha", "model": "child-model"
            }),
            Some(parent.clone()),
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    let schema = env
        .tools()
        .schemas(None, None)
        .unwrap()
        .into_iter()
        .find(|schema| schema.name == "subagent")
        .unwrap();
    assert!(schema.description.contains("this backend's route defaults"));
    assert_eq!(
        seen.lock().unwrap()[0].options,
        (
            Some("alpha".into()),
            Some("child-model".into()),
            None,
            Some(321)
        )
    );
    env.close().await;
}

#[tokio::test]
async fn does_not_inherit_parent_effort_for_a_provider_route_default() {
    let env = Env::new().await;
    env.with_selection_policy().await;
    let mut provider = Scripted::new("test");
    provider.defaults = Some(("alpha".into(), "child-model".into()));
    let seen = provider.seen.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({
        "provider": "test", "maxDepth": "provider-managed", "modelSelection": true
    }))
    .await;
    let adapter = GateAdapter::new();
    adapter.release();
    env.adapter(adapter);
    let parent = env
        .harness
        .create_with(
            SessionId::from("effort-parent"),
            hoh_agent::AgentOptions {
                provider: Some(hoh_agent::Live::Value("alpha".into())),
                model: Some(hoh_agent::Live::Value("child-model".into())),
                reasoning_effort: Some(hoh_agent::Live::Value("high".into())),
                max_tokens: None,
            },
            Default::default(),
        )
        .await
        .unwrap();
    let result = env
        .call(
            "subagent",
            json!({
                "description": "d", "prompt": "p",
                "provider": "alpha", "model": "child-model"
            }),
            Some(parent),
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    assert_eq!(
        seen.lock().unwrap()[0].options,
        (Some("alpha".into()), Some("child-model".into()), None, None)
    );
    env.close().await;
}

#[tokio::test]
async fn defaults_max_depth_to_one_and_forwards_the_label() {
    let env = Env::new().await;
    let provider = Scripted::new("test");
    let seen = provider.seen.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({ "provider": "test" })).await;
    let parent = env
        .harness
        .create(SessionId::from("default-depth"))
        .await
        .unwrap();
    let result = env
        .call(
            "subagent",
            json!({ "description": "d", "prompt": "p" }),
            Some(parent),
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].label.as_deref(), Some("d"));
    assert_eq!(seen[0].max_depth, Some(1));
    env.close().await;
}

#[tokio::test]
async fn rejects_a_numeric_max_depth_on_a_provider_without_the_depth_limit_capability() {
    let env = Env::new().await;
    let mut provider = Scripted::new("no-depth");
    provider.capabilities = Default::default();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    let error = env
        .ctx
        .plugin::<ToolSubagent>(json!({ "provider": "no-depth" }))
        .unwrap()
        .settled()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("provider-managed"), "{error}");
    env.close().await;
}

#[tokio::test]
async fn provider_managed_omits_the_cap_so_a_capability_less_provider_mounts_and_starts() {
    let env = Env::new().await;
    let mut provider = Scripted::new("external");
    provider.capabilities = Default::default();
    let seen = provider.seen.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({ "provider": "external", "maxDepth": "provider-managed" }))
        .await;
    let parent = env
        .harness
        .create(SessionId::from("managed"))
        .await
        .unwrap();
    let result = env
        .call(
            "subagent",
            json!({ "description": "d", "prompt": "p" }),
            Some(parent),
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    assert_eq!(seen.lock().unwrap()[0].max_depth, None);
    env.close().await;
}

#[tokio::test]
async fn skips_provider_startup_for_an_already_aborted_signal() {
    let env = Env::new().await;
    let provider = Scripted::new("spy");
    let started = provider.started.clone();
    let _registration = env
        .subagents()
        .register_provider(Arc::new(provider))
        .unwrap();
    env.mount(json!({ "provider": "spy", "maxDepth": "provider-managed" }))
        .await;
    let parent = env
        .harness
        .create(SessionId::from("aborted"))
        .await
        .unwrap();
    let controller = AbortController::new();
    controller.abort();
    let result = env
        .call_with(
            "subagent",
            json!({ "description": "d", "prompt": "p" }),
            Some(parent),
            controller.signal(),
        )
        .await;
    assert!(result.is_error());
    assert_eq!(started.load(Ordering::SeqCst), 0);
    assert_eq!(
        result.error().unwrap().message,
        "tool call aborted before dispatch"
    );
    env.close().await;
}

#[tokio::test]
async fn starts_a_child_under_a_requested_name_and_rejects_a_sibling_with_the_same_name() {
    let env = Env::new().await;
    let _registration = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("test")))
        .unwrap();
    env.mount(json!({ "provider": "test", "maxDepth": "provider-managed" }))
        .await;
    let parent = env.harness.create(SessionId::from("named")).await.unwrap();
    let named = env
        .call(
            "subagent",
            json!({ "description": "review", "prompt": "review it", "name": "reviewer" }),
            Some(parent.clone()),
        )
        .await;
    assert!(!named.is_error(), "{}", text(&named));
    assert_eq!(text(&named), "started subagent reviewer");
    let duplicate = env
        .call(
            "subagent",
            json!({ "description": "again", "prompt": "review it", "name": "reviewer" }),
            Some(parent),
        )
        .await;
    assert!(duplicate.is_error());
    assert!(
        text(&duplicate)
            .contains("agent name \"reviewer\" is already used by another subagent you created"),
        "{}",
        text(&duplicate)
    );
    env.close().await;
}

#[tokio::test]
async fn description_offers_follow_up_messages_only_for_a_continuable_provider() {
    struct Continuable(Scripted);
    impl SubagentProvider for Continuable {
        fn name(&self) -> &str {
            self.0.name()
        }
        fn capabilities(&self) -> SubagentCapabilities {
            self.0.capabilities()
        }
        fn inherits_parent_context(&self) -> bool {
            true
        }
        fn supports_continuable(&self) -> bool {
            true
        }
    }
    let env = Env::new().await;
    let _fresh = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("fresh")))
        .unwrap();
    let _fork = env
        .subagents()
        .register_provider(Arc::new(Continuable(Scripted::new("fork"))))
        .unwrap();
    env.mount(json!({ "provider": "fresh", "maxDepth": "provider-managed" }))
        .await;
    env.mount(
        json!({ "provider": "fork", "toolName": "subagent_fork", "maxDepth": "provider-managed" }),
    )
    .await;
    let description = |name: &str| {
        env.tools()
            .schemas(None, None)
            .unwrap()
            .into_iter()
            .find(|schema| schema.name == name)
            .unwrap()
            .description
    };
    let fresh = description("subagent");
    assert!(fresh.contains("This backend does not accept follow-up messages."));
    assert!(fresh.starts_with("Delegate a self-contained task"));
    let fork = description("subagent_fork");
    assert!(fork.contains("Use `send_message` to steer it while running"));
    assert!(fork.starts_with("Delegate a task to a subagent that inherits this conversation"));
    env.close().await;
}

#[tokio::test]
async fn shares_one_guidance_paragraph_across_visible_delegation_tools() {
    let env = Env::new().await;
    let _a = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("spawn")))
        .unwrap();
    let _b = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("acp")))
        .unwrap();
    env.mount(json!({ "provider": "spawn", "maxDepth": "provider-managed" }))
        .await;
    env.mount(
        json!({ "provider": "acp", "toolName": "subagent_acp", "maxDepth": "provider-managed" }),
    )
    .await;
    let agent = env
        .harness
        .create(SessionId::from("guidance"))
        .await
        .unwrap();
    let prompt = env
        .ctx
        .get::<hoh_system_prompt::SystemPromptKey>()
        .unwrap()
        .assemble(hoh_agent::assemble_context_for(&agent, None))
        .await
        .unwrap();
    let all: String = prompt
        .sections
        .iter()
        .map(|section| section.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        all.matches("Start independent delegations with `subagent` or `subagent_acp` together")
            .count(),
        1,
        "{all}"
    );
    env.close().await;
}
