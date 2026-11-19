//! Regression tests of the Codex review round over the port (`plans/briefs/fix-s7-tools.md`): every case
//! asserts the expectation of the TypeScript source, cited per test.

mod common;

use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use common::*;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Kernel, Undo};
use hoh_llm::{MessageSource, UserMessage};
use hoh_system_prompt::{PromptSection, PromptText};
use hoh_timeout::AbortController;
use hoh_tools::shim::BoxFuture;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, PostToolDecision, PreToolDecision,
    ScheduledToolPreparation, TOOL_ABORTED_BEFORE_DISPATCH, ToolBatchInput, ToolBatchResult,
    ToolCallView, ToolDefinition, ToolExecutionPipeline, ToolExecutionResult, ToolExecutor,
    ToolModelInput, ToolPromptSection, ToolPromptText, ToolRegistrationOptions, ToolsError,
    ToolsExecute, ToolsPostExecute, ToolsPreExecute, define_tool, is_json_schema_record,
    is_plain_json_array, is_plain_json_record, project_tool_schema,
};
use serde_json::{Map, Number, Value, json};

fn negative_zero() -> Value {
    Value::Number(Number::from_f64(-0.0).expect("finite"))
}

/// A message whose `source` carries `-0`, which is not losslessly JSON-serializable.
fn lossy_source_message() -> UserMessage {
    let mut source = MessageSource::new("test");
    source.data.insert("n".into(), negative_zero());
    hoh_llm::create_user_message(hoh_llm::NewUserMessage {
        content: vec![text("context")],
        source,
    })
}

const LOSSY: &str = "Error: tool result must be losslessly JSON-serializable";

// ------------------------------------------------- 1. canonical results are frozen

/// tools.spec.ts: "freezes core dispatch outcomes before around and post listeners can observe them".
#[tokio::test]
async fn a_wrapper_cannot_change_a_canonical_result_in_place() {
    let env = setup().await;
    register(&env, echo_tool());
    // The outer wrapper is registered first. It matches the enum and reads the public fields: what it
    // sees is what the inner wrapper left behind, and in the source the result is frozen, so the inner
    // `Reflect.set` changed nothing.
    let outer_saw = Arc::new(Mutex::new(Vec::<(serde_json::Value, String)>::new()));
    {
        let outer_saw = outer_saw.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, next| {
                let outer_saw = outer_saw.clone();
                async move {
                    let result = next.run().await?;
                    let ToolExecutionResult::Success(success) = &result else {
                        panic!("a successful call, got {result:?}");
                    };
                    let rendered = success
                        .content()
                        .iter()
                        .filter_map(|block| block.text_value())
                        .collect::<Vec<_>>()
                        .join("\n");
                    lock(&outer_saw).push((success.value().clone(), rendered));
                    Ok(result)
                }
            })
            .unwrap();
    }
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, next| async move {
            let mut result = next.run().await?;
            if let ToolExecutionResult::Success(success) = &mut result {
                // The source's `Reflect.set` on the frozen result fails: nothing changes.
                success.set_value(json!(42));
                success.set_content(vec![text("mutated")]);
            }
            Ok(result)
        })
        .unwrap();

    let result = execute_tool_fixture(
        &env.ctx,
        input("frozen-canonical", "echo", json!({ "text": "original" })),
    )
    .await;
    assert!(!result.is_error(), "{result:?}");
    assert_eq!(result.value(), Some(&json!("original")));
    assert_eq!(result.text(), "original");
    assert_eq!(
        *lock(&outer_saw),
        [(json!("original"), "original".to_string())],
        "the outer wrapper must see the frozen original, not the inner wrapper's edit"
    );
}

