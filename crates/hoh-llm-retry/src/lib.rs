//! Provider-routed model-request recovery at durable agent-step boundaries.
//!
//! A retry is recorded before its cancellable delay. The agent loop then repeats the failed
//! request inside the same open step, while direct `hoh_llm` streams remain single-attempt.

use hoh_agent::{AgentRequestError, Agents, RequestErrorAction};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, FiberState, Inject, Plugin, Service, Undo};
use hoh_llm::{LlmFailure, ResolvedRetryBackoff, ResolvedRetryPolicy};
use hoh_session::SessionEvent;
use hoh_session_projection::{
    ProjectionDefinition, ProjectionHandle, ProjectionRegistration, SessionProjectionRegistry,
    SessionProjections,
};
use hoh_timeout::{AbortController, AbortSignal};
use indexmap::IndexMap;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use serde_json::{Value, json};
use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::{Notify, OnceCell};

hoh_brand::branded_string!(pub RetryId);

/// Durable payload recorded before one provider-routed model-request retry wait.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum LlmRetryEventData {
    Normal {
        retry_id: RetryId,
        turn: u64,
        step: u64,
        provider: String,
        policy_key: String,
        retry: u64,
        max_retries: u64,
        delay_ms: f64,
        failure: LlmFailure,
    },
    Always {
        retry_id: RetryId,
        turn: u64,
        step: u64,
        provider: String,
        policy_key: String,
        retry: u64,
        delay_ms: f64,
        failure: LlmFailure,
    },
}

// The source constructs these objects in this exact key order. Internally tagged serde enums put
// `mode` first, so serialization is written out explicitly for the persistence boundary.
impl Serialize for LlmRetryEventData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Normal {
                retry_id,
                turn,
                step,
                provider,
                policy_key,
                retry,
                max_retries,
                delay_ms,
                failure,
            } => {
                let mut state = serializer.serialize_struct("LlmRetryEventData", 10)?;
                state.serialize_field("retryId", retry_id)?;
                state.serialize_field("turn", turn)?;
                state.serialize_field("step", step)?;
                state.serialize_field("provider", provider)?;
                state.serialize_field("mode", "normal")?;
                state.serialize_field("policyKey", policy_key)?;
                state.serialize_field("retry", retry)?;
                state.serialize_field("maxRetries", max_retries)?;
                state.serialize_field("delayMs", delay_ms)?;
                state.serialize_field("failure", failure)?;
                state.end()
            }
            Self::Always {
                retry_id,
                turn,
                step,
                provider,
                policy_key,
                retry,
                delay_ms,
                failure,
            } => {
                let mut state = serializer.serialize_struct("LlmRetryEventData", 9)?;
                state.serialize_field("retryId", retry_id)?;
                state.serialize_field("turn", turn)?;
                state.serialize_field("step", step)?;
                state.serialize_field("provider", provider)?;
                state.serialize_field("mode", "always")?;
                state.serialize_field("policyKey", policy_key)?;
                state.serialize_field("retry", retry)?;
                state.serialize_field("delayMs", delay_ms)?;
                state.serialize_field("failure", failure)?;
                state.end()
            }
        }
    }
}

/// Durable transition recorded after one retry delay completes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRetryStartedEventData {
    pub retry_id: RetryId,
    pub turn: u64,
    pub step: u64,
    pub retry: u64,
}

/// Empty executor configuration. Provider registrations own `retryPolicy`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Config(IndexMap<String, Value>);

impl JsonSchema for Config {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Config".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        // Schemastery's `object({})` retains unknown keys so apply can issue the source diagnostics.
        Schema::try_from(json!({ "type": "object" })).unwrap_or_else(|_| Schema::from(true))
    }
}

/// Non-serializable hooks used to make timing policy deterministic in tests.
#[derive(Clone, Default)]
pub struct RetryInternals {
    /// Random sample in the inclusive zero-to-one range used for jitter.
    pub random: Option<Arc<dyn Fn() -> f64 + Send + Sync>>,
}

