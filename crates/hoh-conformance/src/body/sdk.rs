//! SDK scenario bodies: `snapshots/sdk/sdk.snapshot.ts`, lines 1209–1779 (the per-scenario blocks
//! and the wire-shape invariants after them).
//!
//! The source also reads the captured model requests (`modelRequests`, files such as
//! `compact-requests.jsonl`) and the per-call SDK results (`results`); the runner of this crate
//! keeps neither, so the assertions over them are not carried. Each function lists them.

use super::{BodyContext, expect, expect_eq, expect_match, fail, of_type, rows, rows_of, text_of};
use crate::compare::{Check, Failure};
use serde_json::{Value, json};

/// The 26 scenarios of the group `in` (conformance/V1-SCENARIOS.tsv, suite sdk).
pub const SCENARIOS_IN: [&str; 26] = [
    "approval",
    "background-wait-steer",
    "bash-tool",
    "command-attachments",
    "command-identity",
    "desktop-context",
    "dynamic-tool-updates",
    "inline-image-prompt",
    "manual-compact",
    "max-tokens-continue",
    "multi-turn",
    "permission-switch",
    "serial-created",
    "session-resume",
    "session-title-after-turn",
    "session-title-stability",
    "subagent-activation-limit",
    "subagent-continuable",
    "subagent-continuable-inheritance",
    "subagent-list-agents",
    "subagent-mixed",
    "subagent-send-message",
    "subagent-spawn-in-process",
    "system-prompt-in-history",
    "text-turn",
    "tool-scheduler-recovery",
];

pub fn check(name: &str, ctx: &BodyContext<'_>) -> Option<Check> {
    if !SCENARIOS_IN.contains(&name) {
        return None;
    }
    Some(scenario(name, ctx).and_then(|()| wire_shape(name, ctx)))
}

fn scenario(name: &str, ctx: &BodyContext<'_>) -> Check {
    match name {
        "command-attachments" => command_attachments(ctx),
        "command-identity" => command_identity(ctx),
        "permission-switch" => permission_switch(ctx),
        "manual-compact" => manual_compact(ctx),
        "dynamic-tool-updates" => dynamic_tool_updates(ctx),
        "tool-scheduler-recovery" => tool_scheduler_recovery(ctx),
        "subagent-activation-limit" => subagent_activation_limit(ctx),
        "system-prompt-in-history" => system_prompt_in_history(ctx),
        "desktop-context" => desktop_context(ctx),
        // The source adds no assertion of its own beyond the wire-shape invariants.
        _ => Ok(()),
    }
}

fn parent(ctx: &BodyContext<'_>, name: &str) -> std::result::Result<Vec<Value>, Failure> {
    rows_of(ctx.actual, 0, name)
}

fn data_of(rows: &[Value], kind: &str, field: &str) -> Vec<Value> {
    of_type(rows, kind)
        .into_iter()
        .map(|r| r["data"][field].clone())
        .collect()
}

fn command_pairs(rows: &[Value]) -> Vec<Value> {
    of_type(rows, "command/run")
        .into_iter()
        .map(|r| json!({"name": r["data"]["name"], "args": r["data"]["args"]}))
        .collect()
}

fn user_sourced<'a>(rows: &'a [Value], kind: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|r| r["type"] == "user/message" && r["data"]["source"]["kind"] == kind)
        .collect()
}

