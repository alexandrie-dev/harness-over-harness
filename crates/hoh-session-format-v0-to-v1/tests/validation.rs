mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v0_to_v1::testing::validation::*;
use hoh_session_format_v0_to_v1::*;
use serde_json::{Value, json};

#[test]
fn has_executable_valid_fixture_for_every_frozen_released_v0_event_type() {
    let f = fixtures();
    let mut actual: Vec<_> = f.as_object().unwrap().keys().map(String::as_str).collect();
    actual.sort();
    let mut expected = RELEASED_V0_EVENT_TYPES.to_vec();
    expected.sort();
    assert_eq!(actual, expected);
    assert_eq!(expected.len(), 51);
    // The current Session vocabulary is a source snapshot: hoh-session is outside the allowed dependencies.
    let current_retired = [
        "assistant/chunk",
        "tool/code-dispatch",
        "tool/code-dispatch-start",
    ];
    assert_eq!(
        RELEASED_V0_EVENT_TYPES
            .iter()
            .copied()
            .filter(|t| !KNOWN_SESSION_EVENT_TYPES.contains(t))
            .collect::<Vec<_>>(),
        current_retired
    );
    assert!(KNOWN_SESSION_EVENT_TYPES.contains(&"tool/ptc-dispatch"));
    assert!(KNOWN_SESSION_EVENT_TYPES.contains(&"tool/ptc-dispatch-start"));
    for (kind, data) in f.as_object().unwrap() {
        payload(kind, data.clone()).unwrap_or_else(|e| panic!("{kind}: {e}"));
    }
}
#[test]
fn refuses_unexpected_member_on_every_known_payload() {
    for (kind, data) in fixtures().as_object().unwrap() {
        assert_error(
            payload(kind, merge(data.clone(), json!({"unexpected":true}))),
            "unexpected member",
        );
    }
}
#[test]
fn refuses_nested_schema_drift_across_core_and_owner_payload_families() {
    let f = fixtures();
    let paths = [
        ("assistant/message", "/message/content/0"),
        ("llm/retry", "/failure"),
        ("request/header", "/header/config"),
        ("schedule/change", "/schedule"),
        ("goal/change", "/goal"),
        ("team/task", "/task"),
        ("todo/write", "/todos/0"),
        ("web/deepseek-search-llm-request", "/body"),
    ];
    for (kind, path) in paths {
        let mut d = f[kind].clone();
        d.pointer_mut(path).unwrap()["extra"] = true.into();
        assert!(payload(kind, d).is_err(), "{kind}");
    }
}
fn mutations(kind: &str, value: &Value, path: Vec<String>, output: &mut Vec<(Vec<String>, Value)>) {
    let inventory = released_v0_event_dispositions();
    if path
        .first()
        .is_some_and(|first| inventory[kind].opaque.contains(first))
        || path
            .last()
            .is_some_and(|last| ["replayState", "parameters"].contains(&last.as_str()))
    {
        return;
    }
    let replacement = match value {
        Value::Null => json!(false),
        Value::String(_) => json!(1),
        Value::Number(_) | Value::Bool(_) => json!("invalid"),
        Value::Array(_) => json!({}),
        Value::Object(_) => json!([]),
    };
    if !path.is_empty() {
        output.push((path.clone(), replacement));
    }
    match value {
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                let mut p = path.clone();
                p.push(i.to_string());
                mutations(kind, v, p, output);
            }
        }
        Value::Object(o) => {
            for (key, v) in o {
                let mut p = path.clone();
                p.push(key.clone());
                mutations(kind, v, p, output);
            }
        }
        _ => {}
    }
}
fn replace_path(value: &mut Value, path: &[String], replacement: Value) {
    let mut current = value;
    for key in &path[..path.len() - 1] {
        current = if current.is_array() {
            &mut current[key.parse::<usize>().unwrap()]
        } else {
            &mut current[key]
        };
    }
    let key = path.last().unwrap();
    if current.is_array() {
        current[key.parse::<usize>().unwrap()] = replacement;
    } else {
        current[key] = replacement;
    }
}
#[test]
fn refuses_type_corruption_at_every_non_opaque_nested_member_in_frozen_fixture_inventory() {
    let mut count = 0;
    for (kind, data) in fixtures().as_object().unwrap() {
        let mut m = Vec::new();
        mutations(kind, data, vec![], &mut m);
        for (path, replacement) in m {
            let mut changed = data.clone();
            replace_path(&mut changed, &path, replacement);
            assert!(payload(kind, changed).is_err(), "{kind}.{}", path.join("."));
            count += 1;
        }
    }
    assert!(count > 250, "{count}");
}
#[test]
fn allows_explicit_opaque_tool_metadata_and_ptc_arguments_losslessly() {
    let f = fixtures();
    for kind in [
        "tool/result",
        "tool/code-dispatch",
        "tool/code-dispatch-start",
    ] {
        let data = f[kind].clone();
        payload(kind, data.clone()).unwrap();
        assert_eq!(data, f[kind]);
    }
}
#[test]
fn refuses_opaque_members_not_lossless_json_without_snapshotting() {
    // Undefined is unrepresentable in Value; negative zero is the remaining non-lossless JSON value.
    let mut data = fixtures()["tool/result"].clone();
    data["meta"] = json!({"value":-0.0});
    assert_error(
        payload("tool/result", data),
        "opaque meta is not lossless JSON",
    );
}
#[test]
fn preserves_unknown_merge_extensible_nested_union_arms() {
    let mut block = user("user-1");
    block["content"] = json!([{"type":"future-block","private":{"preserved":true}}]);
    let source = user_source(
        "user-1",
        json!({"kind":"future-source","private":{"preserved":true}}),
    );
    for (kind, data) in [
        ("user/message", block),
        ("user/message", source),
        (
            "assistant/chunk",
            json!({"turn":1,"step":0,"chunk":{"type":"finish","reason":{"kind":"future-reason","private":{"preserved":true}}}}),
        ),
        (
            "turn/end",
            json!({"turn":1,"reason":{"kind":"future-reason","private":{"preserved":true}}}),
        ),
    ] {
        payload(kind, data).unwrap();
    }
}
#[test]
fn refuses_unknown_v0_events_even_when_marked_ignorable() {
    let error = migrate(&[merge(
        event("plugin/unknown", 0, json!({})),
        json!({"ignorable":true}),
    )])
    .unwrap_err();
    assert!(error.is_unsupported_migration());
    assert!(error.to_string().contains("unknown historical event"));
    assert!(
        error
            .to_string()
            .contains("refuses unknown historical events even when ignorable")
    );
}
#[test]
fn exercises_each_released_test_validation_policy() {
    let mut v0 = artifact(vec![]);
    v0.header = logical(0, "validation");
    let v1 = artifact(vec![]);
    assert_released_v0_source_artifact(&v0).unwrap();
    let mut normalized = v0.clone();
    normalized.events.push(object(event(
        "feedback/record",
        0,
        json!({"text":"retained"}),
    )));
    assert_normalized_released_v0_artifact(&normalized).unwrap();
    assert_released_v1_migration_source(&v1).unwrap();
    assert_released_v1_physical_artifact(&v1).unwrap();
    let forward = artifact(vec![merge(
        event("plugin/future", 0, json!({})),
        json!({"ignorable":true}),
    )]);
    assert_released_v1_migration_source(&forward).unwrap();
    for e in [
        surface(
            "steering/message",
            0,
            json!({"turn":1,"content":[],"source":{"kind":"user"}}),
        ),
        event("compact/start", 0, json!({"turn":null})),
    ] {
        let mut a = v0.clone();
        a.events.push(object(e));
        assert_released_v0_source_artifact(&a).unwrap();
    }
    let mut unknown = v0;
    unknown.events.push(object(merge(
        event("plugin/unknown", 0, json!({})),
        json!({"ignorable":true}),
    )));
    assert_error(
        assert_released_v0_source_artifact(&unknown),
        "unknown historical event",
    );
}
#[test]
fn permits_empty_assistant_sources_only_under_released_policy() {
    let assistant = object(merge(
        surface("assistant/message", 1, json!({})),
        json!({"sourceEventSeqs":[]}),
    ));
    assert_released_surface_metadata(&assistant, 1, "assistant/message", "allow-empty-assistant")
        .unwrap();
    assert_error(
        assert_released_surface_metadata(&assistant, 1, "assistant/message", "forbid-assistant"),
        "obsolete chunk references",
    );
}
fn reference() -> Value {
    json!({"sessionId":"source","label":"Source","capturedFormatVersion":1,"capturedThroughSeq":0,"compacted":false,"originalMessages":1,"retainedMessages":1,"omittedMessages":0,"omittedBytes":0,"truncated":false,"inputIndex":0})
}
fn reference_message(r: Vec<Value>) -> Value {
    user_source(
        "reference",
        json!({"kind":"session-reference","form":"recall","version":1,"references":r}),
    )
}
#[test]
fn keeps_captured_format_version_v1_only_inside_session_reference_sources() {
    let e = surface("user/message", 0, reference_message(vec![reference()]));
    assert_error(migrate(std::slice::from_ref(&e)), "capturedFormatVersion");
    assert_eq!(values(&decode(std::slice::from_ref(&e)).unwrap()), vec![e]);
}
#[test]
fn migrates_released_subagent_descriptors_unchanged() {
    let cases = [
        json!({"version":1,"provider":"spawn","label":"old child"}),
        json!({"version":1,"provider":"p","label":"","agentProvider":"p","persona":"x","toolFilter":{"allow":["read"]}}),
        json!({"version":2,"mode":"one-shot","provider":"p","label":"child"}),
        json!({"version":2,"mode":"continuable","provider":"p","label":"child","agentProvider":"p","agentModel":"m"}),
        json!({"version":2,"mode":"continuable","provider":"p","label":"","agentModel":"m","toolFilter":{"deny":[""]}}),
        json!({"version":3,"mode":"continuable","provider":"p","label":"child","agentReasoningEffort":"high"}),
    ];
    for data in cases {
        let e = event("subagent/descriptor", 0, data);
        assert_eq!(values(&migrate(std::slice::from_ref(&e)).unwrap()), vec![e]);
    }
}
#[test]
fn refuses_malformed_v0_subagent_descriptors_as_malformed() {
    let cases = [
        (
            json!({"version":1,"mode":"continuable","provider":"p","label":"child"}),
            "unexpected member \"mode\"",
        ),
        (
            json!({"version":1,"provider":"p"}),
            "lacks required member \"label\"",
        ),
        (
            json!({"version":1,"provider":"p","label":"child","agentReasoningEffort":"high"}),
            "unexpected member \"agentReasoningEffort\"",
        ),
        (
            json!({"version":2,"mode":"continuable","provider":"p","label":"child","agentReasoningEffort":"high"}),
            "unexpected member \"agentReasoningEffort\"",
        ),
        (
            json!({"version":2,"mode":"one-shot","provider":"p","persona":"x"}),
            "unexpected member \"persona\"",
        ),
        (
            json!({"version":3,"mode":"continuable","provider":"p","label":"child","agentModel":"m"}),
            "agentProvider and agentModel must be paired",
        ),
    ];
    for (data, text) in cases {
        let e = migrate(&[event("subagent/descriptor", 0, data)]).unwrap_err();
        assert!(e.to_string().contains(text), "{e}");
        assert!(!e.to_string().contains("unsupported descriptor version"));
    }
}
#[test]
fn accepts_every_released_nested_union_variant_and_optional_member() {
    let sources = vec![
        json!({"kind":"user","rpcId":"rpc-1","clientTimeZone":"Asia/Shanghai"}),
        json!({"kind":"plugin","plugin":"plain"}),
        json!({"kind":"plugin","plugin":"instructions","form":"instructions"}),
        json!({"kind":"plugin","plugin":"catalog","form":"catalog"}),
        json!({"kind":"plugin","plugin":"snapshot","form":"snapshot","sections":[{"name":"one","text":"value"}]}),
        json!({"kind":"plugin","plugin":"notice","form":"notice","summary":"notice"}),
        json!({"kind":"plugin","plugin":"relay","form":"relay"}),
        json!({"kind":"plugin","plugin":"recall","form":"recall"}),
        json!({"kind":"plugin","plugin":"compact","compactionId":"compact-1","sourceCommandId":"command-1"}),
        json!({"kind":"model","provider":"mock","model":"mock","replayState":{"private":true}}),
        json!({"kind":"tool","callId":"call-1"}),
        json!({"kind":"agent-instructions","form":"instructions","baseline":true,"baselineIdentity":"base","changes":[{"action":"set","scope":".","path":"AGENTS.md","digest":"one"},{"action":"replace","scope":"src","path":"src/AGENTS.md"},{"action":"remove","scope":"old","path":"old/AGENTS.md"}]}),
        json!({"kind":"agent-instructions","form":"instructions","changes":[]}),
        json!({"kind":"session-reference","form":"recall","version":1,"references":[merge(reference(),json!({"capturedThroughSeq":null,"compacted":true,"originalMessages":2,"retainedMessages":1,"omittedMessages":1,"omittedBytes":20,"truncated":true}))]}),
        json!({"kind":"session-reference","form":"recall","version":1,"references":[{"sessionId":"source-without-version","label":"Source","capturedThroughSeq":0,"compacted":false,"originalMessages":1,"retainedMessages":1,"omittedMessages":0,"omittedBytes":0,"truncated":false,"inputIndex":0}]}),
        json!({"kind":"team-message","teamId":"team","messageId":"message","senderId":"sender","senderName":"Sender"}),
        json!({"kind":"goal","goalId":"goal","revision":1,"round":1}),
        json!({"kind":"skill-invocation","name":"skill","form":"instructions"}),
        json!({"kind":"skill-catalog","form":"catalog","update":true,"entries":[{"name":"skill","description":"Skill"}]}),
        json!({"kind":"skill-catalog","form":"catalog","entries":[]}),
        json!({"kind":"coordinator","form":"relay","senderSessionId":"parent"}),
        json!({"kind":"subagent-report","form":"relay","senderSessionId":"child"}),
        json!({"kind":"subagent-settled","form":"notice","summary":"settled","senderSessionId":"child"}),
        json!({"kind":"webhook","provider":"github","source":"repo","deliveryId":"delivery","ruleId":"rule","form":"notice","summary":"push"}),
    ];
    for (i, source) in sources.into_iter().enumerate() {
        payload("user/message", user_source(&format!("source-{i}"), source)).unwrap();
    }
    let blocks = [
        json!({"type":"reasoning","text":"reasoning"}),
        json!({"type":"image","attachment":{"attachmentId":"image","mediaType":"image/png","bytes":10,"width":2,"height":2,"name":"x.png","originalDimensions":{"width":4,"height":4}}}),
        json!({"type":"image","attachment":{"attachmentId":"minimal-image","mediaType":"image/jpeg","bytes":1,"width":1,"height":1}}),
        json!({"type":"tool-call","id":"call","name":"read","arguments":"{}"}),
        json!({"type":"tool-result","toolCallId":"call","content":[{"type":"text","text":"text"}],"isError":true}),
    ];
    for (i, block) in blocks.into_iter().enumerate() {
        let mut message = user(&format!("block-{i}"));
        message["content"] = json!([block]);
        payload("user/message", message).unwrap();
    }
    let chunks = [
        json!({"type":"block-start","index":0,"blockType":"text"}),
        json!({"type":"reasoning-delta","index":0,"text":"r"}),
        json!({"type":"tool-call-delta","index":0,"id":"call","name":"read","argumentsDelta":"{}"}),
        json!({"type":"tool-call-delta","index":0,"id":"call","argumentsDelta":"{}"}),
        json!({"type":"block-end","index":0,"block":{"type":"text","text":"text"}}),
        json!({"type":"usage","usage":{"inputTokens":1,"outputTokens":2,"totalTokens":3,"cacheReadTokens":0,"cacheWriteTokens":0,"reasoningTokens":1}}),
        json!({"type":"finish","reason":{"kind":"stop"},"replayState":{"response":{"id":"response"},"blocks":[{}]}}),
        json!({"type":"finish","reason":{"kind":"tool-calls"}}),
        json!({"type":"finish","reason":{"kind":"max-tokens"}}),
        json!({"type":"finish","reason":{"kind":"aborted","failure":{"message":"abort","code":"ABORT"}}}),
        json!({"type":"finish","reason":{"kind":"error","failure":{"message":"error","code":"ERROR"}}}),
    ];
    for chunk in chunks {
        payload("assistant/chunk", json!({"turn":1,"step":0,"chunk":chunk})).unwrap();
    }
    for reason in [
        json!({"kind":"blocked"}),
        json!({"kind":"max-tokens"}),
        json!({"kind":"interrupted"}),
        json!({"kind":"aborted","reason":{"kind":"user"}}),
        json!({"kind":"aborted","reason":{"kind":"parent"}}),
        json!({"kind":"aborted","reason":{"kind":"disposed"}}),
        json!({"kind":"aborted","reason":{"kind":"legacy"}}),
        json!({"kind":"aborted","reason":{"kind":"hook","reason":"hook"}}),
        json!({"kind":"error","error":{"message":"error","code":"ERROR","status":500,"providerRetryAfterMs":1,"requestId":"id"}}),
    ] {
        payload("turn/end", json!({"turn":1,"reason":reason})).unwrap();
    }
    for (kind, data) in remaining_valid() {
        payload(kind, data).unwrap_or_else(|e| panic!("{kind}: {e}"));
    }
}
fn remaining_valid() -> Vec<(&'static str, Value)> {
    vec![
        (
            "goal/change",
            json!({"kind":"goal/change","version":1,"operation":"clear","cleared":{"id":"g","revision":2},"clearedAt":3}),
        ),
        (
            "goal/change",
            json!({"kind":"goal/change","version":1,"operation":"block","goal":{"id":"g","revision":2,"objective":"x","phase":"blocked","maxGoalRounds":3,"blockedReason":{"code":"waiting","message":"Wait"}},"roundsStarted":1,"createdAt":1,"updatedAt":2}),
        ),
        (
            "schedule/change",
            json!({"version":1,"operation":"create","schedule":{"id":"at","kind":"at","prompt":"x","scheduledAt":"2026-08-31T00:00:00.000Z"}}),
        ),
        (
            "schedule/change",
            json!({"version":1,"operation":"create","schedule":{"id":"every","kind":"every","prompt":"x","everySeconds":300,"scheduledAt":"2026-08-31T00:00:00.000Z"}}),
        ),
        (
            "schedule/change",
            json!({"version":1,"operation":"delete","id":"at"}),
        ),
        (
            "schedule/change",
            json!({"version":1,"operation":"dispatch","id":"every","acceptedAt":"2026-08-31T00:00:00.000Z"}),
        ),
        (
            "llm/retry",
            json!({"retryId":"r","turn":1,"step":0,"provider":"p","mode":"always","policyKey":"k","retry":1,"delayMs":0,"failure":{"message":"x","code":"X"}}),
        ),
        (
            "session/title",
            json!({"title":"User title","messageSeqs":[],"source":{"kind":"user"}}),
        ),
        (
            "session/title",
            json!({"title":"Provider title","messageSeqs":[0],"source":{"kind":"provider","provider":"p","model":{"provider":"p","model":"m"}}}),
        ),
        (
            "session/title",
            json!({"title":"Provider title","messageSeqs":[0],"source":{"kind":"provider","provider":"p"}}),
        ),
        (
            "subagent/descriptor",
            json!({"mode":"one-shot","version":3,"provider":"p","label":"child"}),
        ),
        (
            "subagent/descriptor",
            json!({"mode":"one-shot","version":3,"provider":"p"}),
        ),
        (
            "subagent/descriptor",
            json!({"mode":"continuable","version":3,"provider":"p","label":"child","agentProvider":"p","agentModel":"m","agentReasoningEffort":"high","persona":"persona","toolFilter":{"deny":["write"]}}),
        ),
        (
            "subagent/descriptor",
            json!({"mode":"continuable","version":3,"provider":"p","label":"child"}),
        ),
        ("approval/asked", json!({"id":"approval","toolName":"read"})),
        (
            "team/member",
            json!({"version":1,"teamId":"team","member":{"id":"m","name":"m","description":"d","provider":"p","context":"fork","phase":"failed","error":"failure"}}),
        ),
        (
            "team/task",
            json!({"version":1,"teamId":"team","task":{"id":"task-2","revision":2,"subject":"s","description":"d","status":"completed","ownerId":"m","blockedBy":["task-1"],"writeScopes":[]}}),
        ),
        (
            "command/done",
            json!({"commandId":"c","kind":"error","text":"failed"}),
        ),
        (
            "compaction/end",
            json!({"compactionId":"c","sourceCommandId":"command","turn":1,"error":"failure"}),
        ),
    ]
}
#[test]
fn validates_legacy_round_zero_goal_mutation_messages() {
    let change = json!({"kind":"goal/change","version":1,"operation":"clear","cleared":{"id":"goal","revision":2},"clearedAt":3});
    let mut message = user_source(
        "legacy-goal",
        json!({"kind":"goal","goalId":"goal","revision":2,"round":0,"change":change}),
    );
    message["content"] = json!([{"type":"text","text":"<goal_state>{\"cleared\":{\"id\":\"goal\",\"revision\":2},\"clearedAt\":3}</goal_state>"}]);
    payload("user/message", message.clone()).unwrap();
    let mut bad = message.clone();
    bad["source"]["round"] = 1.into();
    assert_error(payload("user/message", bad), "round must be 0");
    let mut bad = message.clone();
    bad["source"]["goalId"] = "other".into();
    assert_error(payload("user/message", bad), "does not match");
    message["content"] = json!([{"type":"text","text":"text"}]);
    assert_error(payload("user/message", message), "content does not match");
}
fn v1_artifact(kind: &str, data: Value) -> SessionFormatArtifact {
    let e = if ["user/message", "assistant/message", "tool/result"].contains(&kind) {
        surface(kind, 3, data)
    } else {
        event(kind, 3, data)
    };
    let mut a = step(1);
    a["data"]["step"] = 0.into();
    let mut b = step_end(2);
    b["data"]["step"] = 0.into();
    artifact(vec![turn(0, 1), a, b, e])
}
#[test]
fn refuses_malformed_logical_headers_cuts_event_envelopes_and_surface_metadata() {
    let base = v1_artifact("turn/start", json!({"turn":1}));
    for (key, value) in [
        ("version", json!(0)),
        ("id", json!(1)),
        ("createdAt", json!(-1)),
        ("isSeeded", json!("yes")),
        ("delegationDepth", json!(-1)),
        ("cwd", json!(1)),
        ("parentSession", json!(1)),
        ("agentPreset", json!(1)),
        ("origin", json!("other")),
    ] {
        let mut a = base.clone();
        a.header.insert(key.into(), value);
        assert!(assert_released_v1_artifact(&a).is_err());
    }
    let mut a = base.clone();
    a.inherited_event_count = 5;
    assert_error(assert_released_v1_artifact(&a), "exceeds");
    a.inherited_event_count = 1;
    assert_error(assert_released_v1_artifact(&a), "unseeded");
    for e in [
        event("", 0, json!({})),
        event("plugin/unknown", 0, json!({})),
        turn(1, 1),
        merge(turn(0, 1), json!({"ignorable":false})),
        merge(turn(0, 1), json!({"surfaceOp":"append"})),
    ] {
        let mut e = e;
        if e["type"] == "" {
            e["type"] = 1.into();
        }
        assert!(assert_released_v1_artifact(&artifact(vec![e])).is_err());
    }
    for metadata in [
        json!({"sourceEventSeqs":"bad","surfaceOp":"append"}),
        json!({"sourceEventSeqs":[3],"surfaceOp":"append"}),
        json!({"sourceEventSeqs":[1,1],"surfaceOp":"append"}),
        json!({"sourceEventSeqs":[],"surfaceOp":"append"}),
        json!({"surfaceOp":null}),
        json!({"surfaceOp":{"op":"append","start":0,"end":1}}),
        json!({"surfaceOp":{"op":"replace","start":3,"end":1}}),
        json!({"surfaceOp":{"op":"replace","start":1,"end":3}}),
    ] {
        let mut a = v1_artifact("user/message", user("user-1"));
        a.events[3] = object(merge(Value::Object(a.events[3].clone()), metadata));
        assert!(assert_released_v1_artifact(&a).is_err());
    }
    let mut a = v1_artifact("assistant/message", fixtures()["assistant/message"].clone());
    a.events[3].insert("sourceEventSeqs".into(), json!([]));
    assert_released_event_payload(&a.events[3], 1).unwrap();
}
#[test]
fn accepts_remaining_optional_payload_members() {
    let f = fixtures();
    let cases = [
        (
            "agent/inbox/spliced",
            json!({"target":"next-step","start":0,"removedCount":1,"inserted":[],"outcome":"canceled"}),
        ),
        (
            "assistant/message",
            json!({"turn":1,"step":0,"message":assistant(),"interrupted":true,"usage":{"inputTokens":1,"outputTokens":1,"totalTokens":2,"cacheReadTokens":0,"cacheWriteTokens":0,"reasoningTokens":0}}),
        ),
        (
            "compaction/summary",
            merge(
                f["compaction/summary"].clone(),
                json!({"sourceCommandId":"command","maxTokens":10,"usage":{"inputTokens":1,"outputTokens":1}}),
            ),
        ),
        (
            "hook/invoked",
            json!({"turn":1,"point":"Stop","dialect":"codex","matcher":"*","handlerId":"h"}),
        ),
        (
            "hook/result",
            json!({"turn":1,"point":"Stop","handlerId":"h","decision":"pass","exitCode":-1,"stderrSummary":"stderr","durationMs":1}),
        ),
        (
            "request/header",
            json!({"header":{"config":{"provider":"p","model":"m","temperature":0.5,"maxTokens":5,"stop":["stop"]},"system":"","tools":[]},"reason":"change","startsSeries":true}),
        ),
    ];
    for (kind, data) in cases {
        payload(kind, data).unwrap();
    }
}
#[test]
fn refuses_every_relationship_specific_invalid_payload_branch() {
    let f = fixtures();
    let mut cases = vec![
        (
            "command/done",
            json!({"commandId":"c","kind":"success","sourceEventSeq":3}),
        ),
        (
            "session/title-llm-request",
            merge(
                f["session/title-llm-request"].clone(),
                json!({"messageSeqs":[]}),
            ),
        ),
        (
            "session/title",
            json!({"title":"t","messageSeqs":[0,0],"source":{"kind":"fallback"}}),
        ),
        (
            "tool/result",
            json!({"turn":1,"step":0,"message":merge(tool(),json!({"content":[]}))}),
        ),
        (
            "tool/result",
            json!({"turn":1,"step":0,"message":merge(tool(),json!({"content":[{"type":"text","text":"not a result"}]}))}),
        ),
        (
            "user/message",
            user_source(
                "u",
                json!({"kind":"plugin","plugin":"x","form":"notice","summary":"x","sections":[]}),
            ),
        ),
        (
            "user/message",
            reference_message(vec![merge(reference(), json!({"inputIndex":1}))]),
        ),
        (
            "user/message",
            user_source(
                "u",
                json!({"kind":"plugin","plugin":"x","form":"snapshot","sections":[],"summary":"x"}),
            ),
        ),
        (
            "user/message",
            user_source("u", json!({"kind":"plugin","plugin":"compact"})),
        ),
        (
            "assistant/chunk",
            json!({"turn":1,"step":0,"chunk":{"type":"finish","reason":{"kind":"stop"},"replayState":{"response":{},"blocks":{}}}}),
        ),
        (
            "compaction/summary",
            json!({"compactionId":"c","summary":[],"shadowedRange":{"start":0,"end":0},"shadowedSeqs":[0],"shadowedTokenCount":0,"provider":"p","model":"m","llmStreamCall":true}),
        ),
        (
            "hook/result",
            json!({"turn":1,"point":"Stop","handlerId":"h","decision":"pass","durationMs":-0.5}),
        ),
        (
            "llm/retry",
            merge(f["llm/retry"].clone(), json!({"mode":"always"})),
        ),
        (
            "session/title-llm-request",
            merge(
                f["session/title-llm-request"].clone(),
                json!({"maxTokens":0}),
            ),
        ),
        (
            "request/header",
            json!({"header":{"config":{"provider":"p","model":"m"},"adapterDefaults":{"maxTokens":true}},"reason":"initial"}),
        ),
        (
            "compaction/prune",
            json!({"shadowedRange":{"start":1,"end":0},"shadowedSeqs":[0,1],"shadowedTokenCount":1}),
        ),
        (
            "compaction/prune",
            json!({"shadowedRange":{"start":0,"end":2},"shadowedSeqs":[0,1],"shadowedTokenCount":1}),
        ),
        (
            "goal/change",
            json!({"kind":"goal/change","version":1,"operation":"create","goal":{"id":"g","revision":1,"objective":"x","phase":"active","maxGoalRounds":1,"blockedReason":{"code":"x","message":"x"}},"roundsStarted":0,"createdAt":1,"updatedAt":1}),
        ),
        (
            "schedule/change",
            json!({"version":1,"operation":"create","schedule":{"id":"x","kind":"unknown","prompt":"x","scheduledAt":"x"}}),
        ),
        (
            "schedule/change",
            json!({"version":1,"operation":"create","schedule":{"id":"x","kind":"every","prompt":"x","everySeconds":299,"scheduledAt":"2026-08-31T00:00:00.000Z"}}),
        ),
        (
            "schedule/change",
            json!({"version":1,"operation":"delete","id":" x "}),
        ),
        (
            "schedule/change",
            json!({"version":1,"operation":"dispatch","id":"x","acceptedAt":"2026-02-31T00:00:00.000Z"}),
        ),
        (
            "subagent/descriptor",
            json!({"mode":"continuable","version":3,"provider":"p","label":"x","agentProvider":"p"}),
        ),
        (
            "subagent/descriptor",
            json!({"mode":"continuable","version":3,"provider":"p","label":"x","toolFilter":{}}),
        ),
        (
            "subagent/model-selection-policy",
            json!({"allowedModels":[{"provider":"p","model":"m"},{"provider":"p","model":"m"}]}),
        ),
        (
            "subagent/model-selection-policy",
            json!({"allowedModels":[]}),
        ),
        (
            "user/message",
            reference_message(vec![merge(reference(), json!({"capturedFormatVersion":2}))]),
        ),
        (
            "user/message",
            reference_message(vec![merge(
                reference(),
                json!({"originalMessages":2,"retainedMessages":1,"omittedMessages":1,"truncated":false}),
            )]),
        ),
        ("user/message", reference_message(vec![])),
        (
            "user/message",
            reference_message(vec![
                merge(reference(), json!({"sessionId":"same","inputIndex":0})),
                merge(reference(), json!({"sessionId":"same","inputIndex":1})),
            ]),
        ),
    ];
    let body = f["web/deepseek-search-llm-request"]["body"].clone();
    for (key, value) in [
        ("messages", json!([{"role":"user","content":[]}])),
        ("messages", json!([])),
        ("tools", json!([])),
    ] {
        let mut b = body.clone();
        b[key] = value;
        cases.push((
            "web/deepseek-search-llm-request",
            json!({"endpoint":"x","apiVersion":"x","body":b}),
        ));
    }
    assert_eq!(cases.len(), 33);
    for (i, (kind, data)) in cases.into_iter().enumerate() {
        assert!(payload(kind, data).is_err(), "{kind}-{i}");
    }
}
#[test]
fn pins_released_log_instant_profile_at_bounds_and_against_noncanonical_spellings() {
    for accepted in ["0001-01-01T00:00:00.000Z", "9999-12-31T23:59:59.999Z"] {
        payload(
            "schedule/change",
            json!({"version":1,"operation":"dispatch","id":"x","acceptedAt":accepted}),
        )
        .unwrap();
    }
    for rejected in [
        "0000-01-01T00:00:00.000Z",
        "2026-02-30T00:00:00.000Z",
        "2026-08-31T00:00:00.0000Z",
        "2026-08-31T00:00:00.000+00:00",
        " 2026-08-31T00:00:00.000Z",
        "2026-08-31T00:00:00.000Z ",
    ] {
        assert_error(
            payload(
                "schedule/change",
                json!({"version":1,"operation":"dispatch","id":"x","acceptedAt":rejected}),
            ),
            "must be a canonical UTC instant",
        );
    }
}

