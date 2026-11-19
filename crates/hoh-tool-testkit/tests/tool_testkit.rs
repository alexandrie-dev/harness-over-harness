use std::sync::{Arc, Mutex, MutexGuard};

use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{ContentBlock, ToolCallBlock};
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_testkit::{
    ContentToolFixtureOptions, ModelToolFixtureInput, define_content_tool_fixture,
    execute_model_tool_fixture, execute_tool_fixture,
};
use hoh_tools::shim::BoxFuture;
use hoh_tools::{
    ToolBatchInput, ToolBatchResult, ToolCallResult, ToolExecutionInput, ToolExecutionResult,
    ToolExecutionSuccess, ToolExecutor, ToolModelInput, ToolRegistrationOptions, ToolRuntime,
    Tools, ToolsError,
};
use serde_json::{Value, json};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct TestEnv {
    _kernel: Kernel,
    ctx: Context,
}

async fn setup_tools() -> TestEnv {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({}))
        .expect("system prompt mounts")
        .settled()
        .await
        .expect("system prompt settles");
    ctx.plugin::<ToolRuntime>(json!({}))
        .expect("tools mount")
        .settled()
        .await
        .expect("tools settle");
    TestEnv {
        _kernel: kernel,
        ctx,
    }
}

struct SeenBatch {
    call: ToolCallBlock,
    signal: AbortSignal,
    has_agent: bool,
    has_tool_set: bool,
}

struct StubExecutor {
    seen: Arc<Mutex<Vec<SeenBatch>>>,
    response: Mutex<Option<ToolBatchResult>>,
}

impl StubExecutor {
    fn new(response: ToolBatchResult) -> (Arc<Self>, Arc<Mutex<Vec<SeenBatch>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Self {
                seen: seen.clone(),
                response: Mutex::new(Some(response)),
            }),
            seen,
        )
    }
}

impl ToolExecutor for StubExecutor {
    fn describe(
        &self,
        _tools: &[Arc<hoh_tools::ToolDefinition>],
    ) -> Result<ToolModelInput, BoxError> {
        Ok(ToolModelInput::default())
    }

    fn execute(
        &self,
        mut input: ToolBatchInput,
    ) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
        let call = input.calls.remove(0);
        lock(&self.seen).push(SeenBatch {
            call,
            signal: input.signal,
            has_agent: input.agent.is_some(),
            has_tool_set: input.tool_set.is_some(),
        });
        let response = lock(&self.response).take().unwrap_or_default();
        Box::pin(async move { Ok(response) })
    }
}

async fn setup_with_executor(response: ToolBatchResult) -> (TestEnv, Arc<Mutex<Vec<SeenBatch>>>) {
    let env = setup_tools().await;
    let (executor, seen) = StubExecutor::new(response);
    env.ctx
        .get::<Tools>()
        .expect("tools service")
        .set_executor(&env.ctx, executor)
        .expect("executor registers");
    (env, seen)
}

#[tokio::test]
async fn content_fixture_defines_and_executes_content_as_the_canonical_value() {
    let env = setup_tools().await;
    let mut options = ContentToolFixtureOptions::new(
        "echo",
        "echo content",
        json!({ "text": { "type": "string", "required": true } }),
        |args, _| async move {
            Ok(vec![ContentBlock::text(
                args["text"].as_str().expect("validated string"),
            )])
        },
    );
    options.defer_loading = true;
    options.timeout_ms = Some(250.0);
    let definition = define_content_tool_fixture(options).expect("fixture defines");
    assert_eq!(definition.name, "echo");
    assert_eq!(definition.description, "echo content");
    assert_eq!(definition.defer_loading, Some(true));
    assert_eq!(definition.timeout_ms, Some(250.0));
    assert_eq!(
        definition.output.schema,
        json!({ "type": "array", "items": {} })
    );
    env.ctx
        .get::<Tools>()
        .expect("tools service")
        .register(&env.ctx, definition, ToolRegistrationOptions::default())
        .expect("fixture registers");

    let result = execute_tool_fixture(
        &env.ctx,
        ToolExecutionInput::new(
            "call-1".into(),
            "echo",
            json!({ "text": "hello" }),
            AbortController::new().signal(),
        ),
    )
    .await
    .expect("pipeline constructs");
    assert_eq!(
        result.value(),
        Some(&json!([{ "type": "text", "text": "hello" }]))
    );
    assert_eq!(result.content(), &[ContentBlock::text("hello")]);
}

#[tokio::test]
async fn model_fixture_serializes_arguments_and_returns_the_executor_result() {
    let expected = ToolExecutionResult::Success(ToolExecutionSuccess::new(
        json!({ "ok": true }),
        vec![ContentBlock::text("done")],
    ));
    let response = ToolBatchResult {
        results: vec![ToolCallResult {
            call_id: "call-1".into(),
            result: expected.clone(),
        }],
        ..ToolBatchResult::default()
    };
    let (env, seen) = setup_with_executor(response).await;
    let controller = AbortController::new();

    let actual = execute_model_tool_fixture(
        &env.ctx,
        ModelToolFixtureInput::new(
            "call-1".into(),
            "demo",
            json!({ "text": "hello" }),
            controller.signal(),
        ),
    )
    .await
    .expect("executor settles");
    assert_eq!(actual, expected);

    let batches = lock(&seen);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].call.id, "call-1");
    assert_eq!(batches[0].call.name, "demo");
    assert_eq!(batches[0].call.arguments, r#"{"text":"hello"}"#);
    assert!(!batches[0].has_agent);
    assert!(!batches[0].has_tool_set);
    controller.abort_with("same signal");
    let expected_reason = controller.signal().reason().expect("source signal aborts");
    let seen_reason = batches[0].signal.reason().expect("forwarded signal aborts");
    assert!(seen_reason.ptr_eq(&expected_reason));
}

#[tokio::test]
async fn undefined_model_arguments_fail_before_calling_the_executor() {
    let (env, seen) = setup_with_executor(ToolBatchResult::default()).await;
    let mut input = ModelToolFixtureInput::new(
        "call-1".into(),
        "demo",
        Value::Null,
        AbortController::new().signal(),
    );
    input.arguments = None;

    let error = execute_model_tool_fixture(&env.ctx, input)
        .await
        .expect_err("undefined must fail");
    assert_eq!(
        error.downcast_ref::<ToolsError>(),
        Some(&ToolsError::TypeError(
            "Model tool fixture arguments must encode as JSON".into()
        ))
    );
    assert!(lock(&seen).is_empty());
}

#[tokio::test]
async fn an_executor_that_omits_the_result_is_rejected() {
    let (env, seen) = setup_with_executor(ToolBatchResult::default()).await;
    let error = execute_model_tool_fixture(
        &env.ctx,
        ModelToolFixtureInput::new(
            "call-1".into(),
            "demo",
            json!({}),
            AbortController::new().signal(),
        ),
    )
    .await
    .expect_err("missing result must fail");
    assert_eq!(
        error.downcast_ref::<ToolsError>(),
        Some(&ToolsError::Error(
            "Executor omitted the model tool fixture result".into()
        ))
    );
    assert_eq!(lock(&seen).len(), 1);
}