/// pipeline.ts:732 `toolErrorResult` returns a plain, unfrozen object (only successes are marked canonical
/// and frozen): a wrapper's in-place edit of a body failure is a real edit and is what the next layer sees.
#[tokio::test]
async fn a_wrapper_edit_of_a_body_failure_takes_effect() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.execute = Arc::new(|_, _| Box::pin(async { Err("original failure".into()) }));
    register(&env, tool);
    let outer_saw = Arc::new(Mutex::new(Vec::<(String, String, bool)>::new()));
    {
        let outer_saw = outer_saw.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, next| {
                let outer_saw = outer_saw.clone();
                async move {
                    let result = next.run().await?;
                    let ToolExecutionResult::Failure(failure) = &result else {
                        panic!("a failed call, got {result:?}");
                    };
                    lock(&outer_saw).push((
                        failure.error().message.clone(),
                        failure
                            .content()
                            .iter()
                            .filter_map(|block| block.text_value())
                            .collect::<Vec<_>>()
                            .join("\n"),
                        failure.meta().is_some() || failure.additional_contexts().is_some(),
                    ));
                    Ok(result)
                }
            })
            .unwrap();
    }
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, next| async move {
            let mut result = next.run().await?;
            if let ToolExecutionResult::Failure(failure) = &mut result {
                failure.set_error(hoh_tools::ToolFailure {
                    message: "mutated".into(),
                    info: None,
                });
                failure.set_content(vec![text("mutated")]);
                failure.set_meta(Some(json!({ "mutated": true })));
            }
            Ok(result)
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("frozen-failure", "echo", json!({}))).await;
    assert!(result.is_error(), "{result:?}");
    assert_eq!(
        *lock(&outer_saw),
        [("mutated".to_string(), "mutated".to_string(), true)]
    );
    assert_eq!(result.text(), "mutated");
    assert_eq!(
        result.error().map(|error| error.message.as_str()),
        Some("mutated")
    );
}

/// pipeline.ts:594,509: a normalized failure is only marked canonical, never frozen; a post-execute
/// decision may still change it, and the change is what the caller gets.
#[tokio::test]
async fn post_execute_changes_a_normalized_failure_and_the_change_is_visible() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.execute = Arc::new(|_, _| Box::pin(async { Err("original failure".into()) }));
    register(&env, tool);
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let seen = seen.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, _| {
                let seen = seen.clone();
                async move {
                    lock(&seen).push(event.result.text());
                    Ok(PostToolDecision::Accept {
                        content: Some(vec![text("replaced by post-execute")]),
                        value: None,
                        additional_contexts: Some(vec![lossy_free_message()]),
                    })
                }
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("post-failure", "echo", json!({}))).await;
    assert!(result.is_error(), "{result:?}");
    assert_eq!(*lock(&seen), ["Error: original failure"]);
    assert_eq!(result.text(), "replaced by post-execute");
    assert_eq!(
        result.error().map(|error| error.message.as_str()),
        Some("original failure")
    );
    assert_eq!(result.additional_contexts().map(Vec::len), Some(1));
}

fn lossy_free_message() -> UserMessage {
    hoh_llm::create_user_message(hoh_llm::NewUserMessage {
        content: vec![text("post context")],
        source: MessageSource::new("test"),
    })
}

/// pipeline.ts:397,403,612: `finish` materializes and deep-freezes the final outcome, so what the caller
/// gets back cannot be edited: a setter on the returned success or failure changes nothing.
#[tokio::test]
async fn the_returned_final_result_is_frozen_for_success_and_failure() {
    let env = setup().await;
    register(&env, echo_tool());
    let mut failing = echo_tool();
    failing.name = "failing".into();
    failing.execute = Arc::new(|_, _| Box::pin(async { Err("original failure".into()) }));
    register(&env, failing);

    let mut success = execute_tool_fixture(
        &env.ctx,
        input("final-ok", "echo", json!({ "text": "original" })),
    )
    .await;
    let ToolExecutionResult::Success(inner) = &mut success else {
        panic!("a successful call, got {success:?}");
    };
    inner.set_value(json!(42));
    inner.set_content(vec![text("mutated")]);
    inner.set_meta(Some(json!({ "mutated": true })));
    inner.set_concludes_turn(true);
    assert_eq!(success.value(), Some(&json!("original")));
    assert_eq!(success.text(), "original");
    assert_eq!(success.meta(), None);
    assert!(!success.concludes_turn());

    let mut failure =
        execute_tool_fixture(&env.ctx, input("final-err", "failing", json!({}))).await;
    let ToolExecutionResult::Failure(inner) = &mut failure else {
        panic!("a failed call, got {failure:?}");
    };
    inner.set_error(hoh_tools::ToolFailure {
        message: "mutated".into(),
        info: None,
    });
    inner.set_content(vec![text("mutated")]);
    inner.set_additional_contexts(Some(vec![lossy_free_message()]));
    assert_eq!(
        failure.error().map(|error| error.message.as_str()),
        Some("original failure")
    );
    assert_eq!(failure.text(), "Error: original failure");
    assert_eq!(failure.additional_contexts(), None);

    // An immediately final outcome (an unknown tool never dispatches) is frozen as well.
    let mut unknown =
        execute_tool_fixture(&env.ctx, input("final-unknown", "nope", json!({}))).await;
    let before = unknown.text();
    let ToolExecutionResult::Failure(inner) = &mut unknown else {
        panic!("an unknown tool fails, got {unknown:?}");
    };
    inner.set_content(vec![text("mutated")]);
    assert_eq!(unknown.text(), before);
}

