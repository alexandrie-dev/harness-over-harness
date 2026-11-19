//! Regression tests of the second review round (`hoh-tools.142420-hoh-tools.md`): every case asserts
//! the expectation of the TypeScript source, cited per test.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use hoh_error::BoxError;
use hoh_kernel::Kernel;
use hoh_timeout::AbortController;
use hoh_tools::{
    ToolBatchExecutionError, ToolBatchInput, ToolExecutionPipeline, ToolLogStep, ToolsError,
    ToolsResult, execute_tool_batch,
};
use serde_json::json;

/// A thrown value whose string coercion traps (`throw { toString() { throw ... } }`).
#[derive(Debug)]
struct Hostile;

impl std::fmt::Display for Hostile {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("coercion trap")
    }
}

impl std::error::Error for Hostile {}

const UNPRINTABLE: &str = "<unprintable thrown value>";

// ------------------------------------------------- 1. error normalization is total

/// pipeline.ts:669 `toolErrorMessage`: a hostile thrown value falls back to `<unprintable thrown value>`.
#[tokio::test]
async fn a_tool_body_that_throws_an_unprintable_value_fails_with_the_placeholder() {
    let env = setup().await;
    register(
        &env,
        content_tool("hostile", json!({}), |_, _| async {
            Err(Box::new(Hostile) as BoxError)
        }),
    );
    let result = execute_tool_fixture(&env.ctx, input("c", "hostile", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(result.error().expect("failure").message, UNPRINTABLE);
    assert_eq!(result.content(), &[text(&format!("Error: {UNPRINTABLE}"))]);
}

/// scoped.spec.ts:695: a `tools/result` observer throwing an unprintable value is contained and reported.
#[tokio::test]
async fn a_result_observer_that_throws_an_unprintable_value_is_contained_and_logged() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_contained::<ToolsResult>(|_| Err(Box::new(Hostile) as BoxError))
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c", "echo", json!({ "text": "ok" }))).await;
    assert!(!result.is_error());
    let warnings: Vec<_> = env
        .kernel
        .root()
        .logger()
        .buffer()
        .into_iter()
        .map(|record| record.message)
        .collect();
    assert!(
        warnings
            .iter()
            .any(|message| message.contains("tools/result observer failed")
                && message.contains(UNPRINTABLE)),
        "{warnings:?}"
    );
}

// ------------------------------------------------- 2/3. a failed lookup rejects

/// pipeline.ts:106: `executionMode` dereferences `ctx.tools`; without it the call throws a `TypeError`
/// instead of classifying as exclusive.
#[tokio::test]
async fn classification_without_a_tools_service_rejects_with_the_lookup_type_error() {
    let kernel = Kernel::start();
    let pipeline = ToolExecutionPipeline::new(kernel.root()).expect("constructs without a service");
    let error = pipeline
        .execution_mode(&input("c", "anything", json!({})))
        .expect_err("a failed lookup is not a classification");
    let plain = error
        .downcast_ref::<ToolsError>()
        .expect("a bare TypeError");
    assert_eq!(plain.name(), "TypeError");
    assert_eq!(
        plain.message(),
        "Cannot read properties of undefined (reading 'executable')"
    );
    kernel.shutdown().await;
}

/// batch.ts:75: the throw happens before `tool/call` is appended, so the session stays empty.
#[tokio::test]
async fn a_logged_batch_without_a_tools_service_rejects_before_any_session_event() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).expect("constructs without a service");
    let agent = TestAgent::open(&ctx);
    let mut batch = ToolBatchInput::new(vec![model_call("a", "anything", "{}")], live_signal());
    batch.agent = Some(agent.clone());
    batch.log = Some(ToolLogStep { turn: 1, step: 1 });
    let error = match execute_tool_batch(batch, &pipeline, &|| 1).await {
        Ok(_) => panic!("the batch must reject"),
        Err(error) => error,
    };
    let batch_error = error
        .downcast_ref::<ToolBatchExecutionError>()
        .expect("a ToolBatchExecutionError");
    let cause = batch_error
        .cause
        .downcast_ref::<ToolsError>()
        .expect("the lookup TypeError");
    assert_eq!(
        cause.message(),
        "Cannot read properties of undefined (reading 'executable')"
    );
    let logged: Vec<_> = agent
        .session
        .own_events()
        .iter()
        .map(|event| event.event_type().to_owned())
        .filter(|kind| kind.starts_with("tool/"))
        .collect();
    assert!(logged.is_empty(), "{logged:?}");
    kernel.shutdown().await;
}

// ------------------------------------------------- 4. the execution is frozen for observers

/// pipeline.ts:423 `Object.freeze(exec)`: an observer cannot replace the signal.
#[tokio::test]
async fn a_result_observer_cannot_replace_the_signal_of_the_frozen_execution() {
    let env = setup().await;
    register(&env, echo_tool());
    let seen = Arc::new(Mutex::new(Vec::<(bool, bool)>::new()));
    {
        let seen = seen.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |event| {
                let cancelled = Arc::new(AbortController::new());
                cancelled.abort();
                event.exec.set_signal(cancelled.signal());
                lock(&seen).push((event.exec.is_frozen(), event.exec.signal().aborted()));
                Ok(())
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("c", "echo", json!({ "text": "ok" }))).await;
    assert!(!result.is_error());
    assert_eq!(*lock(&seen), vec![(true, false)]);
}

// ------------------------------------------------- 5. the log without an agent

/// batch.ts:245: `agent!.session` on `undefined` throws a `TypeError` with the engine's text.
#[tokio::test]
async fn a_log_without_an_agent_fails_with_the_property_read_type_error() {
    let env = setup().await;
    register(&env, echo_tool());
    let pipeline = ToolExecutionPipeline::new(env.ctx.clone()).unwrap();
    let mut batch = ToolBatchInput::new(
        vec![model_call("a", "echo", r#"{"text":"x"}"#)],
        live_signal(),
    );
    batch.log = Some(ToolLogStep { turn: 1, step: 1 });
    let error = match execute_tool_batch(batch, &pipeline, &|| 1).await {
        Ok(_) => panic!("the batch must reject"),
        Err(error) => error,
    };
    let batch_error = error
        .downcast_ref::<ToolBatchExecutionError>()
        .expect("a ToolBatchExecutionError");
    let cause = batch_error
        .cause
        .downcast_ref::<ToolsError>()
        .expect("a bare TypeError");
    assert_eq!(cause.name(), "TypeError");
    assert_eq!(
        cause.message(),
        "Cannot read properties of undefined (reading 'session')"
    );
}
