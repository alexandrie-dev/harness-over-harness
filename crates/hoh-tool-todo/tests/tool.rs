use std::sync::Arc;

use hoh_agent::{
    AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel, Plugin};
use hoh_llm::{ToolCallBlock, ToolCallId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, Session, SessionId, SessionLogOffset, SessionSeedEventState, SessionSeqCursor,
};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::AbortController;
use hoh_tool_todo::{Config, ToolTodo};
use hoh_tools::shim::BoxFuture;
use hoh_tools::{
    GenericCallView, ToolBatchInput, ToolCallKind, ToolCallView, ToolExecutionPipeline,
    ToolExecutionResult, ToolRuntime, Tools, execute_tool_batch,
};
use serde_json::{Value, json};

struct Harness {
    kernel: Kernel,
    ctx: Context,
    fiber: Fiber,
}

async fn setup(allow_parallel_in_progress: bool) -> Harness {
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
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = ctx
        .plugin::<ToolTodo>(json!({ "allowParallelInProgress": allow_parallel_in_progress }))
        .unwrap();
    fiber.settled().await.unwrap();
    Harness { kernel, ctx, fiber }
}

struct TestAgent {
    key: ScopeKey,
    session: Session,
    ctx: Context,
    options: AgentOptions,
}

impl TestAgent {
    fn new(ctx: &Context, id: &str) -> Arc<Self> {
        Arc::new(Self {
            key: ScopeKey::new(),
            session: Session::create(id.into(), None, None, None, None).unwrap(),
            ctx: ctx.clone(),
            options: AgentOptions::default(),
        })
    }
}

impl hoh_tools::Agent for TestAgent {
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
        panic!("the todo tool does not read the model")
    }

    fn inbox(&self) -> &dyn Inbox {
        panic!("the todo tool does not read the inbox")
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        panic!("the todo tool does not cancel the agent")
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("the todo tool does not run maintenance")
    }

    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("the todo tool does not send input")
    }

    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the todo tool does not send input")
    }

    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the todo tool does not send input")
    }

    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("the todo tool does not send input")
    }
}

async fn execute(
    ctx: &Context,
    arguments: Value,
    agent: Option<Arc<dyn hoh_tools::Agent>>,
) -> ToolExecutionResult {
    let control = AbortController::new();
    let mut input = ToolBatchInput::new(
        vec![ToolCallBlock {
            id: ToolCallId::from("todo-call"),
            name: "todo_write".into(),
            arguments: serde_json::to_string(&arguments).unwrap(),
        }],
        control.signal(),
    );
    input.agent = agent;
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).unwrap();
    execute_tool_batch(input, &pipeline, &|| 1)
        .await
        .unwrap()
        .results
        .remove(0)
        .result
}

fn as_agent(agent: &Arc<TestAgent>) -> Arc<dyn hoh_tools::Agent> {
    agent.clone()
}

fn todo_events(session: &Session) -> Vec<Arc<hoh_session::SessionEvent>> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "todo/write")
        .cloned()
        .collect()
}

fn result_text(result: &ToolExecutionResult) -> String {
    result
        .content()
        .iter()
        .filter_map(|block| block.text_value())
        .collect()
}

fn error_info(result: &ToolExecutionResult) -> Option<(&str, &str)> {
    result.error().and_then(|error| {
        error
            .info
            .as_ref()
            .map(|info| (info.name.as_str(), info.code.as_str()))
    })
}

#[test]
fn plugin_contract_has_required_config_and_dependencies() {
    assert_eq!(ToolTodo::NAME, "tool-todo");
    let dependencies: Vec<&str> = <hoh_tool_todo::Deps as hoh_kernel::Inject>::deps()
        .iter()
        .map(|dependency| dependency.name)
        .collect();
    assert_eq!(dependencies, ["tools", "sessionProjections"]);
    let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
    assert_eq!(schema["required"], json!(["allowParallelInProgress"]));
    assert_eq!(
        schema["properties"]["allowParallelInProgress"]["type"],
        "boolean"
    );
}

