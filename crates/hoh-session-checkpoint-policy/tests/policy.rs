mod support;

use futures_util::StreamExt;
use hoh_agent::{AgentPreStep, ModelSelection, PreStepDecision, agent_events};
use hoh_error::BoxError;
use hoh_kernel::{Plugin, ServiceKey};
use hoh_llm::{GenerateOptions, LlmAdapter, LlmStream};
use hoh_session::SessionStore;
use hoh_session_checkpoint_policy::SessionCheckpointPolicy;
use hoh_timeout::AbortController;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolExecutionInput, ToolRegistrationOptions, define_tool,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::{StubAgent, execute_tool, live_signal, setup};

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct RecordingAdapter(Arc<Mutex<Vec<&'static str>>>);
impl LlmAdapter for RecordingAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        lock(&self.0).push("adapter");
        Ok(LlmStream::chunks([]))
    }
}

fn options(session_id: Option<hoh_session::SessionId>) -> GenerateOptions {
    GenerateOptions {
        provider: "mock".into(),
        model: "mock".into(),
        messages: vec![],
        session_id,
        ..Default::default()
    }
}

async fn drain(stream: LlmStream) -> Result<(), BoxError> {
    futures_util::pin_mut!(stream);
    while let Some(chunk) = stream.next().await {
        chunk?;
    }
    Ok(())
}

fn register_null_tool(
    harness: &support::Harness,
    name: &str,
    body: impl Fn(
        Arc<hoh_tools::ToolExecution>,
    ) -> hoh_tools::shim::BoxFuture<'static, Result<Value, BoxError>>
    + Send
    + Sync
    + 'static,
) {
    let body = Arc::new(body);
    let definition = define_tool(DefineToolOptions::new(
        name,
        "side effect",
        json!({}),
        DefineToolOutput {
            schema: json!({"type":"null"}),
            render: Arc::new(|_, _| vec![]),
            presentation_meta: None,
        },
        move |_, exec| body(exec),
    ))
    .unwrap();
    harness
        .tools
        .register(&harness.ctx, definition, ToolRegistrationOptions::default())
        .unwrap();
}

fn tool_input(
    id: &str,
    name: &str,
    agent: Arc<StubAgent>,
    signal: hoh_timeout::AbortSignal,
) -> ToolExecutionInput {
    let mut input = ToolExecutionInput::new(id.into(), name, json!({}), signal);
    input.agent = Some(agent as Arc<dyn hoh_tools::Agent>);
    input
}

#[tokio::test]
async fn awaits_the_live_session_checkpoint_before_constructing_the_model_stream() {
    let harness = setup().await;
    let session = harness.session("request-checkpoint");
    session
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let entered_tx = Arc::new(Mutex::new(Some(entered_tx)));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));
    SessionStore::on_flush(&harness.ctx, {
        let order = order.clone();
        move |_| {
            let order = order.clone();
            let entered = lock(&entered_tx).take();
            let release = lock(&release_rx).take().unwrap();
            async move {
                lock(&order).push("flush:start");
                entered.unwrap().send(()).unwrap();
                release.await.unwrap();
                lock(&order).push("flush:end");
                Ok(())
            }
        }
    })
    .unwrap();
    harness
        .llm
        .register_adapter(
            &harness.ctx,
            vec!["mock".into()],
            Arc::new(RecordingAdapter(order.clone())),
        )
        .unwrap();
    let stream = harness
        .llm
        .stream(&harness.ctx, options(Some(session.id().clone())));
    let pending = tokio::spawn(drain(stream));
    entered_rx.await.unwrap();
    assert_eq!(*lock(&order), ["flush:start"]);
    release_tx.send(()).unwrap();
    pending.await.unwrap().unwrap();
    assert_eq!(*lock(&order), ["flush:start", "flush:end", "adapter"]);
    harness.kernel.shutdown().await;
}

