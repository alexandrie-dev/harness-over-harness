//! Port of tests/activation.spec.ts: conditional registrations share step admission with direct
//! and nested execution.

mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use common::*;
use hoh_agent::{Agent as _, ModelSelection};
use hoh_error::BoxError;
use hoh_timeout::AbortController;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ResolvedToolSet, TOOL_ABORTED_BEFORE_DISPATCH,
    ToolActivationContext, ToolActivationError, ToolDefinition, ToolExecutionInput,
    ToolExecutionResult, ToolExecutionSuccess, ToolRegistrationOptions, ToolRestriction,
    ToolsExecute, define_tool,
};
use serde_json::json;

struct Harness {
    env: Env,
    agent: Arc<TestAgent>,
    controller: Arc<AbortController>,
    tool: ToolDefinition,
    calls: Arc<AtomicUsize>,
    selection: ModelSelection,
    scope: hoh_scope::Scope,
}

async fn harness() -> Harness {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let tool = {
        let calls = calls.clone();
        define_tool(DefineToolOptions::new(
            "conditional",
            "A conditional operation.",
            json!({}),
            DefineToolOutput {
                schema: json!({ "type": "string" }),
                render: Arc::new(|_, value| vec![text(value.as_str().unwrap())]),
                presentation_meta: None,
            },
            move |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(json!("done")) })
            },
        ))
        .unwrap()
    };
    let agent = TestAgent::new(&env.ctx);
    let scope = hoh_scope::create_scope(&env.ctx, agent.key.clone(), Default::default()).unwrap();
    Harness {
        env,
        agent,
        scope,
        controller: Arc::new(AbortController::new()),
        tool,
        calls,
        selection: ModelSelection {
            provider: "test".into(),
            model: "test".into(),
            reasoning_effort: None,
        },
    }
}

impl Harness {
    fn input(&self) -> ToolExecutionInput {
        let mut input = ToolExecutionInput::new(
            call_id("call"),
            "conditional",
            json!({}),
            self.controller.signal(),
        );
        input.agent = Some(self.agent.clone());
        input
    }
    async fn execute(&self, tool_set: Option<&ResolvedToolSet>) -> ToolExecutionResult {
        let mut input = self.input();
        input.tool_set = tool_set.cloned();
        execute_tool_fixture(&self.env.ctx, input).await
    }
    async fn resolve(&self) -> Result<ResolvedToolSet, BoxError> {
        self.env
            .tools
            .resolve(ToolActivationContext {
                agent: self.agent.clone(),
                model_selection: self.selection.clone(),
                signal: self.controller.signal(),
            })
            .await
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn names(&self, set: &ResolvedToolSet) -> Vec<String> {
        self.env
            .tools
            .schemas(Some(&self.agent.scope_key()), Some(set))
            .unwrap()
            .into_iter()
            .map(|schema| schema.name)
            .collect()
    }
}

fn code(result: &ToolExecutionResult) -> Option<String> {
    result
        .error()
        .and_then(|error| error.info.as_ref())
        .map(|info| info.code.clone())
}

fn flag(value: &Arc<Mutex<bool>>) -> ToolRegistrationOptions {
    let value = value.clone();
    ToolRegistrationOptions::when_sync(move |_| *lock(&value))
}

fn when_true() -> ToolRegistrationOptions {
    ToolRegistrationOptions::when_sync(|_| true)
}

#[tokio::test]
async fn activates_and_deactivates_a_registered_tool_without_allowing_historical_calls() {
    let h = harness().await;
    let enabled = Arc::new(Mutex::new(false));
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), flag(&enabled))
        .unwrap();
    let unavailable = h.resolve().await.unwrap();
    assert!(h.names(&unavailable).is_empty());
    *lock(&enabled) = true;
    assert_eq!(
        code(&h.execute(Some(&unavailable)).await).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    assert_eq!(
        code(&h.execute(None).await).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    let available = h.resolve().await.unwrap();
    assert_eq!(h.names(&available), vec!["conditional"]);
    assert!(!h.execute(Some(&available)).await.is_error());
    *lock(&enabled) = false;
    let disabled = h.resolve().await.unwrap();
    assert_eq!(
        code(&h.execute(Some(&disabled)).await).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    assert_eq!(h.calls(), 1);
}

#[tokio::test]
async fn applies_a_batch_admission_to_the_calls_its_executor_runs() {
    let h = harness().await;
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), when_true())
        .unwrap();
    let tool_set = h.resolve().await.unwrap();
    let run = |set: Option<ResolvedToolSet>| {
        let mut batch = hoh_tools::ToolBatchInput::new(
            vec![model_call("call", "conditional", "{}")],
            h.controller.signal(),
        );
        batch.agent = Some(h.agent.clone());
        batch.tool_set = set;
        let tools = h.env.tools.clone();
        async move { tools.execute(batch).await.unwrap().results.remove(0).result }
    };
    assert!(!run(Some(tool_set)).await.is_error());
    assert_eq!(code(&run(None).await).as_deref(), Some("UNKNOWN_TOOL"));
    assert_eq!(h.calls(), 1);
}

