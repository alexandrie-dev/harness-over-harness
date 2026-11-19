//! Second half of the port of tests/tools.spec.ts: contexts, cancellation ordering, wrapper and
//! listener failure classification, schema snapshots and the remaining DSL / validateArgs cases.

mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use common::*;
use hoh_error::{BoxError, HarnessError};
use hoh_llm::{MessageSource, UserMessage};
use hoh_timeout::{AbortController, AbortError};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, PostToolDecision, PreToolDecision,
    ScheduledToolPreparation, TOOL_ABORTED, TOOL_ABORTED_BEFORE_DISPATCH, ToolDefinition,
    ToolErrorInfo, ToolExecution, ToolExecutionPipeline, ToolExecutionResult, ToolExecutionSource,
    ToolExecutionSuccess, ToolsExecute, ToolsPostExecute, ToolsPreExecute, ToolsResult,
    define_tool, validate_args,
};
use hoh_user_approval::ApprovalRequestEvent;
use serde_json::{Value, json};
use tokio::sync::Notify;

fn msg(kind: &str, body: &str) -> UserMessage {
    hoh_llm::create_user_message(hoh_llm::NewUserMessage {
        content: vec![text(body)],
        source: MessageSource::new(kind),
    })
}

fn kinds(result: &ToolExecutionResult) -> Vec<String> {
    result
        .additional_contexts()
        .map(|contexts| {
            contexts
                .iter()
                .map(|message| message.source.kind.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn info(result: &ToolExecutionResult) -> Option<(String, String)> {
    result
        .error()
        .and_then(|error| error.info.as_ref())
        .map(|info| (info.name.clone(), info.code.clone()))
}

fn named(name: &str, code: &str) -> Option<(String, String)> {
    Some((name.to_owned(), code.to_owned()))
}

/// The source spreads the result (`{ ...result, additionalContexts }`): an edited copy, never the frozen original.
fn with_contexts(result: ToolExecutionResult, contexts: Vec<UserMessage>) -> ToolExecutionResult {
    let mut result = result.clone();
    match &mut result {
        ToolExecutionResult::Success(success) => success.set_additional_contexts(Some(contexts)),
        ToolExecutionResult::Failure(failure) => failure.set_additional_contexts(Some(contexts)),
    }
    result
}

fn named_tool(
    name: &str,
    body: impl Fn(Arc<ToolExecution>) + Send + Sync + 'static,
) -> ToolDefinition {
    let mut tool = echo_tool();
    tool.name = name.into();
    tool.execute = Arc::new(move |_, exec| {
        body(exec);
        Box::pin(async { Ok(json!("body complete")) })
    });
    tool
}

fn counting_tool(name: &str, dispatched: &Arc<AtomicUsize>) -> ToolDefinition {
    let dispatched = dispatched.clone();
    named_tool(name, move |_| {
        dispatched.fetch_add(1, Ordering::SeqCst);
    })
}

/// A started/release pair: `entered` fires once the listener is waiting, `release` lets it go on.
struct Gate {
    entered: Notify,
    release: Notify,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            release: Notify::new(),
        })
    }
    async fn pass(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}

fn request(call: &str, name: &str, signal: &Arc<AbortController>) -> hoh_tools::ToolExecutionInput {
    let mut request = input(call, name, json!({}));
    request.signal = signal.signal();
    request
}

/// Run a call, wait for the gate, cancel, release, and return the settled result.
async fn cancelled_at_gate(
    env: &Env,
    request: hoh_tools::ToolExecutionInput,
    gate: &Arc<Gate>,
    controller: &Arc<AbortController>,
    reason: &str,
) -> ToolExecutionResult {
    let ctx = env.ctx.clone();
    let pending = tokio::spawn(async move { execute_tool_fixture(&ctx, request).await });
    gate.entered.notified().await;
    controller.abort_with(reason.to_owned());
    gate.release.notify_one();
    pending.await.unwrap()
}

// ------------------------------------------------------------------- approval backstop

#[tokio::test]
async fn treats_a_failing_approval_answerer_as_unavailable() {
    let env = setup().await;
    register(&env, echo_tool());
    mount_approval_service(&env).await;
    env.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(|_, _| async {
            Err::<Value, BoxError>("approval answerer exploded".into())
        })
        .unwrap();
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Ok(PreToolDecision::Ask {
                reason: None,
                display_reason: None,
                details: None,
            })
        })
        .unwrap();
    let mut request = input("c1", "echo", json!({}));
    request.agent = Some(TestAgent::open(&env.ctx));
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert!(result.is_error());
    assert_eq!(
        result.text(),
        "Error: tool \"echo\" requires approval, but no approval channel is available"
    );
}

// ------------------------------------------------------------------ finalizers, contexts

