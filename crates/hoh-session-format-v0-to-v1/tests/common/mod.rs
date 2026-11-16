#![allow(dead_code)]
use hoh_session_format::*;
use hoh_session_format_v0_to_v1::*;
use serde_json::{Value, json};

// Snapshot of packages/core/session/src/known-event-types.ts at 39163dd3.
pub const KNOWN_SESSION_EVENT_TYPES: &[&str] = &[
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
    "deliverables/presented",
    "developer/message",
    "feedback/message-delete",
    "feedback/message-put",
    "feedback/record",
    "goal/change",
    "hook/invoked",
    "hook/result",
    "image/offload",
    "job/registered",
    "job/settled",
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
    "subagent/catalog",
    "subagent/descriptor",
    "subagent/model-selection-policy",
    "system/message",
    "team/member",
    "team/message/delivered",
    "team/message/queued",
    "team/mode",
    "team/task",
    "todo/write",
    "tool-workflow/agent-end",
    "tool-workflow/agent-start",
    "tool-workflow/run-end",
    "tool-workflow/run-start",
    "tool/call",
    "tool/ptc-dispatch",
    "tool/ptc-dispatch-start",
    "tool/result",
    "turn/end",
    "turn/start",
    "user/message",
    "web/deepseek-search-llm-request",
    "working-directory/change",
    "workspace/changes",
];
pub fn object(v: Value) -> SessionFormatJsonObject {
    v.as_object().unwrap().clone()
}
pub fn header(version: u64, id: &str) -> Value {
    json!({"type":"session","version":version,"id":id,"createdAt":1,"delegationDepth":0})
}
pub fn logical(version: u64, id: &str) -> SessionFormatHeader {
    object(json!({"version":version,"id":id,"createdAt":1,"isSeeded":false,"delegationDepth":0}))
}
pub fn event(kind: &str, seq: u64, data: Value) -> Value {
    json!({"type":kind,"seq":seq,"time":seq+1,"data":data})
}
pub fn surface(kind: &str, seq: u64, data: Value) -> Value {
    let mut e = event(kind, seq, data);
    e["surfaceOp"] = "append".into();
    e
}
pub fn user(id: &str) -> Value {
    user_source(id, json!({"kind":"user"}))
}
pub fn user_source(id: &str, source: Value) -> Value {
    json!({"id":id,"role":"user","content":[{"type":"text","text":id}],"source":source})
}
pub fn assistant() -> Value {
    json!({"id":"assistant-1","role":"assistant","content":[{"type":"text","text":"text"}],"source":{"kind":"model","provider":"mock","model":"mock"}})
}
pub fn tool() -> Value {
    json!({"id":"tool-1","role":"user","content":[{"type":"tool-result","toolCallId":"call-1","content":[{"type":"text","text":"text"}],"isError":false}],"source":{"kind":"tool","callId":"call-1"}})
}
pub fn decode(rows: &[Value]) -> Result<SessionFormatArtifact, SessionFormatError> {
    testing::restore::restore_v1(
        &header(1, "relationships"),
        rows,
        SessionFormatRecovery::Strict,
    )
}
pub fn migrate(rows: &[Value]) -> Result<SessionFormatArtifact, SessionFormatError> {
    testing::restore::restore_v0_to_v1(&header(0, "legacy"), rows, SessionFormatRecovery::Strict)
}
pub fn payload(kind: &str, data: Value) -> Result<(), SessionFormatError> {
    assert_released_event_payload(&object(event(kind, 3, data)), 1)
}
pub fn assert_error<T>(result: Result<T, SessionFormatError>, text: &str) {
    let e = result.err().expect("expected failure");
    assert!(e.to_string().contains(text), "expected {text:?}, got {e}");
}
pub fn stage(id: &str) -> Box<dyn SessionFormatMigrationStage> {
    let source = logical(0, id);
    let target = SESSION_FORMAT_V0_TO_V1
        .migrate_header(source.clone())
        .unwrap();
    SESSION_FORMAT_V0_TO_V1
        .create_stage(SessionFormatMigrationStageInput {
            source_header: &source,
            target_header: &target,
            source_inherited_event_count: Some(0),
            source_kind: SessionFormatSourceKind::Decoded,
        })
        .unwrap()
}
pub fn transform(
    stage: &mut dyn SessionFormatMigrationStage,
    event: Value,
) -> Result<Value, SessionFormatError> {
    let mut c = SessionFormatEventCollector::new();
    stage.transform_event(object(event), &mut c)?;
    Ok(Value::Object(c.values.remove(0)))
}
pub fn values(artifact: &SessionFormatArtifact) -> Vec<Value> {
    artifact.events.iter().cloned().map(Value::Object).collect()
}
pub fn artifact(events: Vec<Value>) -> SessionFormatArtifact {
    SessionFormatArtifact {
        header: logical(1, "validation"),
        inherited_event_count: 0,
        events: events.into_iter().map(object).collect(),
    }
}
pub fn turn(seq: u64, turn: u64) -> Value {
    event("turn/start", seq, json!({"turn":turn}))
}
pub fn end(seq: u64, turn: u64) -> Value {
    event(
        "turn/end",
        seq,
        json!({"turn":turn,"reason":{"kind":"completed"}}),
    )
}
pub fn step(seq: u64) -> Value {
    event("step/start", seq, json!({"turn":1,"step":1}))
}
pub fn step_end(seq: u64) -> Value {
    event("step/end", seq, json!({"turn":1,"step":1}))
}
pub fn request(seq: u64) -> Value {
    event(
        "request/header",
        seq,
        json!({"header":{"config":{"provider":"p","model":"m"}},"reason":"initial"}),
    )
}
pub fn retry(seq: u64, attempt: u64, id: &str, policy: &str) -> Value {
    event(
        "llm/retry",
        seq,
        json!({"retryId":id,"turn":1,"step":1,"provider":"p","mode":"normal","policyKey":policy,"retry":attempt,"maxRetries":3,"delayMs":1,"failure":{"message":"x","code":"X"}}),
    )
}
pub fn ptc(seq: u64, settle: bool) -> Value {
    let mut d = json!({"rootCallId":"root","parentCallId":"root","subCallId":"sub","name":"a","arguments":{"x":1}});
    if settle {
        d["isError"] = false.into();
        d["content"] = json!([]);
    }
    event(
        if settle {
            "tool/code-dispatch"
        } else {
            "tool/code-dispatch-start"
        },
        seq,
        d,
    )
}
pub fn compaction_start(seq: u64, turn: Value) -> Value {
    event(
        "compaction/start",
        seq,
        json!({"compactionId":"c","turn":turn}),
    )
}
pub fn compaction_end(seq: u64) -> Value {
    event("compaction/end", seq, json!({"compactionId":"c","turn":1}))
}
pub fn summary(seq: u64) -> Value {
    event(
        "compaction/summary",
        seq,
        json!({"compactionId":"c","summary":[{"type":"text","text":"summary"}],"shadowedRange":{"start":1,"end":1},"shadowedSeqs":[1],"shadowedTokenCount":1,"provider":"p","model":"m"}),
    )
}
pub fn merge(mut value: Value, extra: Value) -> Value {
    for (key, v) in extra.as_object().unwrap() {
        value[key] = v.clone();
    }
    value
}
pub fn fixtures() -> Value {
    merge(
        merge(
            json!({
                "agent-preset/selected":{"agentPreset":"default"},
                "agent/inbox/spliced":{"target":"next-turn","start":0,"inserted":[user("user-1")]},
                "approval/asked":{"id":"approval-1","toolName":"bash","callId":"call-1","reason":"needed"},
                "approval/decided":{"id":"approval-1","outcome":"allowed-once"},
                "approval/policy":{"policy":"ask","source":"delegation"},
                "assistant/chunk":{"turn":1,"step":0,"chunk":{"type":"text-delta","index":0,"text":"x"}},
                "assistant/message":{"turn":1,"step":0,"message":assistant(),"usage":{"inputTokens":1,"outputTokens":1}},
                "command/done":{"commandId":"command-1","kind":"success","sourceEventSeq":0},
                "command/run":{"commandId":"command-1","name":"compact","args":" now","source":{"kind":"user"}},
                "compaction/end":{"compactionId":"compact-1","turn":null},
                "compaction/prune":{"shadowedRange":{"start":0,"end":1},"shadowedSeqs":[0,1],"shadowedTokenCount":2},
                "compaction/start":{"compactionId":"compact-1","turn":1},
                "compaction/summary":{"compactionId":"compact-1","summary":[{"type":"text","text":"text"}],"shadowedRange":{"start":0,"end":1},"shadowedSeqs":[0,1],"shadowedTokenCount":2,"provider":"mock","model":"mock","rawOutput":[{"type":"text","text":"text"}],"llmStreamCall":true},
                "feedback/record":{"text":"feedback"},
                "goal/change":{"kind":"goal/change","version":1,"operation":"create","goal":{"id":"goal-1","revision":1,"objective":"ship","phase":"active","maxGoalRounds":3},"roundsStarted":0,"createdAt":1,"updatedAt":1},
                "hook/invoked":{"turn":1,"point":"PreToolUse","dialect":"claude-code","handlerId":"hook-1"},
                "hook/result":{"turn":1,"point":"PreToolUse","handlerId":"hook-1","decision":"pass","durationMs":1},
                "llm/retry":{"retryId":"retry-1","turn":1,"step":0,"provider":"mock","mode":"normal","policyKey":"default","retry":1,"maxRetries":2,"delayMs":10,"failure":{"message":"retry","code":"SERVER"}},
                "llm/retry-started":{"retryId":"retry-1","turn":1,"step":0,"retry":1}
            }),
            json!({
                "model/selection":{"provider":"mock","model":"mock","reasoningEffort":"high"},
                "permission/preset":{"preset":"default"},"plan/mode":{"active":true},
                "request/context":{"provider":"mock","model":"mock","contextWindow":8192},
                "request/header":{"header":{"config":{"provider":"mock","model":"mock","reasoningEffort":"high","maxTokens":100},"adapterDefaults":{"reasoningEffort":true,"maxTokens":true},"system":"system","tools":[{"name":"tool","description":"Tool","parameters":{"type":"object"}}]},"reason":"initial"},
                "sandbox/mode":{"mode":"workspace-write","source":"delegation"},
                "schedule/change":{"version":1,"operation":"create","schedule":{"id":"schedule-1","kind":"after","prompt":"remember","afterSeconds":60,"scheduledAt":"2026-08-31T00:00:00.000Z"}},
                "session-log-deepseek/delivery-accepted":{"sessionId":"validation","throughSeq":0,"sessionFormatVersion":1},
                "session/end-seed":{},"session/title":{"title":"Title","messageSeqs":[0],"source":{"kind":"fallback"}},
                "session/title-llm-request":{"titleProvider":"title-1","messageSeqs":[0],"route":{"provider":"mock","model":"mock"},"system":"title","messages":[user("user-1")],"maxTokens":20},
                "step/end":{"turn":1,"step":0},"step/start":{"turn":1,"step":0},
                "subagent/descriptor":{"mode":"continuable","version":3,"provider":"in-process","label":"child","agentProvider":"mock","agentModel":"mock","toolFilter":{"allow":["read"]}},
                "subagent/model-selection-policy":{"allowedModels":[{"provider":"mock","model":"mock"}]}
            }),
        ),
        json!({
            "team/member":{"version":1,"teamId":"team-1","member":{"id":"member-1","name":"worker","description":"work","provider":"in-process","context":"fresh","phase":"active"}},
            "team/message/delivered":{"version":1,"teamId":"team-1","messageId":"message-1","targetId":"member-1"},
            "team/message/queued":{"version":1,"teamId":"team-1","message":{"id":"message-1","senderId":"lead","senderName":"lead","targetId":"member-1","delivery":"quiet","content":[{"type":"text","text":"text"}]}},
            "team/task":{"version":1,"teamId":"team-1","task":{"id":"task-1","revision":1,"subject":"subject","description":"description","status":"pending","blockedBy":[],"writeScopes":["/work"]}},
            "todo/write":{"todos":[{"content":"work","status":"in_progress"}]},
            "tool-workflow/agent-end":{"runId":"run-1","seq":1,"outcome":"completed"},
            "tool-workflow/agent-start":{"runId":"run-1","seq":1,"label":"worker","phase":"build","childId":"child-1"},
            "tool-workflow/run-end":{"runId":"run-1","stopReason":"completed"},"tool-workflow/run-start":{"runId":"run-1","name":"workflow"},
            "tool/call":{"turn":1,"step":0,"callId":"call-1","name":"read","arguments":"{}"},
            "tool/code-dispatch":{"rootCallId":"root","parentCallId":"parent","subCallId":"sub","name":"read","arguments":{"path":"/work"},"isError":false,"content":[{"type":"text","text":"text"}]},
            "tool/code-dispatch-start":{"rootCallId":"root","parentCallId":"parent","subCallId":"sub","name":"read","arguments":{"path":"/work"}},
            "tool/result":{"turn":1,"step":0,"message":tool(),"error":{"name":"Error","code":"FAILED"},"meta":{"seq":999}},
            "turn/end":{"turn":1,"reason":{"kind":"completed"}},"turn/start":{"turn":1},"user/message":user("user-1"),
            "web/deepseek-search-llm-request":{"endpoint":"https://example.test/messages","apiVersion":"2023-06-01","body":{"model":"deepseek-chat","max_tokens":100,"messages":[{"role":"user","content":[{"type":"text","text":"search"}]}],"tools":[{"type":"web_search_20250305","name":"web_search","max_uses":1}]}}
        }),
    )
}
