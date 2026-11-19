mod support;

use hoh_agent::{AgentRequestError, RequestErrorAction};
use hoh_llm::{LlmFailure, ResolvedRetryBackoff, ResolvedRetryPolicy};
use hoh_llm_retry::{Config, RetryInternals, apply};
use hoh_session_projection::ProjectionDefinition;
use hoh_timeout::AbortController;
use serde_json::Value;
use std::sync::Arc;
use support::Env;

fn policy() -> ResolvedRetryPolicy {
    ResolvedRetryPolicy::Normal {
        max_retries: 1,
        retryable_codes: vec!["SERVER".into()],
        backoff: ResolvedRetryBackoff {
            initial_delay_ms: 1.0,
            max_delay_ms: 1.0,
            jitter_ratio: 0.0,
        },
    }
}

fn request(
    env: &Env,
    signal: hoh_timeout::AbortSignal,
) -> impl std::future::Future<Output = Result<Option<RequestErrorAction>, hoh_error::BoxError>>
+ Send
+ 'static {
    let ctx = env.ctx.clone();
    let agent = env.agent.clone();
    async move {
        ctx.waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(
            AgentRequestError {
                agent,
                turn: 1,
                step: 1,
                provider: "mock".into(),
                failure: LlmFailure::new("temporary failure", "SERVER"),
                retry_policy: Some(policy()),
                signal,
            },
            |_| async { Ok(None) },
        )
        .await
    }
}

#[tokio::test(start_paused = true)]
async fn handles_a_ready_registration_before_settling_the_first_provider_failure() {
    let env = Env::new("initial-retry-ready").await;
    apply(
        env.ctx.clone(),
        env.registry.clone(),
        Config::default(),
        RetryInternals {
            random: Some(Arc::new(|| 0.5)),
        },
    )
    .await
    .unwrap();
    let task = tokio::spawn(request(&env, AbortController::new().signal()));
    for _ in 0..64 {
        if env.event_types().contains(&"llm/retry".to_owned()) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(env.event_types().contains(&"llm/retry".to_owned()));
    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        Some(RequestErrorAction::Retry)
    );
    assert!(env.event_types().contains(&"llm/retry".to_owned()));
    assert!(env.event_types().contains(&"llm/retry-started".to_owned()));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn handles_a_cancelled_registration_wait_before_settling_the_first_provider_failure() {
    let env = Env::new("initial-retry-cancelled").await;
    apply(
        env.ctx.clone(),
        env.registry.clone(),
        Config::default(),
        RetryInternals::default(),
    )
    .await
    .unwrap();
    let controller = AbortController::new();
    controller.abort();
    assert_eq!(request(&env, controller.signal()).await.unwrap(), None);
    assert!(!env.event_types().contains(&"llm/retry".to_owned()));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn handles_a_disposed_registration_before_settling_the_first_provider_failure() {
    let env = Env::new("initial-retry-disposed").await;
    let registry = env.registry.clone();
    let fiber = env
        .ctx
        .inject::<(), _>(move |ctx, ()| {
            let registry = registry.clone();
            async move { apply(ctx, registry, Config::default(), RetryInternals::default()).await }
        })
        .unwrap();
    fiber.settled().await.unwrap();
    fiber.dispose().await;
    assert_eq!(
        request(&env, AbortController::new().signal())
            .await
            .unwrap(),
        None
    );
    assert!(!env.event_types().contains(&"llm/retry".to_owned()));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn handles_a_failed_registration_before_settling_the_first_provider_failure() {
    let env = Env::new("initial-retry-failed").await;
    let conflicting = env.registry.register(
        &env.ctx,
        ProjectionDefinition {
            key: "llmRetry".into(),
            state_version: 2,
            state_schema: Arc::new(|value| Ok(Arc::new(value.cloned().unwrap_or(Value::Null)))),
            init: Arc::new(|_, _| Ok(Arc::new(Value::Null))),
            apply: Arc::new(|state, _| Ok(state.clone())),
            wire: None,
        },
    );
    conflicting.await.unwrap();
    let failure = apply(
        env.ctx.clone(),
        env.registry.clone(),
        Config::default(),
        RetryInternals::default(),
    )
    .await
    .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("already registered at stateVersion 2")
    );
    assert!(!env.event_types().contains(&"llm/retry".to_owned()));
    env.close().await;
}
