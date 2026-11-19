use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::ContentBlock;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal, deadline, timeout_of};
use hoh_tool_call_timeout_policy::{TOOL_TIMEOUT, ToolCallTimeoutPolicy};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_testkit::{
    ContentToolFixtureOptions, ModelToolFixtureInput, define_content_tool_fixture,
    execute_model_tool_fixture,
};
use hoh_tools::{
    TOOL_ABORTED, ToolExecution, ToolExecutionResult, ToolRegistrationOptions, ToolRuntime, Tools,
    ToolsPostExecute,
};
use serde_json::json;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Env {
    _kernel: Kernel,
    ctx: Context,
    policy: Fiber,
}

async fn setup() -> Env {
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
    ctx.plugin::<NativeToolExecutorPlugin>(json!({}))
        .expect("native executor mounts")
        .settled()
        .await
        .expect("native executor settles");
    let policy = ctx
        .plugin::<ToolCallTimeoutPolicy>(json!({}))
        .expect("timeout policy mounts");
    policy.settled().await.expect("timeout policy settles");
    Env {
        _kernel: kernel,
        ctx,
        policy,
    }
}

fn register<F, Fut>(env: &Env, name: &str, timeout_ms: Option<f64>, execute: F)
where
    F: Fn(serde_json::Value, Arc<ToolExecution>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<ContentBlock>, BoxError>> + Send + 'static,
{
    let mut options = ContentToolFixtureOptions::new(name, "test tool", json!({}), execute);
    options.timeout_ms = timeout_ms;
    let definition = define_content_tool_fixture(options).expect("fixture defines");
    env.ctx
        .get::<Tools>()
        .expect("tools service")
        .register(&env.ctx, definition, ToolRegistrationOptions::default())
        .expect("tool registers");
}

async fn execute(
    ctx: &Context,
    call_id: &str,
    name: &str,
    signal: AbortSignal,
) -> ToolExecutionResult {
    execute_model_tool_fixture(
        ctx,
        ModelToolFixtureInput::new(call_id.into(), name, json!({}), signal),
    )
    .await
    .expect("execution pipeline settles")
}

fn assert_timeout(result: &ToolExecutionResult, timeout_ms: f64) {
    let message = format!(
        "tool call timed out after {}ms",
        hoh_util_values::js_number_string(timeout_ms)
    );
    assert!(result.is_error());
    assert_eq!(
        result.content(),
        &[ContentBlock::text(format!("Error: {message}"))]
    );
    let failure = result.error().expect("timeout is a failure");
    assert_eq!(failure.message, message);
    let info = failure.info.as_ref().expect("timeout carries routing info");
    assert_eq!(info.name, "ToolTimeoutError");
    assert_eq!(info.code, TOOL_TIMEOUT);
}

#[tokio::test]
async fn delegates_a_tool_without_a_declared_budget_unchanged() {
    let env = setup().await;
    let seen = Arc::new(Mutex::new(None::<AbortSignal>));
    let captured = seen.clone();
    register(&env, "probe", None, move |_, exec| {
        *lock(&captured) = Some(exec.signal());
        async { Ok(vec![ContentBlock::text("ok")]) }
    });
    let upstream = AbortController::new();
    let result = execute(&env.ctx, "c1", "probe", upstream.signal()).await;
    assert!(!result.is_error());
    assert_eq!(result.content(), &[ContentBlock::text("ok")]);

    upstream.abort_with("caller stopped");
    let upstream_reason = upstream.signal().reason().expect("upstream aborts");
    let seen_reason = lock(&seen)
        .as_ref()
        .expect("tool saw a signal")
        .reason()
        .expect("same signal aborts");
    assert!(seen_reason.ptr_eq(&upstream_reason));
}

#[tokio::test]
async fn keeps_the_result_of_a_budgeted_tool_that_finishes_in_time() {
    let env = setup().await;
    register(&env, "fast", Some(10_000.0), |_, _| async {
        Ok(vec![ContentBlock::text("ok")])
    });
    let result = execute(&env.ctx, "c1", "fast", AbortController::new().signal()).await;
    assert!(!result.is_error());
    assert_eq!(result.content(), &[ContentBlock::text("ok")]);
    assert_eq!(
        result.value(),
        Some(&json!([{ "type": "text", "text": "ok" }]))
    );
}

#[tokio::test(start_paused = true)]
async fn a_budgeted_tool_receives_the_derived_deadline_signal() {
    let env = setup().await;
    let seen = Arc::new(Mutex::new(None::<AbortSignal>));
    let captured = seen.clone();
    register(&env, "probe", Some(100.0), move |_, exec| {
        *lock(&captured) = Some(exec.signal());
        let signal = exec.signal();
        async move {
            signal.cancelled().await;
            Ok(vec![ContentBlock::text("stopped")])
        }
    });
    let ctx = env.ctx.clone();
    let pending =
        tokio::spawn(
            async move { execute(&ctx, "c1", "probe", AbortController::new().signal()).await },
        );
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    let result = pending.await.expect("execution task joins");
    assert_timeout(&result, 100.0);
    let signal = lock(&seen).clone().expect("tool saw a signal");
    assert!(timeout_of(signal.reason().as_ref(), Some(TOOL_TIMEOUT)).is_some());
}

#[tokio::test(start_paused = true)]
async fn restores_the_caller_signal_before_post_execute() {
    let env = setup().await;
    register(&env, "fast", Some(100.0), |_, _| async {
        Ok(vec![ContentBlock::text("ok")])
    });
    let post_signal = Arc::new(Mutex::new(None::<AbortSignal>));
    let captured = post_signal.clone();
    env.ctx
        .on_waterfall::<ToolsPostExecute, hoh_tools::PostToolDecision, _>(move |event, next| {
            *lock(&captured) = Some(event.exec.signal());
            async move { next.run().await }
        })
        .expect("post listener registers");
    let upstream = AbortController::new();
    let result = execute(&env.ctx, "c1", "fast", upstream.signal()).await;
    assert!(!result.is_error());
    tokio::time::advance(Duration::from_millis(150)).await;
    assert!(
        !lock(&post_signal)
            .as_ref()
            .expect("post listener ran")
            .aborted()
    );
    upstream.abort_with("after post");
    assert!(lock(&post_signal).as_ref().unwrap().aborted());
}

#[tokio::test(start_paused = true)]
async fn replaces_a_cooperative_tool_result_when_its_deadline_fires() {
    let env = setup().await;
    register(&env, "slow", Some(100.0), |_, exec| {
        let signal = exec.signal();
        async move {
            signal.cancelled().await;
            Ok(vec![ContentBlock::text("stopped cooperatively")])
        }
    });
    let ctx = env.ctx.clone();
    let pending =
        tokio::spawn(
            async move { execute(&ctx, "c1", "slow", AbortController::new().signal()).await },
        );
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(150)).await;
    assert_timeout(&pending.await.expect("execution task joins"), 100.0);
}