/// Services required by `inject = ['agents', 'sessionProjections']`.
#[derive(Inject)]
pub struct Deps {
    _agents: Service<Agents>,
    projections: Service<SessionProjections>,
}

/// Function-plugin counterpart of `@deepseek-ai/dsh-llm-retry`.
pub struct LlmRetryPlugin;

impl Plugin for LlmRetryPlugin {
    const NAME: &'static str = "llm-retry";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        apply(
            ctx,
            (*deps.projections).clone(),
            (*config.get()).clone(),
            RetryInternals::default(),
        )
        .await
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct RetryError(String);

fn error(message: impl Into<String>) -> BoxError {
    Box::new(RetryError(message.into()))
}

fn validate_config(config: &Config) -> Result<(), BoxError> {
    let Some((key, _)) = config.0.first() else {
        return Ok(());
    };
    if key == "retryPolicy" {
        return Err(error(
            "llm-retry: retryPolicy belongs under each provider configuration",
        ));
    }
    Err(error(format!("llm-retry: unknown key \"{key}\"")))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetryStateEntry {
    retry: u64,
    retry_id: RetryId,
}

type LlmRetryState = IndexMap<String, RetryStateEntry>;

fn retry_state_key(provider: &str, policy_key: &str) -> String {
    hoh_util_values::js_json_stringify(&json!([provider, policy_key]))
}

fn retry_projection() -> ProjectionDefinition<LlmRetryState> {
    ProjectionDefinition {
        key: "llmRetry".into(),
        state_version: 1,
        state_schema: Arc::new(|raw| {
            let raw = raw.ok_or_else(|| error("persisted llmRetry projection state is missing"))?;
            Ok(Arc::new(serde_json::from_value(raw.clone())?))
        }),
        init: Arc::new(|_, _| Ok(Arc::new(IndexMap::new()))),
        apply: Arc::new(apply_retry_projection),
        wire: None,
    }
}

fn apply_retry_projection(
    state: &Arc<LlmRetryState>,
    event: &SessionEvent,
) -> Result<Arc<LlmRetryState>, BoxError> {
    if matches!(event.event_type(), "step/start" | "turn/end") {
        return Ok(Arc::new(IndexMap::new()));
    }
    if event.event_type() != "llm/retry" {
        return Ok(state.clone());
    }
    let data = event.data();
    let provider = data
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(|| error("llm/retry without a string provider"))?;
    let policy_key = data
        .get("policyKey")
        .and_then(Value::as_str)
        .ok_or_else(|| error("llm/retry without a string policyKey"))?;
    let retry = data
        .get("retry")
        .and_then(Value::as_u64)
        .ok_or_else(|| error("llm/retry without a non-negative integer retry"))?;
    let retry_id: RetryId = data
        .get("retryId")
        .and_then(Value::as_str)
        .ok_or_else(|| error("llm/retry without a string retryId"))?
        .into();
    let key = retry_state_key(provider, policy_key);
    if state
        .get(&key)
        .is_some_and(|entry| entry.retry == retry && entry.retry_id == retry_id)
    {
        return Ok(state.clone());
    }
    let mut next = (**state).clone();
    next.insert(key, RetryStateEntry { retry, retry_id });
    Ok(Arc::new(next))
}

struct ProjectionReady {
    registration: Mutex<Option<ProjectionRegistration<LlmRetryState>>>,
    result: OnceCell<Result<ProjectionHandle<LlmRetryState>, SharedError>>,
}

impl ProjectionReady {
    fn new(registration: ProjectionRegistration<LlmRetryState>) -> Self {
        Self {
            registration: Mutex::new(Some(registration)),
            result: OnceCell::new(),
        }
    }

    async fn get(&self) -> Result<ProjectionHandle<LlmRetryState>, BoxError> {
        let settled = self
            .result
            .get_or_init(|| async {
                let registration = lock(&self.registration).take();
                let Some(registration) = registration else {
                    return Err(SharedError::new(error(
                        "llm-retry projection registration was already consumed",
                    )));
                };
                registration.await.map_err(SharedError::new)
            })
            .await;
        match settled {
            Ok(handle) => Ok(handle.clone()),
            Err(cause) => Err(Box::new(cause.clone())),
        }
    }
}

#[derive(Default)]
struct ActiveState {
    closed: bool,
    count: usize,
}

struct Lifetime {
    controller: AbortController,
    state: Mutex<ActiveState>,
    changed: Notify,
}

impl Lifetime {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            controller: AbortController::new(),
            state: Mutex::new(ActiveState::default()),
            changed: Notify::new(),
        })
    }

    fn signal(&self) -> AbortSignal {
        self.controller.signal()
    }

    fn enter(self: &Arc<Self>) -> Option<ActiveGuard> {
        let mut state = lock(&self.state);
        if state.closed {
            return None;
        }
        state.count += 1;
        Some(ActiveGuard {
            lifetime: self.clone(),
        })
    }

    async fn close_and_drain(&self) {
        {
            let mut state = lock(&self.state);
            state.closed = true;
        }
        self.controller
            .abort_with(RetryError("llm-retry plugin disposed".into()));
        loop {
            let changed = self.changed.notified();
            if lock(&self.state).count == 0 {
                return;
            }
            changed.await;
        }
    }
}

