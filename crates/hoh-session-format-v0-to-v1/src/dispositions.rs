use indexmap::IndexMap;

/// Exact frozen top-level payload-member disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasedV0PayloadDisposition {
    pub required: Vec<String>,
    pub optional: Vec<String>,
    pub opaque: Vec<String>,
}

/// Immutable lookup over the frozen inventory, without a global mutable registry.
#[derive(Debug, Clone, Copy)]
pub struct ReleasedV0EventDispositions;
pub const RELEASED_V0_EVENT_DISPOSITIONS: ReleasedV0EventDispositions = ReleasedV0EventDispositions;

impl ReleasedV0EventDispositions {
    pub fn get(&self, kind: &str) -> Option<ReleasedV0PayloadDisposition> {
        INVENTORY
            .iter()
            .find(|entry| entry.0 == kind)
            .map(|(_, required, optional, opaque)| {
                define_released_payload_disposition(required, optional, opaque)
            })
    }
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, ReleasedV0PayloadDisposition)> {
        INVENTORY.iter().map(|(name, required, optional, opaque)| {
            (
                *name,
                define_released_payload_disposition(required, optional, opaque),
            )
        })
    }
}

pub fn define_released_payload_disposition(
    required: &[&str],
    optional: &[&str],
    opaque: &[&str],
) -> ReleasedV0PayloadDisposition {
    ReleasedV0PayloadDisposition {
        required: required.iter().map(|s| (*s).to_owned()).collect(),
        optional: optional.iter().map(|s| (*s).to_owned()).collect(),
        opaque: opaque.iter().map(|s| (*s).to_owned()).collect(),
    }
}

pub fn released_v0_event_dispositions() -> IndexMap<&'static str, ReleasedV0PayloadDisposition> {
    RELEASED_V0_EVENT_DISPOSITIONS.iter().collect()
}