#[tokio::test]
async fn a_copied_result_is_normalized_again_and_its_invalid_value_fails() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, next| async move {
            let result = next.run().await?;
            // A copy is a new object (spread syntax): it loses the canonical mark.
            let mut copy = result.clone();
            if let ToolExecutionResult::Success(success) = &mut copy {
                success.set_value(json!(42));
            }
            Ok(copy)
        })
        .unwrap();

    let result = execute_tool_fixture(
        &env.ctx,
        input("copied", "echo", json!({ "text": "original" })),
    )
    .await;
    assert!(result.is_error());
    assert_eq!(
        result
            .error()
            .and_then(|error| error.info.as_ref())
            .map(|info| info.code.as_str()),
        Some("INVALID_TOOL_OUTPUT")
    );
}

// ------------------------------------------------- 2. atomic executor registration

struct GatedExecutor {
    entered: Mutex<mpsc::Sender<()>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl ToolExecutor for GatedExecutor {
    fn sections(&self) -> Vec<ToolPromptSection> {
        // Inside the registration's setup, after the one-executor check has been made.
        self.entered.lock().unwrap().send(()).unwrap();
        let _ = self
            .release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10));
        Vec::new()
    }
    fn describe(&self, _: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError> {
        Err("not used".into())
    }
    fn execute(&self, _: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
        Box::pin(async { Err("not used".into()) })
    }
}

struct PlainExecutor;

impl ToolExecutor for PlainExecutor {
    fn describe(&self, _: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError> {
        Err("not used".into())
    }
    fn execute(&self, _: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
        Box::pin(async { Err("not used".into()) })
    }
}

/// index.ts:325–349: the check and the insertion are one synchronous step.
#[tokio::test(flavor = "multi_thread")]
async fn exactly_one_of_two_overlapping_executor_registrations_in_a_scope_succeeds() {
    let env = setup_without_executor().await;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first: Arc<dyn ToolExecutor> = Arc::new(GatedExecutor {
        entered: Mutex::new(entered_tx),
        release: Mutex::new(release_rx),
    });
    let service = env.service().clone();
    let ctx = env.ctx.clone();
    let worker = std::thread::spawn(move || service.set_executor(&ctx, first));
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the first registration reached its setup");

    // The second registration overlaps the first one's setup.
    let second = env
        .service()
        .set_executor(&env.ctx, Arc::new(PlainExecutor));
    release_tx.send(()).unwrap();
    let first = worker.join().expect("worker finishes");

    let outcomes = [first.is_ok(), second.is_ok()];
    assert_eq!(outcomes.iter().filter(|ok| **ok).count(), 1, "{outcomes:?}");
    let failure = [&first, &second]
        .into_iter()
        .find_map(|outcome| outcome.as_ref().err())
        .expect("one registration fails");
    assert_eq!(
        failure.to_string(),
        "Only one tool executor may be registered in each scope"
    );
    for disposer in [first, second].into_iter().flatten() {
        disposer.dispose().await;
    }
    env.kernel.shutdown().await;
}

/// A setup that fails after the reservation gives the scope back: the next registration succeeds.
#[tokio::test]
async fn a_failed_executor_setup_releases_its_reservation() {
    let env = setup_without_executor().await;
    let _taken = env
        .prompt
        .section(
            &env.ctx,
            PromptSection {
                name: "duplicate".into(),
                order: 1.0,
                text: PromptText::provider(|_| Ok(String::new())),
                interpolate: None,
                complete: false,
            },
        )
        .unwrap();

    struct Sectioned;
    impl ToolExecutor for Sectioned {
        fn sections(&self) -> Vec<ToolPromptSection> {
            vec![ToolPromptSection {
                name: "duplicate".into(),
                order: 1.0,
                interpolate: None,
                text: ToolPromptText::Static("x".into()),
            }]
        }
        fn describe(&self, _: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError> {
            Err("not used".into())
        }
        fn execute(&self, _: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
            Box::pin(async { Err("not used".into()) })
        }
    }

    let failed = env.service().set_executor(&env.ctx, Arc::new(Sectioned));
    assert!(
        failed.is_err(),
        "the duplicate section name must fail the setup"
    );
    let retried = env
        .service()
        .set_executor(&env.ctx, Arc::new(PlainExecutor));
    assert!(
        retried.is_ok(),
        "{:?}",
        retried.err().map(|e| e.to_string())
    );
    retried.unwrap().dispose().await;
    env.kernel.shutdown().await;
}

// ------------------------------------------------- 3. batch start order

fn parallel_echo(name: &str) -> ToolDefinition {
    let mut tool = echo_tool();
    tool.name = name.into();
    tool.is_concurrency_safe = Some(Arc::new(|_| true));
    tool
}

fn trace_listeners(env: &Env, trace: &Arc<Mutex<Vec<String>>>) {
    {
        let trace = trace.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
                let trace = trace.clone();
                async move {
                    lock(&trace).push(format!("pre({})", event.exec.name));
                    next.run().await
                }
            })
            .unwrap();
    }
}

