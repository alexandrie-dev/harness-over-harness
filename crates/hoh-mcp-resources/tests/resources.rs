use std::sync::{Arc, Mutex, MutexGuard};

use hoh_agent::{
    AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, InternalPlugin, Kernel, Realm};
use hoh_llm::{ToolCallId, UserMessage};
use hoh_mcp_resources::{
    McpResourceFuture, McpResourceProvider, McpResourceRequest, McpResourceRuntime, McpResources,
    render_resource_result,
};
use hoh_scope::{CreateScopeOptions, ScopeKey, bind_scope_parent, create_scope, scope_of};
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_system_prompt::{AssembleContext, SystemPrompt, SystemPromptKey, render_prompt};
use hoh_timeout::AbortController;
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::shim::BoxFuture;
use hoh_tools::{
    Agent, DefineToolOptions, DefineToolOutput, ToolExecution, ToolExecutionInput,
    ToolExecutionResult, ToolRegistrationOptions, ToolRuntime, Tools, define_tool,
};
use serde_json::{Value, json};

const TOOL_NAMES: [&str; 3] = [
    "list_mcp_resource_templates",
    "list_mcp_resources",
    "read_mcp_resource",
];

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    resources_fiber: Fiber,
}

async fn setup() -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<NativeToolExecutorPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let resources_fiber = ctx.plugin::<McpResourceRuntime>(json!({})).unwrap();
    resources_fiber.settled().await.unwrap();
    Harness {
        kernel,
        ctx,
        resources_fiber,
    }
}

#[derive(Clone)]
struct Probe {
    calls: Arc<Mutex<Vec<McpResourceRequest>>>,
    response: Value,
    failure: Option<String>,
}

impl Probe {
    fn responding(response: Value) -> Self {
        Self {
            calls: Arc::default(),
            response,
            failure: None,
        }
    }

    fn failing(message: &str) -> Self {
        Self {
            calls: Arc::default(),
            response: Value::Null,
            failure: Some(message.to_owned()),
        }
    }
}

impl McpResourceProvider for Probe {
    fn request<'a>(
        &'a self,
        request: McpResourceRequest,
        exec: Arc<ToolExecution>,
    ) -> McpResourceFuture<'a> {
        lock(&self.calls).push(request);
        let response = self.response.clone();
        let failure = self.failure.clone();
        Box::pin(async move {
            assert!(!exec.signal().aborted());
            match failure {
                Some(message) => Err(message.into()),
                None => Ok(response),
            }
        })
    }
}

struct TestAgent {
    key: ScopeKey,
    session: Session,
    ctx: Context,
    options: AgentOptions,
}

impl TestAgent {
    fn new(ctx: &Context, id: &str, key: ScopeKey) -> Arc<Self> {
        Arc::new(Self {
            key,
            session: Session::create(id.into(), None, None, None, None).unwrap(),
            ctx: ctx.clone(),
            options: AgentOptions::default(),
        })
    }
}

impl Agent for TestAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }

    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn model(&self) -> &dyn Model {
        panic!("the resource tools do not read the model")
    }

    fn inbox(&self) -> &dyn Inbox {
        panic!("the resource tools do not read the inbox")
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        panic!("the resource tools do not cancel the agent")
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("the resource tools do not run maintenance")
    }

    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("the resource tools do not send input")
    }

    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the resource tools do not send input")
    }

    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the resource tools do not send input")
    }

    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the resource tools do not send input")
    }
}

fn service(ctx: &Context) -> hoh_kernel::Service<McpResources> {
    ctx.get::<McpResources>().unwrap()
}

fn visible_tools(ctx: &Context, scope: Option<&ScopeKey>) -> Vec<String> {
    let mut names: Vec<_> = ctx
        .get::<Tools>()
        .unwrap()
        .schemas(scope, None)
        .unwrap()
        .into_iter()
        .map(|schema| schema.name)
        .filter(|name| TOOL_NAMES.contains(&name.as_str()))
        .collect();
    names.sort();
    names
}

