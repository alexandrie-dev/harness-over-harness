//! Driver-owned durable agent inbox projection and command facade.

use hoh_agent::{
    AgentEvents, AgentInboxClaimed, AgentInboxDiscarded, AgentInboxInserted, Inbox, InboxState,
    InboxTarget, fold_definition,
};
use hoh_error::BoxError;
use hoh_llm::{MessageId, UserMessage};
use hoh_session::{Session, SessionEvent};
use hoh_session_projection::ProjectionDefinition;
use serde_json::{Map, Value, json};
use std::{collections::HashSet, sync::Arc};

fn target_of(value: &Value) -> Result<InboxTarget, BoxError> {
    match value.get("target").and_then(Value::as_str) {
        Some("next-turn") => Ok(InboxTarget::NextTurn),
        Some("next-step") => Ok(InboxTarget::NextStep),
        _ => Err("invalid inbox splice target".into()),
    }
}

/// `toSpliced`: a copy of `list` with `removed` entries at `start` replaced by `inserted`.
fn spliced(
    list: &[UserMessage],
    start: usize,
    removed: usize,
    inserted: &[UserMessage],
) -> Vec<UserMessage> {
    let mut next = list.to_vec();
    next.splice(start..start + removed, inserted.iter().cloned());
    next
}

/// Reject a candidate state that holds one id twice (`message "<id>" is already pending`).
fn assert_unique(
    target: InboxTarget,
    candidate: &[UserMessage],
    state: &InboxState,
) -> Result<(), BoxError> {
    let others = match target {
        InboxTarget::NextTurn => &state.next_step,
        InboxTarget::NextStep => &state.next_turn,
    };
    let ordered: Vec<&UserMessage> = match target {
        InboxTarget::NextTurn => candidate.iter().chain(others.iter()).collect(),
        InboxTarget::NextStep => others.iter().chain(candidate.iter()).collect(),
    };
    let mut ids: HashSet<&str> = HashSet::new();
    for message in ordered {
        if !ids.insert(message.id.as_ref()) {
            return Err(format!("message \"{}\" is already pending", message.id).into());
        }
    }
    Ok(())
}

fn apply_splice(state: &InboxState, event: &SessionEvent) -> Result<InboxState, BoxError> {
    let data = event.data();
    let target = target_of(data)?;
    let inbox = state.list(target);
    let start = data.get("start").and_then(Value::as_u64);
    let removed = match data.get("removedCount") {
        None => Some(0),
        Some(count) => count.as_u64(),
    };
    let (Some(start), Some(removed)) = (start, removed) else {
        return Err("invalid inbox splice".into());
    };
    let (start, removed) = (start as usize, removed as usize);
    if start > inbox.len() || start + removed > inbox.len() {
        return Err("invalid inbox splice".into());
    }
    let inserted: Vec<UserMessage> =
        serde_json::from_value(data.get("inserted").cloned().unwrap_or(json!([])))?;
    let next = spliced(inbox, start, removed, &inserted);
    assert_unique(target, &next, state)?;
    Ok(match target {
        InboxTarget::NextTurn => InboxState {
            next_turn: next,
            next_step: state.next_step.clone(),
        },
        InboxTarget::NextStep => InboxState {
            next_turn: state.next_turn.clone(),
            next_step: next,
        },
    })
}

/// Standard fold that reconstructs pending input and rejects invalid durable splice history
/// (`inbox`, state version 1).
pub fn inbox_projection_definition() -> ProjectionDefinition<InboxState> {
    fold_definition("inbox", 1, InboxState::default, |state, event| {
        if event.event_type() != "agent/inbox/spliced" {
            return Ok(state.clone());
        }
        apply_splice(state, event).map_err(|cause| {
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "invalid persisted inbox splice at session seq {}: {cause}",
                event.seq()
            ))
        })
    })
}

/// Producer of the agent-scoped dispatcher (the agent may be gone when a late call arrives).
pub type DispatchFn = Arc<dyn Fn() -> Option<AgentEvents> + Send + Sync>;
/// Reader of the live Inbox projection state.
pub type ReadInbox = Arc<dyn Fn() -> Result<Arc<InboxState>, BoxError> + Send + Sync>;

/// Driver-owned durable Inbox implementation.
pub struct ReactLoopInbox {
    read_state: ReadInbox,
    session: Session,
    dispatch: DispatchFn,
}

impl ReactLoopInbox {
    pub fn new(read_state: ReadInbox, session: Session, dispatch: DispatchFn) -> Self {
        Self {
            read_state,
            session,
            dispatch,
        }
    }

    /// Whether either pending-message list contains work.
    pub fn has_pending(&self) -> Result<bool, BoxError> {
        let state = (self.read_state)()?;
        Ok(!state.next_turn.is_empty() || !state.next_step.is_empty())
    }

    /// Whether the message with this id is still queued in either list (an unreadable queue
    /// counts as queued: the caller then keeps its conservative behavior).
    pub fn is_queued(&self, id: &str) -> bool {
        match (self.read_state)() {
            Ok(state) => state
                .next_turn
                .iter()
                .chain(state.next_step.iter())
                .any(|message| message.id.as_ref() == id),
            Err(_) => true,
        }
    }

    /// Pending prompts, or the projection's error (`invalid persisted inbox splice at session seq N`).
    pub fn try_next_turn(&self) -> Result<Vec<UserMessage>, BoxError> {
        Ok((self.read_state)()?.next_turn.clone())
    }