struct ActiveGuard {
    lifetime: Arc<Lifetime>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.lifetime.state);
        state.count -= 1;
        let drained = state.count == 0;
        drop(state);
        if drained {
            self.lifetime.changed.notify_waiters();
        }
    }
}

enum DownstreamOutcome {
    Decision(Option<RequestErrorAction>),
    Error(BoxError),
}

fn math_random() -> f64 {
    let mut bytes = [0_u8; 8];
    // JavaScript's Math.random is ambient and infallible; OS entropy is the process invariant here.
    getrandom::fill(&mut bytes).expect("OS random source is unavailable");
    let sample = u64::from_le_bytes(bytes) & ((1_u64 << 53) - 1);
    sample as f64 / (1_u64 << 53) as f64
}

fn js_min(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else {
        left.min(right)
    }
}

fn local_delay(backoff: &ResolvedRetryBackoff, retry: u64, random: &dyn Fn() -> f64) -> f64 {
    let exponent = retry.saturating_sub(1).min(1024) as i32;
    let exponential = js_min(
        backoff.initial_delay_ms * 2_f64.powi(exponent),
        backoff.max_delay_ms,
    );
    let jitter = 1.0 - backoff.jitter_ratio + 2.0 * backoff.jitter_ratio * random();
    js_min(exponential * jitter, backoff.max_delay_ms)
}

fn retry_policy_key(policy: &ResolvedRetryPolicy) -> String {
    let value = match policy {
        ResolvedRetryPolicy::Always { backoff } => json!([
            "always",
            backoff.initial_delay_ms,
            backoff.max_delay_ms,
            backoff.jitter_ratio
        ]),
        ResolvedRetryPolicy::Normal {
            max_retries,
            retryable_codes,
            backoff,
        } => {
            let mut codes = retryable_codes.clone();
            codes.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
            json!([
                "normal",
                max_retries,
                codes,
                backoff.initial_delay_ms,
                backoff.max_delay_ms,
                backoff.jitter_ratio
            ])
        }
    };
    hoh_util_values::js_json_stringify(&value)
}

async fn cancellable_delay(delay_ms: f64, signal: AbortSignal) -> bool {
    if signal.aborted() {
        return false;
    }
    let timer = AbortSignal::timeout(delay_ms);
    tokio::select! {
        biased;
        _ = signal.cancelled() => false,
        _ = timer.cancelled() => true,
    }
}

