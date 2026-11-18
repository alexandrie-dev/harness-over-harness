use crate::repair::{OpenTurnCloseCause, closer_event_value, open_turn_closers};
use crate::{Result, SessionError, SessionEvent, SessionSeq};
use serde_json::json;
use std::sync::Arc;
pub fn build_fork_seed(
    events: &[Arc<SessionEvent>],
    boundary: SessionSeq,
) -> Result<Vec<Arc<SessionEvent>>> {
    let index = u64::from(boundary) as usize;
    let e = events
        .get(index)
        .ok_or_else(|| SessionError::message("fork boundary does not exist"))?;
    let mut prefix = events[..=index].to_vec();
    let seq = SessionSeq::new(u64::from(boundary) + 1)?;
    prefix.push(Arc::new(SessionEvent::from_value(closer_event_value(
        "session/end-seed",
        seq,
        e.as_value().get("time"),
        json!({"inherited":true}),
    ))));
    prefix.extend(open_turn_closers(&prefix, OpenTurnCloseCause::Forked)?);
    Ok(prefix)
}