async fn prompt(ctx: &Context, scope: Option<ScopeKey>) -> String {
    let assembly = ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(AssembleContext::new(scope))
        .await
        .unwrap();
    render_prompt(&assembly).unwrap()
}

async fn call(
    ctx: &Context,
    name: &str,
    arguments: Value,
    agent: Option<Arc<dyn Agent>>,
) -> ToolExecutionResult {
    let mut input = ToolExecutionInput::new(
        ToolCallId::from("resource-test"),
        name,
        arguments,
        AbortController::new().signal(),
    );
    input.agent = agent;
    execute_tool_fixture(ctx, input).await.unwrap()
}

fn success_value(result: &ToolExecutionResult) -> &Value {
    match result {
        ToolExecutionResult::Success(success) => success.value(),
        ToolExecutionResult::Failure(failure) => {
            panic!("unexpected failure: {:?}", failure.error())
        }
    }
}

fn assert_unknown_tool(result: &ToolExecutionResult) {
    let error = result.error().expect("tool call must fail");
    assert_eq!(error.info.as_ref().unwrap().name, "ToolNotFoundError");
    assert_eq!(error.info.as_ref().unwrap().code, "UNKNOWN_TOOL");
}

async fn assert_resource_tools_unknown(ctx: &Context, agent: Option<Arc<dyn Agent>>) {
    for (name, arguments) in [
        ("list_mcp_resources", json!({ "server": "missing" })),
        (
            "list_mcp_resource_templates",
            json!({ "server": "missing" }),
        ),
        (
            "read_mcp_resource",
            json!({ "server": "missing", "uri": "docs://guide" }),
        ),
    ] {
        assert_unknown_tool(&call(ctx, name, arguments, agent.clone()).await);
    }
}

#[test]
fn request_accessors_match_protocol_methods() {
    let list = McpResourceRequest::List {
        cursor: Some("opaque".into()),
    };
    assert_eq!(list.method(), "resources/list");
    assert_eq!(list.cursor(), Some("opaque"));
    let templates = McpResourceRequest::ListTemplates { cursor: None };
    assert_eq!(templates.method(), "resources/templates/list");
    assert_eq!(templates.cursor(), None);
    let read = McpResourceRequest::Read {
        uri: "docs://guide".into(),
    };
    assert_eq!(read.method(), "resources/read");
    assert_eq!(read.cursor(), None);
}

#[test]
fn renderer_redacts_string_blobs_recursively_without_mutating_the_value() {
    let value = json!({
        "contents": [
            { "uri": "docs://text", "text": "Read this guide." },
            { "uri": "docs://binary", "blob": "AQIDBA==" },
            { "blob": 7 },
            { "nested": { "blob": "😀" } }
        ]
    });
    let before = value.clone();
    let rendered = render_resource_result("docs", &value);
    assert_eq!(rendered.len(), 1);
    assert_eq!(
        rendered[0].text_value().unwrap(),
        "MCP server: docs\n{\"contents\":[{\"uri\":\"docs://text\",\"text\":\"Read this guide.\"},{\"uri\":\"docs://binary\",\"blob\":\"[binary resource: 8 base64 characters; available to programmatic callers]\"},{\"blob\":7},{\"nested\":{\"blob\":\"[binary resource: 2 base64 characters; available to programmatic callers]\"}}]}"
    );
    assert_eq!(value, before);
}

#[test]
fn renderer_uses_ecmascript_number_spelling() {
    let value = json!({ "small": 1e-7, "large": 1e21, "ordinary": 1e20 });
    assert_eq!(
        render_resource_result("numbers", &value)[0]
            .text_value()
            .unwrap(),
        "MCP server: numbers\n{\"small\":1e-7,\"large\":1e+21,\"ordinary\":100000000000000000000}"
    );
}