fn event_data(
    event: &AgentRequestError,
    policy: &ResolvedRetryPolicy,
    policy_key: String,
    retry: u64,
    retry_id: RetryId,
    delay_ms: f64,
) -> LlmRetryEventData {
    match policy {
        ResolvedRetryPolicy::Normal { max_retries, .. } => LlmRetryEventData::Normal {
            retry_id,
            turn: event.turn,
            step: event.step,
            provider: event.provider.clone(),
            policy_key,
            retry,
            max_retries: *max_retries,
            delay_ms,
            failure: event.failure.clone(),
        },
        ResolvedRetryPolicy::Always { .. } => LlmRetryEventData::Always {
            retry_id,
            turn: event.turn,
            step: event.step,
            provider: event.provider.clone(),
            policy_key,
            retry,
            delay_ms,
            failure: event.failure.clone(),
        },
    }
}

async fn backoff(
    event: &AgentRequestError,
    policy: &ResolvedRetryPolicy,
    policy_key: String,
    retry: u64,
    retry_id: RetryId,
    delay_ms: f64,
    lifetime: &Lifetime,
) -> Result<Option<RequestErrorAction>, BoxError> {
    let fused_signal = AbortSignal::any([event.signal.clone(), lifetime.signal()]);
    let data = event_data(event, policy, policy_key, retry, retry_id.clone(), delay_ms);
    event
        .agent
        .session()
        .append("llm/retry", serde_json::to_value(data)?, None)?;
    if !cancellable_delay(delay_ms, fused_signal).await {
        return Ok(None);
    }
    event.agent.session().append(
        "llm/retry-started",
        serde_json::to_value(LlmRetryStartedEventData {
            retry_id,
            turn: event.turn,
            step: event.step,
            retry,
        })?,
        None,
    )?;
    Ok(Some(RequestErrorAction::Retry))
}

async fn recover(
    ctx: Context,
    event: Arc<AgentRequestError>,
    next: hoh_kernel::Next<Option<RequestErrorAction>>,
    ready: Arc<ProjectionReady>,
    lifetime: Arc<Lifetime>,
    random: Arc<dyn Fn() -> f64 + Send + Sync>,
) -> Result<Option<RequestErrorAction>, BoxError> {
    let mut next = Some(next);
    let take_next = |next: &mut Option<hoh_kernel::Next<Option<RequestErrorAction>>>| {
        next.take()
            .ok_or_else(|| error("llm-retry downstream recovery was already consumed"))
    };
    let Some(policy) = event.retry_policy.as_ref() else {
        return take_next(&mut next)?.run().await;
    };
    let projection = ready.get().await?;
    if event.signal.aborted()
        || !matches!(
            ctx.fiber().state(),
            FiberState::Loading | FiberState::Active
        )
        || lifetime.signal().aborted()
    {
        return Ok(None);
    }

    if matches!(policy, ResolvedRetryPolicy::Always { .. }) {
        let fused_signal = AbortSignal::any([event.signal.clone(), lifetime.signal()]);
        let downstream = match take_next(&mut next)?.run().await {
            Ok(decision) => DownstreamOutcome::Decision(decision),
            Err(cause) => DownstreamOutcome::Error(cause),
        };
        if fused_signal.aborted() {
            return Ok(None);
        }
        match downstream {
            DownstreamOutcome::Decision(Some(RequestErrorAction::Retry)) => {
                return Ok(Some(RequestErrorAction::Retry));
            }
            DownstreamOutcome::Error(cause) => ctx.logger().warn(format_args!(
                "llm-retry: provider \"{}\" always policy ignored a downstream recovery failure: {cause}",
                event.provider
            )),
            DownstreamOutcome::Decision(None) => {}
        }
    } else if let ResolvedRetryPolicy::Normal {
        retryable_codes, ..
    } = policy
        && !retryable_codes.contains(&event.failure.code)
    {
        return take_next(&mut next)?.run().await;
    }

    let policy_key = retry_policy_key(policy);
    let retry_state = projection.state_of(event.agent.session())?;
    let previous = retry_state.get(&retry_state_key(&event.provider, &policy_key));
    let previous_retry = previous.map_or(0, |entry| entry.retry);
    if let ResolvedRetryPolicy::Normal { max_retries, .. } = policy
        && previous_retry >= *max_retries
    {
        return take_next(&mut next)?.run().await;
    }
    let retry = previous_retry + 1;
    let retry_id = previous
        .map(|entry| entry.retry_id.clone())
        .unwrap_or_else(|| hoh_util_crypto::random_uuid().as_str().into());
    let backoff_policy = match policy {
        ResolvedRetryPolicy::Normal { backoff, .. } | ResolvedRetryPolicy::Always { backoff } => {
            backoff
        }
    };
    let delay_ms = match event.failure.provider_retry_after_ms {
        Some(provider_delay) if provider_delay.is_finite() && provider_delay > 0.0 => {
            if provider_delay > backoff_policy.max_delay_ms {
                if matches!(policy, ResolvedRetryPolicy::Normal { .. }) {
                    return take_next(&mut next)?.run().await;
                }
                local_delay(backoff_policy, retry, random.as_ref())
            } else {
                provider_delay
            }
        }
        _ => local_delay(backoff_policy, retry, random.as_ref()),
    };
    backoff(
        &event, policy, policy_key, retry, retry_id, delay_ms, &lifetime,
    )
    .await
}