/// sdk.snapshot.ts:1253–1297 (without `modelRequests`: its length and the request containing the
/// image message and "You are in plan mode.").
fn command_attachments(ctx: &BodyContext<'_>) -> Check {
    let events = parent(ctx, "command-attachments")?;
    let run = |name: &str, args: &str| json!({"name": name, "args": args});
    expect_eq(
        "command/run",
        &json!(command_pairs(&events)),
        &json!([
            run("plan", " Reply SDK_COMMAND_IMAGE_OK without tools."),
            run("plan", " off"),
            run("compact", ""),
            run("permission", " read-only"),
            run("goal", ""),
            run("goal", " pause"),
            run("goal", " resume"),
            run("goal", " clear"),
        ]),
    )?;
    expect_eq(
        "command/done ids",
        &json!(data_of(&events, "command/done", "commandId")),
        &json!(data_of(&events, "command/run", "commandId")),
    )?;
    let completed: Vec<Value> = of_type(&events, "command/done")
        .into_iter()
        .map(|r| json!({"kind": r["data"]["kind"], "text": r["data"]["text"]}))
        .collect();
    let goal = json!({"kind": "error",
        "text": "Attachments only accompany a goal objective: /goal <objective> or /goal edit <objective>."});
    expect_eq(
        "command/done",
        &json!(completed),
        &json!([
            {"kind": "success", "text": "Plan mode on. Use /plan off to leave."},
            {"kind": "error", "text": "Attachments cannot accompany /plan off."},
            {"kind": "error", "text": "/compact does not accept attachments"},
            {"kind": "error", "text": "/permission does not accept attachments"},
            goal, goal, goal, goal,
        ]),
    )?;
    expect_eq(
        "plan/mode",
        &json!(data_of(&events, "plan/mode", "active")),
        &json!([true]),
    )?;
    expect_eq(
        "permission/preset",
        &json!(data_of(&events, "permission/preset", "preset")),
        &json!(["workspace-write"]),
    )?;
    expect(of_type(&events, "goal/change").is_empty(), || {
        "a goal/change was recorded for rejected goal commands".into()
    })?;
    for kind in ["turn/start", "step/start"] {
        let count = of_type(&events, kind).len();
        expect(count == 1, || format!("expected 1 {kind}, got {count}"))?;
    }
    let users = user_sourced(&events, "user");
    expect(users.len() == 1, || {
        format!("expected 1 user message, got {}", users.len())
    })?;
    // A manifest without attachments declares no id: the key is omitted, never `null`.
    let mut image = json!({"mediaType": "image/png", "bytes": 69, "width": 1, "height": 1});
    if let Some(id) = ctx.manifest["input"]["attachments"][0]["id"].as_str() {
        image["attachmentId"] = json!(id);
    }
    expect_eq(
        "user message content",
        &users[0]["data"]["content"],
        &json!([
            {"type": "image", "attachment": image},
            {"type": "text", "text": "Reply SDK_COMMAND_IMAGE_OK without tools."},
        ]),
    )?;
    let last = events
        .iter()
        .rev()
        .find(|r| r["type"] == "assistant/message")
        .cloned()
        .unwrap_or(Value::Null);
    expect_match(
        "final assistant message",
        &last,
        &json!({"data": {"message": {"content": [{"type": "text", "text": "SDK_COMMAND_IMAGE_OK"}]}}}),
    )
}

/// sdk.snapshot.ts:1348–1365 (without the two `modelRequests` assertions).
fn command_identity(ctx: &BodyContext<'_>) -> Check {
    let events = parent(ctx, "command-identity")?;
    expect_eq(
        "command/run names",
        &json!(data_of(&events, "command/run", "name")),
        &json!(["access", "access", "access"]),
    )?;
    expect_eq(
        "command/done kinds",
        &json!(data_of(&events, "command/done", "kind")),
        &json!(["success", "success", "error"]),
    )?;
    expect_eq(
        "permission/preset",
        &json!(data_of(&events, "permission/preset", "preset")),
        &json!(["danger-full-access", "read-only"]),
    )?;
    let users = user_sourced(&events, "user");
    expect_eq(
        "first user message",
        users
            .first()
            .map_or(&Value::Null, |u| &u["data"]["content"]),
        &json!([{"type": "text", "text": "/permission read-only"}]),
    )
}

