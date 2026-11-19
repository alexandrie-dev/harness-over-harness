use crate::{
    ContextPressureProjection, ShadowPriceClaim, TokenUsageProjection, error,
    fold_surface_projection, normalize_nonnegative_safe_integers,
};
use hoh_error::BoxError;
use hoh_llm::{AssistantStreamRecord, RawStreamChunkType, StreamChunk, TokenUsage};
use hoh_session::SessionEvent;
use hoh_session_projection::{ProjectionDefinition, ProjectionWire};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LastUsage {
    turn: u64,
    step: u64,
    buckets: TokenUsageProjection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[doc(hidden)]
pub struct TokenUsageState {
    totals: TokenUsageProjection,
    last: Option<LastUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextPressureState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_tokens: Option<u64>,
    pub surface_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampled_surface_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<ShadowPriceClaim>,
}

fn parse_required<T: DeserializeOwned>(value: Option<&Value>, label: &str) -> Result<T, BoxError> {
    let mut value = value
        .cloned()
        .ok_or_else(|| error(format!("{label} state is missing")))?;
    normalize_nonnegative_safe_integers(&mut value);
    Ok(serde_json::from_value(value)?)
}

fn view_schema<T: DeserializeOwned + Serialize>(
    value: Option<&Value>,
    label: &str,
) -> Result<Arc<Value>, BoxError> {
    let parsed: T = parse_required(value, label)?;
    Ok(Arc::new(serde_json::to_value(parsed)?))
}

fn usage_of(event: &SessionEvent) -> Result<Option<TokenUsage>, BoxError> {
    if event.event_type() == "assistant/message"
        && let Some(value) = event.data().get("usage")
    {
        return Ok(Some(serde_json::from_value(value.clone())?));
    }
    if !matches!(
        event.event_type(),
        "assistant/message" | "assistant/attempt"
    ) {
        return Ok(None);
    }
    let Some(stream) = event.data().get("stream") else {
        return Ok(None);
    };
    let stream: Vec<AssistantStreamRecord> = serde_json::from_value(stream.clone())?;
    Ok(
        match hoh_llm::last_assistant_stream_chunk(&stream, RawStreamChunkType::Usage) {
            Some(StreamChunk::Usage { usage, .. }) => Some(usage),
            _ => None,
        },
    )
}

fn buckets_from(usage: &TokenUsage) -> TokenUsageProjection {
    TokenUsageProjection {
        uncached_input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens.unwrap_or(0),
        cache_write_tokens: usage.cache_write_tokens.unwrap_or(0),
    }
}

fn coordinates(event: &SessionEvent) -> Result<(u64, u64), BoxError> {
    let turn = event
        .data()
        .get("turn")
        .and_then(super::surface_projection::safe_count)
        .ok_or_else(|| error("token usage: event has an invalid turn"))?;
    let step = event
        .data()
        .get("step")
        .and_then(super::surface_projection::safe_count)
        .ok_or_else(|| error("token usage: event has an invalid step"))?;
    Ok((turn, step))
}

fn add_replacing(
    totals: &TokenUsageProjection,
    previous: Option<&TokenUsageProjection>,
    next: &TokenUsageProjection,
) -> Result<TokenUsageProjection, BoxError> {
    let prior = previous.cloned().unwrap_or_default();
    let replacing = |total: u64, old: u64, new: u64| {
        u64::try_from(i128::from(total) - i128::from(old) + i128::from(new)).map_err(|_| {
            error("token usage: replacement produced a negative or overflowing bucket")
        })
    };
    Ok(TokenUsageProjection {
        uncached_input_tokens: replacing(
            totals.uncached_input_tokens,
            prior.uncached_input_tokens,
            next.uncached_input_tokens,
        )?,
        output_tokens: replacing(
            totals.output_tokens,
            prior.output_tokens,
            next.output_tokens,
        )?,
        cache_read_tokens: replacing(
            totals.cache_read_tokens,
            prior.cache_read_tokens,
            next.cache_read_tokens,
        )?,
        cache_write_tokens: replacing(
            totals.cache_write_tokens,
            prior.cache_write_tokens,
            next.cache_write_tokens,
        )?,
    })
}

pub fn token_usage_projection_definition() -> ProjectionDefinition<TokenUsageState> {
    ProjectionDefinition {
        key: "tokenUsage".into(),
        state_version: 2,
        state_schema: Arc::new(|value| Ok(Arc::new(parse_required(value, "tokenUsage")?))),
        init: Arc::new(|_, _| {
            Ok(Arc::new(TokenUsageState {
                totals: TokenUsageProjection::default(),
                last: None,
            }))
        }),
        apply: Arc::new(|state, event| {
            if event.event_type() == "llm/retry-started" {
                let (turn, step) = coordinates(event)?;
                if state
                    .last
                    .as_ref()
                    .is_some_and(|last| last.turn == turn && last.step == step)
                {
                    return Ok(Arc::new(TokenUsageState {
                        totals: state.totals.clone(),
                        last: None,
                    }));
                }
                return Ok(state.clone());
            }
            if !matches!(
                event.event_type(),
                "assistant/message" | "assistant/attempt"
            ) {
                return Ok(state.clone());
            }
            let Some(usage) = usage_of(event)? else {
                return Ok(state.clone());
            };
            let (turn, step) = coordinates(event)?;
            let buckets = buckets_from(&usage);
            let previous = state
                .last
                .as_ref()
                .filter(|last| last.turn == turn && last.step == step)
                .map(|last| &last.buckets);
            if previous == Some(&buckets) {
                return Ok(state.clone());
            }
            Ok(Arc::new(TokenUsageState {
                totals: add_replacing(&state.totals, previous, &buckets)?,
                last: Some(LastUsage {
                    turn,
                    step,
                    buckets,
                }),
            }))
        }),
        wire: Some(ProjectionWire {
            view_schema: Arc::new(|value| {
                view_schema::<TokenUsageProjection>(value, "tokenUsage view")
            }),
            view: Arc::new(|state| Ok(Some(Arc::new(serde_json::to_value(&state.totals)?)))),
            summary: true,
        }),
    }
}

fn pressure_from(usage: &TokenUsage) -> u64 {
    usage
        .input_tokens
        .saturating_add(usage.cache_read_tokens.unwrap_or(0))
        .saturating_add(usage.cache_write_tokens.unwrap_or(0))
}

fn projected(state: &ContextPressureState) -> ContextPressureProjection {
    let projected_tokens =
        state
            .pressure_tokens
            .zip(state.sampled_surface_tokens)
            .map(|(pressure, sampled)| {
                let value =
                    i128::from(pressure) + i128::from(state.surface_tokens) - i128::from(sampled);
                u64::try_from(value.max(0)).unwrap_or(u64::MAX)
            });
    ContextPressureProjection {
        pressure_tokens: state.pressure_tokens,
        projected_tokens,
        context_window: state.context_window,
    }
}

pub fn context_pressure_projection_definition() -> ProjectionDefinition<ContextPressureState> {
    ProjectionDefinition {
        key: "contextPressure".into(),
        state_version: 5,
        state_schema: Arc::new(|value| {
            let state: ContextPressureState = parse_required(value, "contextPressure")?;
            if state.context_window == Some(0)
                || state.surface_tokens < 0
                || state
                    .sampled_surface_tokens
                    .is_some_and(|tokens| tokens < 0)
            {
                return Err(error(
                    "contextPressure persisted capacity and surface totals are invalid",
                ));
            }
            Ok(Arc::new(state))
        }),
        init: Arc::new(|_, _| {
            Ok(Arc::new(ContextPressureState {
                context_window: None,
                pressure_tokens: None,
                surface_tokens: 0,
                sampled_surface_tokens: None,
                claim: None,
            }))
        }),
        apply: Arc::new(|state, event| {
            let fold = fold_surface_projection(state.claim.as_ref(), event)?;
            let mut next = state.as_ref().clone();
            if event.event_type() == "request/context" {
                next.context_window = event
                    .data()
                    .get("contextWindow")
                    .and_then(super::surface_projection::safe_count)
                    .filter(|value| *value > 0);
            }
            if let Some(usage) = usage_of(event)? {
                next.pressure_tokens = Some(pressure_from(&usage));
                next.sampled_surface_tokens = Some(next.surface_tokens);
            }
            if fold.delta_tokens != 0 {
                next.surface_tokens = next
                    .surface_tokens
                    .checked_add(fold.delta_tokens)
                    .ok_or_else(|| error("context pressure surface total is out of range"))?;
            }
            next.claim = fold.claim;
            if &next == state.as_ref() {
                Ok(state.clone())
            } else {
                Ok(Arc::new(next))
            }
        }),
        wire: Some(ProjectionWire {
            view_schema: Arc::new(|value| {
                view_schema::<ContextPressureProjection>(value, "contextPressure view")
            }),
            view: Arc::new(|state| Ok(Some(Arc::new(serde_json::to_value(projected(state))?)))),
            summary: false,
        }),
    }
}