#[tokio::test(start_paused = true)]
async fn formats_fractional_timeout_like_javascript_number_to_string() {
    let env = setup().await;
    register(&env, "tiny", Some(1e-7), |_, exec| {
        let signal = exec.signal();
        async move {
            signal.cancelled().await;
            Ok(vec![ContentBlock::text("stopped")])
        }
    });
    let ctx = env.ctx.clone();
    let pending =
        tokio::spawn(
            async move { execute(&ctx, "c1", "tiny", AbortController::new().signal()).await },
        );
    tokio::task::yield_now().await;
    // Node and hoh-timeout clamp a positive fractional timer below 1ms to 1ms.
    tokio::time::advance(Duration::from_millis(1)).await;
    let result = pending.await.expect("execution task joins");
    assert_timeout(&result, 1e-7);
    assert_eq!(
        result.content(),
        &[ContentBlock::text(
            "Error: tool call timed out after 1e-7ms"
        )]
    );
}

#[tokio::test(start_paused = true)]
async fn replaces_a_provider_abort_error_when_the_policy_signal_was_its_own() {
    let env = setup().await;
    register(&env, "aborter", Some(100.0), |_, exec| {
        let signal = exec.signal();
        async move {
            signal.cancelled().await;
            Err(Box::new(HarnessError::named(
                "WebAbortError",
                "web fetch aborted",
                "WEB_ABORTED",
            )) as BoxError)
        }
    });
    let ctx = env.ctx.clone();
    let pending = tokio::spawn(async move {
        execute(&ctx, "c1", "aborter", AbortController::new().signal()).await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(150)).await;
    assert_timeout(&pending.await.expect("execution task joins"), 100.0);
}

#[tokio::test(start_paused = true)]
async fn preserves_registry_aborted_when_the_caller_aborts_first() {
    let env = setup().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let tool_entered = entered.clone();
    register(&env, "slow", Some(100.0), move |_, exec| {
        tool_entered.notify_one();
        let signal = exec.signal();
        async move {
            signal.cancelled().await;
            Ok(vec![ContentBlock::text("stopped cooperatively")])
        }
    });
    let upstream = Arc::new(AbortController::new());
    let caller = upstream.clone();
    let ctx = env.ctx.clone();
    let pending = tokio::spawn(async move { execute(&ctx, "c1", "slow", caller.signal()).await });
    entered.notified().await;
    upstream.abort_with("user cancelled");
    tokio::task::yield_now().await;
    let result = pending.await.expect("execution task joins");
    assert!(result.is_error());
    let failure = result.error().expect("aborted is a failure");
    assert_eq!(failure.message, "tool call aborted");
    assert_eq!(failure.info.as_ref().unwrap().name, "AbortError");
    assert_eq!(failure.info.as_ref().unwrap().code, TOOL_ABORTED);
    assert_eq!(
        result.content(),
        &[ContentBlock::text("Error: tool call aborted")]
    );
}

#[tokio::test(start_paused = true)]
async fn preserves_timeout_when_it_wins_before_a_later_caller_abort() {
    let env = setup().await;
    let (saw_abort_tx, saw_abort_rx) = tokio::sync::oneshot::channel();
    let saw_abort = Arc::new(Mutex::new(Some(saw_abort_tx)));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new(Mutex::new(Some(release_rx)));
    register(&env, "slow-cleanup", Some(100.0), move |_, exec| {
        let signal = exec.signal();
        let saw_abort = saw_abort.clone();
        let release = release.clone();
        async move {
            signal.cancelled().await;
            if let Some(sender) = lock(&saw_abort).take() {
                let _ = sender.send(());
            }
            let receiver = lock(&release).take().expect("one execution");
            let _ = receiver.await;
            Ok(vec![ContentBlock::text("cleanup complete")])
        }
    });
    let upstream = Arc::new(AbortController::new());
    let caller = upstream.clone();
    let ctx = env.ctx.clone();
    let pending = tokio::spawn(async move {
        execute(&ctx, "timeout-first", "slow-cleanup", caller.signal()).await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    saw_abort_rx.await.expect("tool observes timeout");
    upstream.abort_with("too late to replace timeout");
    release_tx.send(()).expect("release cleanup");
    assert_timeout(&pending.await.expect("execution task joins"), 100.0);
}

#[tokio::test(start_paused = true)]
async fn does_not_misclassify_a_differently_coded_outer_deadline() {
    let env = setup().await;
    register(&env, "slow", Some(200.0), |_, exec| {
        let signal = exec.signal();
        async move {
            signal.cancelled().await;
            Ok(vec![ContentBlock::text("outer stopped")])
        }
    });
    let outer = deadline(None, 50.0, "OUTER_TIMEOUT").expect("outer deadline");
    let ctx = env.ctx.clone();
    let pending =
        tokio::spawn(async move { execute(&ctx, "c1", "slow", outer.signal.clone()).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(50)).await;
    let result = pending.await.expect("execution task joins");
    assert_eq!(
        result.error().unwrap().info.as_ref().unwrap().code,
        TOOL_ABORTED
    );
}

#[test]
fn exposes_the_owned_code_and_plugin_name() {
    assert_eq!(TOOL_TIMEOUT, "TOOL_TIMEOUT");
    assert_eq!(
        <ToolCallTimeoutPolicy as hoh_kernel::Plugin>::NAME,
        "timeout-policy"
    );
}

#[tokio::test(start_paused = true)]
async fn disposing_the_plugin_removes_its_execute_listener() {
    let env = setup().await;
    register(&env, "late", Some(100.0), |_, _| async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(vec![ContentBlock::text("late result")])
    });

    let ctx = env.ctx.clone();
    let first =
        tokio::spawn(
            async move { execute(&ctx, "c1", "late", AbortController::new().signal()).await },
        );
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(150)).await;
    assert_timeout(&first.await.expect("first task joins"), 100.0);

    env.policy.dispose().await;
    let ctx = env.ctx.clone();
    let second =
        tokio::spawn(
            async move { execute(&ctx, "c2", "late", AbortController::new().signal()).await },
        );
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(150)).await;
    let result = second.await.expect("second task joins");
    assert!(!result.is_error());
    assert_eq!(result.content(), &[ContentBlock::text("late result")]);
}

#[tokio::test]
async fn boots_through_the_real_plugin_and_native_executor_load_path() {
    let env = setup().await;
    register(&env, "fast", Some(5_000.0), |_, _| async {
        Ok(vec![ContentBlock::text("ok")])
    });
    let result = execute(&env.ctx, "c1", "fast", AbortController::new().signal()).await;
    assert!(!result.is_error());
    assert_eq!(result.content(), &[ContentBlock::text("ok")]);
}