/// sdk.snapshot.ts:1366–1409 (the request-side half, `modelRequests`, is not carried).
fn permission_switch(ctx: &BodyContext<'_>) -> Check {
    let events = parent(ctx, "permission-switch")?;
    let run = |args: &str| json!({"name": "permission", "args": args});
    expect_eq(
        "command/run",
        &json!(command_pairs(&events)),
        &json!([run(" read-only"), run(" read-only"), run(" missing-preset")]),
    )?;
    expect_eq(
        "command/done kinds",
        &json!(data_of(&events, "command/done", "kind")),
        &json!(["success", "success", "error"]),
    )?;
    expect_eq(
        "command/done ids",
        &json!(data_of(&events, "command/done", "commandId")),
        &json!(data_of(&events, "command/run", "commandId")),
    )?;
    expect_eq(
        "permission/preset",
        &json!(data_of(&events, "permission/preset", "preset")),
        &json!(["danger-full-access", "read-only"]),
    )?;
    expect_eq(
        "sandbox/mode",
        &json!(data_of(&events, "sandbox/mode", "mode")),
        &json!(["danger-full-access", "read-only"]),
    )?;
    expect_eq(
        "approval/policy",
        &json!(data_of(&events, "approval/policy", "policy")),
        &json!(["never", "ask"]),
    )?;
    expect_eq(
        "turn/start",
        &json!(data_of(&events, "turn/start", "turn")),
        &json!([1, 2]),
    )?;
    let steps = of_type(&events, "step/start").len();
    expect(steps == 2, || format!("expected 2 step/start, got {steps}"))?;
    let notices = user_sourced(&events, "user-approval").len();
    expect(notices == 1, || {
        format!("expected 1 user-approval notice, got {notices}")
    })
}

/// sdk.snapshot.ts:1410–1465 (the `modelRequests` block, from `purpose` to
/// `SDK_COMPACT_READY`, is not carried).
fn manual_compact(ctx: &BodyContext<'_>) -> Check {
    let events = parent(ctx, "manual-compact")?;
    let commands = of_type(&events, "command/run");
    expect(commands.len() == 1, || {
        format!("expected 1 command/run, got {}", commands.len())
    })?;
    let command = &commands[0]["data"];
    expect(command["name"] == "compact", || {
        format!("command is {}, not compact", command["name"])
    })?;
    let summaries = of_type(&events, "compaction/summary");
    expect(summaries.len() == 1, || {
        format!("expected 1 compaction/summary, got {}", summaries.len())
    })?;
    let summary = summaries[0];
    expect_match(
        "compaction/summary",
        &summary["data"],
        &json!({"sourceCommandId": command["commandId"], "llmStreamCall": true}),
    )?;
    let bracket: Vec<&Value> = events
        .iter()
        .filter(|r| r["type"] == "compaction/start" || r["type"] == "compaction/end")
        .collect();
    expect_eq(
        "compaction bracket",
        &json!(
            bracket
                .iter()
                .map(|r| r["type"].clone())
                .collect::<Vec<_>>()
        ),
        &json!(["compaction/start", "compaction/end"]),
    )?;
    for event in bracket {
        expect_match(
            "compaction bracket data",
            &event["data"],
            &json!({"turn": null, "sourceCommandId": command["commandId"]}),
        )?;
    }
    let done = events
        .iter()
        .find(|r| r["type"] == "command/done")
        .map_or(Value::Null, |r| r["data"].clone());
    expect_match(
        "command/done",
        &done,
        &json!({"commandId": command["commandId"], "kind": "success", "sourceEventSeq": summary["seq"]}),
    )?;
    let checkpoint = events
        .iter()
        .find(|r| {
            r["type"] == "user/message" && r["data"]["source"]["kind"] == "compact-checkpoint"
        })
        .map_or(Value::Null, |r| r["surfaceOp"].clone());
    expect_eq(
        "checkpoint surfaceOp",
        &checkpoint,
        &json!({"op": "replace",
            "startSeq": summary["data"]["shadowedRange"]["start"],
            "endSeq": summary["data"]["shadowedRange"]["end"]}),
    )?;
    expect_eq(
        "turn/start",
        &json!(data_of(&events, "turn/start", "turn")),
        &json!([1, 2]),
    )?;
    let steps = of_type(&events, "step/start").len();
    expect(steps == 2, || format!("expected 2 step/start, got {steps}"))?;
    let id = rows(&ctx.actual[0])?
        .first()
        .map_or(Value::Null, |h| h["id"].clone());
    let observed: Vec<Value> = ctx
        .notifications
        .iter()
        .filter(|n| n["params"]["sessionId"] == id)
        .filter_map(notification_event)
        .filter(|e| e["type"] == "command/run" || e["type"] == "command/done")
        .map(|e| e["type"].clone())
        .collect();
    expect_eq(
        "command notifications",
        &json!(observed),
        &json!(["command/run", "command/done"]),
    )
}