/// batch.ts:163: `startCall` runs `dispatch` synchronously, so `execute(A)` precedes `pre(B)`.
#[tokio::test]
async fn enters_the_execute_wrapper_of_a_call_before_preparing_the_next_one() {
    let env = setup().await;
    register(&env, parallel_echo("A"));
    register(&env, parallel_echo("B"));
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    trace_listeners(&env, &trace);
    {
        let trace = trace.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let trace = trace.clone();
                async move {
                    lock(&trace).push(format!("execute({})", event.exec.name));
                    next.run().await
                }
            })
            .unwrap();
    }
    let mut batch = ToolBatchInput::new(
        vec![
            model_call("a", "A", r#"{"text":"a"}"#),
            model_call("b", "B", r#"{"text":"b"}"#),
        ],
        live_signal(),
    );
    batch.max_parallel_tool_calls = Some(2);
    let result = env.service().execute(batch).await.unwrap();
    assert_eq!(result.results.len(), 2);
    assert_eq!(
        *lock(&trace),
        ["pre(A)", "execute(A)", "pre(B)", "execute(B)"]
    );
}

#[tokio::test]
async fn a_wrapper_that_aborts_the_batch_stops_the_next_call_from_being_prepared() {
    let env = setup().await;
    register(&env, parallel_echo("A"));
    register(&env, parallel_echo("B"));
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    trace_listeners(&env, &trace);
    let controller = Arc::new(AbortController::new());
    {
        let trace = trace.clone();
        let controller = controller.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let trace = trace.clone();
                let controller = controller.clone();
                async move {
                    lock(&trace).push(format!("execute({})", event.exec.name));
                    if event.exec.name == "A" {
                        controller.abort();
                    }
                    next.run().await
                }
            })
            .unwrap();
    }
    let mut batch = ToolBatchInput::new(
        vec![
            model_call("a", "A", r#"{"text":"a"}"#),
            model_call("b", "B", r#"{"text":"b"}"#),
        ],
        controller.signal(),
    );
    batch.max_parallel_tool_calls = Some(2);
    let result = env.service().execute(batch).await.unwrap();
    assert_eq!(*lock(&trace), ["pre(A)", "execute(A)"]);
    assert_eq!(result.results.len(), 2);
    let skipped = &result.results[1].result;
    assert_eq!(
        skipped
            .error()
            .and_then(|error| error.info.as_ref())
            .map(|info| info.code.as_str()),
        Some(TOOL_ABORTED_BEFORE_DISPATCH)
    );
}

/// Wrapper layers ahead of the logging one; far deeper than any fixed number of scheduler turns.
const DEEP_CHAIN: usize = 40;

/// Passthrough wrappers ahead of the logging one: each nested layer runs on a task of its own, so the
/// logging wrapper is entered only after several scheduler turns.
fn nested_passthrough(env: &Env, layers: usize) {
    for _ in 0..layers {
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, next| async move {
                next.run().await
            })
            .unwrap();
    }
}

/// batch.ts:163 on a multi-thread runtime and behind nested wrapper layers: the dispatch announces its
/// start, so no schedule lets `pre(B)` overtake `execute(A)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_execute_wrapper_is_entered_before_the_next_preparation_on_every_schedule() {
    for round in 0..40 {
        let env = setup().await;
        register(&env, parallel_echo("A"));
        register(&env, parallel_echo("B"));
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        trace_listeners(&env, &trace);
        nested_passthrough(&env, DEEP_CHAIN);
        {
            let trace = trace.clone();
            env.ctx
                .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                    let trace = trace.clone();
                    async move {
                        lock(&trace).push(format!("execute({})", event.exec.name));
                        next.run().await
                    }
                })
                .unwrap();
        }
        let mut batch = ToolBatchInput::new(
            vec![
                model_call("a", "A", r#"{"text":"a"}"#),
                model_call("b", "B", r#"{"text":"b"}"#),
            ],
            live_signal(),
        );
        batch.max_parallel_tool_calls = Some(2);
        let result = env.service().execute(batch).await.unwrap();
        assert_eq!(result.results.len(), 2);
        assert_eq!(
            *lock(&trace),
            ["pre(A)", "execute(A)", "pre(B)", "execute(B)"],
            "round {round}"
        );
        env.kernel.shutdown().await;
    }
}