/// Install provider-routed normal or unbounded request recovery.
///
/// Recovery admitted during activation awaits projection registration. A registration failure
/// propagates; cancellation or plugin disposal suppresses retries.
pub async fn apply(
    ctx: Context,
    projections: SessionProjectionRegistry,
    config: Config,
    internals: RetryInternals,
) -> Result<(), BoxError> {
    validate_config(&config)?;
    let ready = Arc::new(ProjectionReady::new(
        projections.register(&ctx, retry_projection()),
    ));
    let lifetime = Lifetime::new();
    let random = internals.random.unwrap_or_else(|| Arc::new(math_random));
    let listener = {
        let ctx = ctx.clone();
        let ready = ready.clone();
        let lifetime = lifetime.clone();
        ctx.clone()
            .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(
                move |event, next| {
                    let ctx = ctx.clone();
                    let ready = ready.clone();
                    let lifetime = lifetime.clone();
                    let random = random.clone();
                    async move {
                        let Some(_active) = lifetime.enter() else {
                            return Ok(None);
                        };
                        recover(ctx, event, next, ready, lifetime, random).await
                    }
                },
            )?
    };
    ctx.try_effect(
        "llm-retry: abort and drain active recovery",
        |undos| -> Result<(), BoxError> {
            let lifetime = lifetime.clone();
            undos.push(Undo::future(move || async move {
                lifetime.close_and_drain().await;
                Ok(())
            }))?;
            // Effects unwind in reverse: remove the listener before aborting and draining work.
            undos.push(listener)?;
            Ok(())
        },
    )?;
    ready.get().await?;
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl fmt::Debug for RetryInternals {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RetryInternals")
            .field("random", &self.random.as_ref().map(|_| "<function>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_projection_keeps_the_same_state_when_a_recorded_retry_is_applied_again() {
        let initial = Arc::new(IndexMap::new());
        let event = SessionEvent::from_value(json!({
            "type":"llm/retry",
            "data":{
                "provider":"mock",
                "policyKey":"policy",
                "retry":1,
                "retryId":"retry-1"
            }
        }));
        let state = apply_retry_projection(&initial, &event).unwrap();
        let repeated = apply_retry_projection(&state, &event).unwrap();
        assert!(Arc::ptr_eq(&state, &repeated));
    }
}
