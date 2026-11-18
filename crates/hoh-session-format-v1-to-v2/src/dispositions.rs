use hoh_session_format_v0_to_v1::{
    RELEASED_V0_EVENT_DISPOSITIONS, ReleasedV0PayloadDisposition,
    define_released_payload_disposition,
};
use indexmap::IndexMap;
use std::sync::LazyLock;

static DISPOSITIONS: LazyLock<IndexMap<&'static str, ReleasedV0PayloadDisposition>> =
    LazyLock::new(build_dispositions);

/// Immutable released-v2 payload inventory.
#[derive(Debug, Clone, Copy)]
pub struct ReleasedV2EventDispositions;
pub const RELEASED_V2_EVENT_DISPOSITIONS: ReleasedV2EventDispositions = ReleasedV2EventDispositions;
impl ReleasedV2EventDispositions {
    pub fn get(&self, kind: &str) -> Option<ReleasedV0PayloadDisposition> {
        DISPOSITIONS.get(kind).cloned()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, ReleasedV0PayloadDisposition)> {
        DISPOSITIONS
            .iter()
            .map(|(&kind, disposition)| (kind, disposition.clone()))
    }
}
pub fn released_v2_event_dispositions() -> IndexMap<&'static str, ReleasedV0PayloadDisposition> {
    DISPOSITIONS.clone()
}
fn build_dispositions() -> IndexMap<&'static str, ReleasedV0PayloadDisposition> {
    #[cfg(test)]
    BUILD_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut retained: IndexMap<_, _> = RELEASED_V0_EVENT_DISPOSITIONS
        .iter()
        .filter(|(kind, _)| {
            !matches!(
                *kind,
                "assistant/chunk"
                    | "assistant/message"
                    | "session-log-deepseek/delivery-accepted"
                    | "session/end-seed"
            )
        })
        .collect();
    retained.insert(
        "assistant/attempt",
        define_released_payload_disposition(&["turn", "step", "stream"], &[], &[]),
    );
    retained.insert(
        "assistant/message",
        define_released_payload_disposition(
            &["turn", "step", "message", "stream"],
            &["usage", "interrupted"],
            &[],
        ),
    );
    retained.insert(
        "session-log-deepseek/delivery-accepted",
        define_released_payload_disposition(
            &["sessionId", "throughSeq"],
            &["sessionFormatVersion"],
            &[],
        ),
    );
    retained.insert(
        "session/end-seed",
        define_released_payload_disposition(&[], &["inherited"], &[]),
    );
    retained
}

#[cfg(test)]
static BUILD_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub const RELEASED_V2_EVENT_TYPES: &[&str] = &[
    "agent-preset/selected",
    "agent/inbox/spliced",
    "approval/asked",
    "approval/decided",
    "approval/policy",
    "assistant/attempt",
    "assistant/message",
    "command/done",
    "command/run",
    "compaction/end",
    "compaction/prune",
    "compaction/start",
    "compaction/summary",
    "feedback/record",
    "goal/change",
    "hook/invoked",
    "hook/result",
    "llm/retry",
    "llm/retry-started",
    "model/selection",
    "permission/preset",
    "plan/mode",
    "request/context",
    "request/header",
    "sandbox/mode",
    "schedule/change",
    "session-log-deepseek/delivery-accepted",
    "session/end-seed",
    "session/title",
    "session/title-llm-request",
    "step/end",
    "step/start",
    "subagent/descriptor",
    "subagent/model-selection-policy",
    "team/member",
    "team/message/delivered",
    "team/message/queued",
    "team/task",
    "todo/write",
    "tool-workflow/agent-end",
    "tool-workflow/agent-start",
    "tool-workflow/run-end",
    "tool-workflow/run-start",
    "tool/call",
    "tool/code-dispatch",
    "tool/code-dispatch-start",
    "tool/result",
    "turn/end",
    "turn/start",
    "user/message",
    "web/deepseek-search-llm-request",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_frozen_inventory_only_once() {
        for _ in 0..3 {
            assert!(
                RELEASED_V2_EVENT_DISPOSITIONS
                    .get("assistant/attempt")
                    .is_some()
            );
            assert_eq!(RELEASED_V2_EVENT_DISPOSITIONS.iter().count(), 51);
            assert_eq!(released_v2_event_dispositions().len(), 51);
        }
        let mut owned = RELEASED_V2_EVENT_DISPOSITIONS
            .get("assistant/attempt")
            .unwrap();
        owned.required.clear();
        let mut map = released_v2_event_dispositions();
        map.shift_remove("assistant/attempt");
        assert_eq!(
            RELEASED_V2_EVENT_DISPOSITIONS
                .get("assistant/attempt")
                .unwrap()
                .required,
            ["turn", "step", "stream"]
        );
        assert_eq!(BUILD_COUNT.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