/// The same order on a single-thread runtime, where no other worker could help: the start is
/// acknowledged by the call reaching its body, so the 40 nested layers (each on a task of its own) are all
/// entered before `pre(B)`, with the logging wrapper innermost.
#[tokio::test]
async fn a_deep_wrapper_chain_is_entered_before_the_next_preparation() {
    let env = setup().await;
    register(&env, parallel_echo("A"));
    register(&env, parallel_echo("B"));
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    trace_listeners(&env, &trace);
    nested_passthrough(&env, DEEP_CHAIN);
    {
        let trace = trace.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let trace = trace.clone();
                async move {
                    lock(&trace).push(format!("execute({})", event.exec.name));
                    next.run().await
                }
            })
            .unwrap();
    }
    let mut batch = ToolBatchInput::new(
        vec![
            model_call("a", "A", r#"{"text":"a"}"#),
            model_call("b", "B", r#"{"text":"b"}"#),
        ],
        live_signal(),
    );
    batch.max_parallel_tool_calls = Some(2);
    let result = env.service().execute(batch).await.unwrap();
    assert_eq!(result.results.len(), 2);
    assert_eq!(
        *lock(&trace),
        ["pre(A)", "execute(A)", "pre(B)", "execute(B)"]
    );
}

/// A wrapper that awaits before calling `next`: the source's `dispatch` returns to the scheduler at that
/// first suspension, so `pre(B)` runs right after it. Here A's wrapper is released only by `pre(B)`, and
/// no timer may be involved (the paused clock must not move).
#[tokio::test(start_paused = true)]
async fn the_next_preparation_follows_the_first_suspension_of_a_wrapper_at_once() {
    let env = setup().await;
    register(&env, parallel_echo("A"));
    register(&env, parallel_echo("B"));
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    trace_listeners(&env, &trace);
    let release = Arc::new(tokio::sync::Notify::new());
    {
        let release = release.clone();
        let trace = trace.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
                let release = release.clone();
                let trace = trace.clone();
                async move {
                    if event.exec.name == "B" {
                        lock(&trace).push("release(A)".into());
                        release.notify_one();
                    }
                    next.run().await
                }
            })
            .unwrap();
    }
    {
        let release = release.clone();
        let trace = trace.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let release = release.clone();
                let trace = trace.clone();
                async move {
                    lock(&trace).push(format!("execute({})", event.exec.name));
                    if event.exec.name == "A" {
                        // Released only by `pre(B)`: the scheduler must get there from this suspension.
                        release.notified().await;
                        lock(&trace).push("resumed(A)".into());
                    }
                    next.run().await
                }
            })
            .unwrap();
    }
    let mut batch = ToolBatchInput::new(
        vec![
            model_call("a", "A", r#"{"text":"a"}"#),
            model_call("b", "B", r#"{"text":"b"}"#),
        ],
        live_signal(),
    );
    batch.max_parallel_tool_calls = Some(2);
    let started = tokio::time::Instant::now();
    let result = env.service().execute(batch).await.unwrap();
    assert_eq!(result.results.len(), 2);
    assert!(
        started.elapsed() < std::time::Duration::from_millis(5),
        "no timer may delay the next preparation: {:?}",
        started.elapsed()
    );
    let trace = lock(&trace).clone();
    let at = |entry: &str| trace.iter().position(|seen| seen == entry).expect(entry);
    assert!(
        at("pre(A)") < at("execute(A)")
            && at("execute(A)") < at("pre(B)")
            && at("release(A)") < at("resumed(A)"),
        "{trace:?}"
    );
}

/// The same trace when the wrapper of A aborts the batch: B is never prepared.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_aborting_wrapper_stops_the_next_preparation_on_every_schedule() {
    for round in 0..40 {
        let env = setup().await;
        register(&env, parallel_echo("A"));
        register(&env, parallel_echo("B"));
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        trace_listeners(&env, &trace);
        nested_passthrough(&env, DEEP_CHAIN);
        let controller = Arc::new(AbortController::new());
        {
            let trace = trace.clone();
            let controller = controller.clone();
            env.ctx
                .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                    let trace = trace.clone();
                    let controller = controller.clone();
                    async move {
                        lock(&trace).push(format!("execute({})", event.exec.name));
                        if event.exec.name == "A" {
                            controller.abort();
                        }
                        next.run().await
                    }
                })
                .unwrap();
        }
        let mut batch = ToolBatchInput::new(
            vec![
                model_call("a", "A", r#"{"text":"a"}"#),
                model_call("b", "B", r#"{"text":"b"}"#),
            ],
            controller.signal(),
        );
        batch.max_parallel_tool_calls = Some(2);
        let result = env.service().execute(batch).await.unwrap();
        assert_eq!(*lock(&trace), ["pre(A)", "execute(A)"], "round {round}");
        assert_eq!(result.results.len(), 2);
        env.kernel.shutdown().await;
    }
}

