//! Shared scripted child boundary and environment for the execution tests.
#![allow(dead_code)]
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, AgentLoopTestHarness, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_session::SessionId;
use hoh_subagent::{
    ResolvedSubagentStartRequest, SubagentCapabilities, SubagentPlugin, SubagentProvider,
    SubagentResult, SubagentRun, Subagents,
};
use hoh_timeout::AbortController;
use hoh_tool_subagent::ToolSubagent;
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::{ToolExecutionInput, Tools};
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::Notify;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Clone, Debug)]
pub struct Seen {
    pub cwd: String,
    pub label: Option<String>,
    pub max_depth: Option<u64>,
    pub has_agent_options: bool,
    /// provider, model, reasoning effort, max tokens of the requested child options.
    pub options: (Option<String>, Option<String>, Option<String>, Option<u64>),
}

/// Scripted child boundary: records each start, then finishes when `gate` opens.
pub struct Scripted {
    pub name: String,
    pub capabilities: SubagentCapabilities,
    pub inherits: bool,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    pub started: Arc<AtomicUsize>,
    pub gate: Option<Arc<Notify>>,
    pub open: Arc<AtomicUsize>,
    pub expect_started: usize,
    pub defaults: Option<(String, String)>,
}

impl Scripted {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            capabilities: SubagentCapabilities {
                agent_options: true,
                depth_limit: true,
                output_schema: true,
                ..Default::default()
            },
            inherits: false,
            seen: Arc::default(),
            started: Arc::default(),
            gate: None,
            open: Arc::default(),
            expect_started: 0,
            defaults: None,
        }
    }
}

pub struct Run {
    pub id: SessionId,
}

impl SubagentRun for Run {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn result(&self) -> BoxFuture<Result<SubagentResult, BoxError>> {
        Box::pin(async {
            Ok(SubagentResult::completed(vec![
                hoh_llm::ContentBlock::text("scripted subagent reply"),
            ]))
        })
    }
    fn dispose(&self) -> BoxFuture<Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
}

impl SubagentProvider for Scripted {
    fn name(&self) -> &str {
        &self.name
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
    fn agent_route_defaults(&self) -> Option<(&str, &str)> {
        self.defaults
            .as_ref()
            .map(|(provider, model)| (provider.as_str(), model.as_str()))
    }
    fn start(
        &self,
        request: ResolvedSubagentStartRequest,
    ) -> Option<BoxFuture<Result<Arc<dyn SubagentRun>, BoxError>>> {
        let count = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        self.seen.lock().unwrap().push(Seen {
            cwd: request.cwd.clone(),
            label: request.label.clone(),
            max_depth: request.max_depth,
            has_agent_options: request.agent_options.is_some(),
            options: request
                .agent_options
                .as_ref()
                .map_or_else(Default::default, |options| {
                    (
                        options.provider(),
                        options.model(),
                        options.reasoning_effort().map(|effort| effort.to_string()),
                        options.max_tokens(),
                    )
                }),
        });
        let gate = self.gate.clone();
        let open = self.open.clone();
        let expect = self.expect_started;
        let parent = request.parent.id().clone();
        Some(Box::pin(async move {
            // Children that must overlap wait until all of them have started.
            if let Some(gate) = gate {
                if count >= expect {
                    open.store(1, Ordering::SeqCst);
                    gate.notify_waiters();
                }
                loop {
                    let notified = gate.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    if open.load(Ordering::SeqCst) != 0 {
                        break;
                    }
                    notified.await;
                }
            }
            Ok(Arc::new(Run {
                id: SessionId::from(format!("scripted-subagent:{parent}:{count}")),
            }) as Arc<dyn SubagentRun>)
        }))
    }
}

pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    pub harness: AgentLoopTestHarness,
}