    /// Pending next-step input, or the projection's error.
    pub fn try_next_step(&self) -> Result<Vec<UserMessage>, BoxError> {
        Ok((self.read_state)()?.next_step.clone())
    }

    /// Remove and return the complete batch proposed for one step: next-step input followed by the
    /// queued turn, when `target` asks for it.
    pub fn claim(&self, target: InboxTarget, turn: u64) -> Result<Vec<UserMessage>, BoxError> {
        let steps = (self.read_state)()?.next_step.len();
        let mut claimed = self.mutate(InboxTarget::NextStep, 0.0, steps as f64, vec![], false)?;
        if target == InboxTarget::NextTurn {
            claimed.extend(self.mutate(InboxTarget::NextTurn, 0.0, 1.0, vec![], false)?);
        }
        if let Some(dispatch) = (self.dispatch)() {
            for message in &claimed {
                let message = message.clone();
                dispatch.emit(|agent| AgentInboxClaimed {
                    agent,
                    message,
                    turn,
                });
            }
        }
        Ok(claimed)
    }

    fn locate(&self, message_id: &MessageId) -> Result<Option<(InboxTarget, usize)>, BoxError> {
        let state = (self.read_state)()?;
        for target in [InboxTarget::NextTurn, InboxTarget::NextStep] {
            if let Some(index) = state.list(target).iter().position(|m| &m.id == message_id) {
                return Ok(Some((target, index)));
            }
        }
        Ok(None)
    }

    /// Commit one normalized mutation and publish its live events.
    fn mutate(
        &self,
        target: InboxTarget,
        start: f64,
        delete_count: f64,
        inserted: Vec<UserMessage>,
        discard_removed: bool,
    ) -> Result<Vec<UserMessage>, BoxError> {
        let state = (self.read_state)()?;
        let inbox = state.list(target);
        let len = inbox.len() as f64;
        let truncated_start = start.trunc();
        let offset = if truncated_start.is_nan() {
            0.0
        } else {
            truncated_start
        };
        let actual_start = if offset < 0.0 {
            (len + offset).max(0.0)
        } else {
            offset.min(len)
        } as usize;
        let truncated_delete = delete_count.trunc();
        let actual_delete = (if truncated_delete.is_nan() {
            0.0
        } else {
            truncated_delete
        })
        .max(0.0)
        .min((inbox.len() - actual_start) as f64) as usize;
        if actual_delete == 0 && inserted.is_empty() {
            return Ok(vec![]);
        }
        let candidate = spliced(inbox, actual_start, actual_delete, &inserted);
        assert_unique(target, &candidate, &state)?;
        let canceled = discard_removed && actual_delete > 0;
        let mut data = Map::new();
        data.insert("target".into(), target.as_str().into());
        data.insert("start".into(), actual_start.into());
        if actual_delete != 0 {
            data.insert("removedCount".into(), actual_delete.into());
        }
        data.insert("inserted".into(), serde_json::to_value(&inserted)?);
        if canceled {
            data.insert("outcome".into(), "canceled".into());
        }
        let removed = inbox[actual_start..actual_start + actual_delete].to_vec();
        self.session
            .append("agent/inbox/spliced", Value::Object(data), None)?;
        if let Some(dispatch) = (self.dispatch)() {
            if discard_removed {
                for message in &removed {
                    let message = message.clone();
                    dispatch.emit(|agent| AgentInboxDiscarded { agent, message });
                }
            }
            for message in inserted {
                dispatch.emit(|agent| AgentInboxInserted { agent, message });
            }
        }
        Ok(removed)
    }
}

impl Inbox for ReactLoopInbox {
    /// The trait getter cannot return an error; a broken persisted splice surfaces as a panic
    /// carrying the projection's message (the source getter throws). `try_next_turn` is the
    /// error-returning form.
    fn next_turn(&self) -> Vec<UserMessage> {
        self.try_next_turn()
            .unwrap_or_else(|error| panic!("{error}"))
    }
    fn next_step(&self) -> Vec<UserMessage> {
        self.try_next_step()
            .unwrap_or_else(|error| panic!("{error}"))
    }
    fn clear(&self) -> Result<(), BoxError> {
        let steps = (self.read_state)()?.next_step.len();
        self.splice(InboxTarget::NextStep, 0.0, steps as f64, vec![])?;
        let turns = (self.read_state)()?.next_turn.len();
        self.splice(InboxTarget::NextTurn, 0.0, turns as f64, vec![])?;
        Ok(())
    }
    fn append(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
        let end = (self.read_state)()?.list(target).len();
        self.splice(target, end as f64, 0.0, vec![message])?;
        Ok(())
    }
    fn prepend(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
        self.splice(target, 0.0, 0.0, vec![message])?;
        Ok(())
    }
    fn replace(&self, message_id: &MessageId, new_message: UserMessage) -> Result<bool, BoxError> {
        let Some((target, index)) = self.locate(message_id)? else {
            return Ok(false);
        };
        self.splice(target, index as f64, 1.0, vec![new_message])?;
        Ok(true)
    }
    fn remove(&self, message_id: &MessageId) -> Result<bool, BoxError> {
        let Some((target, index)) = self.locate(message_id)? else {
            return Ok(false);
        };
        self.splice(target, index as f64, 1.0, vec![])?;
        Ok(true)
    }
    fn splice(
        &self,
        target: InboxTarget,
        start: f64,
        delete_count: f64,
        inserted: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        self.mutate(target, start, delete_count, inserted, true)
    }
}
