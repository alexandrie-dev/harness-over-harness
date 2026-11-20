//! Terminal lifecycle classification from the child's authoritative log.

use std::sync::Arc;

use hoh_agent::fold_consumed_work;
use hoh_kernel::{Contained, Emit};
use hoh_session::SessionEvent;

use crate::types::SubagentProvider;
use crate::types::SubagentStopReason;
use crate::types::{SubagentRunEndInfo, SubagentRunInfo};

hoh_kernel::event!(SubagentRunInfo = "subagent/start", Contained);
hoh_kernel::event!(SubagentRunEndInfo = "subagent/end", Contained);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentProviderRemoved(pub String);
hoh_kernel::event!(
    SubagentProviderRemoved = "subagent/provider-removed",
    Contained
);

#[derive(Clone)]
pub struct SubagentProviderAdded(pub Arc<dyn SubagentProvider>);
hoh_kernel::event!(SubagentProviderAdded = "subagent/provider-added", Emit);

pub fn emit_subagent_start(ctx: &hoh_kernel::Context, info: SubagentRunInfo) {
    let logger = ctx.logger();
    ctx.dispatch::<SubagentRunInfo>(Arc::new(info))
        .invoke(move |failure| {
            logger.warn(format_args!(
                "subagent: subagent/start listener failed: {failure:?}"
            ));
        });
}

pub fn emit_subagent_end(ctx: &hoh_kernel::Context, info: SubagentRunEndInfo) {
    let logger = ctx.logger();
    ctx.dispatch::<SubagentRunEndInfo>(Arc::new(info))
        .invoke(move |failure| {
            logger.warn(format_args!(
                "subagent: subagent/end listener failed: {failure:?}"
            ));
        });
}

pub fn emit_provider_removed(ctx: &hoh_kernel::Context, name: String) {
    let logger = ctx.logger();
    ctx.dispatch::<SubagentProviderRemoved>(Arc::new(SubagentProviderRemoved(name)))
        .invoke(move |failure| {
            logger.warn(format_args!(
                "subagent: subagent/provider-removed listener failed: {failure:?}"
            ));
        });
}

pub fn epoch_stop_reason(events: &[Arc<SessionEvent>]) -> SubagentStopReason {
    let consumed = fold_consumed_work(events);
    let kind = consumed
        .end
        .as_ref()
        .and_then(|event| event.data().get("reason"))
        .and_then(|reason| reason.get("kind"))
        .and_then(serde_json::Value::as_str);
    match kind {
        Some("max-tokens") => SubagentStopReason::MaxTokens,
        Some("aborted" | "interrupted") => SubagentStopReason::Aborted,
        Some("error") => SubagentStopReason::Error,
        Some("blocked") => SubagentStopReason::Refusal,
        None | Some("completed") if consumed.dropped_unrun => SubagentStopReason::Aborted,
        None | Some("completed") => SubagentStopReason::Completed,
        Some(_) => SubagentStopReason::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_session::SessionEvent;
    use serde_json::json;

    fn event(seq: u64, kind: &str, data: serde_json::Value) -> Arc<SessionEvent> {
        Arc::new(SessionEvent::from_value(json!({
            "seq": seq,
            "time": seq,
            "type": kind,
            "data": data,
        })))
    }

    #[test]
    fn blocked_claim_is_refusal() {
        let events = vec![
            event(0, "turn/start", json!({"turn": 1})),
            event(
                1,
                "agent/inbox/spliced",
                json!({"removedCount": 1, "inserted": []}),
            ),
            event(
                2,
                "turn/end",
                json!({"turn": 1, "reason": {"kind": "blocked"}}),
            ),
        ];
        assert_eq!(epoch_stop_reason(&events), SubagentStopReason::Refusal);
    }
}
