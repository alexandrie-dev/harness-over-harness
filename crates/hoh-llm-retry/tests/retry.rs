mod support;

use hoh_agent::{AgentRequestError, RequestErrorAction};
use hoh_llm::{LlmFailure, ResolvedRetryBackoff, ResolvedRetryPolicy};
use hoh_llm_retry::{
    Config, LlmRetryEventData, LlmRetryStartedEventData, RetryId, RetryInternals, apply,
};
use hoh_session::SessionEventEmitted;
use hoh_timeout::AbortController;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use support::Env;

fn backoff(initial: f64, max: f64, jitter: f64) -> ResolvedRetryBackoff {
    ResolvedRetryBackoff {
        initial_delay_ms: initial,
        max_delay_ms: max,
        jitter_ratio: jitter,
    }
}

fn normal(
    max_retries: u64,
    codes: &[&str],
    initial: f64,
    max: f64,
    jitter: f64,
) -> ResolvedRetryPolicy {
    ResolvedRetryPolicy::Normal {
        max_retries,
        retryable_codes: codes.iter().map(|code| (*code).to_owned()).collect(),
        backoff: backoff(initial, max, jitter),
    }
}

fn always(initial: f64, max: f64, jitter: f64) -> ResolvedRetryPolicy {
    ResolvedRetryPolicy::Always {
        backoff: backoff(initial, max, jitter),
    }
}

async fn install(env: &Env, samples: impl IntoIterator<Item = f64>) {
    let samples = Arc::new(std::sync::Mutex::new(
        samples
            .into_iter()
            .collect::<std::collections::VecDeque<_>>(),
    ));
    apply(
        env.ctx.clone(),
        env.registry.clone(),
        Config::default(),
        RetryInternals {
            random: Some(Arc::new(move || {
                samples
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pop_front()
                    .unwrap_or(0.5)
            })),
        },
    )
    .await
    .unwrap();
}

fn spawn_request(
    env: &Env,
    provider: &str,
    failure: LlmFailure,
    policy: Option<ResolvedRetryPolicy>,
    signal: hoh_timeout::AbortSignal,
) -> tokio::task::JoinHandle<Result<Option<RequestErrorAction>, hoh_error::BoxError>> {
    let ctx = env.ctx.clone();
    let agent = env.agent.clone();
    let provider = provider.to_owned();
    tokio::spawn(async move {
        ctx.waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(
            AgentRequestError {
                agent,
                turn: 1,
                step: 1,
                provider,
                failure,
                retry_policy: policy,
                signal,
            },
            |_| async { Ok(None) },
        )
        .await
    })
}

