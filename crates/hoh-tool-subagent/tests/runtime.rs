use hoh_agent::{AgentSessionMeta, assemble_context_for};
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, AgentLoopTestHarness, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_session::SessionId;
use hoh_subagent::{
    ResolvedSubagentStartRequest, SubagentCapabilities, SubagentPlugin, SubagentProvider, Subagents,
};
use hoh_system_prompt::SystemPromptKey;
use hoh_timeout::AbortController;
use hoh_tool_subagent::{
    ToolSubagent,
    model_selection_settings::{SubagentModelSelection, SubagentModelSelectionSettings},
};
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::{ToolExecutionInput, Tools};
use serde_json::json;
use std::{future::Future, pin::Pin, sync::Arc};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

fn prompt_text(prompt: &hoh_system_prompt::PromptAssembly) -> String {
    prompt
        .sections
        .iter()
        .map(|section| section.text.as_str())
        .chain(prompt.contexts.iter().map(|context| context.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone)]
struct Provider {
    capabilities: SubagentCapabilities,
    inherits: bool,
}

impl Provider {
    fn supported() -> Arc<Self> {
        Arc::new(Self {
            capabilities: SubagentCapabilities {
                agent_options: true,
                agent_preset: true,
                depth_limit: true,
                ..Default::default()
            },
            inherits: false,
        })
    }
}

impl SubagentProvider for Provider {
    fn name(&self) -> &str {
        "test"
    }

    fn capabilities(&self) -> SubagentCapabilities {
        self.capabilities
    }

    fn inherits_parent_context(&self) -> bool {
        self.inherits
    }

    fn supports_start(&self) -> bool {
        true
    }

    fn start(
        &self,
        _request: ResolvedSubagentStartRequest,
    ) -> Option<BoxFuture<Result<Arc<dyn hoh_subagent::SubagentRun>, BoxError>>> {
        Some(Box::pin(async {
            Err("test provider was unexpectedly started".into())
        }))
    }
}

struct Env {
    kernel: Kernel,
    ctx: Context,
    harness: AgentLoopTestHarness,
}

impl Env {
    async fn new() -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_agent_loop_test_dependencies(
            &ctx,
            AgentLoopTestDependenciesOptions {
                working_directory: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
        ctx.plugin::<SubagentPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        Self {
            kernel,
            ctx,
            harness,
        }
    }

    fn tools(&self) -> hoh_kernel::Service<Tools> {
        self.ctx.get::<Tools>().unwrap()
    }

    fn subagents(&self) -> hoh_kernel::Service<Subagents> {
        self.ctx.get::<Subagents>().unwrap()
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn provider_lifecycle_mounts_and_removes_the_tool() {
    let env = Env::new().await;
    let fiber = env
        .ctx
        .plugin::<ToolSubagent>(json!({ "provider": "test" }))
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(env.tools().get("subagent", None, None).is_none());

    let registration = env
        .subagents()
        .register_provider(Provider::supported())
        .unwrap();
    assert!(env.tools().get("subagent", None, None).is_some());
    drop(registration);
    assert!(env.tools().get("subagent", None, None).is_none());
    env.close().await;
}

#[tokio::test]
async fn mounted_tool_has_exact_required_surface_and_activation_output() {
    let env = Env::new().await;
    let _provider = env
        .subagents()
        .register_provider(Provider::supported())
        .unwrap();
    env.ctx
        .plugin::<ToolSubagent>(json!({ "provider": "test" }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let schema = env
        .tools()
        .schemas(None, None)
        .unwrap()
        .into_iter()
        .find(|schema| schema.name == "subagent")
        .unwrap();
    let properties = schema.parameters["properties"].as_object().unwrap();
    assert_eq!(
        properties.keys().cloned().collect::<Vec<_>>(),
        ["cwd", "description", "name", "prompt"]
    );
    assert_eq!(
        schema.parameters["required"],
        json!(["description", "prompt"])
    );
    let definition = env.tools().get("subagent", None, None).unwrap();
    assert_eq!(definition.output.schema["additionalProperties"], false);
    assert_eq!(
        definition.output.schema["properties"]["kind"]["const"],
        "activation"
    );
    assert!(definition.is_concurrency_safe.as_ref().unwrap()(&json!({
        "description": "small task", "prompt": "do it"
    })));
    env.close().await;
}

#[tokio::test]
async fn call_without_agent_fails_before_provider_start() {
    let env = Env::new().await;
    let _provider = env
        .subagents()
        .register_provider(Provider::supported())
        .unwrap();
    env.ctx
        .plugin::<ToolSubagent>(json!({ "provider": "test" }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let input = ToolExecutionInput::new(
        "call-1".into(),
        "subagent",
        json!({ "description": "small task", "prompt": "do it" }),
        AbortController::new().signal(),
    );
    let result = execute_tool_fixture(&env.ctx, input).await.unwrap();
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().info.as_ref().unwrap().code,
        "TOOL_CONTEXT_UNAVAILABLE"
    );
    assert!(
        result
            .content()
            .iter()
            .filter_map(hoh_llm::ContentBlock::text_value)
            .collect::<String>()
            .contains("exec.agent was undefined")
    );
    env.close().await;
}

#[tokio::test]
async fn guidance_is_visible_only_while_own_definition_is_visible() {
    let env = Env::new().await;
    let registration = env
        .subagents()
        .register_provider(Provider::supported())
        .unwrap();
    env.ctx
        .plugin::<ToolSubagent>(json!({ "provider": "test" }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let agent = env.harness.create(SessionId::from("parent")).await.unwrap();
    let prompt = env
        .ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(assemble_context_for(&agent, None))
        .await
        .unwrap();
    let assembled_text = prompt_text(&prompt);
    assert!(
        assembled_text.contains("Start independent delegations with `subagent`"),
        "{assembled_text}"
    );
    drop(registration);
    let prompt = env
        .ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(assemble_context_for(&agent, None))
        .await
        .unwrap();
    assert!(!prompt_text(&prompt).contains("Start independent delegations"));
    env.close().await;
}

#[tokio::test]
async fn max_depth_context_warns_at_the_boundary() {
    let env = Env::new().await;
    let _provider = env
        .subagents()
        .register_provider(Provider::supported())
        .unwrap();
    env.ctx
        .plugin::<ToolSubagent>(json!({ "provider": "test", "maxDepth": 1 }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let agent = env
        .harness
        .create_with(
            SessionId::from("child"),
            Default::default(),
            AgentSessionMeta {
                origin: Some("subagent".into()),
                delegation_depth: Some(1),
                parent_session: Some(SessionId::from("parent")),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let prompt = env
        .ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(assemble_context_for(&agent, None))
        .await
        .unwrap();
    assert!(prompt_text(&prompt).contains("current 1, maximum 1, remaining levels 0"));
    assert!(prompt_text(&prompt).contains("Do not call `subagent` at this depth"));
    env.close().await;
}

#[tokio::test]
async fn unsupported_provider_addition_rolls_back_without_a_tool() {
    let env = Env::new().await;
    env.ctx
        .plugin::<ToolSubagent>(json!({ "provider": "test" }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.subagents().register_provider(Arc::new(Provider {
            capabilities: SubagentCapabilities::default(),
            inherits: false,
        }))
    }));
    assert!(result.is_err());
    assert!(env.subagents().get_provider("test").is_none());
    assert!(env.tools().get("subagent", None, None).is_none());
    env.close().await;
}

#[tokio::test]
async fn model_selection_setting_records_fresh_policy_and_child_inherits_it() {
    let env = Env::new().await;
    env.ctx
        .plugin::<SubagentModelSelectionSettings>(json!({
            "enabled": true,
            "allowedModels": [
                { "provider": "mock", "model": "fast" },
                { "provider": "mock", "model": "deep" }
            ]
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let service = env.ctx.get::<SubagentModelSelection>().unwrap();
    assert_eq!(service.current().unwrap().unwrap().len(), 2);

    let parent = env
        .harness
        .create(SessionId::from("policy-parent"))
        .await
        .unwrap();
    let parent_events = parent.session().snapshot_events(None, None);
    let parent_policy = parent_events
        .iter()
        .filter(|event| event.event_type() == "subagent/model-selection-policy")
        .collect::<Vec<_>>();
    assert_eq!(parent_policy.len(), 1);
    assert_eq!(
        parent_policy[0].data()["allowedModels"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let child = env
        .harness
        .create_with(
            SessionId::from("policy-child"),
            Default::default(),
            AgentSessionMeta {
                origin: Some("subagent".into()),
                parent_session: Some(parent.id().clone()),
                delegation_depth: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let child_events = child.session().snapshot_events(None, None);
    let child_policy = child_events
        .iter()
        .filter(|event| event.event_type() == "subagent/model-selection-policy")
        .collect::<Vec<_>>();
    assert_eq!(child_policy.len(), 1);
    assert_eq!(child_policy[0].data(), parent_policy[0].data());
    env.close().await;
}