/// `notificationEvent` (sdk.snapshot.ts:646–651).
fn notification_event(notification: &Value) -> Option<&Value> {
    (notification["method"] == "session.event" && notification["params"]["event"].is_object())
        .then(|| &notification["params"]["event"])
}

/// sdk.snapshot.ts:1535–1568. The source compares the SDK `results` events too; the notifications
/// and the durable log stand in for them.
fn dynamic_tool_updates(ctx: &BodyContext<'_>) -> Check {
    const SELECTED: [&str; 5] = [
        "request/header",
        "request/context",
        "developer/message",
        "tool/call",
        "tool/result",
    ];
    let selected = |kind: &Value| kind.as_str().is_some_and(|k| SELECTED.contains(&k));
    let events: Vec<Value> = parent(ctx, "dynamic-tool-updates")?
        .into_iter()
        .filter(|r| selected(&r["type"]))
        .collect();
    let subscribed: Vec<Value> = ctx
        .notifications
        .iter()
        .filter_map(notification_event)
        .filter(|e| selected(&e["type"]))
        .cloned()
        .collect();
    expect_eq(
        "SDK notifications retain the dynamic-tool events",
        &json!(subscribed),
        &json!(events),
    )?;
    let headers = of_type(&events, "request/header");
    expect(headers.len() == 3, || {
        format!("expected 3 request/header, got {}", headers.len())
    })?;
    let names: Vec<Vec<String>> = headers
        .iter()
        .map(|h| {
            h["data"]["header"]["tools"]
                .as_array()
                .into_iter()
                .flatten()
                // Recorded fixtures keep the bare names of the tools.
                .filter_map(|t| t["name"].as_str().or_else(|| t.as_str()).map(str::to_owned))
                .collect()
        })
        .collect();
    expect_eq(
        "snapshot_ping in the tool sets",
        &json!(
            names
                .iter()
                .map(|n| n.iter().any(|t| t == "snapshot_ping"))
                .collect::<Vec<_>>()
        ),
        &json!([false, true, false]),
    )?;
    let added: Vec<&String> = names[1].iter().filter(|n| !names[0].contains(n)).collect();
    expect_eq("added tools", &json!(added), &json!(["snapshot_ping"]))?;
    expect_eq("restored tools", &json!(names[2]), &json!(names[0]))?;
    let updates = of_type(&events, "developer/message");
    expect(updates.len() == 2, || {
        format!("expected 2 developer/message, got {}", updates.len())
    })?;
    expect_match(
        "tool-registry updates",
        &json!(updates),
        &json!([
            {"data": {"headerSeq": headers[1]["seq"], "message": {
                "role": "developer", "source": {"kind": "tool-registry"},
                "content": [{"type": "tool-addition", "toolName": "snapshot_ping"}]}},
             "surfaceOp": "append"},
            {"data": {"message": {
                "role": "developer", "source": {"kind": "tool-registry"},
                "content": [{"type": "tool-removal", "toolName": "snapshot_ping"}]}},
             "surfaceOp": "append"},
        ]),
    )?;
    expect(updates[1]["data"].get("headerSeq").is_none(), || {
        "the removal update carries a headerSeq".into()
    })?;
    expect_eq(
        "tool/call names",
        &json!(data_of(&events, "tool/call", "name")),
        &json!(["read", "snapshot_ping"]),
    )?;
    let last = of_type(&events, "tool/result")
        .last()
        .map_or(Value::Null, |r| (*r).clone());
    expect_match(
        "last tool/result",
        &last,
        &json!({"data": {"message": {"toolCallId": "call_dynamic_ping", "isError": false,
            "content": [{"type": "text", "text": "pong"}]}}}),
    )
}