impl Env {
    pub async fn new() -> Self {
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
    pub fn tools(&self) -> hoh_kernel::Service<Tools> {
        self.ctx.get::<Tools>().unwrap()
    }
    pub fn subagents(&self) -> hoh_kernel::Service<Subagents> {
        self.ctx.get::<Subagents>().unwrap()
    }
    pub async fn mount(&self, config: Value) {
        self.ctx
            .plugin::<ToolSubagent>(config)
            .unwrap()
            .settled()
            .await
            .unwrap();
    }
    pub async fn call(
        &self,
        name: &str,
        args: Value,
        agent: Option<Arc<dyn hoh_agent::Agent>>,
    ) -> hoh_tools::ToolExecutionResult {
        self.call_with(name, args, agent, AbortController::new().signal())
            .await
    }
    pub async fn call_with(
        &self,
        name: &str,
        args: Value,
        agent: Option<Arc<dyn hoh_agent::Agent>>,
        signal: hoh_timeout::AbortSignal,
    ) -> hoh_tools::ToolExecutionResult {
        let mut input = ToolExecutionInput::new(
            format!("call-{name}-{}", rand_id()).into(),
            name,
            args,
            signal,
        );
        input.agent = agent;
        execute_tool_fixture(&self.ctx, input).await.unwrap()
    }
    pub async fn close(self) {
        self.kernel.shutdown().await;
    }
}

pub fn rand_id() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

pub fn text(result: &hoh_tools::ToolExecutionResult) -> String {
    result
        .content()
        .iter()
        .filter_map(hoh_llm::ContentBlock::text_value)
        .collect()
}

/// Adapter whose model resolution (the route preflight) waits for an explicit release.
pub struct GateAdapter {
    gate: Notify,
    open: AtomicBool,
    pub resolving: AtomicUsize,
    pub fail: AtomicBool,
}

impl GateAdapter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            gate: Notify::new(),
            open: AtomicBool::new(false),
            resolving: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        })
    }
    pub fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
        self.gate.notify_waiters();
    }
    pub async fn until_resolving(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while self.resolving.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the preflight never reached the adapter");
    }
}

impl hoh_llm::LlmAdapter for GateAdapter {
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> hoh_llm::LlmFuture<'a, hoh_llm::LlmResolvedModelInfo> {
        Box::pin(async move {
            self.resolving.fetch_add(1, Ordering::SeqCst);
            loop {
                let notified = self.gate.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.open.load(Ordering::SeqCst) {
                    break;
                }
                notified.await;
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err("selected route unavailable".into());
            }
            Ok(hoh_llm::LlmResolvedModelInfo {
                model: hoh_llm::LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: model.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
        })
    }
    fn stream(&self, _options: hoh_llm::GenerateOptions) -> Result<hoh_llm::LlmStream, BoxError> {
        Err("the gate adapter does not stream".into())
    }
}

impl Env {
    pub async fn with_selection_policy(&self) {
        self.ctx
            .plugin::<hoh_tool_subagent::model_selection_settings::SubagentModelSelectionSettings>(
                json!({
                    "enabled": true,
                    "allowedModels": [
                        { "provider": "alpha", "model": "selected-model" },
                        { "provider": "alpha", "model": "child-model" },
                    ]
                }),
            )
            .unwrap()
            .settled()
            .await
            .unwrap();
    }
    pub fn adapter(&self, adapter: Arc<GateAdapter>) {
        self.ctx
            .get::<hoh_llm::Llm>()
            .unwrap()
            .register_adapter(&self.ctx, vec!["alpha".into()], adapter)
            .unwrap();
    }
}

type ListHook = Box<dyn Fn() + Send + Sync>;

/// Catalog adapter of `list-models.spec.ts`: two advertised models, resolution with reasoning
/// metadata for `fast` and without for `plain`, plus failure and cancellation hooks.
#[derive(Default)]
pub struct CatalogAdapter {
    pub empty: bool,
    pub list_calls: AtomicUsize,
    pub resolve_calls: AtomicUsize,
    pub fail_list: Mutex<Option<String>>,
    pub fail_resolve: Mutex<Option<String>>,
    pub on_list: Mutex<Option<ListHook>>,
}

impl CatalogAdapter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            empty: true,
            ..Default::default()
        })
    }
}

