//! Durable per-step context with the current time and elapsed time.
//!
//! Port of `@deepseek-ai/dsh-time-context` (`packages/context/time-context`).

pub mod request_zone;
pub mod timestamp;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use hoh_agent::{Agent, AgentPreStep, Agents, PreStepDecision};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{
    ContentBlock, ContextFormed, ContextSnapshotSection, MessageSource, NewUserMessage,
    UserMessage, create_user_message,
};
use hoh_session::{MAX_SAFE_INTEGER, SessionEvent, SessionSeq};
use hoh_session_projection::{ProjectionDefinition, ProjectionHandle, SessionProjections};
use hoh_util_time::Duration;
use hoh_util_values::{js_json_stringify, js_number_string};
use request_zone::{
    BrowserTimeZoneContext, derive_browser_time_zone_context, render_browser_time_zone_context,
    reported_browser_time_zone,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use timestamp::{TimestampFormatter, create_timestamp_formatter, format_timestamp};
use tokio::sync::watch;

/// Cordis/Loader plugin name.
pub const NAME: &str = "time-context";

const DEFAULT_REFRESH_INTERVAL_MS: f64 = 600_000.0;

/// Plain JavaScript `Error`/`TypeError` equivalents raised by this package.
#[derive(Debug)]
pub enum TimeContextError {
    Error {
        message: String,
        cause: Option<BoxError>,
    },
    TypeError {
        message: String,
        cause: Option<BoxError>,
    },
    RangeError(String),
}

impl TimeContextError {
    /// JavaScript error class name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Error { .. } => "Error",
            Self::TypeError { .. } => "TypeError",
            Self::RangeError(_) => "RangeError",
        }
    }
}

impl std::fmt::Display for TimeContextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error { message, .. }
            | Self::TypeError { message, .. }
            | Self::RangeError(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for TimeContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Error {
                cause: Some(cause), ..
            }
            | Self::TypeError {
                cause: Some(cause), ..
            } => Some(cause.as_ref()),
            Self::Error { cause: None, .. }
            | Self::TypeError { cause: None, .. }
            | Self::RangeError(_) => None,
        }
    }
}

/// Request-preparation clock formatting and durable refresh scheduling.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Fallback display zone; omission resolves the process zone at plugin load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// Minimum milliseconds between durable injections in one Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_interval_ms: Option<f64>,
}

/// State of the `timeContext` durable projection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeContextState {
    pub last_message_time: Option<Number>,
    pub last_injection_time: Option<Number>,
    pub last_turn_injection_time: Option<Number>,
    pub browser_turn: Option<Number>,
    pub browser_time_zones: Vec<String>,
}