// ------------------------------------------------- 5. complete message validation

fn context_tool(defer: bool) -> ToolDefinition {
    let mut options = DefineToolOptions::new(
        "contextual",
        "",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![text(value.as_str().unwrap())]),
            presentation_meta: None,
        },
        move |_, exec| {
            if defer {
                exec.defer_context(lossy_source_message());
            }
            Box::pin(async { Ok(json!("done")) })
        },
    );
    options.defer_loading = false;
    define_tool(options).unwrap()
}

/// pipeline.ts:613–626,694–699: the complete serialized message is checked, not only its content.
#[tokio::test]
async fn rejects_a_deferred_context_whose_source_is_not_lossless() {
    let env = setup().await;
    register(&env, context_tool(true));
    let result = execute_tool_fixture(&env.ctx, input("c", "contextual", json!({}))).await;
    assert!(result.is_error(), "{result:?}");
    assert_eq!(result.text(), LOSSY);
}

#[tokio::test]
async fn rejects_a_post_policy_context_whose_source_is_not_lossless() {
    let env = setup().await;
    register(&env, context_tool(false));
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Accept {
                content: None,
                value: None,
                additional_contexts: Some(vec![lossy_source_message()]),
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c", "contextual", json!({}))).await;
    assert!(result.is_error(), "{result:?}");
    assert_eq!(result.text(), LOSSY);
}

// ------------------------------------------------- 7. constructor without a tools service

/// pipeline.ts:66–73,172: the constructor only reads `ctx.tools`; the lookup in `createExecution` sits
/// outside the `try`, so `prepare` rejects with the engine's bare `TypeError` for `undefined.executable`.
#[tokio::test]
async fn builds_a_pipeline_without_a_tools_service_and_prepare_rejects_with_a_plain_type_error() {
    let kernel = Kernel::start();
    let pipeline = ToolExecutionPipeline::new(kernel.root()).expect("constructs without a service");
    let error = match pipeline
        .try_prepare(input("c", "anything", json!({})))
        .await
    {
        Ok(_) => panic!("the lookup without a registry must reject"),
        Err(error) => error,
    };
    let plain = error
        .downcast_ref::<ToolsError>()
        .expect("a bare TypeError, not a harness error");
    assert_eq!(plain.name(), "TypeError");
    assert_eq!(
        plain.message(),
        "Cannot read properties of undefined (reading 'executable')"
    );
    // The infallible form keeps its signature and settles as a plain failure carrying that same error.
    let ScheduledToolPreparation::FinalResult { result, .. } =
        pipeline.prepare(input("c", "anything", json!({}))).await
    else {
        panic!("the infallible form settles as a failure");
    };
    let failure = result.error().expect("a failure result");
    assert_eq!(
        failure.message,
        "Cannot read properties of undefined (reading 'executable')"
    );
    assert!(
        failure.info.is_none(),
        "no invented TOOL_EXECUTOR_UNAVAILABLE"
    );
    kernel.shutdown().await;
}

// ------------------------------------------------- 9. plain errors stay plain

/// schema.ts:576: `defineTool` with `timeoutMs: 0` throws a bare `Error`.
#[tokio::test]
async fn a_bad_timeout_is_a_plain_error_without_harness_identity() {
    let mut options = DefineToolOptions::new(
        "slow",
        "",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, _| Vec::new()),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(json!("x")) }),
    );
    options.timeout_ms = Some(0.0);
    let error = define_tool(options).err().expect("rejected");
    assert!(HarnessError::of(error.as_ref()).is_none());
    assert_eq!(
        error.downcast_ref::<ToolsError>(),
        Some(&ToolsError::Error(
            "defineTool(slow): timeoutMs must be a positive finite number".into()
        ))
    );
}