#[tokio::test]
async fn missing_or_non_boolean_policy_fails_config_resolution() {
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
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(
        ctx.plugin::<ToolTodo>(json!({}))
            .unwrap()
            .settled()
            .await
            .is_err()
    );
    assert!(
        ctx.plugin::<ToolTodo>(json!({ "allowParallelInProgress": "no" }))
            .unwrap()
            .settled()
            .await
            .is_err()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn registers_exact_model_facing_schema() {
    let harness = setup(true).await;
    let schema = harness
        .ctx
        .get::<Tools>()
        .unwrap()
        .schemas(None, None)
        .unwrap()
        .into_iter()
        .find(|schema| schema.name == "todo_write")
        .unwrap();
    let properties = schema.parameters["properties"].as_object().unwrap();
    assert_eq!(properties.keys().collect::<Vec<_>>(), ["todos"]);
    assert_eq!(properties["todos"]["type"], "array");
    let item_properties = properties["todos"]["items"]["properties"]
        .as_object()
        .unwrap();
    assert_eq!(
        item_properties.keys().collect::<Vec<_>>(),
        ["content", "status"]
    );
    assert_eq!(
        item_properties["status"]["enum"],
        json!(["pending", "in_progress", "completed"])
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn appends_whole_list_and_returns_counts() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "writer");
    let todos = json!([
        { "content": "plan", "status": "in_progress" },
        { "content": "build", "status": "pending" }
    ]);
    let result = execute(
        &harness.ctx,
        json!({ "todos": todos }),
        Some(as_agent(&agent)),
    )
    .await;
    assert_eq!(
        result.value(),
        Some(&json!({
            "todos": todos,
            "counts": { "pending": 1, "inProgress": 1, "completed": 0 }
        }))
    );
    assert_eq!(
        result_text(&result),
        "Updated todo list: 1 pending, 1 in progress, 0 completed."
    );
    assert_eq!(todo_events(&agent.session)[0].data()["todos"], todos);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn stores_trimmed_content() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "trim");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [{ "content": "  plan the work  ", "status": "pending" }] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(!result.is_error());
    assert_eq!(
        todo_events(&agent.session)[0].data()["todos"],
        json!([{ "content": "plan the work", "status": "pending" }])
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn stores_content_trimmed_with_the_ecmascript_character_set() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "ecmascript-trim");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [
            { "content": "\u{FEFF}plan the work\u{FEFF}", "status": "pending" },
            { "content": "\u{0085}", "status": "completed" }
        ] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(!result.is_error(), "{}", result_text(&result));
    assert_eq!(
        todo_events(&agent.session)[0].data()["todos"],
        json!([
            { "content": "plan the work", "status": "pending" },
            { "content": "\u{0085}", "status": "completed" }
        ])
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn bom_only_and_bom_trimmed_duplicate_content_are_rejected() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "ecmascript-trim-errors");
    let empty = execute(
        &harness.ctx,
        json!({ "todos": [{ "content": "\u{FEFF}", "status": "pending" }] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert_eq!(
        empty.error().unwrap().message,
        "invalid arguments: todos[0].content must be a non-empty string"
    );
    assert_eq!(error_info(&empty), Some(("ToolArgsError", "INVALID_ARGS")));

    let duplicate = execute(
        &harness.ctx,
        json!({ "todos": [
            { "content": "a\u{FEFF}", "status": "pending" },
            { "content": "a", "status": "completed" }
        ] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert_eq!(
        duplicate.error().unwrap().message,
        "invalid arguments: todos[1].content duplicates \"a\""
    );
    assert_eq!(
        error_info(&duplicate),
        Some(("ToolArgsError", "INVALID_ARGS"))
    );
    assert!(todo_events(&agent.session).is_empty());
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn latest_write_is_the_current_whole_list() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "writer-2");
    execute(
        &harness.ctx,
        json!({ "todos": [{ "content": "a", "status": "pending" }] }),
        Some(as_agent(&agent)),
    )
    .await;
    let second = json!([
        { "content": "a", "status": "completed" },
        { "content": "b", "status": "in_progress" }
    ]);
    execute(
        &harness.ctx,
        json!({ "todos": second }),
        Some(as_agent(&agent)),
    )
    .await;
    let events = todo_events(&agent.session);
    assert_eq!(events.len(), 2);
    assert_eq!(events.last().unwrap().data()["todos"], second);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn malformed_status_is_rejected_before_execute() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "bad-status");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [{ "content": "x", "status": "doing" }] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(result.is_error());
    assert!(todo_events(&agent.session).is_empty());
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn non_array_todos_is_rejected() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "not-array");
    let result = execute(
        &harness.ctx,
        json!({ "todos": "nope" }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(result.is_error());
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn parallel_policy_accepts_several_active_items() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "parallel");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [
            { "content": "run subagent a", "status": "in_progress" },
            { "content": "run subagent b", "status": "in_progress" },
            { "content": "merge results", "status": "pending" }
        ] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert_eq!(result.value().unwrap()["counts"]["inProgress"], 2);
    assert_eq!(todo_events(&agent.session).len(), 1);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn single_policy_rejects_several_active_items_without_writing() {
    let harness = setup(false).await;
    let agent = TestAgent::new(&harness.ctx, "single-active");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [
            { "content": "run subagent a", "status": "in_progress" },
            { "content": "run subagent b", "status": "in_progress" }
        ] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(result.is_error());
    assert!(
        result
            .error()
            .unwrap()
            .message
            .contains("at most one task may be in_progress (got 2)")
    );
    assert!(todo_events(&agent.session).is_empty());
    assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn single_policy_accepts_one_active_item() {
    let harness = setup(false).await;
    let agent = TestAgent::new(&harness.ctx, "one-active");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [
            { "content": "run subagent a", "status": "in_progress" },
            { "content": "run subagent b", "status": "pending" }
        ] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(!result.is_error());
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn descriptions_follow_the_parallel_policy() {
    let single = setup(false).await;
    let single_description = single
        .ctx
        .get::<Tools>()
        .unwrap()
        .get("todo_write", None, None)
        .unwrap()
        .description
        .clone();
    assert!(single_description.contains("keep exactly one todo `in_progress`"));
    assert!(!single_description.contains("several only"));
    single.kernel.shutdown().await;

    let parallel = setup(true).await;
    let parallel_description = parallel
        .ctx
        .get::<Tools>()
        .unwrap()
        .get("todo_write", None, None)
        .unwrap()
        .description
        .clone();
    assert!(parallel_description.contains("several only when work runs in parallel"));
    assert!(!parallel_description.contains("exactly one"));
    parallel.kernel.shutdown().await;
}

#[tokio::test]
async fn empty_content_is_a_structured_args_error() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "empty");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [{ "content": "   ", "status": "pending" }] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(result.error().unwrap().message.contains("non-empty string"));
    assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn duplicate_trimmed_content_is_a_structured_args_error() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "duplicate");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [
            { "content": "dup", "status": "pending" },
            { "content": " dup ", "status": "completed" }
        ] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(
        result
            .error()
            .unwrap()
            .message
            .contains("todos[1].content duplicates \"dup\"")
    );
    assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn unknown_item_keys_fail_at_the_schema_boundary() {
    let harness = setup(true).await;
    let agent = TestAgent::new(&harness.ctx, "unknown-key");
    let result = execute(
        &harness.ctx,
        json!({ "todos": [{ "content": "a", "status": "pending", "children": [] }] }),
        Some(as_agent(&agent)),
    )
    .await;
    assert!(result.is_error());
    assert!(
        result
            .error()
            .unwrap()
            .message
            .contains("declared property")
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn non_agent_caller_is_rejected() {
    let harness = setup(true).await;
    let result = execute(
        &harness.ctx,
        json!({ "todos": [{ "content": "a", "status": "pending" }] }),
        None,
    )
    .await;
    assert_eq!(
        result.error().unwrap().message,
        "todo_write requires an owning agent session"
    );
    assert_eq!(
        error_info(&result),
        Some(("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE"))
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn call_presentation_has_stable_title_kind_and_raw_list() {
    let harness = setup(true).await;
    let definition = harness
        .ctx
        .get::<Tools>()
        .unwrap()
        .get("todo_write", None, None)
        .unwrap();
    let todos = json!([{ "content": "a", "status": "pending" }]);
    assert_eq!(
        definition.present_call.as_ref().unwrap()(&json!({ "todos": todos })),
        Some(ToolCallView::Generic(GenericCallView {
            title: "Update todo list".into(),
            kind: Some(ToolCallKind::Other),
            raw_input: Some(todos),
            content: None,
            locations: None,
        }))
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn disposing_plugin_unregisters_the_tool() {
    let harness = setup(true).await;
    assert!(
        harness
            .ctx
            .get::<Tools>()
            .unwrap()
            .get("todo_write", None, None)
            .is_some()
    );
    harness.fiber.disposer().unwrap().dispose().await;
    assert!(
        harness
            .ctx
            .get::<Tools>()
            .unwrap()
            .get("todo_write", None, None)
            .is_none()
    );
    harness.kernel.shutdown().await;
}

#[test]
fn todo_event_snapshot_isolated_from_later_input_mutation() {
    let session = Session::create("isolation".into(), None, None, None, None).unwrap();
    let mut todos = vec![json!({ "content": "plan", "status": "in_progress" })];
    session
        .append("todo/write", json!({ "todos": todos }), None)
        .unwrap();
    todos.push(json!({ "content": "sneak in", "status": "pending" }));
    todos[0]["status"] = json!("completed");
    assert_eq!(
        todo_events(&session)[0].data()["todos"],
        json!([{ "content": "plan", "status": "in_progress" }])
    );
}

#[test]
fn todo_event_does_not_add_a_derived_message_or_surface_node() {
    let session = Session::create("no-surface".into(), None, None, None, None).unwrap();
    let before = session.derive_messages().unwrap().len();
    let event = session
        .append(
            "todo/write",
            json!({ "todos": [{ "content": "a task", "status": "pending" }] }),
            None,
        )
        .unwrap();
    assert_eq!(session.derive_messages().unwrap().len(), before);
    assert!(
        !session
            .surface()
            .unwrap()
            .nodes()
            .unwrap()
            .contains(&event.seq())
    );
}

#[test]
fn todo_event_round_trips_through_seeded_replay() {
    let original = Session::create("original".into(), None, None, None, None).unwrap();
    original
        .append("turn/start", json!({ "turn": 1 }), None)
        .unwrap();
    original
        .append(
            "todo/write",
            json!({ "todos": [{ "content": "only", "status": "completed" }] }),
            None,
        )
        .unwrap();
    original
        .append(
            "turn/end",
            json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    let seed = original.snapshot_events(None, None);
    let replayed = Session::from_restore(
        original.id().clone(),
        seed.as_ref(),
        (*original.header()).clone(),
        SessionLogOffset::default(),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(
        replayed
            .snapshot_events(None, Some(original.seq()))
            .as_ref(),
        original.snapshot_events(None, None).as_ref()
    );
    assert_eq!(replayed.first_live_seq(), original.seq());
    assert!(matches!(
        SessionSeqCursor::from(todo_events(&replayed)[0].seq()),
        SessionSeqCursor::Seq(_)
    ));
}