#[test]
fn direct_payload_diagnostics_keep_raw_event_fields() {
    for (seq, text) in [
        (Some(json!(1.5)), "1.5"),
        (Some(json!("abc")), "abc"),
        (None, "undefined"),
    ] {
        let mut e = object(event("feedback/record", 0, json!({"text":1})));
        if let Some(seq) = seq {
            e.insert("seq".into(), seq);
        } else {
            e.shift_remove("seq");
        }
        let error = assert_released_event_payload(&e, 0).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("feedback/record {text} text must be a non-empty string")
        );
    }
    let mut e = object(event("feedback/record", 0, json!({})));
    e.shift_remove("type");
    e.insert("seq".into(), json!(1.5));
    assert_eq!(
        assert_released_event_payload(&e, 0)
            .unwrap_err()
            .to_string(),
        "format v0 contains unknown historical event type undefined at seq 1.5; migration refuses unknown historical events even when ignorable"
    );
    assert_eq!(
        assert_released_payload_semantics(&e, 0)
            .unwrap_err()
            .to_string(),
        "released payload validator is missing event undefined"
    );
}

#[test]
fn programmatic_objects_report_the_first_unexpected_key_in_js_order() {
    let mut e = object(event("feedback/record", 0, json!({"text":"kept"})));
    e["data"] = serde_json::from_str(r#"{"text":"kept","2":true,"1":false}"#).unwrap();
    assert_eq!(
        assert_released_event_payload(&e, 0)
            .unwrap_err()
            .to_string(),
        "feedback/record 0 data has unexpected member \"1\""
    );
}

#[test]
fn inherited_object_member_names_remain_unknown_event_types() {
    for kind in ["toString", "constructor", "hasOwnProperty"] {
        let error =
            assert_released_event_payload(&object(event(kind, 0, json!({}))), 0).unwrap_err();
        assert!(error.is_unsupported_migration());
        assert_eq!(
            error.to_string(),
            format!(
                "format v0 contains unknown historical event type \"{kind}\" at seq 0; migration refuses unknown historical events even when ignorable"
            )
        );
    }
}