// The source inventory is already in its en localeCompare order (all names are ASCII).
pub const RELEASED_V0_EVENT_TYPES: &[&str] = &[
    "agent-preset/selected",
    "agent/inbox/spliced",
    "approval/asked",
    "approval/decided",
    "approval/policy",
    "assistant/chunk",
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

type InventoryEntry = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);
const INVENTORY: &[InventoryEntry] = &[
    ("agent-preset/selected", &["agentPreset"], &[], &[]),
    (
        "agent/inbox/spliced",
        &["target", "start", "inserted"],
        &["removedCount", "outcome"],
        &[],
    ),
    (
        "approval/asked",
        &["id", "toolName"],
        &["callId", "reason"],
        &[],
    ),
    ("approval/decided", &["id", "outcome"], &[], &[]),
    ("approval/policy", &["policy"], &["source"], &[]),
    ("assistant/chunk", &["turn", "step", "chunk"], &[], &[]),
    (
        "assistant/message",
        &["turn", "step", "message"],
        &["usage", "interrupted"],
        &[],
    ),
    (
        "command/done",
        &["commandId", "kind"],
        &["text", "sourceEventSeq"],
        &[],
    ),
    (
        "command/run",
        &["commandId", "name", "source"],
        &["args"],
        &[],
    ),
    (
        "compaction/end",
        &["compactionId", "turn"],
        &["sourceCommandId", "error"],
        &[],
    ),
    (
        "compaction/prune",
        &["shadowedRange", "shadowedSeqs", "shadowedTokenCount"],
        &[],
        &[],
    ),
    (
        "compaction/start",
        &["compactionId", "turn"],
        &["sourceCommandId"],
        &[],
    ),
    (
        "compaction/summary",
        &[
            "compactionId",
            "summary",
            "shadowedRange",
            "shadowedSeqs",
            "shadowedTokenCount",
            "provider",
            "model",
        ],
        &[
            "sourceCommandId",
            "maxTokens",
            "usage",
            "rawOutput",
            "llmStreamCall",
        ],
        &[],
    ),
    ("feedback/record", &["text"], &[], &[]),
    (
        "goal/change",
        &["kind", "version", "operation"],
        &[
            "goal",
            "roundsStarted",
            "createdAt",
            "updatedAt",
            "cleared",
            "clearedAt",
        ],
        &[],
    ),
    (
        "hook/invoked",
        &["turn", "point", "dialect", "handlerId"],
        &["matcher"],
        &[],
    ),
    (
        "hook/result",
        &["turn", "point", "handlerId", "decision", "durationMs"],
        &["exitCode", "stderrSummary"],
        &[],
    ),
    (
        "llm/retry",
        &[
            "retryId",
            "turn",
            "step",
            "provider",
            "mode",
            "policyKey",
            "retry",
            "delayMs",
            "failure",
        ],
        &["maxRetries"],
        &[],
    ),
    (
        "llm/retry-started",
        &["retryId", "turn", "step", "retry"],
        &[],
        &[],
    ),
    (
        "model/selection",
        &["provider", "model"],
        &["reasoningEffort"],
        &[],
    ),
    ("permission/preset", &["preset"], &[], &[]),
    ("plan/mode", &["active"], &[], &[]),
    (
        "request/context",
        &["provider", "model"],
        &["contextWindow"],
        &[],
    ),
    (
        "request/header",
        &["header", "reason"],
        &["startsSeries"],
        &[],
    ),
    ("sandbox/mode", &["mode"], &["source"], &[]),
    (
        "schedule/change",
        &["version", "operation"],
        &["schedule", "id", "acceptedAt"],
        &[],
    ),
    (
        "session-log-deepseek/delivery-accepted",
        &["sessionId", "throughSeq"],
        &[],
        &[],
    ),
    ("session/end-seed", &[], &[], &[]),
    (
        "session/title",
        &["title", "messageSeqs", "source"],
        &[],
        &[],
    ),
    (
        "session/title-llm-request",
        &[
            "titleProvider",
            "messageSeqs",
            "route",
            "system",
            "messages",
            "maxTokens",
        ],
        &[],
        &[],
    ),
    ("step/end", &["turn", "step"], &[], &[]),
    ("step/start", &["turn", "step"], &[], &[]),
    (
        "subagent/descriptor",
        &["version", "provider"],
        &[
            "mode",
            "label",
            "agentProvider",
            "agentModel",
            "agentReasoningEffort",
            "persona",
            "toolFilter",
        ],
        &[],
    ),
    (
        "subagent/model-selection-policy",
        &["allowedModels"],
        &[],
        &[],
    ),
    ("team/member", &["version", "teamId", "member"], &[], &[]),
    (
        "team/message/delivered",
        &["version", "teamId", "messageId", "targetId"],
        &[],
        &[],
    ),
    (
        "team/message/queued",
        &["version", "teamId", "message"],
        &[],
        &[],
    ),
    ("team/task", &["version", "teamId", "task"], &[], &[]),
    ("todo/write", &["todos"], &[], &[]),
    (
        "tool-workflow/agent-end",
        &["runId", "seq", "outcome"],
        &[],
        &[],
    ),
    (
        "tool-workflow/agent-start",
        &["runId", "seq", "label", "childId"],
        &["phase"],
        &[],
    ),
    ("tool-workflow/run-end", &["runId", "stopReason"], &[], &[]),
    ("tool-workflow/run-start", &["runId", "name"], &[], &[]),
    (
        "tool/call",
        &["turn", "step", "callId", "name", "arguments"],
        &[],
        &[],
    ),
    (
        "tool/code-dispatch",
        &[
            "rootCallId",
            "parentCallId",
            "subCallId",
            "name",
            "arguments",
            "isError",
            "content",
        ],
        &[],
        &["arguments"],
    ),
    (
        "tool/code-dispatch-start",
        &[
            "rootCallId",
            "parentCallId",
            "subCallId",
            "name",
            "arguments",
        ],
        &[],
        &["arguments"],
    ),
    (
        "tool/result",
        &["turn", "step", "message"],
        &["error", "meta"],
        &["meta"],
    ),
    ("turn/end", &["turn", "reason"], &[], &[]),
    ("turn/start", &["turn"], &[], &[]),
    (
        "user/message",
        &["role", "id", "content", "source"],
        &[],
        &[],
    ),
    (
        "web/deepseek-search-llm-request",
        &["endpoint", "apiVersion", "body"],
        &[],
        &[],
    ),
];