#[tokio::test]
async fn keeps_admissions_specific_to_the_requesting_agent() {
    let h = harness().await;
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), when_true())
        .unwrap();
    let tool_set = h.resolve().await.unwrap();
    let mut input = h.input();
    input.agent = Some(TestAgent::new(&h.env.ctx));
    input.tool_set = Some(tool_set);
    let result = execute_tool_fixture(&h.env.ctx, input).await;
    assert_eq!(code(&result).as_deref(), Some("UNKNOWN_TOOL"));
    assert_eq!(h.calls(), 0);
}

#[tokio::test]
async fn applies_live_restrictions_and_closes_step_admission_permanently() {
    let h = harness().await;
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), when_true())
        .unwrap();
    let tool_set = h.resolve().await.unwrap();
    let lift = h
        .env
        .tools
        .restrict(
            &h.scope.ctx,
            ToolRestriction {
                allow: None,
                deny: Some(vec!["conditional".into()]),
            },
        )
        .unwrap();
    assert_eq!(
        code(&h.execute(Some(&tool_set)).await).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    lift.dispose().await;
    assert!(!h.execute(Some(&tool_set)).await.is_error());
    tool_set.dispose();
    assert!(h.names(&tool_set).is_empty());
    assert_eq!(
        code(&h.execute(Some(&tool_set)).await).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    assert_eq!(h.calls(), 1);
}

#[tokio::test]
async fn settles_calls_of_a_cancelled_step_through_cancellation_with_their_finalizers() {
    let h = harness().await;
    let mut tool = h.tool.clone();
    tool.finalize_content = Some(Arc::new(|_, _| Some(vec![text("finalized")])));
    h.env.tools.register(&h.env.ctx, tool, when_true()).unwrap();
    let tool_set = h.resolve().await.unwrap();
    h.controller.abort();
    let result = h.execute(Some(&tool_set)).await;
    assert_eq!(code(&result).as_deref(), Some(TOOL_ABORTED_BEFORE_DISPATCH));
    assert_eq!(result.content(), &[text("finalized")]);
    assert_eq!(h.calls(), 0);
}

#[tokio::test]
async fn reports_a_wrapper_result_completed_at_cancellation_as_cancelled_rather_than_unknown() {
    let h = harness().await;
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), when_true())
        .unwrap();
    let tool_set = h.resolve().await.unwrap();
    {
        let controller = h.controller.clone();
        h.env
            .ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, _| {
                controller.abort();
                async {
                    Ok(ToolExecutionResult::Success(ToolExecutionSuccess::new(
                        json!("wrapped"),
                        vec![text("wrapped")],
                    )))
                }
            })
            .unwrap();
    }
    let result = h.execute(Some(&tool_set)).await;
    assert_eq!(code(&result).as_deref(), Some(TOOL_ABORTED_BEFORE_DISPATCH));
}

#[tokio::test]
async fn does_not_substitute_a_same_name_registration_after_admission() {
    let h = harness().await;
    let unregister = h
        .env
        .tools
        .register(&h.env.ctx, h.tool.clone(), when_true())
        .unwrap();
    let old = h.resolve().await.unwrap();
    unregister.dispose().await;
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), when_true())
        .unwrap();
    assert_eq!(
        code(&h.execute(Some(&old)).await).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    let fresh = h.resolve().await.unwrap();
    assert!(!h.execute(Some(&fresh)).await.is_error());
    assert_eq!(h.calls(), 1);
}