/// sdk.snapshot.ts:1569–1587, over the durable log (`results[].events`). The final response of
/// the second call is the text of the last assistant message.
fn tool_scheduler_recovery(ctx: &BodyContext<'_>) -> Check {
    let events = parent(ctx, "tool-scheduler-recovery")?;
    let ends = of_type(&events, "turn/end");
    expect(ends.len() == 2, || {
        format!("expected 2 calls (turn/end), got {}", ends.len())
    })?;
    expect_eq(
        "final response",
        &json!(crate::compare::final_text(&events)),
        &json!("SCHEDULER_RECOVERY_OK"),
    )?;
    expect_eq(
        "turn/end reasons",
        &json!(data_of(&events, "turn/end", "reason")),
        &json!([
            {"kind": "error", "error": {"message": "Snapshot scheduler preparation failed", "code": "UNKNOWN"}},
            {"kind": "completed"},
        ]),
    )?;
    expect_eq(
        "tool/call ids",
        &json!(data_of(&events, "tool/call", "callId")),
        &json!(["scheduler-complete", "scheduler-fail"]),
    )?;
    let results: Vec<Value> = of_type(&events, "tool/result")
        .into_iter()
        .map(|r| {
            let m = &r["data"]["message"];
            json!([m["role"], m["toolCallId"], m["isError"]])
        })
        .collect();
    expect_eq(
        "tool/result",
        &json!(results),
        &json!([
            ["tool", "scheduler-complete", false],
            ["tool", "scheduler-fail", true],
            ["tool", "scheduler-unstarted", true],
        ]),
    )?;
    let todos = of_type(&events, "todo/write").len();
    expect(todos == 1, || format!("expected 1 todo/write, got {todos}"))
}

/// sdk.snapshot.ts:1588–1595.
fn subagent_activation_limit(ctx: &BodyContext<'_>) -> Check {
    expect(ctx.actual.len() == 2, || {
        format!("expected 2 Sessions, got {}", ctx.actual.len())
    })?;
    let events = parent(ctx, "subagent-activation-limit")?;
    let denied = events
        .iter()
        .find(|r| {
            r["type"] == "tool/result"
                && hoh_util_values::js_json_stringify(r).contains("call_over_capacity")
        })
        .cloned()
        .unwrap_or(Value::Null);
    expect(denied["data"]["message"]["isError"] == true, || {
        "the over-capacity tool result is not an error".into()
    })?;
    let text = text_of(&denied["data"]["message"]);
    expect(
        denied["data"]["message"]["content"]
            .as_array()
            .is_some_and(|c| c.len() == 1)
            && text.contains("subagent limit reached (active child limit: 1)"),
        || format!("over-capacity result text: {text:?}"),
    )
}

/// sdk.snapshot.ts:1707–1726, over the events the SDK delivered.
fn system_prompt_in_history(ctx: &BodyContext<'_>) -> Check {
    let events: Vec<&Value> = ctx
        .notifications
        .iter()
        .filter_map(notification_event)
        .collect();
    let systems: Vec<&&Value> = events
        .iter()
        .filter(|e| e["type"] == "system/message")
        .collect();
    expect(systems.len() == 2, || {
        format!("expected 2 system/message, got {}", systems.len())
    })?;
    expect_eq(
        "system/message surfaceOp",
        &json!(
            systems
                .iter()
                .map(|e| e["surfaceOp"].clone())
                .collect::<Vec<_>>()
        ),
        &json!(["append", "append"]),
    )?;
    let prompts: Vec<&str> = systems
        .iter()
        .map(|e| {
            e["data"]["message"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
        })
        .collect();
    let guidance = "Snapshot guidance added after the first read";
    expect(!prompts[0].contains(guidance), || {
        "the first prompt already holds the later guidance".into()
    })?;
    expect(
        prompts[1].contains(guidance) && prompts[1] != prompts[0],
        || "the second prompt lacks the later guidance".into(),
    )?;
    let second = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e["type"] == "system/message")
        .map(|(i, _)| i)
        .nth(1);
    let first_result = events.iter().position(|e| e["type"] == "tool/result");
    expect(second > first_result, || {
        "the second system/message precedes the first tool/result".into()
    })?;
    let headers = events
        .iter()
        .filter(|e| e["type"] == "request/header")
        .count();
    expect(headers == 1, || {
        format!("expected 1 request/header, got {headers}")
    })?;
    let contexts: Vec<Value> = events
        .iter()
        .filter(|e| e["type"] == "request/context")
        .map(|e| (*e).clone())
        .collect();
    expect_match(
        "request/context",
        &json!(contexts),
        &json!([{"data": {"systemPromptUpdate": "in-history"}}]),
    )?;
    expect(
        events
            .iter()
            .filter(|e| !e["surfaceOp"].is_null())
            .all(|e| e["surfaceOp"] == "append"),
        || "an event carries a surfaceOp other than append".into(),
    )
}