/// The failure propagates through a tool as a plain error: the source omits `error.info`.
#[tokio::test]
async fn a_plain_define_tool_error_propagated_through_a_tool_carries_no_info() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.name = "propagates".into();
    tool.execute = Arc::new(|_, _| {
        Box::pin(async {
            let mut options = DefineToolOptions::new(
                "inner",
                "",
                json!({}),
                DefineToolOutput {
                    schema: json!({ "type": "string" }),
                    render: Arc::new(|_, _| Vec::new()),
                    presentation_meta: None,
                },
                |_, _| Box::pin(async { Ok(json!("x")) }),
            );
            options.timeout_ms = Some(0.0);
            define_tool(options)?;
            Ok(json!("unreachable"))
        })
    });
    register(&env, tool);
    let result = execute_tool_fixture(&env.ctx, input("c", "propagates", json!({}))).await;
    let failure = result.error().expect("failed");
    assert_eq!(
        failure.message,
        "defineTool(inner): timeoutMs must be a positive finite number"
    );
    assert_eq!(failure.info, None);
}

/// schema.ts:644: a lossy parameter projection is a plain error as well.
#[test]
fn a_lossy_parameter_projection_is_a_plain_error() {
    let mut parameters = Map::new();
    parameters.insert("zero".into(), negative_zero());
    let mut tool = echo_tool();
    tool.name = "lossy".into();
    tool.parameters = parameters;
    let error = project_tool_schema(&tool, true).expect_err("rejected");
    assert_eq!(
        error,
        ToolsError::Error(
            "tool \"lossy\" parameters must be lossless JSON before schema projection".into()
        )
    );
}

// ------------------------------------------------- 10. configuration

/// index.ts:101–104: the source's object schema ignores unrelated fields and rejects only the removed ones.
#[tokio::test]
async fn the_plugin_accepts_unrelated_configuration_fields() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<hoh_system_prompt::SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = ctx
        .plugin::<hoh_tools::ToolRuntime>(json!({ "unused": true }))
        .expect("mounts with an unrelated field");
    fiber.settled().await.expect("settles");
    kernel.shutdown().await;
}

// ------------------------------------------------- 11. explicit null

/// presentation.ts:67: an explicit `rawInput: null` survives; absence stays absence.
#[test]
fn raw_input_distinguishes_an_explicit_null_from_an_absent_field() {
    let explicit = json!({ "card": "generic", "title": "x", "rawInput": null });
    let view: ToolCallView = serde_json::from_value(explicit.clone()).unwrap();
    let ToolCallView::Generic(generic) = &view else {
        panic!("generic card");
    };
    assert_eq!(generic.raw_input, Some(Value::Null));
    assert_eq!(serde_json::to_value(&view).unwrap(), explicit);

    let absent = json!({ "card": "generic", "title": "x" });
    let view: ToolCallView = serde_json::from_value(absent.clone()).unwrap();
    let ToolCallView::Generic(generic) = &view else {
        panic!("generic card");
    };
    assert_eq!(generic.raw_input, None);
    assert_eq!(serde_json::to_value(&view).unwrap(), absent);
}

// ------------------------------------------------- 12. public JSON-schema helpers

/// json-schema.ts:115,152,161.
#[test]
fn exports_the_record_and_array_predicates() {
    assert!(is_plain_json_record(&json!({})));
    assert!(!is_plain_json_record(&json!([])));
    assert!(is_json_schema_record(&json!({ "a": 1 })));
    assert!(!is_json_schema_record(&json!([])));
    assert!(is_plain_json_array(&json!([])));
    assert!(is_plain_json_array(&json!([1, "a"])));
    for scalar in [json!(null), json!(true), json!(1), json!("x")] {
        assert!(!is_plain_json_record(&scalar), "{scalar}");
        assert!(!is_json_schema_record(&scalar), "{scalar}");
        assert!(!is_plain_json_array(&scalar), "{scalar}");
    }
    assert!(!is_plain_json_array(&json!({})));
}

// ------------------------------------------------- 13. composite disposer

/// The trace of the plugin below; one test owns it.
static COMPOSITE_ORDER: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The source's plugin of tools.spec.ts:2381: one effect of the plugin context nests the unregistration
/// between two teardown steps.
struct CompositePlugin;