impl hoh_llm::LlmAdapter for CatalogAdapter {
    fn provider_info(&self, provider: &str) -> Result<hoh_llm::LlmProviderInfo, BoxError> {
        Ok(hoh_llm::LlmProviderInfo {
            id: provider.into(),
            name: format!("{} API", provider.to_uppercase()),
        })
    }
    fn list_models<'a>(
        &'a self,
        provider: &'a str,
    ) -> hoh_llm::LlmFuture<'a, Vec<hoh_llm::LlmModelInfo>> {
        Box::pin(async move {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(hook) = self.on_list.lock().unwrap().as_ref() {
                hook();
            }
            if let Some(message) = self.fail_list.lock().unwrap().clone() {
                return Err(message.into());
            }
            if self.empty {
                return Ok(vec![]);
            }
            Ok(vec![
                hoh_llm::LlmModelInfo {
                    provider: provider.into(),
                    id: "fast".into(),
                    name: "Fast".into(),
                    description: Some("Focused work.".into()),
                    ..Default::default()
                },
                hoh_llm::LlmModelInfo {
                    provider: provider.into(),
                    id: "plain".into(),
                    name: "Plain".into(),
                    ..Default::default()
                },
            ])
        })
    }
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> hoh_llm::LlmFuture<'a, hoh_llm::LlmResolvedModelInfo> {
        Box::pin(async move {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(message) = self.fail_resolve.lock().unwrap().clone() {
                return Err(message.into());
            }
            let effort =
                |id: &str, name: &str, description: Option<&str>| hoh_llm::LlmReasoningEffortInfo {
                    id: id.into(),
                    name: name.into(),
                    description: description.map(Into::into),
                };
            if model == "plain" {
                return Ok(hoh_llm::LlmResolvedModelInfo {
                    model: hoh_llm::LlmModelInfo {
                        provider: provider.into(),
                        id: model.into(),
                        name: "Plain".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                });
            }
            Ok(hoh_llm::LlmResolvedModelInfo {
                model: hoh_llm::LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: "Fast".into(),
                    description: Some("Focused work.".into()),
                    ..Default::default()
                },
                reasoning: Some(hoh_llm::LlmModelReasoningInfo {
                    efforts: vec![
                        effort("low", "Low", None),
                        effort("high", "High", Some("Quality first.")),
                    ],
                    default_effort: Some("high".into()),
                }),
                ..Default::default()
            })
        })
    }
    fn stream(&self, _options: hoh_llm::GenerateOptions) -> Result<hoh_llm::LlmStream, BoxError> {
        Err("the catalog adapter does not stream".into())
    }
}

impl Env {
    /// `setupListTool`: a recorded Session allowlist and a discovery owner.
    pub async fn with_routes(&self, routes: Value) {
        self.ctx
            .plugin::<hoh_tool_subagent::model_selection_settings::SubagentModelSelectionSettings>(
                json!({ "enabled": true, "allowedModels": routes }),
            )
            .unwrap()
            .settled()
            .await
            .unwrap();
    }
    pub fn catalog(
        &self,
        providers: &[&str],
        adapter: Arc<CatalogAdapter>,
    ) -> hoh_llm::AdapterRegistrationHandle {
        self.ctx
            .get::<hoh_llm::Llm>()
            .unwrap()
            .register_adapter(
                &self.ctx,
                providers.iter().map(|name| name.to_string()).collect(),
                adapter,
            )
            .unwrap()
    }
}

/// Kernel without the `llm` service: discovery stays registered but its calls are rejected.
pub struct NoLlmEnv {
    pub kernel: Kernel,
    pub ctx: Context,
}

impl NoLlmEnv {
    pub async fn new() -> Self {
        Self::new_with(json!({ "provider": "unused", "modelDiscovery": true })).await
    }
    pub async fn new_with(tool_config: Value) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        async fn mount<P: hoh_kernel::Plugin>(ctx: &Context) {
            ctx.plugin::<P>(json!({})).unwrap().settled().await.unwrap();
        }
        mount::<hoh_session::SessionPlugin>(&ctx).await;
        mount::<hoh_session_projection::SessionProjectionRegistry>(&ctx).await;
        mount::<hoh_system_prompt::SystemPrompt>(&ctx).await;
        mount::<hoh_fs_local::FsLocalPlugin>(&ctx).await;
        mount::<hoh_working_directory::WorkingDirectory>(&ctx).await;
        mount::<hoh_tools::ToolRuntime>(&ctx).await;
        mount::<hoh_tool_executor_native::NativeToolExecutorPlugin>(&ctx).await;
        mount::<hoh_agent::AgentRegistryPlugin>(&ctx).await;
        mount::<SubagentPlugin>(&ctx).await;
        assert!(ctx.get::<hoh_llm::Llm>().is_none());
        ctx.plugin::<ToolSubagent>(tool_config)
            .unwrap()
            .settled()
            .await
            .unwrap();
        Self { kernel, ctx }
    }
    pub fn tools(&self) -> hoh_kernel::Service<Tools> {
        self.ctx.get::<Tools>().unwrap()
    }
    pub fn subagents(&self) -> hoh_kernel::Service<Subagents> {
        self.ctx.get::<Subagents>().unwrap()
    }
    pub async fn call(
        &self,
        agent: Option<Arc<dyn hoh_agent::Agent>>,
    ) -> hoh_tools::ToolExecutionResult {
        self.call_tool("list_subagent_models", json!({}), agent)
            .await
    }
    pub async fn call_tool(
        &self,
        name: &str,
        args: Value,
        agent: Option<Arc<dyn hoh_agent::Agent>>,
    ) -> hoh_tools::ToolExecutionResult {
        let mut input = ToolExecutionInput::new(
            format!("no-llm-{}", rand_id()).into(),
            name,
            args,
            AbortController::new().signal(),
        );
        input.agent = agent;
        execute_tool_fixture(&self.ctx, input).await.unwrap()
    }
}