/// `runtimeContext` of the `desktop-context` entry of SDK_ASSERTIONS (sdk.snapshot.ts:1738–1753).
fn desktop_context(ctx: &BodyContext<'_>) -> Check {
    let events = parent(ctx, "desktop-context")?;
    let contexts: Vec<String> = events
        .iter()
        .filter(|r| r["type"] == "user/message" && r["data"]["source"]["kind"] == "runtime-context")
        .flat_map(|r| {
            r["data"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect();
    expect(contexts.len() == 1, || {
        format!("expected 1 runtime context, got {}", contexts.len())
    })?;
    let context = &contexts[0];
    let includes = ["Current working directory:"];
    let excludes = [
        "Current DeepSeek Harness implementation checkout:",
        "Current DeepSeek Harness Web GUI URL:",
    ];
    for clause in includes {
        expect(context.contains(clause), || {
            format!("runtime context lacks {clause:?}")
        })?;
    }
    for clause in excludes {
        expect(!context.contains(clause), || {
            format!("runtime context holds {clause:?}")
        })?;
    }
    let system = events
        .iter()
        .find(|r| r["type"] == "system/message")
        .and_then(|r| r["data"]["message"]["content"][0]["text"].as_str())
        .ok_or_else(|| fail("session log has no system/message text"))?;
    for clause in includes {
        expect(!system.contains(clause), || {
            format!("the system prompt holds the runtime clause {clause:?}")
        })?;
    }
    Ok(())
}

/// sdk.snapshot.ts:1766–1778: `serial-created` and the subagent notification invariants.
fn wire_shape(name: &str, ctx: &BodyContext<'_>) -> Check {
    let methods: Vec<&str> = ctx
        .notifications
        .iter()
        .filter_map(|n| n["method"].as_str())
        .collect();
    if name == "serial-created" {
        expect(!methods.contains(&"subagent.started"), || {
            "serial-created observed subagent.started".into()
        })?;
        let events = parent(ctx, name)?;
        expect_eq(
            "turn/end reasons",
            &json!(data_of(&events, "turn/end", "reason")),
            &json!([{"kind": "aborted", "reason": {"kind": "user"}}, {"kind": "completed"}]),
        )?;
    }
    if ctx.actual.len() > 1 {
        for method in ["subagent.started", "subagent.finished"] {
            expect(methods.contains(&method), || {
                format!("no {method} notification for a multi-Session scenario")
            })?;
        }
        let headers: Vec<Value> = ctx
            .actual
            .iter()
            .map(|log| rows(log).map(|r| r.first().cloned().unwrap_or(Value::Null)))
            .collect::<std::result::Result<_, _>>()?;
        for notification in ctx
            .notifications
            .iter()
            .filter(|n| n["method"] == "subagent.finished")
        {
            let child = headers
                .iter()
                .find(|h| h["id"] == notification["params"]["childSessionId"]);
            let parent_session = child.map_or(Value::Null, |h| h["parentSession"].clone());
            expect(
                notification["params"]["parentSessionId"] == parent_session,
                || {
                    format!(
                        "subagent.finished parent {} differs from the child header parent {parent_session}",
                        notification["params"]["parentSessionId"]
                    )
                },
            )?;
        }
    }
    Ok(())
}
