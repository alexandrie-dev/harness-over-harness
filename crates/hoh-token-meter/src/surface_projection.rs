use crate::{error, estimate_message};
use hoh_error::BoxError;
use hoh_llm::Message;
use hoh_session::{
    MAX_SAFE_INTEGER, SessionEvent, SessionSeq, SurfaceOp, derive_event_message, is_surface_event,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowPriceClaim {
    pub start: SessionSeq,
    pub end: SessionSeq,
    pub tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceTokensFold {
    pub delta_tokens: i64,
    pub claim: Option<ShadowPriceClaim>,
}

pub(crate) fn safe_count(value: &Value) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return (value <= MAX_SAFE_INTEGER).then_some(value);
    }
    let value = value.as_f64()?;
    (value.is_finite()
        && value >= 0.0
        && !value.is_sign_negative()
        && value.fract() == 0.0
        && value <= MAX_SAFE_INTEGER as f64)
        .then_some(value as u64)
}

pub(crate) fn claim_of(event: &SessionEvent) -> Result<Option<ShadowPriceClaim>, BoxError> {
    if !matches!(
        event.event_type(),
        "compaction/summary" | "compaction/prune"
    ) {
        return Ok(None);
    }
    let range = event
        .data()
        .get("shadowedRange")
        .and_then(Value::as_object)
        .ok_or_else(|| error("token surface: shadow price has an invalid range"))?;
    let start = range
        .get("start")
        .and_then(safe_count)
        .ok_or_else(|| error("token surface: shadow price has an invalid start"))?;
    let end = range
        .get("end")
        .and_then(safe_count)
        .ok_or_else(|| error("token surface: shadow price has an invalid end"))?;
    let tokens = event
        .data()
        .get("shadowedTokenCount")
        .and_then(safe_count)
        .ok_or_else(|| error("token surface: shadow price has an invalid token count"))?;
    Ok(Some(ShadowPriceClaim {
        start: SessionSeq::new(start)?,
        end: SessionSeq::new(end)?,
        tokens,
    }))
}

/// Fold one committed event onto a bounded running surface-token total.
pub fn fold_surface_projection(
    claim: Option<&ShadowPriceClaim>,
    event: &SessionEvent,
) -> Result<SurfaceTokensFold, BoxError> {
    if let Some(claim) = claim_of(event)? {
        return Ok(SurfaceTokensFold {
            delta_tokens: 0,
            claim: Some(claim),
        });
    }
    if !is_surface_event(event) {
        return Ok(SurfaceTokensFold {
            delta_tokens: 0,
            claim: None,
        });
    }
    let tokens = match derive_event_message(event, None) {
        Some(value) => estimate_message(&serde_json::from_value::<Message>((*value).clone())?),
        None => 0,
    };
    let op: SurfaceOp = serde_json::from_value(
        event
            .surface_op()
            .cloned()
            .ok_or_else(|| error("token surface: event is missing surfaceOp"))?,
    )?;
    match op {
        SurfaceOp::Append(_) => Ok(SurfaceTokensFold {
            delta_tokens: i64::try_from(tokens)
                .map_err(|_| error("token surface: token count is out of range"))?,
            claim: None,
        }),
        SurfaceOp::Replace(replace) => {
            let Some(claim) = claim else {
                return Ok(SurfaceTokensFold {
                    delta_tokens: 0,
                    claim: None,
                });
            };
            if claim.start != replace.start_seq || claim.end != replace.end_seq {
                return Err(error(format!(
                    "token surface: replace at seq {} over range {}-{} has no adjacent shadow price (armed claim covers {}-{})",
                    event.seq().get(),
                    replace.start_seq.get(),
                    replace.end_seq.get(),
                    claim.start.get(),
                    claim.end.get()
                )));
            }
            let delta = i64::try_from(i128::from(tokens) - i128::from(claim.tokens))
                .map_err(|_| error("token surface: signed token delta is out of range"))?;
            Ok(SurfaceTokensFold {
                delta_tokens: delta,
                claim: None,
            })
        }
    }
}
