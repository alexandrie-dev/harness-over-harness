//! How one agent log accounts for the work it consumed.
//!
//! The turn and step vocabulary alone cannot answer this: a turn that stops before its first step
//! leaves a `turn/end` shaped exactly like the balanced no-op turns a rejection or an empty claim
//! produces. The missing fact is the inbox's own record: each mutation is logged with
//! `removedCount`, and a cancellation is marked `outcome: 'canceled'`.

use hoh_session::SessionEvent;
use std::{collections::HashSet, sync::Arc};

/// How one agent log accounts for the work it consumed.
#[derive(Clone, Debug, Default)]
pub struct ConsumedWork {
    /// The latest closed turn that accounts for consumed work: one that entered a model step, or
    /// one that claimed inbox input and then failed, was stopped, or was rejected.
    pub end: Option<Arc<SessionEvent>>,
    /// Whether accepted work was cancelled out of the inbox, unrun, after that turn.
    pub dropped_unrun: bool,
}

/// Whether a turn that consumed input but never reached a step ends in a way that accounts for
/// that input. Only a `completed` end does not. An unnameable ending must not read as success.
fn accounts_for_claim(reason: &serde_json::Value) -> bool {
    !matches!(
        reason.get("kind").and_then(|k| k.as_str()),
        Some("completed")
    )
}

/// Fold one agent log, or an owned suffix of one, into its account of consumed work. Single pass.
pub fn fold_consumed_work(events: &[Arc<SessionEvent>]) -> ConsumedWork {
    let mut stepped: HashSet<u64> = HashSet::new();
    let mut claimed: HashSet<u64> = HashSet::new();
    let mut open: Option<u64> = None;
    let mut end: Option<Arc<SessionEvent>> = None;
    let mut dropped_unrun = false;
    for event in events {
        let data = event.data();
        let turn_of = |value: &serde_json::Value| value.get("turn").and_then(|t| t.as_u64());
        match event.event_type() {
            "turn/start" => open = turn_of(data),
            "step/start" => {
                if let Some(turn) = turn_of(data) {
                    stepped.insert(turn);
                }
            }
            "agent/inbox/spliced" => {
                // Absent `removedCount` is a plain insertion.
                if data.get("removedCount").is_none() {
                    continue;
                }
                // A replacement keeps the work pending under a new identity, so only a
                // cancellation that leaves nothing behind drops it.
                if data.get("outcome").and_then(|o| o.as_str()) == Some("canceled") {
                    dropped_unrun |= data
                        .get("inserted")
                        .and_then(|i| i.as_array())
                        .is_none_or(|inserted| inserted.is_empty());
                } else if let Some(turn) = open {
                    // Claims are the loop's own step-boundary reads, always inside a turn.
                    claimed.insert(turn);
                }
            }
            "turn/end" => {
                let turn = turn_of(data);
                open = None;
                let null = serde_json::Value::Null;
                let reason = data.get("reason").unwrap_or(&null);
                let accounted = turn.is_some_and(|turn| {
                    stepped.remove(&turn) || (claimed.remove(&turn) && accounts_for_claim(reason))
                });
                if accounted {
                    end = Some(event.clone());
                    // Anything dropped before this turn closed is what its own ending reports.
                    dropped_unrun = false;
                }
            }
            _ => {}
        }
    }
    ConsumedWork { end, dropped_unrun }
}
