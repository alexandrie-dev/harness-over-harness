use crate::{
    ContextBreakdownProjection, error, estimate_message, estimate_tools_tokens,
    normalize_nonnegative_safe_integers,
};
use hoh_error::BoxError;
use hoh_llm::Message;
use hoh_session::{
    SessionEvent, SessionSeq, SurfaceOp, canonical_header, derive_event_message, is_surface_event,
};
use hoh_session_projection::{ProjectionDefinition, ProjectionWire};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SystemNode {
    seq: SessionSeq,
    tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BreakdownClaim {
    start: SessionSeq,
    end: SessionSeq,
    tokens: u64,
    system_seqs: Vec<SessionSeq>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[doc(hidden)]
pub struct ContextBreakdownState {
    systems: Vec<SystemNode>,
    surface_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim: Option<BreakdownClaim>,
    breakdown: Arc<Value>,
}

fn decoded_breakdown(value: &Value) -> Result<ContextBreakdownProjection, BoxError> {
    Ok(serde_json::from_value(value.clone())?)
}

fn breakdown_value(value: &ContextBreakdownProjection) -> Arc<Value> {
    Arc::new(serde_json::to_value(value).expect("context breakdown is JSON"))
}

fn tokens_of(event: &SessionEvent) -> Result<u64, BoxError> {
    Ok(match derive_event_message(event, None) {
        Some(message) => estimate_message(&serde_json::from_value::<Message>((*message).clone())?),
        None => 0,
    })
}

fn op_of(event: &SessionEvent) -> Result<SurfaceOp, BoxError> {
    Ok(serde_json::from_value(
        event
            .surface_op()
            .cloned()
            .ok_or_else(|| error("context breakdown: event is missing surfaceOp"))?,
    )?)
}

fn with_totals(
    state: &ContextBreakdownState,
    systems: Vec<SystemNode>,
    surface_tokens: u64,
) -> Result<ContextBreakdownState, BoxError> {
    let previous = decoded_breakdown(&state.breakdown)?;
    let system_tokens = systems
        .iter()
        .rev()
        .find(|system| system.tokens > 0)
        .map_or(0, |system| system.tokens);
    let message_tokens = surface_tokens.saturating_sub(system_tokens);
    let breakdown =
        if system_tokens == previous.system_tokens && message_tokens == previous.message_tokens {
            state.breakdown.clone()
        } else {
            breakdown_value(&ContextBreakdownProjection {
                system_tokens,
                tools_tokens: previous.tools_tokens,
                message_tokens,
            })
        };
    Ok(ContextBreakdownState {
        systems,
        surface_tokens,
        claim: None,
        breakdown,
    })
}

fn place_surface_event(
    state: &ContextBreakdownState,
    claim: Option<&BreakdownClaim>,
    event: &SessionEvent,
    tokens: u64,
) -> Result<(Vec<SystemNode>, u64), BoxError> {
    let is_system = event.event_type() == "system/message";
    let node = SystemNode {
        seq: event.seq(),
        tokens,
    };
    match op_of(event)? {
        SurfaceOp::Append(_) => {
            let mut systems = state.systems.clone();
            if is_system {
                systems.push(node);
            }
            Ok((systems, state.surface_tokens.saturating_add(tokens)))
        }
        SurfaceOp::Replace(replace) => {
            if let Some(claim) = claim {
                if claim.start != replace.start_seq || claim.end != replace.end_seq {
                    return Err(error(format!(
                        "context breakdown: replace at seq {} over range {}-{} has no adjacent shadow price (armed claim covers {}-{})",
                        event.seq().get(),
                        replace.start_seq.get(),
                        replace.end_seq.get(),
                        claim.start.get(),
                        claim.end.get()
                    )));
                }
                if is_system {
                    return Err(error(format!(
                        "context breakdown: replace at seq {} puts a system message in place of a metered range; its order among the surviving system prompts is unknown",
                        event.seq().get()
                    )));
                }
                let shadowed: HashSet<SessionSeq> = claim.system_seqs.iter().copied().collect();
                let systems = state
                    .systems
                    .iter()
                    .filter(|system| !shadowed.contains(&system.seq))
                    .cloned()
                    .collect();
                let total = i128::from(state.surface_tokens) + i128::from(tokens)
                    - i128::from(claim.tokens);
                return Ok((systems, u64::try_from(total.max(0)).unwrap_or(u64::MAX)));
            }
            let index = (replace.start_seq == replace.end_seq)
                .then(|| {
                    state
                        .systems
                        .iter()
                        .position(|system| system.seq == replace.start_seq)
                })
                .flatten();
            let Some(index) = index else {
                return Ok((state.systems.clone(), state.surface_tokens));
            };
            let replaced = state.systems[index].tokens;
            let mut systems = state.systems.clone();
            if is_system {
                systems[index] = node;
            } else {
                systems.remove(index);
            }
            let total =
                i128::from(state.surface_tokens) + i128::from(tokens) - i128::from(replaced);
            Ok((systems, u64::try_from(total.max(0)).unwrap_or(u64::MAX)))
        }
    }
}

fn parse_state(value: Option<&Value>) -> Result<ContextBreakdownState, BoxError> {
    let mut value = value
        .cloned()
        .ok_or_else(|| error("contextBreakdown state is missing"))?;
    normalize_nonnegative_safe_integers(&mut value);
    let state: ContextBreakdownState = serde_json::from_value(value)?;
    let _: ContextBreakdownProjection = decoded_breakdown(&state.breakdown)?;
    Ok(state)
}

fn shadow_claim(
    state: &ContextBreakdownState,
    event: &SessionEvent,
) -> Result<BreakdownClaim, BoxError> {
    let claim = super::surface_projection::claim_of(event)?
        .ok_or_else(|| error("context breakdown: missing shadow price"))?;
    let shadowed: HashSet<SessionSeq> = event
        .data()
        .get("shadowedSeqs")
        .and_then(Value::as_array)
        .ok_or_else(|| error("context breakdown: shadowedSeqs is missing"))?
        .iter()
        .map(|value| {
            super::surface_projection::safe_count(value)
                .ok_or_else(|| error("context breakdown: shadowedSeqs contains an invalid seq"))
                .and_then(SessionSeq::new)
        })
        .collect::<Result<_, BoxError>>()?;
    Ok(BreakdownClaim {
        start: claim.start,
        end: claim.end,
        tokens: claim.tokens,
        system_seqs: state
            .systems
            .iter()
            .filter(|system| shadowed.contains(&system.seq))
            .map(|system| system.seq)
            .collect(),
    })
}

pub fn context_breakdown_projection_definition() -> ProjectionDefinition<ContextBreakdownState> {
    ProjectionDefinition {
        key: "contextBreakdown".into(),
        state_version: 6,
        state_schema: Arc::new(|value| Ok(Arc::new(parse_state(value)?))),
        init: Arc::new(|_, _| {
            Ok(Arc::new(ContextBreakdownState {
                systems: vec![],
                surface_tokens: 0,
                claim: None,
                breakdown: breakdown_value(&ContextBreakdownProjection::default()),
            }))
        }),
        apply: Arc::new(|state, event| {
            if matches!(
                event.event_type(),
                "compaction/summary" | "compaction/prune"
            ) {
                let mut next = state.as_ref().clone();
                next.claim = Some(shadow_claim(state, event)?);
                return Ok(Arc::new(next));
            }
            let claim = state.claim.as_ref();
            let mut unclaimed = state.as_ref().clone();
            unclaimed.claim = None;
            if event.event_type() == "request/header" {
                let header = event
                    .data()
                    .get("header")
                    .ok_or_else(|| error("context breakdown: request/header is missing header"))?;
                let header = canonical_header(header);
                let tools_tokens = estimate_tools_tokens(Some(&header));
                let previous = decoded_breakdown(&unclaimed.breakdown)?;
                if tools_tokens == previous.tools_tokens {
                    return if claim.is_none() {
                        Ok(state.clone())
                    } else {
                        Ok(Arc::new(unclaimed))
                    };
                }
                unclaimed.breakdown = breakdown_value(&ContextBreakdownProjection {
                    tools_tokens,
                    ..previous
                });
                return Ok(Arc::new(unclaimed));
            }
            if !is_surface_event(event) {
                return if claim.is_none() {
                    Ok(state.clone())
                } else {
                    Ok(Arc::new(unclaimed))
                };
            }
            let tokens = tokens_of(event)?;
            let (systems, surface_tokens) = place_surface_event(&unclaimed, claim, event, tokens)?;
            Ok(Arc::new(with_totals(&unclaimed, systems, surface_tokens)?))
        }),
        wire: Some(ProjectionWire {
            view_schema: Arc::new(|value| {
                let value = value
                    .cloned()
                    .ok_or_else(|| error("contextBreakdown view is missing"))?;
                let parsed: ContextBreakdownProjection = serde_json::from_value(value)?;
                Ok(Arc::new(serde_json::to_value(parsed)?))
            }),
            view: Arc::new(|state| Ok(Some(state.breakdown.clone()))),
            summary: false,
        }),
    }
}