/// Services declared by the source plugin.
#[derive(Inject)]
pub struct Deps {
    agents: Service<Agents>,
    session_projections: Service<SessionProjections>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn type_error(message: impl Into<String>) -> BoxError {
    Box::new(TimeContextError::TypeError {
        message: message.into(),
        cause: None,
    })
}

fn plain_error(message: impl Into<String>, cause: Option<BoxError>) -> BoxError {
    Box::new(TimeContextError::Error {
        message: message.into(),
        cause,
    })
}

fn validate_refresh_interval(value: f64) -> Result<u64, BoxError> {
    if !value.is_finite()
        || value.fract() != 0.0
        || !(0.0..=MAX_SAFE_INTEGER as f64).contains(&value)
    {
        return Err(type_error(format!(
            "time-context: refreshIntervalMs must be a non-negative safe integer, got {}",
            js_number_string(value)
        )));
    }
    Ok(value as u64)
}

fn is_throttled(refresh_interval_ms: u64, now: i64, last_injection: Option<f64>) -> bool {
    refresh_interval_ms > 0
        && last_injection.is_some_and(|last| {
            now as f64 >= last && now as f64 - last < refresh_interval_ms as f64
        })
}

fn number_as_f64(value: Option<&Number>) -> Option<f64> {
    value.and_then(Number::as_f64)
}

fn event_number(value: &Value) -> Option<Number> {
    value.as_number().cloned()
}

fn fold_state(
    state: &Arc<TimeContextState>,
    event: &SessionEvent,
) -> Result<Arc<TimeContextState>, BoxError> {
    match event.event_type() {
        "turn/start" => {
            let mut next = (**state).clone();
            next.last_turn_injection_time = None;
            next.browser_turn = event_number(&event.data()["turn"]);
            next.browser_time_zones.clear();
            Ok(Arc::new(next))
        }
        "turn/end" => {
            if state.last_turn_injection_time.is_none() {
                return Ok(state.clone());
            }
            let mut next = (**state).clone();
            next.last_turn_injection_time = None;
            Ok(Arc::new(next))
        }
        "user/message" => {
            let message: UserMessage = serde_json::from_value(event.data().clone())?;
            let injected = message.source.kind == NAME;
            let time = Number::from(event.time());
            let mut next = (**state).clone();
            let mut changed = false;
            if next.last_message_time.as_ref() != Some(&time) {
                next.last_message_time = Some(time.clone());
                changed = true;
            }
            if state.browser_turn.is_some()
                && let Some(time_zone) = reported_browser_time_zone(&message)
                && !state
                    .browser_time_zones
                    .iter()
                    .any(|zone| zone == time_zone)
            {
                next.browser_time_zones.push(time_zone.to_owned());
                changed = true;
            }
            if injected {
                next.last_injection_time = Some(time.clone());
                next.last_turn_injection_time = Some(time);
                changed = true;
            }
            Ok(if changed {
                Arc::new(next)
            } else {
                state.clone()
            })
        }
        "assistant/message" | "tool/result" => {
            let time = Number::from(event.time());
            if state.last_message_time.as_ref() == Some(&time) {
                return Ok(state.clone());
            }
            let mut next = (**state).clone();
            next.last_message_time = Some(time);
            Ok(Arc::new(next))
        }
        _ => Ok(state.clone()),
    }
}

fn projection_definition() -> ProjectionDefinition<TimeContextState> {
    ProjectionDefinition {
        key: "timeContext".into(),
        state_version: 3,
        state_schema: Arc::new(|value| {
            let value = value
                .cloned()
                .ok_or_else(|| BoxError::from("time-context projection state is required"))?;
            let object = value
                .as_object()
                .ok_or_else(|| BoxError::from("time-context projection state must be an object"))?;
            for key in [
                "lastMessageTime",
                "lastInjectionTime",
                "lastTurnInjectionTime",
                "browserTurn",
                "browserTimeZones",
            ] {
                if !object.contains_key(key) {
                    return Err(format!(
                        "time-context projection state is missing required field {key}"
                    )
                    .into());
                }
            }
            Ok(Arc::new(serde_json::from_value(value)?))
        }),
        init: Arc::new(|_, _| Ok(Arc::new(TimeContextState::default()))),
        apply: Arc::new(fold_state),
        wire: None,
    }
}

fn format_duration(elapsed_ms: f64) -> Result<String, BoxError> {
    let parts = Duration::from_milliseconds(elapsed_ms.max(0.0))?.decompose()?;
    let mut output = Vec::new();
    if parts.days > 0.0 {
        output.push(format!("{}d", parts.days as u64));
    }
    if parts.hours > 0.0 {
        output.push(format!("{}h", parts.hours as u64));
    }
    if parts.minutes > 0.0 {
        output.push(format!("{}m", parts.minutes as u64));
    }
    output.push(format!("{}s", parts.seconds as u64));
    Ok(output.join(" "))
}

fn request_messages(
    agent: &dyn Agent,
    turn: u64,
    proposed: &[UserMessage],
) -> Result<Vec<UserMessage>, BoxError> {
    let mut entered = Vec::new();
    for raw in (0..agent.session().seq().get()).rev() {
        let Some(event) = agent.session().event_at(SessionSeq::new(raw)?) else {
            continue;
        };
        if event.event_type() == "turn/start" && event.data()["turn"].as_u64() == Some(turn) {
            entered.reverse();
            entered.extend_from_slice(proposed);
            return Ok(entered);
        }
        if event.event_type() == "user/message" {
            entered.push(serde_json::from_value(event.data().clone())?);
        }
    }
    Ok(proposed.to_vec())
}

fn render_text(
    now: i64,
    turn: u64,
    step: u64,
    previous: Option<f64>,
    formatter: &TimestampFormatter,
    time_zone: &str,
    browser_context: &BrowserTimeZoneContext,
) -> Result<String, BoxError> {
    let elapsed = match previous {
        None => "unavailable".into(),
        Some(previous) => format_duration(now as f64 - previous)?,
    };
    let baseline = if step == 1 {
        "model-visible message"
    } else {
        "step context"
    };
    Ok(format!(
        "Time sampled while preparing turn {turn}, step {step}: {}\n{}\nElapsed since the preceding {baseline}: {elapsed}.",
        format_timestamp(now, formatter, time_zone)?,
        render_browser_time_zone_context(browser_context)
    ))
}

fn time_context_message(text: String) -> UserMessage {
    let mut source = MessageSource::new(NAME);
    let formed = ContextFormed::Snapshot {
        sections: vec![ContextSnapshotSection {
            name: NAME.into(),
            text: text.clone(),
        }],
    };
    let formed = serde_json::to_value(formed).expect("context form is JSON");
    if let Value::Object(data) = formed {
        source.data.extend(data);
    }
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source,
    })
}

fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(i64::MAX as u128) as i64,
        Err(error) => -(error.duration().as_millis().min(i64::MAX as u128) as i64),
    }
}

#[derive(Clone)]
struct FormatterCache {
    fallback_time_zone: String,
    formatters: Arc<Mutex<HashMap<String, TimestampFormatter>>>,
}

impl FormatterCache {
    fn new(configured: Option<&str>) -> Result<Self, BoxError> {
        let fallback = create_timestamp_formatter(configured).map_err(|cause| {
            let message = match configured {
                None => "time-context: failed to resolve the system time zone".to_owned(),
                Some(zone) => format!(
                    "time-context: invalid IANA timeZone {}",
                    js_json_stringify(&Value::String(zone.to_owned()))
                ),
            };
            plain_error(message, Some(cause))
        })?;
        let fallback_time_zone = fallback.time_zone().to_owned();
        let mut formatters = HashMap::new();
        formatters.insert(fallback_time_zone.clone(), fallback);
        Ok(Self {
            fallback_time_zone,
            formatters: Arc::new(Mutex::new(formatters)),
        })
    }

    fn formatter_for(&self, selected: &str) -> Result<TimestampFormatter, BoxError> {
        if let Some(formatter) = lock(&self.formatters).get(selected).cloned() {
            return Ok(formatter);
        }
        let formatter = create_timestamp_formatter(Some(selected))?;
        lock(&self.formatters).insert(selected.to_owned(), formatter.clone());
        Ok(formatter)
    }
}

type ProjectionOutcome = Result<ProjectionHandle<TimeContextState>, SharedError>;

async fn await_projection(
    mut ready: watch::Receiver<Option<ProjectionOutcome>>,
) -> ProjectionOutcome {
    loop {
        if let Some(outcome) = ready.borrow().clone() {
            return outcome;
        }
        if ready.changed().await.is_err() {
            return Err(SharedError::new(BoxError::from(
                "time-context projection registration ended before settling",
            )));
        }
    }
}

/// Opt-in request clock context plugin.
pub struct TimeContext;