async fn assert_delegates_without_checkpoint(session_id: Option<hoh_session::SessionId>) {
    let harness = setup().await;
    let order = Arc::new(Mutex::new(Vec::new()));
    SessionStore::on_flush(&harness.ctx, {
        let order = order.clone();
        move |_| {
            lock(&order).push("flush");
            async { Ok(()) }
        }
    })
    .unwrap();
    harness
        .llm
        .register_adapter(
            &harness.ctx,
            vec!["mock".into()],
            Arc::new(RecordingAdapter(order.clone())),
        )
        .unwrap();
    drain(harness.llm.stream(&harness.ctx, options(session_id)))
        .await
        .unwrap();
    assert_eq!(*lock(&order), ["adapter"]);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn delegates_a_request_without_a_session_without_checkpointing() {
    assert_delegates_without_checkpoint(None).await;
}

#[tokio::test]
async fn delegates_an_already_detached_session_id_without_checkpointing() {
    assert_delegates_without_checkpoint(Some(hoh_session::SessionId::from("detached"))).await;
}

#[tokio::test]
async fn does_not_dispatch_the_adapter_when_the_checkpoint_rejects() {
    let harness = setup().await;
    let session = harness.session("request-failure");
    SessionStore::on_flush(&harness.ctx, |_| async { Err("disk unavailable".into()) }).unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    harness
        .llm
        .register_adapter(
            &harness.ctx,
            vec!["mock".into()],
            Arc::new(RecordingAdapter(order.clone())),
        )
        .unwrap();
    let error = drain(
        harness
            .llm
            .stream(&harness.ctx, options(Some(session.id().clone()))),
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "disk unavailable");
    assert!(lock(&order).is_empty());
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn awaits_the_checkpoint_before_a_top_level_tool_body() {
    let harness = setup().await;
    let session = harness.session("tool-checkpoint");
    let agent = StubAgent::new(&harness.ctx, session);
    let order = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let entered_tx = Arc::new(Mutex::new(Some(entered_tx)));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));
    SessionStore::on_flush(&harness.ctx, {
        let order = order.clone();
        move |_| {
            let order = order.clone();
            let entered = lock(&entered_tx).take().unwrap();
            let release = lock(&release_rx).take().unwrap();
            async move {
                lock(&order).push("flush:start");
                entered.send(()).unwrap();
                release.await.unwrap();
                lock(&order).push("flush:end");
                Ok(())
            }
        }
    })
    .unwrap();
    register_null_tool(&harness, "write", {
        let order = order.clone();
        move |_| {
            let order = order.clone();
            Box::pin(async move {
                lock(&order).push("tool");
                Ok(Value::Null)
            })
        }
    });
    let ctx = harness.ctx.clone();
    let pending = tokio::spawn(async move {
        execute_tool(&ctx, tool_input("write-1", "write", agent, live_signal())).await
    });
    entered_rx.await.unwrap();
    assert_eq!(*lock(&order), ["flush:start"]);
    release_tx.send(()).unwrap();
    assert!(!pending.await.unwrap().is_error());
    assert_eq!(*lock(&order), ["flush:start", "flush:end", "tool"]);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn cancellation_during_the_tool_checkpoint_returns_the_canonical_result() {
    let harness = setup().await;
    let session = harness.session("tool-checkpoint-cancel");
    let agent = StubAgent::new(&harness.ctx, session);
    let controller = AbortController::new();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let entered_tx = Arc::new(Mutex::new(Some(entered_tx)));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));
    SessionStore::on_flush(&harness.ctx, move |_| {
        let entered = lock(&entered_tx).take().unwrap();
        let release = lock(&release_rx).take().unwrap();
        async move {
            entered.send(()).unwrap();
            release.await.unwrap();
            Ok(())
        }
    })
    .unwrap();
    let ran = Arc::new(Mutex::new(false));
    register_null_tool(&harness, "write", {
        let ran = ran.clone();
        move |_| {
            *lock(&ran) = true;
            Box::pin(async { Ok(Value::Null) })
        }
    });
    let ctx = harness.ctx.clone();
    let signal = controller.signal();
    let pending = tokio::spawn(async move {
        execute_tool(&ctx, tool_input("write-cancelled", "write", agent, signal)).await
    });
    entered_rx.await.unwrap();
    controller.abort();
    release_tx.send(()).unwrap();
    let result = pending.await.unwrap();
    assert!(result.is_error());
    assert_eq!(
        result.content(),
        [hoh_llm::ContentBlock::text(
            "Error: tool call aborted before dispatch"
        )]
    );
    let error = result.error().unwrap();
    assert_eq!(error.message, "tool call aborted before dispatch");
    assert_eq!(error.info.as_ref().unwrap().name, "AbortError");
    assert_eq!(
        error.info.as_ref().unwrap().code,
        hoh_tools::TOOL_ABORTED_BEFORE_DISPATCH
    );
    assert!(!*lock(&ran));
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn rejected_tool_checkpoint_becomes_an_error_without_running_the_body() {
    let harness = setup().await;
    let session = harness.session("tool-failure");
    let agent = StubAgent::new(&harness.ctx, session);
    SessionStore::on_flush(&harness.ctx, |_| async { Err("disk unavailable".into()) }).unwrap();
    let ran = Arc::new(Mutex::new(false));
    register_null_tool(&harness, "write", {
        let ran = ran.clone();
        move |_| {
            *lock(&ran) = true;
            Box::pin(async { Ok(Value::Null) })
        }
    });
    let result = execute_tool(
        &harness.ctx,
        tool_input("write-2", "write", agent, live_signal()),
    )
    .await;
    assert!(result.is_error());
    assert_eq!(
        result.content(),
        [hoh_llm::ContentBlock::text("Error: disk unavailable")]
    );
    assert!(!*lock(&ran));
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn nested_tool_dispatch_reuses_the_outer_checkpoint() {
    let harness = setup().await;
    let session = harness.session("nested-tool");
    let agent = StubAgent::new(&harness.ctx, session);
    let flushes = Arc::new(Mutex::new(0));
    SessionStore::on_flush(&harness.ctx, {
        let flushes = flushes.clone();
        move |_| {
            *lock(&flushes) += 1;
            async { Ok(()) }
        }
    })
    .unwrap();
    register_null_tool(&harness, "inner", |_| Box::pin(async { Ok(Value::Null) }));
    register_null_tool(&harness, "outer", {
        let ctx = harness.ctx.clone();
        move |exec| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let mut nested =
                    ToolExecutionInput::new("inner-1".into(), "inner", json!({}), exec.signal());
                nested.agent = exec.agent.clone();
                nested.parent = Some(exec.token);
                let result = execute_tool(&ctx, nested).await;
                assert!(!result.is_error());
                Ok(Value::Null)
            })
        }
    });
    let result = execute_tool(
        &harness.ctx,
        tool_input("outer-1", "outer", agent, live_signal()),
    )
    .await;
    assert!(!result.is_error());
    assert_eq!(*lock(&flushes), 1);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn checkpoints_during_pre_step_processing() {
    let harness = setup().await;
    let session = harness.session("post-step");
    let agent: Arc<dyn hoh_agent::Agent> = StubAgent::new(&harness.ctx, session.clone());
    let flushed = Arc::new(Mutex::new(Vec::new()));
    SessionStore::on_flush(&harness.ctx, {
        let flushed = flushed.clone();
        move |session| {
            lock(&flushed).push(session.id().clone());
            async { Ok(()) }
        }
    })
    .unwrap();
    let signal = live_signal();
    let decision = agent_events(&harness.ctx, &agent)
        .waterfall::<AgentPreStep, PreStepDecision, _>(
            move |agent| AgentPreStep {
                agent,
                messages: vec![],
                turn: 1,
                step: 1,
                signal,
                model_selection: ModelSelection {
                    provider: "mock".into(),
                    model: "mock".into(),
                    reasoning_effort: None,
                },
            },
            |_| async {
                Ok(PreStepDecision::Enter {
                    messages: vec![],
                    starts_request_series: false,
                })
            },
        )
        .await
        .unwrap();
    assert!(matches!(decision, PreStepDecision::Enter { .. }));
    assert_eq!(*lock(&flushed), [session.id().clone()]);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn removes_its_wrappers_when_the_owning_fiber_is_disposed() {
    let harness = setup().await;
    let session = harness.session("disposed-policy");
    let flushes = Arc::new(Mutex::new(0));
    SessionStore::on_flush(&harness.ctx, {
        let flushes = flushes.clone();
        move |_| {
            *lock(&flushes) += 1;
            async { Ok(()) }
        }
    })
    .unwrap();
    harness
        .llm
        .register_adapter(
            &harness.ctx,
            vec!["mock".into()],
            Arc::new(RecordingAdapter(Default::default())),
        )
        .unwrap();
    drain(
        harness
            .llm
            .stream(&harness.ctx, options(Some(session.id().clone()))),
    )
    .await
    .unwrap();
    assert_eq!(*lock(&flushes), 1);
    harness.policy.dispose().await;
    drain(
        harness
            .llm
            .stream(&harness.ctx, options(Some(session.id().clone()))),
    )
    .await
    .unwrap();
    assert_eq!(*lock(&flushes), 1);
    harness.kernel.shutdown().await;
}

#[test]
fn keeps_the_loader_safe_namespace_plugin_shape() {
    assert_eq!(SessionCheckpointPolicy::NAME, "session-checkpoint-policy");
    assert_eq!(<hoh_llm::Llm as ServiceKey>::NAME, "llm");
    assert_eq!(
        <hoh_session_persistence::SessionPersistenceService as ServiceKey>::NAME,
        "sessionPersistence"
    );
    assert_eq!(<hoh_session::Sessions as ServiceKey>::NAME, "sessions");
    assert_eq!(<hoh_tools::Tools as ServiceKey>::NAME, "tools");
}