#[tokio::test]
async fn runs_the_snapshotted_final_content_transform_after_outer_pipeline_normalization() {
    let env = setup().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tool = content_tool_with(
        "bounded",
        json!({}),
        |_, _| async { Ok(vec![text("body")]) },
        {
            let seen = seen.clone();
            move |options| {
                options.finalize_content = Some(Arc::new(move |exec, result| {
                    lock(&seen).push((exec.name.clone(), result.is_error()));
                    Some(vec![text("bounded failure")])
                }));
            }
        },
    );
    let disposer = Arc::new(Mutex::new(Some(register(&env, tool))));
    {
        let disposer = disposer.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, _| {
                if let Some(disposer) = lock(&disposer).take() {
                    drop(disposer.dispose());
                }
                async {
                    Err::<PreToolDecision, BoxError>(Box::new(HarnessError::new(
                        "policy failed",
                        "POLICY_FAILED",
                    )))
                }
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("bounded", "bounded", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(result.content(), &[text("bounded failure")]);
    assert_eq!(result.error().unwrap().message, "policy failed");
    assert_eq!(info(&result), named("HarnessError", "POLICY_FAILED"));
    assert_eq!(*lock(&seen), vec![("bounded".to_owned(), true)]);
}

#[tokio::test]
async fn keeps_the_normalized_content_when_the_final_content_transform_returns_none() {
    let env = setup().await;
    let finalized = Arc::new(AtomicUsize::new(0));
    let mut tool = echo_tool();
    tool.name = "identity-finalizer".into();
    {
        let finalized = finalized.clone();
        tool.finalize_content = Some(Arc::new(move |_, _| {
            finalized.fetch_add(1, Ordering::SeqCst);
            None
        }));
    }
    register(&env, tool);
    let result = execute_tool_fixture(&env.ctx, input("f", "identity-finalizer", json!({}))).await;
    assert!(!result.is_error());
    assert_eq!(result.content(), &[text("")]);
    assert_eq!(finalized.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn post_execute_additional_contexts_ride_on_the_result_for_the_loop_to_buffer() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Accept {
                content: None,
                value: None,
                additional_contexts: Some(vec![msg("test", "fyi")]),
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert_eq!(kinds(&result), vec!["test"]);
    assert_eq!(
        result.additional_contexts().unwrap()[0].content,
        vec![text("fyi")]
    );
}

#[tokio::test]
async fn preserves_tool_deferred_execute_wrapper_and_post_execute_contexts_in_the_source_order() {
    let env = setup().await;
    register(
        &env,
        content_tool("composite", json!({}), |_, exec| async move {
            exec.defer_context(msg("nested-1", "nested-1"));
            exec.defer_context(msg("nested-2", "nested-2"));
            Ok(vec![text("done")])
        }),
    );
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, next| async move {
            let result = next.run().await?;
            let mut contexts = result.additional_contexts().cloned().unwrap_or_default();
            contexts.push(msg("wrapper", "wrapper"));
            Ok(with_contexts(result, contexts))
        })
        .unwrap();
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, next| async move {
            let downstream = next.run().await?;
            let mut contexts = vec![msg("post", "post")];
            let PostToolDecision::Accept {
                content,
                value,
                additional_contexts,
            } = downstream
            else {
                panic!("downstream accepts");
            };
            contexts.extend(additional_contexts.unwrap_or_default());
            Ok(PostToolDecision::Accept {
                content,
                value,
                additional_contexts: Some(contexts),
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("composite", "composite", json!({}))).await;
    assert_eq!(
        kinds(&result),
        vec!["nested-1", "nested-2", "wrapper", "post"]
    );
}

#[tokio::test]
async fn keeps_deferred_contexts_when_a_composite_throws_but_drops_them_when_the_outer_call_is_blocked()
 {
    let env = setup().await;
    register(
        &env,
        named_tool("failing-composite", |exec| {
            exec.defer_context(msg("nested", "nested"));
        }),
    );
    // The body fails after deferring: replace its body with a throwing one that still defers.
    let mut failing = named_tool("failing-composite-2", |exec| {
        exec.defer_context(msg("nested", "nested"))
    });
    failing.execute = Arc::new(|_, exec| {
        exec.defer_context(msg("nested", "nested"));
        Box::pin(async { Err::<Value, BoxError>("outer failure".into()) })
    });
    register(&env, failing);
    let failed =
        execute_tool_fixture(&env.ctx, input("failed", "failing-composite-2", json!({}))).await;
    assert!(failed.is_error());
    assert_eq!(kinds(&failed), vec!["nested"]);

    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Block {
                feedback: vec![text("blocked")],
                info: None,
                additional_contexts: Some(vec![msg("blocker", "block-only")]),
            })
        })
        .unwrap();
    let blocked =
        execute_tool_fixture(&env.ctx, input("blocked", "failing-composite", json!({}))).await;
    assert!(blocked.is_error());
    assert_eq!(kinds(&blocked), vec!["blocker"]);
}

// ----------------------------------------------------------------- cancellation ordering

#[tokio::test]
async fn preserves_a_pre_execute_denial_that_settles_after_cancellation() {
    let env = setup().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    register(&env, counting_tool("denied-after-cancel", &dispatched));
    let gate = Gate::new();
    {
        let gate = gate.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, _| {
                let gate = gate.clone();
                async move {
                    gate.pass().await;
                    Ok(PreToolDecision::Deny {
                        reason: "policy denied the call".into(),
                        info: None,
                    })
                }
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    let result = cancelled_at_gate(
        &env,
        request("denied-after-cancel", "denied-after-cancel", &controller),
        &gate,
        &controller,
        "cancelled while policy decided",
    )
    .await;
    assert_eq!(result.content(), &[text("Error: policy denied the call")]);
    assert_eq!(result.error().unwrap().message, "policy denied the call");
    assert!(result.error().unwrap().info.is_none());
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn preserves_an_async_pre_execute_failure_that_settles_after_cancellation() {
    let env = setup().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    register(&env, counting_tool("must-not-run", &dispatched));
    let gate = Gate::new();
    {
        let gate = gate.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, _| {
                let gate = gate.clone();
                async move {
                    gate.pass().await;
                    Err::<PreToolDecision, BoxError>("gate interrupted".into())
                }
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    let result = cancelled_at_gate(
        &env,
        request("cancelled-pre-error", "must-not-run", &controller),
        &gate,
        &controller,
        "cancelled in policy",
    )
    .await;
    assert_eq!(result.content(), &[text("Error: gate interrupted")]);
    assert_eq!(result.error().unwrap().message, "gate interrupted");
    assert!(result.error().unwrap().info.is_none());
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rechecks_caller_cancellation_after_an_async_around_dispatch_wrapper_delegates() {
    let env = setup().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    register(&env, counting_tool("must-not-run", &dispatched));
    let gate = Gate::new();
    let replacement = Arc::new(AbortController::new());
    {
        let (gate, replacement) = (gate.clone(), replacement.clone());
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let (gate, replacement) = (gate.clone(), replacement.clone());
                async move {
                    let upstream = event.exec.signal();
                    event.exec.set_signal(replacement.signal());
                    gate.pass().await;
                    let result = next.run().await;
                    event.exec.set_signal(upstream);
                    result
                }
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    let result = cancelled_at_gate(
        &env,
        request("cancelled-in-around", "must-not-run", &controller),
        &gate,
        &controller,
        "cancelled in wrapper",
    )
    .await;
    assert!(result.is_error());
    assert_eq!(
        info(&result),
        named("AbortError", TOOL_ABORTED_BEFORE_DISPATCH)
    );
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn skips_dispatch_when_an_around_wrapper_supplies_an_already_aborted_signal() {
    let env = setup().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    register(&env, counting_tool("must-not-run", &dispatched));
    let replacement = Arc::new(AbortController::new());
    replacement.abort_with("wrapper cancelled");
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
            let upstream = event.exec.signal();
            event.exec.set_signal(replacement.signal());
            async move {
                let result = next.run().await;
                event.exec.set_signal(upstream);
                result
            }
        })
        .unwrap();
    let controller = Arc::new(AbortController::new());
    let result = execute_tool_fixture(
        &env.ctx,
        request("cancelled-wrapper", "must-not-run", &controller),
    )
    .await;
    let error = result.error().unwrap();
    assert_eq!(error.message, "tool call aborted before dispatch");
    assert_eq!(
        error.info,
        Some(ToolErrorInfo::new(
            "AbortError",
            TOOL_ABORTED_BEFORE_DISPATCH
        ))
    );
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn uses_aborted_before_dispatch_when_cancellation_overtakes_a_wrapper_short_circuit() {
    let env = setup().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    register(&env, counting_tool("short-circuited", &dispatched));
    let gate = Gate::new();
    {
        let gate = gate.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, _| {
                let gate = gate.clone();
                async move {
                    gate.pass().await;
                    let mut success = ToolExecutionSuccess::new(
                        json!("wrapper success"),
                        vec![text("wrapper success")],
                    );
                    success.set_additional_contexts(Some(vec![msg("wrapper", "wrapper context")]));
                    Ok(ToolExecutionResult::Success(success))
                }
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    let result = cancelled_at_gate(
        &env,
        request("cancelled-short-circuit", "short-circuited", &controller),
        &gate,
        &controller,
        "cancelled while wrapper waited",
    )
    .await;
    assert_eq!(
        result.content(),
        &[text("Error: tool call aborted before dispatch")]
    );
    assert_eq!(
        info(&result),
        named("AbortError", TOOL_ABORTED_BEFORE_DISPATCH)
    );
    assert_eq!(kinds(&result), vec!["wrapper"]);
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn replaces_a_late_wrapper_success_with_aborted_and_preserves_deferred_contexts() {
    let env = setup().await;
    register(
        &env,
        named_tool("completed-before-wrapper", |exec| {
            exec.defer_context(msg("child", "completed child work"))
        }),
    );
    let gate = Gate::new();
    {
        let gate = gate.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, next| {
                let gate = gate.clone();
                async move {
                    let result = next.run().await?;
                    gate.pass().await;
                    Ok(result)
                }
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    let result = cancelled_at_gate(
        &env,
        request(
            "cancelled-after-body",
            "completed-before-wrapper",
            &controller,
        ),
        &gate,
        &controller,
        "cancelled while wrapper settled",
    )
    .await;
    assert_eq!(result.content(), &[text("Error: tool call aborted")]);
    assert_eq!(info(&result), named("AbortError", TOOL_ABORTED));
    assert_eq!(kinds(&result), vec!["child"]);
}

#[tokio::test]
async fn replaces_a_late_post_execute_success_with_aborted_and_preserves_contexts() {
    let env = setup().await;
    register(
        &env,
        named_tool("completed-before-post", |exec| {
            exec.defer_context(msg("child", "completed child work"))
        }),
    );
    let gate = Gate::new();
    {
        let gate = gate.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, next| {
                let gate = gate.clone();
                async move {
                    next.run().await?;
                    gate.pass().await;
                    Ok(PostToolDecision::Accept {
                        content: None,
                        value: None,
                        additional_contexts: Some(vec![msg("post", "post context")]),
                    })
                }
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    let result = cancelled_at_gate(
        &env,
        request("cancelled-in-post", "completed-before-post", &controller),
        &gate,
        &controller,
        "cancelled while post policy waits",
    )
    .await;
    assert_eq!(result.content(), &[text("Error: tool call aborted")]);
    assert_eq!(info(&result), named("AbortError", TOOL_ABORTED));
    assert_eq!(kinds(&result), vec!["child", "post"]);
}

#[tokio::test]
async fn preserves_an_around_dispatch_failure_that_settles_after_cancellation() {
    let env = setup().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    register(&env, counting_tool("wrapper-failure", &dispatched));
    let gate = Gate::new();
    {
        let gate = gate.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, _| {
                let gate = gate.clone();
                async move {
                    gate.pass().await;
                    Err::<ToolExecutionResult, BoxError>(Box::new(HarnessError::new(
                        "wrapper failed",
                        "WRAPPER_FAILURE",
                    )))
                }
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    let result = cancelled_at_gate(
        &env,
        request("wrapper-failure", "wrapper-failure", &controller),
        &gate,
        &controller,
        "cancelled while wrapper failed",
    )
    .await;
    assert_eq!(result.content(), &[text("Error: wrapper failed")]);
    assert_eq!(info(&result), named("HarnessError", "WRAPPER_FAILURE"));
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn preserves_a_tool_owned_failure_after_the_body_observes_cancellation() {
    let env = setup().await;
    let entered = Arc::new(Notify::new());
    let mut tool = echo_tool();
    tool.name = "tool-failure".into();
    {
        let entered = entered.clone();
        tool.execute = Arc::new(move |_, exec: Arc<ToolExecution>| {
            entered.notify_one();
            Box::pin(async move {
                exec.signal().cancelled().await;
                Err::<Value, BoxError>(Box::new(HarnessError::new("tool failed", "TOOL_FAILURE")))
            })
        });
    }
    register(&env, tool);
    let controller = Arc::new(AbortController::new());
    let ctx = env.ctx.clone();
    let call = request("tool-failure", "tool-failure", &controller);
    let pending = tokio::spawn(async move { execute_tool_fixture(&ctx, call).await });
    entered.notified().await;
    controller.abort_with("cancelled running body");
    let result = pending.await.unwrap();
    assert_eq!(result.content(), &[text("Error: tool failed")]);
    assert_eq!(info(&result), named("HarnessError", "TOOL_FAILURE"));
}

#[tokio::test]
async fn preserves_a_post_policy_failure_that_settles_after_cancellation() {
    let env = setup().await;
    register(&env, echo_tool());
    let gate = Gate::new();
    {
        let gate = gate.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, _| {
                let gate = gate.clone();
                async move {
                    gate.pass().await;
                    Err::<PostToolDecision, BoxError>(Box::new(HarnessError::new(
                        "post-policy failed",
                        "POST_FAILURE",
                    )))
                }
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    let result = cancelled_at_gate(
        &env,
        request("post-failure", "echo", &controller),
        &gate,
        &controller,
        "cancelled while post-policy failed",
    )
    .await;
    assert_eq!(result.content(), &[text("Error: post-policy failed")]);
    assert_eq!(info(&result), named("HarnessError", "POST_FAILURE"));
}

/// How a body or listener fails once cancellation has been observed.
enum Fail {
    Reason,
    DependencyAbort,
    Domain,
    Distinct,
}

fn fail_with(kind: &Fail, exec: &ToolExecution) -> BoxError {
    match kind {
        Fail::Reason => Box::new(exec.signal().reason().expect("aborted")),
        Fail::DependencyAbort => Box::new(AbortError::new("This operation was aborted")),
        Fail::Domain => Box::new(HarnessError::new("search cancelled", "SEARCH_ABORTED")),
        Fail::Distinct => "cleanup failed".into(),
    }
}

async fn body_failing_after_cancellation(kind: Fail) -> ToolExecutionResult {
    let env = setup().await;
    let entered = Arc::new(Notify::new());
    let mut tool = echo_tool();
    tool.name = "throws-on-abort".into();
    {
        let entered = entered.clone();
        let kind = Arc::new(kind);
        tool.execute = Arc::new(move |_, exec: Arc<ToolExecution>| {
            entered.notify_one();
            let kind = kind.clone();
            Box::pin(async move {
                exec.signal().cancelled().await;
                Err::<Value, BoxError>(fail_with(&kind, &exec))
            })
        });
    }
    register(&env, tool);
    let controller = Arc::new(AbortController::new());
    let ctx = env.ctx.clone();
    let call = request("throws-on-abort", "throws-on-abort", &controller);
    let pending = tokio::spawn(async move { execute_tool_fixture(&ctx, call).await });
    entered.notified().await;
    controller.abort_with("caller stopped");
    pending.await.unwrap()
}

#[tokio::test]
async fn classifies_the_signal_reason_thrown_after_cancellation_as_aborted() {
    let result = body_failing_after_cancellation(Fail::Reason).await;
    assert_eq!(result.content(), &[text("Error: caller stopped")]);
    assert_eq!(result.error().unwrap().message, "caller stopped");
    assert_eq!(info(&result), named("AbortError", TOOL_ABORTED));
}

#[tokio::test]
async fn classifies_a_dependency_abort_error_thrown_after_cancellation_as_aborted() {
    let result = body_failing_after_cancellation(Fail::DependencyAbort).await;
    assert_eq!(
        result.content(),
        &[text("Error: This operation was aborted")]
    );
    assert_eq!(
        result.error().unwrap().message,
        "This operation was aborted"
    );
    assert_eq!(info(&result), named("AbortError", TOOL_ABORTED));
}

#[tokio::test]
async fn keeps_a_harness_error_thrown_after_cancellation_domain_abort() {
    let result = body_failing_after_cancellation(Fail::Domain).await;
    assert_eq!(result.content(), &[text("Error: search cancelled")]);
    assert_eq!(info(&result), named("HarnessError", "SEARCH_ABORTED"));
}

#[tokio::test]
async fn keeps_a_distinct_failure_thrown_during_cancellation_without_info() {
    let result = body_failing_after_cancellation(Fail::Distinct).await;
    assert_eq!(result.content(), &[text("Error: cleanup failed")]);
    assert!(result.is_error());
    assert!(result.error().unwrap().info.is_none());
}

#[tokio::test]
async fn keeps_a_plain_failure_without_info_when_the_call_was_not_cancelled() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.name = "plain-failure".into();
    tool.execute = Arc::new(|_, _| Box::pin(async { Err::<Value, BoxError>("disk full".into()) }));
    register(&env, tool);
    let result =
        execute_tool_fixture(&env.ctx, input("plain-failure", "plain-failure", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(result.error().unwrap().message, "disk full");
    assert!(result.error().unwrap().info.is_none());
}

#[tokio::test]
async fn lets_argument_materialization_failure_win_over_a_pre_aborted_signal() {
    let env = setup().await;
    let observed = Arc::new(AtomicUsize::new(0));
    {
        let observed = observed.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    controller.abort_with("already cancelled");
    let mut call = input("invalid-pre-aborted", "missing", json!({ "invalid": -0.0 }));
    call.signal = controller.signal();
    let result = execute_tool_fixture(&env.ctx, call).await;
    assert_eq!(
        result.content(),
        &[text(
            "Error: tool execution arguments must be losslessly JSON-serializable"
        )]
    );
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().message,
        "tool execution arguments must be losslessly JSON-serializable"
    );
    assert!(result.error().unwrap().info.is_none());
    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reports_cancellation_before_an_executor_refusal_without_entering_policy() {
    let env = setup().await;
    let policy_calls = Arc::new(AtomicUsize::new(0));
    {
        let policy_calls = policy_calls.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, next| {
                policy_calls.fetch_add(1, Ordering::SeqCst);
                async move { next.run().await }
            })
            .unwrap();
    }
    let echo = Arc::new(echo_tool());
    let source = ToolExecutionSource {
        get: Some(Arc::new(move |_, _, _| Some(echo.clone()))),
        reject: Some(Arc::new(|_, _| {
            Some("executor refuses direct calls".into())
        })),
    };
    let pipeline = ToolExecutionPipeline::with_source(env.ctx.clone(), source).unwrap();
    let controller = Arc::new(AbortController::new());
    controller.abort();
    let prepared = pipeline
        .prepare(request("cancelled-refusal", "echo", &controller))
        .await
        .unwrap();
    let ScheduledToolPreparation::FinalResult { exec, result } = prepared else {
        panic!("a cancelled call must not be dispatched");
    };
    let result = pipeline
        .complete(&exec, hoh_tools::ScheduledToolDispatch::FinalResult(result))
        .await;
    assert_eq!(
        info(&result).map(|(_, code)| code),
        Some(TOOL_ABORTED_BEFORE_DISPATCH.to_owned())
    );
    assert_eq!(policy_calls.load(Ordering::SeqCst), 0);
}

// --------------------------------------------------------------- around-dispatch results

#[tokio::test]
async fn revalidates_a_cached_canonical_result_returned_from_a_different_dispatch() {
    let env = setup().await;
    let mut string_output = echo_tool();
    string_output.name = "string-output".into();
    string_output.execute = Arc::new(|_, _| Box::pin(async { Ok(json!("cached")) }));
    register(&env, string_output);
    let object_ran = Arc::new(AtomicUsize::new(0));
    let object_output = {
        let object_ran = object_ran.clone();
        define_tool(DefineToolOptions::new(
            "object-output",
            "Return one closed object.",
            json!({}),
            DefineToolOutput {
                schema: json!({
                    "type": "object",
                    "properties": { "ok": { "type": "boolean", "required": true } },
                    "additionalProperties": false,
                }),
                render: Arc::new(|_, value| vec![text(&value["ok"].to_string())]),
                presentation_meta: None,
            },
            move |_, _| {
                object_ran.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(json!({ "ok": true })) })
            },
        ))
        .unwrap()
    };
    register(&env, object_output);
    let cached = Arc::new(Mutex::new(None::<ToolExecutionResult>));
    {
        let cached = cached.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let cached = cached.clone();
                async move {
                    if event.exec.name == "string-output" {
                        let result = next.run().await?;
                        *lock(&cached) = Some(result.clone());
                        return Ok(result);
                    }
                    if event.exec.name == "object-output" {
                        return lock(&cached)
                            .clone()
                            .ok_or_else(|| "expected the first dispatch result".into());
                    }
                    next.run().await
                }
            })
            .unwrap();
    }
    let first =
        execute_tool_fixture(&env.ctx, input("cached-first", "string-output", json!({}))).await;
    let second =
        execute_tool_fixture(&env.ctx, input("cached-second", "object-output", json!({}))).await;
    assert_eq!(first.value(), Some(&json!("cached")));
    assert_eq!(object_ran.load(Ordering::SeqCst), 0);
    assert!(second.is_error());
    assert_eq!(
        info(&second),
        named("ToolOutputError", "INVALID_TOOL_OUTPUT")
    );
}

#[tokio::test]
async fn preserves_additional_contexts_supplied_by_an_around_dispatch_result() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, _| async {
            let mut success = ToolExecutionSuccess::new(
                json!("short-circuited with context"),
                vec![text("short-circuited with context")],
            );
            success.set_additional_contexts(Some(vec![msg("test", "from around dispatch")]));
            Ok(ToolExecutionResult::Success(success))
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("around-context", "echo", json!({}))).await;
    let contexts = result.additional_contexts().unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].content, vec![text("from around dispatch")]);
    assert_eq!(contexts[0].source.kind, "test");
}

#[tokio::test]
async fn returns_an_is_error_result_when_a_tools_execute_listener_throws() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, _| async {
            Err::<ToolExecutionResult, BoxError>("wrapper broke".into())
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert_eq!(result.content(), &[text("Error: wrapper broke")]);
    assert!(result.is_error());
    assert_eq!(result.error().unwrap().message, "wrapper broke");
    assert!(result.error().unwrap().info.is_none());
}

/// Cases of the spec's `it.each` over a failure thrown by a `tools/execute` listener after the
/// caller cancelled.
async fn listener_failing_after_cancellation(
    delegate: bool,
    kind: Option<Fail>,
) -> ToolExecutionResult {
    let env = setup().await;
    register(&env, echo_tool());
    let controller = Arc::new(AbortController::new());
    {
        let controller = controller.clone();
        let kind = Arc::new(kind);
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let (controller, kind) = (controller.clone(), kind.clone());
                async move {
                    if delegate {
                        next.run().await?;
                    }
                    controller.abort_with("caller stopped");
                    Err::<ToolExecutionResult, BoxError>(match kind.as_ref() {
                        Some(Fail::Domain) => {
                            Box::new(HarnessError::new("wrapper failed", "WRAPPER_FAILED"))
                        }
                        Some(Fail::DependencyAbort) => Box::new(AbortError::new("wrapper stopped")),
                        Some(other) => fail_with(other, &event.exec),
                        None => "wrapper broke".into(),
                    })
                }
            })
            .unwrap();
    }
    let mut call = input("c1", "echo", json!({ "text": "hi" }));
    call.signal = controller.signal();
    execute_tool_fixture(&env.ctx, call).await
}

#[tokio::test]
async fn classifies_failures_thrown_by_a_tools_execute_listener_after_cancellation() {
    let reason_after_body = listener_failing_after_cancellation(true, Some(Fail::Reason)).await;
    assert_eq!(info(&reason_after_body), named("AbortError", TOOL_ABORTED));
    let reason_before_body = listener_failing_after_cancellation(false, Some(Fail::Reason)).await;
    assert_eq!(
        info(&reason_before_body),
        named("AbortError", TOOL_ABORTED_BEFORE_DISPATCH)
    );
    let abort_error = listener_failing_after_cancellation(true, Some(Fail::DependencyAbort)).await;
    assert_eq!(info(&abort_error), named("AbortError", TOOL_ABORTED));
    let harness = listener_failing_after_cancellation(true, Some(Fail::Domain)).await;
    assert_eq!(info(&harness), named("HarnessError", "WRAPPER_FAILED"));
    let plain = listener_failing_after_cancellation(true, None).await;
    assert!(plain.is_error());
    assert!(plain.error().unwrap().info.is_none());
}

#[tokio::test]
async fn keeps_a_plain_tools_execute_listener_failure_without_info_when_the_caller_did_not_cancel()
{
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, _| async {
            Err::<ToolExecutionResult, BoxError>(Box::new(AbortError::new("wrapper stopped")))
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert_eq!(result.error().unwrap().message, "wrapper stopped");
    assert!(result.error().unwrap().info.is_none());
}

// ------------------------------------------------------------------ schema projection

#[tokio::test]
async fn schemas_snapshots_tool_schemas_instead_of_exposing_registry_objects() {
    let env = setup().await;
    register(&env, echo_tool());
    let mut first = env.tools.schemas(None, None).unwrap();
    first[0].description = "mutated".into();
    first[0].parameters.insert("mutated".into(), json!(true));
    let again = serde_json::to_value(env.tools.schemas(None, None).unwrap()).unwrap();
    assert_eq!(
        again,
        json!([{
            "name": "echo",
            "description": "echo arguments back",
            "parameters": { "type": "object", "properties": { "text": { "type": "string" } } },
        }])
    );
}

#[test]
fn schemas_snapshots_deeply_nested_parameters_without_native_recursion() {
    // The default test-thread stack is far smaller than the recursion this depth needs.
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let env = setup().await;
                let depth = 5_000;
                let mut nested = json!({ "type": "string" });
                for _ in 0..depth {
                    nested = json!({ "oneOf": [nested, { "type": "null" }] });
                }
                let mut tool = echo_tool();
                tool.name = "deep-schema".into();
                tool.parameters = serde_json::from_value(json!({
                    "type": "object",
                    "properties": { "nested": nested },
                }))
                .unwrap();
                register(&env, tool);
                let schemas = env.tools.schemas(None, None).unwrap();
                let mut cursor = &schemas[0].parameters["properties"]["nested"];
                let mut layers = 0;
                while let Some(one_of) = cursor.get("oneOf") {
                    cursor = &one_of[0];
                    layers += 1;
                }
                assert_eq!(layers, depth);
                assert_eq!(cursor, &json!({ "type": "string" }));
                // Leak the nested values: their recursive drop is not under test.
                std::mem::forget(schemas);
            });
        })
        .unwrap()
        .join()
        .unwrap();
}

#[tokio::test]
async fn rejects_schema_projection_when_a_raw_registration_is_not_lossless_json() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.name = "lossy-schema".into();
    tool.parameters = serde_json::from_value(json!({ "type": "object", "default": -0.0 })).unwrap();
    register(&env, tool);
    let error = env.tools.schemas(None, None).expect_err("rejected");
    assert!(error.to_string().contains(
        "tool \"lossy-schema\" parameters must be lossless JSON before schema projection"
    ));
}

// ------------------------------------------------------------------------ registry get

#[tokio::test]
async fn get_returns_the_registered_tool_definition_and_none_for_unknown_names() {
    let env = setup().await;
    register(&env, echo_tool());
    let found = env.tools.get("echo", None, None).expect("registered");
    assert_eq!(found.name, "echo");
    assert_eq!(found.description, "echo arguments back");
    assert!(env.tools.get("nope", None, None).is_none());
}

// ------------------------------------------------------------------ DSL and validateArgs

#[test]
fn handles_nested_object_and_array_of_object_specs() {
    let nested = hoh_tools::parameter_schema_spec_to_json_schema(&json!({
        "config": {
            "type": "object",
            "additionalProperties": true,
            "required": true,
            "properties": {
                "host": { "type": "string", "required": true },
                "port": { "type": "number" },
            },
        },
    }))
    .unwrap();
    assert_eq!(
        Value::Object(nested),
        json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "additionalProperties": true,
                    "properties": { "host": { "type": "string" }, "port": { "type": "number" } },
                    "required": ["host"],
                },
            },
            "required": ["config"],
        })
    );
    let servers = hoh_tools::parameter_schema_spec_to_json_schema(&json!({
        "servers": {
            "type": "array",
            "items": {
                "type": "object",
                "additionalProperties": true,
                "properties": {
                    "host": { "type": "string", "required": true },
                    "port": { "type": "number" },
                },
            },
        },
    }))
    .unwrap();
    assert_eq!(
        Value::Object(servers),
        json!({
            "type": "object",
            "properties": {
                "servers": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": true,
                        "properties": { "host": { "type": "string" }, "port": { "type": "number" } },
                        "required": ["host"],
                    },
                },
            },
        })
    );
}

#[tokio::test]
async fn define_tool_returns_a_valid_definition_that_registers_projects_and_executes() {
    let env = setup().await;
    let tool = define_tool(DefineToolOptions::new(
        "typed-echo",
        "A typed echo tool",
        json!({ "text": { "type": "string", "required": true }, "uppercase": { "type": "boolean" } }),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![text(value.as_str().unwrap())]),
            presentation_meta: None,
        },
        |args, _| {
            Box::pin(async move {
                let value = args["text"].as_str().unwrap();
                Ok(json!(if args["uppercase"] == json!(true) {
                    value.to_uppercase()
                } else {
                    value.to_owned()
                }))
            })
        },
    ))
    .unwrap();
    register(&env, tool);
    assert_eq!(
        serde_json::to_value(env.tools.schemas(None, None).unwrap()).unwrap(),
        json!([{
            "name": "typed-echo",
            "description": "A typed echo tool",
            "parameters": {
                "type": "object",
                "properties": { "text": { "type": "string" }, "uppercase": { "type": "boolean" } },
                "required": ["text"],
            },
        }])
    );
    let result = execute_tool_fixture(
        &env.ctx,
        input(
            "c1",
            "typed-echo",
            json!({ "text": "hello", "uppercase": true }),
        ),
    )
    .await;
    assert!(!result.is_error());
    assert_eq!(result.value(), Some(&json!("HELLO")));
    assert_eq!(result.content(), &[text("HELLO")]);
}

fn violations(spec: Value, args: Value) -> Vec<String> {
    validate_args(&spec, &args).unwrap()
}

#[test]
fn validate_args_does_not_apply_defaults() {
    assert!(
        violations(
            json!({ "limit": { "type": "number", "default": 25 } }),
            json!({})
        )
        .is_empty()
    );
}

#[test]
fn validate_args_enforces_type_correct_scalar_enum_declarations() {
    let spec = json!({ "n": { "type": "number", "enum": [1, 2] } });
    assert!(violations(spec.clone(), json!({ "n": 1 })).is_empty());
    assert_eq!(
        violations(spec, json!({ "n": 3 })),
        vec!["\"n\" must be one of [1,2]"]
    );
    let invalid = validate_args(
        &json!({ "n": { "type": "number", "enum": ["1", "2"] } }),
        &json!({ "n": 1 }),
    );
    assert!(invalid.expect_err("rejected").is_named("JsonSchemaError"));
}

#[test]
fn validate_args_checks_enum_membership_and_nested_bag_objects() {
    let spec = json!({ "color": { "type": "string", "enum": ["red", "green"] } });
    assert!(violations(spec.clone(), json!({ "color": "red" })).is_empty());
    assert_eq!(
        violations(spec, json!({ "color": "blue" })),
        vec!["\"color\" must be one of [\"red\",\"green\"]"]
    );
    let nested = json!({
        "config": {
            "type": "object",
            "additionalProperties": true,
            "required": true,
            "properties": { "host": { "type": "string", "required": true }, "port": { "type": "number" } },
        },
        "bag": { "type": "object", "additionalProperties": true },
    });
    assert!(
        violations(
            nested.clone(),
            json!({ "config": { "host": "h" }, "bag": { "anything": true } })
        )
        .is_empty()
    );
    assert_eq!(
        violations(nested, json!({ "config": { "port": 9 }, "bag": 5 })),
        vec![
            "missing required property \"config.host\"",
            "\"bag\" must be an object"
        ]
    );
}

#[test]
fn validate_args_recurses_into_array_items_and_checks_the_array_type() {
    let spec = json!({
        "tags": { "type": "array", "items": { "type": "string" } },
        "raw": { "type": "array" },
    });
    assert!(
        violations(
            spec.clone(),
            json!({ "tags": ["a", "b"], "raw": [1, {}, "x"] })
        )
        .is_empty()
    );
    assert_eq!(
        violations(spec.clone(), json!({ "tags": ["a", 2] })),
        vec!["\"tags[1]\" must be a string"]
    );
    assert_eq!(
        violations(spec, json!({ "tags": "nope" })),
        vec!["\"tags\" must be an array"]
    );
}

#[tokio::test]
async fn a_schema_invalid_call_surfaces_the_structured_error_on_the_result() {
    let env = setup().await;
    register(
        &env,
        content_tool(
            "reader",
            json!({ "path": { "type": "string", "required": true } }),
            |args, _| async move { Ok(vec![text(args["path"].as_str().unwrap())]) },
        ),
    );
    let result = execute_tool_fixture(&env.ctx, input("c1", "reader", json!({}))).await;
    assert!(result.is_error());
    let error = result.error().unwrap();
    assert_eq!(
        error.message,
        "invalid arguments: missing required property \"path\""
    );
    assert_eq!(
        error.info,
        Some(ToolErrorInfo::new("ToolArgsError", "INVALID_ARGS"))
    );
}

#[test]
fn a_tool_without_present_call_or_present_result_leaves_them_absent() {
    let tool = content_tool(
        "plain",
        json!({ "x": { "type": "string", "required": true } }),
        |_, _| async { Ok(vec![]) },
    );
    assert!(tool.present_call.is_none());
    assert!(tool.present_result.is_none());
}

#[test]
fn present_result_validates_softly_and_never_throws() {
    let tool = content_tool_with(
        "demo",
        json!({ "path": { "type": "string", "required": true } }),
        |_, _| async { Ok(vec![]) },
        |options| {
            options.present_result = Some(Arc::new(|args, result| {
                Some(hoh_tools::ToolResultView::Generic(
                    hoh_tools::GenericResultView {
                        title: Some(args["path"].as_str().unwrap().to_owned()),
                        content: Some(result.content.clone()),
                    },
                ))
            }));
        },
    );
    let present = tool.present_result.as_ref().unwrap();
    let result = hoh_tools::ToolResult {
        content: vec![],
        is_error: false,
        meta: None,
    };
    assert!(present(&json!({ "wrong": 1 }), &result).is_none());
    assert!(present(&json!({ "path": "/a" }), &result).is_some());
}