async fn wait_for_events(env: &Env, event_type: &str, count: usize) {
    for _ in 0..64 {
        if env
            .event_types()
            .iter()
            .filter(|kind| kind.as_str() == event_type)
            .count()
            >= count
        {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("event {event_type} did not reach count {count}");
}

fn retry_events(env: &Env) -> Vec<Value> {
    env.agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "llm/retry")
        .map(|event| event.data().clone())
        .collect()
}

#[test]
fn keeps_the_browser_safe_retry_payload_identical_to_the_session_event() {
    let normal = LlmRetryEventData::Normal {
        retry_id: RetryId::from("retry-1"),
        turn: 1,
        step: 2,
        provider: "mock".into(),
        policy_key: "policy".into(),
        retry: 3,
        max_retries: 4,
        delay_ms: 500.0,
        failure: LlmFailure::new("busy", "SERVER"),
    };
    let text = hoh_util_values::js_json_stringify(&serde_json::to_value(normal).unwrap());
    assert_eq!(
        text,
        r#"{"retryId":"retry-1","turn":1,"step":2,"provider":"mock","mode":"normal","policyKey":"policy","retry":3,"maxRetries":4,"delayMs":500,"failure":{"message":"busy","code":"SERVER"}}"#
    );
    let started = LlmRetryStartedEventData {
        retry_id: "retry-1".into(),
        turn: 1,
        step: 2,
        retry: 3,
    };
    assert_eq!(
        serde_json::to_value(started).unwrap(),
        json!({"retryId":"retry-1","turn":1,"step":2,"retry":3})
    );
}

#[tokio::test(start_paused = true)]
async fn records_the_scheduled_delay_before_retrying_the_request() {
    let env = Env::new("retry-success").await;
    install(&env, [0.5]).await;
    let controller = AbortController::new();
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("busy", "RATE_LIMIT"),
        Some(normal(2, &["SERVER", "RATE_LIMIT"], 500.0, 10_000.0, 0.0)),
        controller.signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    assert!(!task.is_finished());
    let event = &retry_events(&env)[0];
    assert_eq!(event["provider"], "mock");
    assert_eq!(
        event["policyKey"],
        r#"["normal",2,["RATE_LIMIT","SERVER"],500,10000,0]"#
    );
    assert_eq!(event["retry"], 1);
    assert_eq!(event["maxRetries"], 2);
    assert_eq!(event["delayMs"], json!(500.0));
    tokio::time::advance(Duration::from_millis(499)).await;
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    assert!(env.event_types().contains(&"llm/retry-started".to_owned()));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn applies_bounded_exponential_jitter_and_stops_after_the_configured_budget() {
    let env = Env::new("retry-exhausted").await;
    install(&env, [0.0, 1.0]).await;
    let policy = normal(2, &["SERVER"], 500.0, 10_000.0, 0.1);

    let first = spawn_request(
        &env,
        "mock",
        LlmFailure::new("busy one", "SERVER"),
        Some(policy.clone()),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    assert_eq!(retry_events(&env)[0]["delayMs"], json!(450.0));
    tokio::time::advance(Duration::from_millis(450)).await;
    assert_eq!(
        first.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );

    let second = spawn_request(
        &env,
        "mock",
        LlmFailure::new("busy two", "SERVER"),
        Some(policy.clone()),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 2).await;
    assert_eq!(retry_events(&env)[1]["delayMs"], json!(1_100.0));
    tokio::time::advance(Duration::from_millis(1_100)).await;
    assert_eq!(
        second.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );

    let third = spawn_request(
        &env,
        "mock",
        LlmFailure::new("busy three", "SERVER"),
        Some(policy),
        AbortController::new().signal(),
    );
    assert_eq!(third.await.unwrap().unwrap(), None);
    assert_eq!(retry_events(&env).len(), 2);
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn accepts_the_zero_delay_lower_jitter_bound() {
    let env = Env::new("retry-zero-delay").await;
    install(&env, [0.0]).await;
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("busy", "SERVER"),
        Some(normal(1, &["SERVER"], 1.0, 1.0, 1.0)),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    assert_eq!(retry_events(&env)[0]["delayMs"], json!(0.0));
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn uses_a_bounded_provider_retry_after_verbatim() {
    let env = Env::new("retry-after-accepted").await;
    install(&env, [0.0]).await;
    let mut failure = LlmFailure::new("wait", "RATE_LIMIT");
    failure.provider_retry_after_ms = Some(2_000.0);
    let task = spawn_request(
        &env,
        "mock",
        failure,
        Some(normal(1, &["RATE_LIMIT"], 500.0, 10_000.0, 1.0)),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    assert_eq!(retry_events(&env)[0]["delayMs"], json!(2_000.0));
    tokio::time::advance(Duration::from_millis(2_000)).await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn delegates_an_over_cap_provider_retry_after_in_normal_mode() {
    let env = Env::new("retry-after-rejected").await;
    install(&env, [0.5]).await;
    let mut failure = LlmFailure::new("wait too long", "RATE_LIMIT");
    failure.provider_retry_after_ms = Some(10_001.0);
    let task = spawn_request(
        &env,
        "mock",
        failure,
        Some(normal(1, &["RATE_LIMIT"], 500.0, 10_000.0, 0.0)),
        AbortController::new().signal(),
    );
    assert_eq!(task.await.unwrap().unwrap(), None);
    assert!(retry_events(&env).is_empty());
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn uses_local_jittered_backoff_when_always_mode_receives_an_over_cap_retry_after() {
    let env = Env::new("retry-always-over-cap").await;
    install(&env, [1.0]).await;
    let mut failure = LlmFailure::new("wait too long", "AUTH");
    failure.provider_retry_after_ms = Some(10.0);
    let task = spawn_request(
        &env,
        "mock",
        failure,
        Some(always(2.0, 4.0, 0.5)),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    assert_eq!(retry_events(&env)[0]["delayMs"], json!(3.0));
    tokio::time::advance(Duration::from_millis(3)).await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn delegates_non_transient_failures_without_scheduling_a_timer() {
    let env = Env::new("retry-auth").await;
    install(&env, [0.5]).await;
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("bad key", "AUTH"),
        Some(normal(2, &["SERVER"], 500.0, 10_000.0, 0.0)),
        AbortController::new().signal(),
    );
    assert_eq!(task.await.unwrap().unwrap(), None);
    assert!(retry_events(&env).is_empty());
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn delegates_when_no_final_adapter_policy_served_the_failed_request() {
    let env = Env::new("retry-no-serving-policy").await;
    install(&env, [0.5]).await;
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("no adapter", "NO_ADAPTER"),
        None,
        AbortController::new().signal(),
    );
    assert_eq!(task.await.unwrap().unwrap(), None);
    assert!(retry_events(&env).is_empty());
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn starts_a_fresh_retry_budget_for_another_provider_and_after_step_start() {
    let env = Env::new("retry-provider-budgets").await;
    install(&env, [0.5, 0.5, 0.5]).await;
    let policy = normal(1, &["SERVER"], 1.0, 1.0, 0.0);
    for (provider, expected_count) in [("mock", 1), ("other", 2)] {
        let task = spawn_request(
            &env,
            provider,
            LlmFailure::new("failed", "SERVER"),
            Some(policy.clone()),
            AbortController::new().signal(),
        );
        wait_for_events(&env, "llm/retry", expected_count).await;
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(
            task.await.unwrap().unwrap(),
            Some(RequestErrorAction::Retry)
        );
    }
    env.agent
        .session()
        .append("step/start", json!({"turn":1,"step":2}), None)
        .unwrap();
    let reset = spawn_request(
        &env,
        "mock",
        LlmFailure::new("failed again", "SERVER"),
        Some(policy),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 3).await;
    assert_eq!(retry_events(&env)[2]["retry"], 1);
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        reset.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn changed_policy_key_resets_retry_history() {
    let env = Env::new("retry-serving-registration").await;
    install(&env, [0.5, 0.5]).await;
    for (delay, expected_count) in [(1_u64, 1_usize), (3, 2)] {
        let task = spawn_request(
            &env,
            "mock",
            LlmFailure::new("failed", "AUTH"),
            Some(always(delay as f64, delay as f64, 0.0)),
            AbortController::new().signal(),
        );
        wait_for_events(&env, "llm/retry", expected_count).await;
        assert_eq!(retry_events(&env)[expected_count - 1]["retry"], 1);
        tokio::time::advance(Duration::from_millis(delay)).await;
        assert_eq!(
            task.await.unwrap().unwrap(),
            Some(RequestErrorAction::Retry)
        );
    }
    assert_ne!(
        retry_events(&env)[0]["policyKey"],
        retry_events(&env)[1]["policyKey"]
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn keeps_always_mode_unbounded_with_bounded_jittered_backoff() {
    let env = Env::new("retry-always-unbounded").await;
    install(&env, [1.0, 1.0, 1.0, 1.0]).await;
    let policy = always(1.0, 4.0, 0.1);
    let expected: [f64; 4] = [1.1, 2.2, 4.0, 4.0];
    for (index, delay) in expected.into_iter().enumerate() {
        let task = spawn_request(
            &env,
            "mock",
            LlmFailure::new("auth", "AUTH"),
            Some(policy.clone()),
            AbortController::new().signal(),
        );
        wait_for_events(&env, "llm/retry", index + 1).await;
        assert_eq!(retry_events(&env)[index]["delayMs"], json!(delay));
        tokio::time::advance(Duration::from_millis(delay.ceil() as u64)).await;
        assert_eq!(
            task.await.unwrap().unwrap(),
            Some(RequestErrorAction::Retry)
        );
    }
    assert!(
        retry_events(&env)
            .iter()
            .all(|event| event.get("maxRetries").is_none())
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn lets_downstream_specialized_recovery_run_before_always_fallback() {
    let env = Env::new("retry-always-composition").await;
    install(&env, [0.5]).await;
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(|_, _| async {
            Ok(Some(RequestErrorAction::Retry))
        })
        .unwrap();
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("specialized", "AUTH"),
        Some(always(500.0, 10_000.0, 0.0)),
        AbortController::new().signal(),
    );
    assert_eq!(
        task.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    assert!(retry_events(&env).is_empty());
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn falls_back_to_always_retry_when_downstream_recovery_throws() {
    let env = Env::new("retry-always-downstream-error").await;
    install(&env, [0.5]).await;
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(|_, _| async {
            Err("downstream recovery failed".into())
        })
        .unwrap();
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("fallback", "AUTH"),
        Some(always(1.0, 1.0, 0.0)),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    assert!(env.ctx.logger().buffer().iter().any(|record| {
        record
            .message
            .contains("always policy ignored a downstream recovery failure")
    }));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn falls_back_to_always_retry_when_downstream_recovery_fails_immediately() {
    let env = Env::new("retry-always-downstream-sync-error").await;
    install(&env, [0.5]).await;
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(|_, _| {
            std::future::ready(Err("downstream recovery failed".into()))
        })
        .unwrap();
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("fallback", "AUTH"),
        Some(always(1.0, 1.0, 0.0)),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn lets_turn_cancellation_win_during_backoff_without_starting_retry() {
    let env = Env::new("retry-cancel").await;
    install(&env, [0.5]).await;
    let controller = AbortController::new();
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("permanent", "AUTH"),
        Some(always(500.0, 10_000.0, 0.0)),
        controller.signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    controller.abort();
    assert_eq!(task.await.unwrap().unwrap(), None);
    assert!(!env.event_types().contains(&"llm/retry-started".to_owned()));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn handles_synchronous_cancellation_from_the_retry_status_event() {
    let env = Env::new("retry-event-cancel").await;
    install(&env, [0.5]).await;
    let controller = Arc::new(AbortController::new());
    let cancel = controller.clone();
    env.ctx
        .on_contained::<SessionEventEmitted>(move |event| {
            if event.event.event_type() == "llm/retry" {
                cancel.abort();
            }
            Ok(())
        })
        .unwrap();
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("temporary", "SERVER"),
        Some(normal(1, &["SERVER"], 500.0, 10_000.0, 0.0)),
        controller.signal(),
    );
    assert_eq!(task.await.unwrap().unwrap(), None);
    assert_eq!(retry_events(&env).len(), 1);
    assert!(!env.event_types().contains(&"llm/retry-started".to_owned()));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn aborts_and_drains_a_captured_backoff_before_plugin_disposal_completes() {
    let env = Env::new("retry-hmr").await;
    let registry = env.registry.clone();
    let fiber = env
        .ctx
        .inject::<(), _>(move |ctx, ()| {
            let registry = registry.clone();
            async move {
                apply(
                    ctx,
                    registry,
                    Config::default(),
                    RetryInternals {
                        random: Some(Arc::new(|| 0.5)),
                    },
                )
                .await
            }
        })
        .unwrap();
    fiber.settled().await.unwrap();
    let task = spawn_request(
        &env,
        "mock",
        LlmFailure::new("temporary", "TRANSPORT"),
        Some(always(500.0, 10_000.0, 0.0)),
        AbortController::new().signal(),
    );
    wait_for_events(&env, "llm/retry", 1).await;
    fiber.dispose().await;
    assert_eq!(task.await.unwrap().unwrap(), None);
    assert!(!env.event_types().contains(&"llm/retry-started".to_owned()));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn drains_delegated_recovery_before_completing_plugin_disposal() {
    let env = Env::new("retry-delegated-disposal").await;
    let registry = env.registry.clone();
    let fiber = env
        .ctx
        .inject::<(), _>(move |ctx, ()| {
            let registry = registry.clone();
            async move { apply(ctx, registry, Config::default(), RetryInternals::default()).await }
        })
        .unwrap();
    fiber.settled().await.unwrap();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release = Arc::new(std::sync::Mutex::new(Some(release_rx)));
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_listener = entered.clone();
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |_, _| {
            let release = release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            entered_listener.notify_one();
            async move {
                if let Some(release) = release {
                    let _ = release.await;
                }
                Ok(Some(RequestErrorAction::Retry))
            }
        })
        .unwrap();
    let request = spawn_request(
        &env,
        "mock",
        LlmFailure::new("bad key", "AUTH"),
        Some(always(500.0, 10_000.0, 0.0)),
        AbortController::new().signal(),
    );
    entered.notified().await;
    let disposing = tokio::spawn(async move { fiber.dispose().await });
    tokio::task::yield_now().await;
    assert!(!disposing.is_finished());
    let _ = release_tx.send(());
    disposing.await.unwrap();
    assert_eq!(request.await.unwrap().unwrap(), None);
    assert!(retry_events(&env).is_empty());
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn drains_delegated_recovery_before_turn_cancellation_settles() {
    let env = Env::new("retry-delegated-cancel").await;
    install(&env, [0.5]).await;
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release = Arc::new(std::sync::Mutex::new(Some(release_rx)));
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_listener = entered.clone();
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |_, _| {
            let release = release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            entered_listener.notify_one();
            async move {
                if let Some(release) = release {
                    let _ = release.await;
                }
                Ok(Some(RequestErrorAction::Retry))
            }
        })
        .unwrap();
    let controller = AbortController::new();
    let request = spawn_request(
        &env,
        "mock",
        LlmFailure::new("bad key", "AUTH"),
        Some(always(500.0, 10_000.0, 0.0)),
        controller.signal(),
    );
    entered.notified().await;
    controller.abort();
    tokio::task::yield_now().await;
    assert!(!request.is_finished());
    let _ = release_tx.send(());
    assert_eq!(request.await.unwrap().unwrap(), None);
    assert!(retry_events(&env).is_empty());
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn handles_synchronous_cancellation_while_entering_delegated_recovery() {
    let env = Env::new("retry-delegated-sync-cancel").await;
    install(&env, [0.5]).await;
    let controller = Arc::new(AbortController::new());
    let cancel = controller.clone();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release = Arc::new(std::sync::Mutex::new(Some(release_rx)));
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_listener = entered.clone();
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |_, _| {
            cancel.abort();
            let release = release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            entered_listener.notify_one();
            async move {
                if let Some(release) = release {
                    let _ = release.await;
                }
                Ok(Some(RequestErrorAction::Retry))
            }
        })
        .unwrap();
    let request = spawn_request(
        &env,
        "mock",
        LlmFailure::new("bad key", "AUTH"),
        Some(always(500.0, 10_000.0, 0.0)),
        controller.signal(),
    );
    entered.notified().await;
    assert!(!request.is_finished());
    let _ = release_tx.send(());
    assert_eq!(request.await.unwrap().unwrap(), None);
    assert!(retry_events(&env).is_empty());
    env.close().await;
}

async fn earlier_listener_cancellation_prevents_policy(policy: ResolvedRetryPolicy, id: &str) {
    let env = Env::new(id).await;
    let controller = Arc::new(AbortController::new());
    let cancel = controller.clone();
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |_, next| {
            cancel.abort();
            next.run()
        })
        .unwrap();
    install(&env, [0.5]).await;
    let request = spawn_request(
        &env,
        "mock",
        LlmFailure::new("temporary", "SERVER"),
        Some(policy),
        controller.signal(),
    );
    assert_eq!(request.await.unwrap().unwrap(), None);
    assert!(retry_events(&env).is_empty());
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn lets_an_earlier_recovery_listener_cancel_before_normal_retry_policy_runs() {
    earlier_listener_cancellation_prevents_policy(
        normal(1, &["SERVER"], 500.0, 10_000.0, 0.0),
        "retry-pre-cancel-normal",
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn lets_an_earlier_recovery_listener_cancel_before_always_retry_policy_runs() {
    earlier_listener_cancellation_prevents_policy(
        always(500.0, 10_000.0, 0.0),
        "retry-pre-cancel-always",
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn rejects_retry_policy_configured_on_the_executor() {
    let env = Env::new("bad-config-policy").await;
    let config: Config = serde_json::from_value(json!({"retryPolicy":{"mode":"always"}})).unwrap();
    let error = apply(
        env.ctx.clone(),
        env.registry.clone(),
        config,
        RetryInternals::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "llm-retry: retryPolicy belongs under each provider configuration"
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_unknown_executor_config() {
    let env = Env::new("bad-config-key").await;
    let config: Config = serde_json::from_value(json!({"retryPolciy":{}})).unwrap();
    let error = apply(
        env.ctx.clone(),
        env.registry.clone(),
        config,
        RetryInternals::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "llm-retry: unknown key \"retryPolciy\"");
    env.close().await;
}