#[tokio::test]
async fn does_not_restore_a_registration_removed_during_its_asynchronous_condition() {
    let h = harness().await;
    let condition = Arc::new(tokio::sync::Notify::new());
    let entered = Arc::new(tokio::sync::Notify::new());
    let options = {
        let (condition, entered) = (condition.clone(), entered.clone());
        ToolRegistrationOptions {
            when: Some(Arc::new(move |_| {
                let (condition, entered) = (condition.clone(), entered.clone());
                Box::pin(async move {
                    entered.notify_one();
                    condition.notified().await;
                    Ok(true)
                })
            })),
        }
    };
    let unregister = h
        .env
        .tools
        .register(&h.env.ctx, h.tool.clone(), options)
        .unwrap();
    let pending = h.resolve();
    tokio::pin!(pending);
    let tool_set = tokio::select! {
        _ = entered.notified() => {
            unregister.dispose().await;
            condition.notify_one();
            pending.await.unwrap()
        }
        result = &mut pending => result.unwrap(),
    };
    assert!(h.names(&tool_set).is_empty());
    assert_eq!(
        code(&h.execute(Some(&tool_set)).await).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    assert_eq!(h.calls(), 0);
}

#[tokio::test]
async fn propagates_cancellation_after_an_asynchronous_condition_settles() {
    let h = harness().await;
    let condition = Arc::new(tokio::sync::Notify::new());
    let entered = Arc::new(tokio::sync::Notify::new());
    let options = {
        let (condition, entered) = (condition.clone(), entered.clone());
        let agent_key = h.agent.scope_key();
        let selection = h.selection.clone();
        ToolRegistrationOptions {
            when: Some(Arc::new(move |context| {
                assert!(!context.signal.aborted());
                assert_eq!(context.agent.scope_key(), agent_key);
                assert_eq!(context.model_selection.provider, selection.provider);
                let (condition, entered) = (condition.clone(), entered.clone());
                Box::pin(async move {
                    entered.notify_one();
                    condition.notified().await;
                    Ok(true)
                })
            })),
        }
    };
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), options)
        .unwrap();
    let pending = h.resolve();
    tokio::pin!(pending);
    let outcome = tokio::select! {
        _ = entered.notified() => {
            h.controller.abort();
            condition.notify_one();
            pending.await
        }
        result = &mut pending => result,
    };
    let error = outcome.err().expect("cancelled");
    assert!(hoh_timeout::is_abort_error(error.as_ref()));
    assert_eq!(h.calls(), 0);
}

#[tokio::test]
async fn reports_condition_failures_without_treating_them_as_disabled_tools() {
    let h = harness().await;
    let options = ToolRegistrationOptions {
        when: Some(Arc::new(|_| {
            Box::pin(async { Err::<bool, BoxError>("credential storage unavailable".into()) })
        })),
    };
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), options)
        .unwrap();
    let error = h.resolve().await.err().expect("fails");
    let harness_error = hoh_error::HarnessError::of(error.as_ref()).expect("harness error");
    assert_eq!(harness_error.name(), ToolActivationError::NAME);
    assert_eq!(harness_error.code(), "TOOL_ACTIVATION_FAILED");
    assert_eq!(
        harness_error.cause().map(|cause| cause.to_string()),
        Some("credential storage unavailable".to_owned())
    );
    assert_eq!(h.calls(), 0);
}

/// index.ts:277–282: a throwing condition rejects with `ToolActivationError`; a panic is that throw.
async fn assert_activation_failure_of(options: ToolRegistrationOptions) {
    let h = harness().await;
    h.env
        .tools
        .register(&h.env.ctx, h.tool.clone(), options)
        .unwrap();
    let error = h.resolve().await.err().expect("fails");
    let harness_error = hoh_error::HarnessError::of(error.as_ref()).expect("harness error");
    assert_eq!(harness_error.name(), ToolActivationError::NAME);
    assert_eq!(harness_error.code(), "TOOL_ACTIVATION_FAILED");
    assert_eq!(
        harness_error.cause().map(|cause| cause.to_string()),
        Some("boom".to_owned())
    );
    assert_eq!(h.calls(), 0);
}

#[tokio::test]
async fn normalizes_a_panicking_synchronous_condition_to_an_activation_failure() {
    assert_activation_failure_of(ToolRegistrationOptions::when_sync(|_| panic!("boom"))).await;
}

#[tokio::test]
async fn normalizes_a_panic_while_polling_a_condition_future_to_an_activation_failure() {
    assert_activation_failure_of(ToolRegistrationOptions {
        when: Some(Arc::new(|_| {
            Box::pin(async {
                if true {
                    panic!("boom");
                }
                Ok::<bool, BoxError>(true)
            })
        })),
    })
    .await;
}

#[tokio::test]
async fn cancellation_takes_precedence_over_a_panicking_condition() {
    let h = harness().await;
    let controller = h.controller.clone();
    h.env
        .tools
        .register(
            &h.env.ctx,
            h.tool.clone(),
            ToolRegistrationOptions::when_sync(move |_| {
                controller.abort();
                panic!("boom")
            }),
        )
        .unwrap();
    let error = h.resolve().await.err().expect("fails");
    assert!(hoh_timeout::is_abort_error(error.as_ref()));
}