impl Plugin for TimeContext {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        let refresh_interval_ms = validate_refresh_interval(
            config
                .refresh_interval_ms
                .unwrap_or(DEFAULT_REFRESH_INTERVAL_MS),
        )?;
        let cache = FormatterCache::new(config.time_zone.as_deref())?;
        let registration = deps
            .session_projections
            .register(&ctx, projection_definition());
        let (ready_tx, ready_rx) = watch::channel::<Option<ProjectionOutcome>>(None);
        let owner = ctx.clone();
        let listener_cache = cache.clone();
        let listener_ready = ready_rx.clone();
        ctx.listen()
            .prepend()
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
                let owner = owner.clone();
                let cache = listener_cache.clone();
                let ready = listener_ready.clone();
                async move {
                    let projection = await_projection(ready)
                        .await
                        .map_err(|error| Box::new(error) as BoxError)?;
                    owner.ensure_active()?;
                    let decision = next.run().await?;
                    let PreStepDecision::Enter {
                        mut messages,
                        starts_request_series,
                    } = decision
                    else {
                        return Ok(decision);
                    };
                    if event.signal.aborted() {
                        return Ok(PreStepDecision::Enter {
                            messages,
                            starts_request_series,
                        });
                    }
                    let now = now_ms();
                    let state = projection.state_of(event.agent.session())?;
                    if is_throttled(
                        refresh_interval_ms,
                        now,
                        number_as_f64(state.last_injection_time.as_ref()),
                    ) {
                        return Ok(PreStepDecision::Enter {
                            messages,
                            starts_request_series,
                        });
                    }
                    let previous = if event.step == 1 {
                        number_as_f64(state.last_message_time.as_ref())
                    } else {
                        number_as_f64(state.last_turn_injection_time.as_ref())
                    };
                    let same_turn =
                        number_as_f64(state.browser_turn.as_ref()) == Some(event.turn as f64);
                    let browser = if same_turn {
                        derive_browser_time_zone_context(&messages, &state.browser_time_zones)?
                    } else {
                        derive_browser_time_zone_context(
                            &request_messages(event.agent.as_ref(), event.turn, &messages)?,
                            &[],
                        )?
                    };
                    let selected = match &browser {
                        BrowserTimeZoneContext::Resolved { time_zone } => time_zone.as_str(),
                        BrowserTimeZoneContext::Mixed { .. } | BrowserTimeZoneContext::Missing => {
                            cache.fallback_time_zone.as_str()
                        }
                    };
                    let formatter = cache.formatter_for(selected)?;
                    let text = render_text(
                        now, event.turn, event.step, previous, &formatter, selected, &browser,
                    )?;
                    messages.push(time_context_message(text));
                    Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    })
                }
            })?;
        // Reading the service is intentional: the source inject contract requires `agents` even
        // though scoped AgentPreStep dispatch already carries the subject.
        let _ = &deps.agents;
        let outcome = registration.await.map_err(SharedError::new);
        let _ = ready_tx.send(Some(outcome.clone()));
        outcome
            .map(|_| ())
            .map_err(|error| Box::new(error) as BoxError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_wording_and_refresh_validation_match_javascript_numbers() {
        assert_eq!(format_duration(90_061_000.0).unwrap(), "1d 1h 1m 1s");
        assert_eq!(format_duration(-5_000.0).unwrap(), "0s");
        for (value, text) in [
            (-1.0, "-1"),
            (0.5, "0.5"),
            (9_007_199_254_740_992.0, "9007199254740992"),
            (f64::INFINITY, "Infinity"),
            (f64::NAN, "NaN"),
        ] {
            let error = validate_refresh_interval(value).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "time-context: refreshIntervalMs must be a non-negative safe integer, got {text}"
                )
            );
        }
        assert_eq!(validate_refresh_interval(-0.0).unwrap(), 0);
        assert_eq!(
            validate_refresh_interval(MAX_SAFE_INTEGER as f64).unwrap(),
            MAX_SAFE_INTEGER
        );
    }

    #[test]
    fn backward_clock_and_exact_refresh_threshold_match_source_boundaries() {
        assert!(!is_throttled(60_000, 95_000, Some(100_000.0)));
        assert!(is_throttled(600_000, 699_999, Some(100_000.0)));
        assert!(!is_throttled(600_000, 700_000, Some(100_000.0)));
        assert!(is_throttled(1_000, 100_999, Some(100_000.0)));
        assert!(!is_throttled(1_000, 101_000, Some(100_000.0)));
        assert!(!is_throttled(0, 100_000, Some(100_000.0)));
        assert!(!is_throttled(1_000, 100_000, None));
    }

    #[test]
    fn fixed_rendering_matches_the_model_visible_source_text() {
        let formatter = create_timestamp_formatter(Some("Asia/Shanghai")).unwrap();
        let text = render_text(
            1_784_077_261_000,
            1,
            1,
            Some(1_783_987_200_000.0),
            &formatter,
            "Asia/Shanghai",
            &BrowserTimeZoneContext::Resolved {
                time_zone: "Asia/Shanghai".into(),
            },
        )
        .unwrap();
        assert_eq!(
            text,
            "Time sampled while preparing turn 1, step 1: 2026-07-15T09:01:01+08:00[Asia/Shanghai]\n\
Browser time zone for this request: Asia/Shanghai. Interpret otherwise-unqualified dates and times in this zone.\n\
Elapsed since the preceding model-visible message: 1d 1h 1m 1s."
        );
    }

    #[test]
    fn projection_schema_requires_every_nullable_key_like_zod_object() {
        let schema = projection_definition().state_schema;
        let complete = serde_json::json!({
            "lastMessageTime": null,
            "lastInjectionTime": null,
            "lastTurnInjectionTime": null,
            "browserTurn": null,
            "browserTimeZones": []
        });
        assert!(schema(Some(&complete)).is_ok());
        for key in [
            "lastMessageTime",
            "lastInjectionTime",
            "lastTurnInjectionTime",
            "browserTurn",
            "browserTimeZones",
        ] {
            let mut malformed = complete.clone();
            malformed.as_object_mut().unwrap().remove(key);
            assert!(schema(Some(&malformed)).is_err(), "missing {key}");
        }
    }
}