#[derive(hoh_kernel::Inject)]
struct CompositeDeps {
    tools: hoh_kernel::Service<hoh_tools::Tools>,
}

impl hoh_kernel::Plugin for CompositePlugin {
    const NAME: &'static str = "composite-teardown";
    type Config = hoh_kernel::NoConfig;
    type Deps = CompositeDeps;
    async fn apply(
        ctx: hoh_kernel::Context,
        deps: CompositeDeps,
        _config: hoh_kernel::Cfg<hoh_kernel::NoConfig>,
    ) -> Result<(), BoxError> {
        let tools = deps.tools.clone();
        let inner = ctx.clone();
        ctx.try_effect("composite", |undos| -> Result<(), BoxError> {
            undos.push(Undo::sync(|| {
                lock(&COMPOSITE_ORDER).push("disposed-last".into());
            }))?;
            let mut nested = echo_tool();
            nested.name = "nested".into();
            undos.push(tools.register(&inner, nested, ToolRegistrationOptions::default())?)?;
            lock(&COMPOSITE_ORDER).push("registered".into());
            let probe = tools.clone();
            undos.push(Undo::future(move || async move {
                // Yield during the earlier teardown: a wrapper would run the removal concurrently.
                tokio::task::yield_now().await;
                lock(&COMPOSITE_ORDER).push(
                    if probe.get("nested", None, None).is_some() {
                        "first: still registered"
                    } else {
                        "first: already gone"
                    }
                    .into(),
                );
                Ok(())
            }))?;
            Ok(())
        })?;
        Ok(())
    }
}

/// tools.spec.ts:2381: `register()` returns the exact effect disposer, so a composite effect nests the
/// unregistration at its LIFO position. The source disposes the owning plugin: that trigger would expose
/// a wrapper disposer that removes the tool concurrently with its siblings.
#[tokio::test]
async fn register_returns_the_exact_disposer_and_a_plugin_teardown_nests_the_removal() {
    let env = setup().await;
    lock(&COMPOSITE_ORDER).clear();
    let fiber = env
        .ctx
        .plugin::<CompositePlugin>(json!({}))
        .expect("plugin mounts");
    fiber.settled().await.expect("plugin settles");
    assert!(env.service().get("nested", None, None).is_some());
    fiber.dispose().await;
    assert_eq!(
        *lock(&COMPOSITE_ORDER),
        ["registered", "first: still registered", "disposed-last"]
    );
    assert!(env.service().get("nested", None, None).is_none());
    env.kernel.shutdown().await;
}

// ------------------------------------------------- 8'. a forged output schema is a body failure

/// pipeline.ts:560 + json-schema.ts assertNever: the validator throws inside `createSuccessResult`, the
/// body catch normalizes that exception into a failure result, and post-execute still runs.
#[tokio::test]
async fn a_forged_output_schema_fails_the_body_and_post_execute_still_runs() {
    let env = setup().await;
    let mut forged = echo_tool();
    forged.name = "forged".into();
    forged.output.schema = json!({ "type": "tuple" });
    forged.execute = Arc::new(|_, _| Box::pin(async { Ok(json!(1)) }));
    let forged = Arc::new(forged);
    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let observed = observed.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
                let observed = observed.clone();
                async move {
                    lock(&observed).push(
                        event
                            .result
                            .error()
                            .map(|error| error.message.clone())
                            .unwrap_or_else(|| "no error".into()),
                    );
                    next.run().await
                }
            })
            .unwrap();
    }
    let source = hoh_tools::ToolExecutionSource {
        get: Some(Arc::new(move |_, _, _| Some(forged.clone()))),
        reject: None,
    };
    let pipeline = ToolExecutionPipeline::with_source(env.ctx.clone(), source).unwrap();
    let prepared = pipeline.prepare(input("c", "forged", json!({}))).await;
    let exec = prepared.exec().clone();
    let ScheduledToolPreparation::Dispatch(dispatching) = prepared else {
        panic!("the forged tool is visible and must dispatch");
    };
    let outcome = pipeline.dispatch(dispatching).await;
    let result = pipeline.complete(&exec, outcome).await;
    assert!(result.is_error(), "{result:?}");
    assert_eq!(
        *lock(&observed),
        ["unreachable variant in JsonSchemaType: \"tuple\""],
        "post-execute observed the normalized exception"
    );
    assert_eq!(
        result.error().map(|error| error.message.as_str()),
        Some("unreachable variant in JsonSchemaType: \"tuple\"")
    );
}
