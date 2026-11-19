use hoh_session::{MAX_SAFE_INTEGER, SessionEvent};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnTokenUsageRoute {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTokenUsage {
    pub uncached_input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<TurnTokenUsageRoute>>,
}

#[derive(Clone)]
struct NormalizedAttempt {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    route: Option<TurnTokenUsageRoute>,
}

#[derive(Clone)]
enum AttemptState {
    Idle,
    Open {
        turn: u64,
        step: u64,
        sample: Option<Value>,
    },
    FinishClosed {
        turn: u64,
        step: u64,
    },
    Settled {
        turn: u64,
        step: u64,
        by: SettledBy,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettledBy {
    Message,
    Retry,
}

fn safe_count(value: &Value) -> Option<u64> {
    super::surface_projection::safe_count(value)
}

fn field(usage: &Value, key: &str) -> Option<u64> {
    safe_count(usage.get(key)?)
}

fn optional_field(usage: &Value, key: &str) -> Result<Option<u64>, ()> {
    match usage.get(key) {
        None => Ok(None),
        Some(value) => safe_count(value).map(Some).ok_or(()),
    }
}

fn safe_sum(values: impl IntoIterator<Item = u64>) -> Option<u64> {
    let mut total = 0_u64;
    for value in values {
        total = total.checked_add(value)?;
        if total > MAX_SAFE_INTEGER {
            return None;
        }
    }
    Some(total)
}

fn normalize_usage(usage: &Value, route: Option<TurnTokenUsageRoute>) -> Option<NormalizedAttempt> {
    let input_tokens = field(usage, "inputTokens")?;
    let output_tokens = field(usage, "outputTokens")?;
    let cache_read_tokens = optional_field(usage, "cacheReadTokens").ok()?;
    let cache_write_tokens = optional_field(usage, "cacheWriteTokens").ok()?;
    let reasoning_tokens = optional_field(usage, "reasoningTokens").ok()?;
    if reasoning_tokens.is_some_and(|tokens| tokens > output_tokens) {
        return None;
    }
    let known_prompt = safe_sum(
        [Some(input_tokens), cache_read_tokens, cache_write_tokens]
            .into_iter()
            .flatten(),
    )?;
    let total_tokens = match usage.get("totalTokens") {
        Some(value) => {
            let total = safe_count(value)?;
            let exact_prompt = total.checked_sub(output_tokens)?;
            if exact_prompt < known_prompt
                || (cache_read_tokens.is_some()
                    && cache_write_tokens.is_some()
                    && exact_prompt != known_prompt)
            {
                return None;
            }
            total
        }
        None => {
            cache_read_tokens?;
            cache_write_tokens?;
            safe_sum([known_prompt, output_tokens])?
        }
    };
    Some(NormalizedAttempt {
        input_tokens,
        output_tokens,
        total_tokens,
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens,
        route,
    })
}

fn aggregate(attempts: &[NormalizedAttempt]) -> Option<TurnTokenUsage> {
    if attempts.is_empty() {
        return None;
    }
    let uncached_input_tokens = safe_sum(attempts.iter().map(|a| a.input_tokens))?;
    let output_tokens = safe_sum(attempts.iter().map(|a| a.output_tokens))?;
    let total_tokens = safe_sum(attempts.iter().map(|a| a.total_tokens))?;
    let optional_sum = |get: fn(&NormalizedAttempt) -> Option<u64>| {
        attempts
            .iter()
            .map(get)
            .collect::<Option<Vec<_>>>()
            .and_then(safe_sum)
    };
    let cache_read_tokens = optional_sum(|a| a.cache_read_tokens);
    let cache_write_tokens = optional_sum(|a| a.cache_write_tokens);
    let reasoning_tokens = optional_sum(|a| a.reasoning_tokens);
    let routes = attempts
        .iter()
        .map(|attempt| attempt.route.clone())
        .collect::<Option<Vec<_>>>()
        .map(|routes| {
            let mut unique = IndexMap::new();
            for route in routes {
                unique
                    .entry((route.provider.clone(), route.model.clone()))
                    .or_insert(route);
            }
            unique.into_values().collect()
        });
    Some(TurnTokenUsage {
        uncached_input_tokens,
        output_tokens,
        total_tokens,
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens,
        routes,
    })
}

fn coordinates(event: &SessionEvent) -> Option<(u64, u64)> {
    Some((
        safe_count(event.data().get("turn")?)?,
        safe_count(event.data().get("step")?)?,
    ))
}

fn state_coordinates(state: &AttemptState) -> Option<(u64, u64)> {
    match state {
        AttemptState::Idle => None,
        AttemptState::Open { turn, step, .. }
        | AttemptState::FinishClosed { turn, step }
        | AttemptState::Settled { turn, step, .. } => Some((*turn, *step)),
    }
}

fn stream_usage(event: &SessionEvent) -> Option<Value> {
    event
        .data()
        .get("stream")?
        .as_array()?
        .iter()
        .rev()
        .find_map(|record| {
            let chunk = (record.get("type")? == "chunk")
                .then(|| record.get("chunk"))
                .flatten()?;
            (chunk.get("type")? == "usage")
                .then(|| chunk.get("usage").cloned())
                .flatten()
        })
}

fn message_route(event: &SessionEvent) -> Option<TurnTokenUsageRoute> {
    let source = event.data().get("message")?.get("source")?;
    let provider = source.get("provider")?.as_str()?;
    let model = source.get("model")?.as_str()?;
    (!provider.is_empty() && !model.is_empty()).then(|| TurnTokenUsageRoute {
        provider: provider.into(),
        model: model.into(),
    })
}

fn close_open(
    state: &AttemptState,
    route: Option<TurnTokenUsageRoute>,
    attempts: &mut Vec<NormalizedAttempt>,
) -> bool {
    let AttemptState::Open {
        sample: Some(sample),
        ..
    } = state
    else {
        return false;
    };
    let Some(normalized) = normalize_usage(sample, route) else {
        return false;
    };
    attempts.push(normalized);
    true
}

/// Fold a complete turn's attempt lifecycle into exact provider accounting.
pub fn derive_turn_token_usage(events: &[SessionEvent]) -> Option<TurnTokenUsage> {
    let mut state = AttemptState::Idle;
    let mut attempts = vec![];
    let mut turn = None;
    let mut saw_end = false;
    let mut invalid = false;
    for event in events {
        if invalid {
            break;
        }
        match event.event_type() {
            "turn/start" => {
                let candidate = event.data().get("turn").and_then(safe_count);
                if turn.is_some() || !matches!(state, AttemptState::Idle) || candidate.is_none() {
                    invalid = true;
                } else {
                    turn = candidate;
                }
            }
            _ if turn.is_none() => invalid = true,
            "turn/end" => {
                let event_turn = event.data().get("turn").and_then(safe_count);
                if event_turn != turn || !matches!(state, AttemptState::Idle) || saw_end {
                    invalid = true;
                } else {
                    saw_end = true;
                }
            }
            _ if saw_end => invalid = true,
            "step/start" => {
                let Some((event_turn, step)) = coordinates(event) else {
                    invalid = true;
                    continue;
                };
                if Some(event_turn) != turn || !matches!(state, AttemptState::Idle) {
                    invalid = true;
                } else {
                    state = AttemptState::Open {
                        turn: event_turn,
                        step,
                        sample: None,
                    };
                }
            }
            "llm/retry-started" => {
                let Some(coords) = coordinates(event) else {
                    invalid = true;
                    continue;
                };
                if Some(coords.0) != turn
                    || state_coordinates(&state) != Some(coords)
                    || !matches!(
                        state,
                        AttemptState::Settled {
                            by: SettledBy::Retry,
                            ..
                        }
                    )
                {
                    invalid = true;
                } else {
                    state = AttemptState::Open {
                        turn: coords.0,
                        step: coords.1,
                        sample: None,
                    };
                }
            }
            "assistant/attempt" => {
                let Some(coords) = coordinates(event) else {
                    invalid = true;
                    continue;
                };
                if Some(coords.0) != turn
                    || state_coordinates(&state) != Some(coords)
                    || !matches!(state, AttemptState::Open { .. })
                {
                    invalid = true;
                    continue;
                }
                let prior = match &state {
                    AttemptState::Open { sample, .. } => sample.clone(),
                    _ => None,
                };
                state = AttemptState::Open {
                    turn: coords.0,
                    step: coords.1,
                    sample: stream_usage(event).or(prior),
                };
                if close_open(&state, None, &mut attempts) {
                    state = AttemptState::FinishClosed {
                        turn: coords.0,
                        step: coords.1,
                    };
                } else {
                    invalid = true;
                }
            }
            "assistant/message" => {
                let Some(coords) = coordinates(event) else {
                    invalid = true;
                    continue;
                };
                if Some(coords.0) != turn
                    || state_coordinates(&state) != Some(coords)
                    || !matches!(state, AttemptState::Open { .. })
                {
                    invalid = true;
                    continue;
                }
                let prior = match &state {
                    AttemptState::Open { sample, .. } => sample.clone(),
                    _ => None,
                };
                state = AttemptState::Open {
                    turn: coords.0,
                    step: coords.1,
                    sample: event
                        .data()
                        .get("usage")
                        .cloned()
                        .or_else(|| stream_usage(event))
                        .or(prior),
                };
                if close_open(&state, message_route(event), &mut attempts) {
                    state = AttemptState::Settled {
                        turn: coords.0,
                        step: coords.1,
                        by: SettledBy::Message,
                    };
                } else {
                    invalid = true;
                }
            }
            "llm/retry" => {
                let Some(coords) = coordinates(event) else {
                    invalid = true;
                    continue;
                };
                if Some(coords.0) != turn || state_coordinates(&state) != Some(coords) {
                    invalid = true;
                    continue;
                }
                if matches!(state, AttemptState::Settled { .. })
                    || (matches!(state, AttemptState::Open { .. })
                        && !close_open(&state, None, &mut attempts))
                {
                    invalid = true;
                } else {
                    state = AttemptState::Settled {
                        turn: coords.0,
                        step: coords.1,
                        by: SettledBy::Retry,
                    };
                }
            }
            "step/end" => {
                let Some(coords) = coordinates(event) else {
                    invalid = true;
                    continue;
                };
                if Some(coords.0) != turn || state_coordinates(&state) != Some(coords) {
                    invalid = true;
                    continue;
                }
                if matches!(state, AttemptState::Open { .. })
                    && !close_open(&state, None, &mut attempts)
                {
                    invalid = true;
                } else {
                    state = AttemptState::Idle;
                }
            }
            _ => {}
        }
    }
    if invalid || !saw_end || !matches!(state, AttemptState::Idle) {
        None
    } else {
        aggregate(&attempts)
    }
}