#[tokio::test]
async fn no_servers_contribute_no_tools_or_prompt_text() {
    let h = setup().await;
    assert!(visible_tools(&h.ctx, None).is_empty());
    assert!(!prompt(&h.ctx, None).await.to_lowercase().contains("mcp"));
    assert_resource_tools_unknown(&h.ctx, None).await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn system_prompt_is_an_optional_late_bound_dependency() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<NativeToolExecutorPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();

    let without_prompt = root.isolate("systemPrompt", Realm::new());
    assert!(!without_prompt.has("systemPrompt"));
    let resources = without_prompt
        .plugin::<McpResourceRuntime>(json!({}))
        .unwrap();
    resources.settled().await.unwrap();
    let runtime = without_prompt.get::<McpResources>().unwrap();
    runtime
        .register(
            &without_prompt,
            "docs",
            Arc::new(Probe::responding(json!({}))),
        )
        .unwrap();
    assert_eq!(visible_tools(&root, None), TOOL_NAMES);
    assert!(!prompt(&root, None).await.contains("MCP resource servers"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn shared_tools_live_until_the_last_provider_and_allow_replacement() {
    let h = setup().await;
    let runtime = service(&h.ctx);
    let first = runtime
        .register(&h.ctx, "first", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    let second = runtime
        .register(&h.ctx, "second", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    assert_eq!(visible_tools(&h.ctx, None), TOOL_NAMES);
    drop(first.dispose());
    assert_eq!(visible_tools(&h.ctx, None), TOOL_NAMES);
    let remaining = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "second" }),
        None,
    )
    .await;
    assert_eq!(success_value(&remaining), &json!({}));
    drop(second.dispose());
    assert!(visible_tools(&h.ctx, None).is_empty());

    let replacement = runtime
        .register(&h.ctx, "second", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    assert_eq!(visible_tools(&h.ctx, None), TOOL_NAMES);
    drop(replacement.dispose());
    assert!(visible_tools(&h.ctx, None).is_empty());

    let reverse_first = runtime
        .register(
            &h.ctx,
            "first",
            Arc::new(Probe::responding(json!({ "source": "first" }))),
        )
        .unwrap();
    let reverse_second = runtime
        .register(&h.ctx, "second", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    drop(reverse_second.dispose());
    assert_eq!(visible_tools(&h.ctx, None), TOOL_NAMES);
    let remaining = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "first" }),
        None,
    )
    .await;
    assert_eq!(success_value(&remaining), &json!({ "source": "first" }));
    drop(reverse_first.dispose());
    assert!(visible_tools(&h.ctx, None).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn independent_owners_in_one_scope_share_the_same_tool_lease() {
    let h = setup().await;
    let key = ScopeKey::new();
    let first_owner = create_scope(&h.ctx, key.clone(), Default::default()).unwrap();
    let second_owner = create_scope(&h.ctx, key.clone(), Default::default()).unwrap();
    let runtime = service(&h.ctx);
    let first = runtime
        .register(
            &first_owner.ctx,
            "first",
            Arc::new(Probe::responding(json!({}))),
        )
        .unwrap();
    let second = runtime
        .register(
            &second_owner.ctx,
            "second",
            Arc::new(Probe::responding(json!({}))),
        )
        .unwrap();
    assert_eq!(visible_tools(&h.ctx, Some(&key)), TOOL_NAMES);
    drop(first.dispose());
    assert_eq!(visible_tools(&h.ctx, Some(&key)), TOOL_NAMES);
    let remaining = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "second" }),
        Some(TestAgent::new(&h.ctx, "same-scope", key.clone())),
    )
    .await;
    assert_eq!(success_value(&remaining), &json!({}));
    drop(second.dispose());
    assert!(visible_tools(&h.ctx, Some(&key)).is_empty());

    let reverse_first = runtime
        .register(
            &first_owner.ctx,
            "first",
            Arc::new(Probe::responding(json!({ "source": "first" }))),
        )
        .unwrap();
    let reverse_second = runtime
        .register(
            &second_owner.ctx,
            "second",
            Arc::new(Probe::responding(json!({}))),
        )
        .unwrap();
    drop(reverse_second.dispose());
    assert_eq!(visible_tools(&h.ctx, Some(&key)), TOOL_NAMES);
    let remaining = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "first" }),
        Some(TestAgent::new(&h.ctx, "same-scope-reverse", key.clone())),
    )
    .await;
    assert_eq!(success_value(&remaining), &json!({ "source": "first" }));
    drop(reverse_first.dispose());
    assert!(visible_tools(&h.ctx, Some(&key)).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn provider_disposal_waits_for_the_owned_scoped_tool_fiber() {
    let h = setup().await;
    let key = ScopeKey::new();
    let owner = create_scope(&h.ctx, key.clone(), Default::default()).unwrap();
    owner.ctx.fiber().settled().await.unwrap();
    let captured = Arc::new(Mutex::new(None::<Fiber>));
    let captured_by_listener = captured.clone();
    let observer = h
        .ctx
        .on::<InternalPlugin>(move |event| {
            if event.fiber.name().as_ref() == "scope"
                && !event.fiber.is_disposed()
                && lock(&captured_by_listener).is_none()
            {
                *lock(&captured_by_listener) = Some(event.fiber.clone());
            }
        })
        .unwrap();
    let remove = service(&h.ctx)
        .register(&owner.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    for _ in 0..32 {
        if lock(&captured).is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    let tool_scope = lock(&captured)
        .clone()
        .expect("runtime-owned tool scope was created");
    let disposing = remove.dispose();
    assert!(visible_tools(&h.ctx, Some(&key)).is_empty());
    assert!(!tool_scope.is_disposed());
    disposing.await;
    assert!(tool_scope.is_disposed());
    drop(observer.dispose());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn duplicate_server_is_rejected_without_disturbing_the_owner() {
    let h = setup().await;
    let runtime = service(&h.ctx);
    let owner = runtime
        .register(&h.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    let error = runtime
        .register(&h.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "MCP resource server \"docs\" is already registered in this scope"
    );
    assert_eq!(visible_tools(&h.ctx, None), TOOL_NAMES);
    drop(owner.dispose());

    let key = ScopeKey::new();
    let scoped = create_scope(&h.ctx, key.clone(), Default::default()).unwrap();
    let scoped_owner = runtime
        .register(
            &scoped.ctx,
            "docs",
            Arc::new(Probe::responding(json!({ "source": "owner" }))),
        )
        .unwrap();
    let error = runtime
        .register(
            &scoped.ctx,
            "docs",
            Arc::new(Probe::responding(json!({ "source": "duplicate" }))),
        )
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "MCP resource server \"docs\" is already registered in this scope"
    );
    let result = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "docs" }),
        Some(TestAgent::new(&h.ctx, "scoped-owner", key.clone())),
    )
    .await;
    assert_eq!(success_value(&result), &json!({ "source": "owner" }));
    drop(scoped_owner.dispose());
    assert!(visible_tools(&h.ctx, Some(&key)).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn routes_all_operations_with_opaque_arguments_and_marks_them_parallel_safe() {
    let h = setup().await;
    let probe = Probe::responding(json!({ "resources": [], "nextCursor": "next" }));
    service(&h.ctx)
        .register(&h.ctx, "docs", Arc::new(probe.clone()))
        .unwrap();
    let tools = h.ctx.get::<Tools>().unwrap();
    for name in TOOL_NAMES {
        let definition = tools.get(name, None, None).unwrap();
        assert!(definition.is_concurrency_safe.as_ref().unwrap()(&json!({
            "server": "docs",
            "uri": "docs://guide"
        })));
    }

    let listed = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "docs", "cursor": "page-2" }),
        None,
    )
    .await;
    assert_eq!(
        success_value(&listed),
        &json!({ "resources": [], "nextCursor": "next" })
    );
    call(
        &h.ctx,
        "list_mcp_resource_templates",
        json!({ "server": "docs", "cursor": "template-page" }),
        None,
    )
    .await;
    call(
        &h.ctx,
        "read_mcp_resource",
        json!({ "server": "docs", "uri": "docs://guide" }),
        None,
    )
    .await;
    assert_eq!(
        *lock(&probe.calls),
        vec![
            McpResourceRequest::List {
                cursor: Some("page-2".into())
            },
            McpResourceRequest::ListTemplates {
                cursor: Some("template-page".into())
            },
            McpResourceRequest::Read {
                uri: "docs://guide".into()
            }
        ]
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn binary_value_stays_canonical_while_model_content_is_redacted() {
    let h = setup().await;
    let value = json!({ "contents": [
        { "uri": "docs://text", "text": "Read this guide." },
        { "uri": "docs://binary", "blob": "AQIDBA==" }
    ] });
    service(&h.ctx)
        .register(&h.ctx, "docs", Arc::new(Probe::responding(value.clone())))
        .unwrap();
    let result = call(
        &h.ctx,
        "read_mcp_resource",
        json!({ "server": "docs", "uri": "docs://text" }),
        None,
    )
    .await;
    assert_eq!(success_value(&result), &value);
    let text = result.content()[0].text_value().unwrap();
    assert!(text.starts_with("MCP server: docs\n"));
    assert!(text.contains("binary resource: 8 base64 characters"));
    assert!(!text.contains("AQIDBA=="));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn missing_parameters_and_unavailable_servers_fail_before_dispatch() {
    let h = setup().await;
    let probe = Probe::responding(json!({}));
    service(&h.ctx)
        .register(&h.ctx, "docs", Arc::new(probe.clone()))
        .unwrap();
    let missing = call(
        &h.ctx,
        "read_mcp_resource",
        json!({ "uri": "docs://text" }),
        None,
    )
    .await;
    assert!(missing.is_error());
    let unavailable = call(
        &h.ctx,
        "read_mcp_resource",
        json!({ "server": "missing", "uri": "docs://text" }),
        None,
    )
    .await;
    let error = unavailable.error().unwrap();
    assert_eq!(
        error.message,
        "invalid arguments: server: MCP resource server \"missing\" is unavailable in this agent's scope"
    );
    assert_eq!(error.info.as_ref().unwrap().name, "ToolArgsError");
    assert_eq!(error.info.as_ref().unwrap().code, "INVALID_ARGS");
    assert!(lock(&probe.calls).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn scoped_provider_shadows_global_for_owner_and_descendants() {
    let h = setup().await;
    let runtime = service(&h.ctx);
    let global = Probe::responding(json!({ "source": "global" }));
    let local = Probe::responding(json!({ "source": "local" }));
    let global_dispose = runtime
        .register(&h.ctx, "docs", Arc::new(global.clone()))
        .unwrap();
    let owner_key = ScopeKey::new();
    let child_key = ScopeKey::new();
    bind_scope_parent(&child_key, owner_key.clone()).unwrap();
    let owner_scope =
        create_scope(&h.ctx, owner_key.clone(), CreateScopeOptions::default()).unwrap();
    let local_dispose = runtime
        .register(&owner_scope.ctx, "docs", Arc::new(local.clone()))
        .unwrap();
    let owner = TestAgent::new(&h.ctx, "owner", owner_key.clone());
    let child = TestAgent::new(&h.ctx, "child", child_key.clone());
    let other_key = ScopeKey::new();
    let other = TestAgent::new(&h.ctx, "other", other_key.clone());
    assert_eq!(visible_tools(&h.ctx, Some(&owner_key)), TOOL_NAMES);
    assert_eq!(visible_tools(&h.ctx, Some(&child_key)), TOOL_NAMES);
    for agent in [owner.clone(), child.clone()] {
        let result = call(
            &h.ctx,
            "list_mcp_resources",
            json!({ "server": "docs" }),
            Some(agent),
        )
        .await;
        assert_eq!(success_value(&result), &json!({ "source": "local" }));
    }
    let result = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "docs" }),
        Some(other),
    )
    .await;
    assert_eq!(success_value(&result), &json!({ "source": "global" }));
    drop(local_dispose.dispose());
    let inherited = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "docs" }),
        Some(owner),
    )
    .await;
    assert_eq!(success_value(&inherited), &json!({ "source": "global" }));
    assert_eq!(lock(&local.calls).len(), 2);
    assert_eq!(lock(&global.calls).len(), 2);

    drop(global_dispose.dispose());
    assert!(visible_tools(&h.ctx, Some(&owner_key)).is_empty());
    assert!(visible_tools(&h.ctx, Some(&child_key)).is_empty());
    assert!(visible_tools(&h.ctx, Some(&other_key)).is_empty());
    assert!(
        !prompt(&h.ctx, Some(owner_key.clone()))
            .await
            .contains("MCP resource servers")
    );

    runtime
        .register(
            &owner_scope.ctx,
            "docs",
            Arc::new(Probe::responding(json!({ "source": "replacement" }))),
        )
        .unwrap();
    assert_eq!(visible_tools(&h.ctx, Some(&owner_key)), TOOL_NAMES);
    assert_eq!(visible_tools(&h.ctx, Some(&child_key)), TOOL_NAMES);
    owner_scope.dispose().await;
    owner_scope.dispose().await;
    assert!(visible_tools(&h.ctx, Some(&owner_key)).is_empty());
    assert!(visible_tools(&h.ctx, Some(&child_key)).is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn scoped_tools_are_invisible_to_unrelated_callers_without_global_servers() {
    let h = setup().await;
    let owner_key = ScopeKey::new();
    let owner_scope = create_scope(&h.ctx, owner_key.clone(), Default::default()).unwrap();
    service(&h.ctx)
        .register(
            &owner_scope.ctx,
            "private",
            Arc::new(Probe::responding(json!({}))),
        )
        .unwrap();
    assert_eq!(visible_tools(&h.ctx, Some(&owner_key)), TOOL_NAMES);
    assert!(visible_tools(&h.ctx, None).is_empty());
    let unrelated_key = ScopeKey::new();
    assert!(visible_tools(&h.ctx, Some(&unrelated_key)).is_empty());
    assert!(!prompt(&h.ctx, None).await.contains("MCP resource servers"));
    assert!(
        !prompt(&h.ctx, Some(unrelated_key.clone()))
            .await
            .contains("private")
    );
    assert_resource_tools_unknown(
        &h.ctx,
        Some(TestAgent::new(&h.ctx, "unrelated", unrelated_key)),
    )
    .await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn prompt_lists_only_visible_names_in_utf16_order_and_preserves_literals() {
    let h = setup().await;
    let runtime = service(&h.ctx);
    for name in ["😀", "\u{E000}", "docs{{literal}}"] {
        runtime
            .register(&h.ctx, name, Arc::new(Probe::responding(json!({}))))
            .unwrap();
    }
    assert!(
        prompt(&h.ctx, None)
            .await
            .contains("server argument: [\"docs{{literal}}\",\"😀\",\"\"].")
    );

    let owner_key = ScopeKey::new();
    let owner_scope = create_scope(&h.ctx, owner_key.clone(), Default::default()).unwrap();
    let shadow = runtime
        .register(
            &owner_scope.ctx,
            "docs{{literal}}",
            Arc::new(Probe::responding(json!({}))),
        )
        .unwrap();
    let private = runtime
        .register(
            &owner_scope.ctx,
            "private",
            Arc::new(Probe::responding(json!({}))),
        )
        .unwrap();
    let owner_prompt = prompt(&h.ctx, Some(owner_key.clone())).await;
    assert!(
        owner_prompt.contains("server argument: [\"docs{{literal}}\",\"private\",\"😀\",\"\"].")
    );
    assert_eq!(owner_prompt.matches("\"docs{{literal}}\"").count(), 1);
    assert!(
        !prompt(&h.ctx, Some(ScopeKey::new()))
            .await
            .contains("private")
    );
    drop(private.dispose());
    drop(shadow.dispose());
    assert!(!prompt(&h.ctx, Some(owner_key)).await.contains("private"));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn provider_failure_does_not_withdraw_tools_or_prompt_name() {
    let h = setup().await;
    service(&h.ctx)
        .register(
            &h.ctx,
            "docs",
            Arc::new(Probe::failing("MCP server disconnected")),
        )
        .unwrap();
    let result = call(
        &h.ctx,
        "list_mcp_resources",
        json!({ "server": "docs" }),
        None,
    )
    .await;
    assert!(result.is_error());
    assert_eq!(visible_tools(&h.ctx, None), TOOL_NAMES);
    assert!(
        prompt(&h.ctx, None)
            .await
            .contains("server argument: [\"docs\"]")
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn partial_tool_registration_rolls_back_provider_and_first_tool() {
    let h = setup().await;
    let tools = h.ctx.get::<Tools>().unwrap();
    let conflict = define_tool(DefineToolOptions::new(
        "list_mcp_resource_templates",
        "Conflict",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "null" }),
            render: Arc::new(|_, _| Vec::new()),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(Value::Null) }),
    ))
    .unwrap();
    let remove_conflict = tools
        .register(&h.ctx, conflict, ToolRegistrationOptions::default())
        .unwrap();
    let error = service(&h.ctx)
        .register(&h.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "tool \"list_mcp_resource_templates\" is already registered (for a per-agent variant, register through that agent's `agent.ctx` instead)"
    );
    assert!(tools.get("list_mcp_resources", None, None).is_none());
    assert!(!prompt(&h.ctx, None).await.contains("MCP resource servers"));
    drop(remove_conflict.dispose());
    service(&h.ctx)
        .register(&h.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    assert_eq!(visible_tools(&h.ctx, None), TOOL_NAMES);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn scoped_partial_tool_registration_rolls_back_cleanly() {
    let h = setup().await;
    let key = ScopeKey::new();
    let scoped = create_scope(&h.ctx, key.clone(), Default::default()).unwrap();
    let tools = h.ctx.get::<Tools>().unwrap();
    let conflict = define_tool(DefineToolOptions::new(
        "list_mcp_resource_templates",
        "Scoped conflict",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "null" }),
            render: Arc::new(|_, _| Vec::new()),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(Value::Null) }),
    ))
    .unwrap();
    let remove_conflict = tools
        .register(&scoped.ctx, conflict, ToolRegistrationOptions::default())
        .unwrap();
    let error = service(&h.ctx)
        .register(&scoped.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "tool \"list_mcp_resource_templates\" is already registered in this scope"
    );
    assert!(tools.get("list_mcp_resources", Some(&key), None).is_none());
    assert!(
        !prompt(&h.ctx, Some(key.clone()))
            .await
            .contains("MCP resource servers")
    );
    drop(remove_conflict.dispose());
    service(&h.ctx)
        .register(&scoped.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    assert_eq!(visible_tools(&h.ctx, Some(&key)), TOOL_NAMES);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn unloading_runtime_withdraws_tools_prompt_and_service() {
    let h = setup().await;
    service(&h.ctx)
        .register(&h.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    let tools = h.ctx.get::<Tools>().unwrap();
    let prompt_service = h.ctx.get::<SystemPromptKey>().unwrap();
    h.resources_fiber.dispose().await;
    assert!(h.ctx.get::<McpResources>().is_none());
    assert!(tools.get("list_mcp_resources", None, None).is_none());
    let assembly = prompt_service
        .assemble(AssembleContext::default())
        .await
        .unwrap();
    assert!(
        !render_prompt(&assembly)
            .unwrap()
            .contains("MCP resource servers")
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn disposing_scoped_owner_withdraws_its_tools_and_name() {
    let h = setup().await;
    let key = ScopeKey::new();
    let scoped = create_scope(&h.ctx, key.clone(), Default::default()).unwrap();
    service(&h.ctx)
        .register(&scoped.ctx, "docs", Arc::new(Probe::responding(json!({}))))
        .unwrap();
    assert_eq!(scope_of(&scoped.ctx), Some(key.clone()));
    assert_eq!(visible_tools(&h.ctx, Some(&key)), TOOL_NAMES);
    scoped.dispose().await;
    scoped.dispose().await;
    assert!(visible_tools(&h.ctx, Some(&key)).is_empty());
    assert!(
        !prompt(&h.ctx, Some(key))
            .await
            .contains("MCP resource servers")
    );
    h.kernel.shutdown().await;
}
