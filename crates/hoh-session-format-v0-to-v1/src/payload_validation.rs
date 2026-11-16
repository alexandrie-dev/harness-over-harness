use crate::validation_helpers::{event_label, event_type, field, has, keys, record};
use hoh_session_format::{
    SessionFormatError, SessionFormatEvent, session_format_count, session_format_safe_integer,
};
use hoh_util_time::{InstantResult, four_digit_year_epoch};
use hoh_util_values::deep_equal_json;
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};
use std::collections::HashSet;

type Result<T = ()> = std::result::Result<T, SessionFormatError>;
fn error(text: impl Into<String>) -> SessionFormatError {
    SessionFormatError::new(text)
}

pub fn assert_released_payload_semantics(event: &SessionFormatEvent, version: u64) -> Result {
    let kind = event_type(event);
    let seq = field(event, "seq").as_f64().unwrap_or(f64::NAN);
    let label = event_label(event);
    let data = record(field(event, "data"), &format!("{label} data"))?;
    // Each arm preserves the source's validation order, including optional fields.
    match kind {
        "agent-preset/selected" => {
            string_value(&data["agentPreset"], &format!("{label} agentPreset"))?
        }
        "agent/inbox/spliced" => {
            literal(
                &data["target"],
                &["next-turn", "next-step"],
                &format!("{label} target"),
            )?;
            count(&data["start"], &format!("{label} start"))?;
            optional(data, "removedCount", &label, count)?;
            array(&data["inserted"], &format!("{label} inserted"), |v, _| {
                message_value(
                    v,
                    &format!("{label} inserted message"),
                    version,
                    Some("user"),
                )
            })?;
            if has(data, "outcome") {
                literal(&data["outcome"], &["canceled"], &format!("{label} outcome"))?;
            }
        }
        "approval/asked" => {
            nonempty_fields(data, &["id", "toolName"], &label)?;
            optional(data, "callId", &label, non_empty_string)?;
            optional(data, "reason", &label, string_value)?;
        }
        "approval/decided" => {
            non_empty_string(&data["id"], &format!("{label} id"))?;
            literal(
                &data["outcome"],
                &["allowed-once", "rejected", "cancelled", "unavailable"],
                &format!("{label} outcome"),
            )?;
        }
        "approval/policy" => {
            literal(
                &data["policy"],
                &["ask", "never"],
                &format!("{label} policy"),
            )?;
            if has(data, "source") {
                literal(&data["source"], &["delegation"], &format!("{label} source"))?;
            }
        }
        "assistant/chunk" => {
            coordinates(data, &label)?;
            stream_chunk_value(&data["chunk"], &format!("{label} chunk"))?;
        }
        "assistant/message" => {
            coordinates(data, &label)?;
            message_value(
                &data["message"],
                &format!("{label} message"),
                version,
                Some("assistant"),
            )?;
            optional(data, "usage", &label, token_usage_value)?;
            optional(data, "interrupted", &label, true_value)?;
        }
        "command/done" => {
            non_empty_string(&data["commandId"], &format!("{label} commandId"))?;
            literal(
                &data["kind"],
                &["success", "error"],
                &format!("{label} kind"),
            )?;
            optional(data, "text", &label, string_value)?;
            if has(data, "sourceEventSeq") {
                earlier_seq(
                    &data["sourceEventSeq"],
                    seq,
                    &format!("{label} sourceEventSeq"),
                )?;
            }
        }
        "command/run" => {
            nonempty_fields(data, &["commandId", "name"], &label)?;
            optional(data, "args", &label, string_value)?;
            let source = exact(&data["source"], &format!("{label} source"), &["kind"], &[])?;
            literal(&source["kind"], &["user"], &format!("{label} source kind"))?;
        }
        "compaction/start" | "compaction/end" => {
            non_empty_string(&data["compactionId"], &format!("{label} compactionId"))?;
            optional(data, "sourceCommandId", &label, non_empty_string)?;
            if !has(data, "turn") || data["turn"] != Value::Null {
                count(&data["turn"], &format!("{label} turn"))?;
            }
            optional(data, "error", &label, string_value)?;
        }
        "compaction/prune" => shadowed_value(data, seq, &label)?,
        "compaction/summary" => {
            if data["llmStreamCall"] == true && !has(data, "rawOutput") {
                return Err(error(format!("{label} llmStreamCall requires rawOutput")));
            }
            non_empty_string(&data["compactionId"], &format!("{label} compactionId"))?;
            optional(data, "sourceCommandId", &label, non_empty_string)?;
            content_blocks_value(&data["summary"], &format!("{label} summary"), version)?;
            shadowed_value(data, seq, &label)?;
            nonempty_fields(data, &["provider", "model"], &label)?;
            optional(data, "maxTokens", &label, count)?;
            optional(data, "usage", &label, token_usage_value)?;
            if has(data, "rawOutput") {
                content_blocks_value(&data["rawOutput"], &format!("{label} rawOutput"), version)?;
            }
            optional(data, "llmStreamCall", &label, true_value)?;
        }
        "feedback/record" => non_empty_string(&data["text"], &format!("{label} text"))?,
        "goal/change" => goal_change_value(data, &label)?,
        "hook/invoked" => {
            count(&data["turn"], &format!("{label} turn"))?;
            non_empty_string(&data["point"], &format!("{label} point"))?;
            literal(
                &data["dialect"],
                &["claude-code", "codex"],
                &format!("{label} dialect"),
            )?;
            optional(data, "matcher", &label, string_value)?;
            non_empty_string(&data["handlerId"], &format!("{label} handlerId"))?;
        }
        "hook/result" => {
            count(&data["turn"], &format!("{label} turn"))?;
            nonempty_fields(data, &["point", "handlerId", "decision"], &label)?;
            optional(data, "exitCode", &label, safe_integer)?;
            optional(data, "stderrSummary", &label, string_value)?;
            if finite_number(&data["durationMs"], &format!("{label} durationMs"))? < 0.0 {
                return Err(error(format!("{label} durationMs must be non-negative")));
            }
        }
        "llm/retry" => {
            non_empty_string(&data["retryId"], &format!("{label} retryId"))?;
            coordinates(data, &label)?;
            non_empty_string(&data["provider"], &format!("{label} provider"))?;
            literal(
                &data["mode"],
                &["normal", "always"],
                &format!("{label} mode"),
            )?;
            non_empty_string(&data["policyKey"], &format!("{label} policyKey"))?;
            let retry = positive(&data["retry"], &format!("{label} retry"))?;
            if data["mode"] == "normal" {
                let max = positive(&data["maxRetries"], &format!("{label} maxRetries"))?;
                if retry > max {
                    return Err(error(format!("{label} retry exceeds maxRetries")));
                }
            } else if has(data, "maxRetries") {
                return Err(error(format!("{label} always mode must omit maxRetries")));
            }
            let delay = finite_number(&data["delayMs"], &format!("{label} delayMs"))?;
            if delay < 0.0 {
                return Err(error(format!("{label} delayMs must be non-negative")));
            }
            if delay > 2_147_483_647.0 {
                return Err(error(format!("{label} delayMs exceeds the timer range")));
            }
            llm_failure_value(&data["failure"], &format!("{label} failure"))?;
        }
        "llm/retry-started" => {
            non_empty_string(&data["retryId"], &format!("{label} retryId"))?;
            coordinates(data, &label)?;
            positive(&data["retry"], &format!("{label} retry"))?;
        }
        "model/selection" => {
            nonempty_fields(data, &["provider", "model"], &label)?;
            optional(data, "reasoningEffort", &label, non_empty_string)?;
        }
        "permission/preset" => non_empty_string(&data["preset"], &format!("{label} preset"))?,
        "plan/mode" => boolean_value(&data["active"], &format!("{label} active"))?,
        "request/context" => {
            nonempty_fields(data, &["provider", "model"], &label)?;
            optional(data, "contextWindow", &label, positive)?;
        }
        "request/header" => {
            request_header_value(&data["header"], &format!("{label} header"))?;
            literal(
                &data["reason"],
                &["initial", "resume", "change", "series"],
                &format!("{label} reason"),
            )?;
            optional(data, "startsSeries", &label, true_value)?;
        }
        "sandbox/mode" => {
            literal(
                &data["mode"],
                &["read-only", "workspace-write", "danger-full-access"],
                &format!("{label} mode"),
            )?;
            if has(data, "source") {
                literal(&data["source"], &["delegation"], &format!("{label} source"))?;
            }
        }
        "schedule/change" => schedule_change_value(data, &label)?,
        "session-log-deepseek/delivery-accepted" => {
            let accepted = if has(data, "sessionFormatVersion") {
                count(
                    &data["sessionFormatVersion"],
                    &format!("{label} sessionFormatVersion"),
                )?
            } else {
                0
            };
            if accepted != version {
                return Ok(());
            }
            non_empty_string(&data["sessionId"], &format!("{label} sessionId"))?;
            earlier_seq(&data["throughSeq"], seq, &format!("{label} throughSeq"))?;
        }
        "session/end-seed" => {}
        "session/title" => {
            non_empty_string(&data["title"], &format!("{label} title"))?;
            seq_array(
                &data["messageSeqs"],
                seq,
                &format!("{label} messageSeqs"),
                false,
            )?;
            title_source_value(&data["source"], &format!("{label} source"))?;
        }
        "session/title-llm-request" => {
            non_empty_string(&data["titleProvider"], &format!("{label} titleProvider"))?;
            seq_array(
                &data["messageSeqs"],
                seq,
                &format!("{label} messageSeqs"),
                true,
            )?;
            model_route_value(&data["route"], &format!("{label} route"))?;
            string_value(&data["system"], &format!("{label} system"))?;
            array(&data["messages"], &format!("{label} messages"), |v, _| {
                message_value(v, &format!("{label} message"), version, None)
            })?;
            positive(&data["maxTokens"], &format!("{label} maxTokens"))?;
        }
        "step/end" | "step/start" => coordinates(data, &label)?,
        "subagent/descriptor" => subagent_descriptor_value(data, &label)?,
        "subagent/model-selection-policy" => {
            allowed_models_value(&data["allowedModels"], &format!("{label} allowedModels"))?
        }
        "team/member" => {
            team_selector(data, &label)?;
            team_member_value(&data["member"], &format!("{label} member"))?;
        }
        "team/message/delivered" => {
            team_selector(data, &label)?;
            nonempty_fields(data, &["messageId", "targetId"], &label)?;
        }
        "team/message/queued" => {
            team_selector(data, &label)?;
            team_message_value(&data["message"], &format!("{label} message"), version)?;
        }
        "team/task" => {
            team_selector(data, &label)?;
            team_task_value(&data["task"], &format!("{label} task"))?;
        }
        "todo/write" => {
            array(&data["todos"], &format!("{label} todos"), |v, l| {
                let item = exact(v, l, &["content", "status"], &[])?;
                string_value(&item["content"], &format!("{l} content"))?;
                literal(
                    &item["status"],
                    &["pending", "in_progress", "completed"],
                    &format!("{l} status"),
                )
            })?;
        }
        "tool-workflow/agent-end" => {
            workflow_identity(data, &label)?;
            literal(
                &data["outcome"],
                &["completed", "failed", "cancelled"],
                &format!("{label} outcome"),
            )?;
        }
        "tool-workflow/agent-start" => {
            workflow_identity(data, &label)?;
            string_value(&data["label"], &format!("{label} label"))?;
            optional(data, "phase", &label, string_value)?;
            non_empty_string(&data["childId"], &format!("{label} childId"))?;
        }
        "tool-workflow/run-end" => {
            non_empty_string(&data["runId"], &format!("{label} runId"))?;
            literal(
                &data["stopReason"],
                &["completed", "cancelled", "error"],
                &format!("{label} stopReason"),
            )?;
        }
        "tool-workflow/run-start" => nonempty_fields(data, &["runId", "name"], &label)?,
        "tool/call" => {
            coordinates(data, &label)?;
            nonempty_fields(data, &["callId", "name"], &label)?;
            string_value(&data["arguments"], &format!("{label} arguments"))?;
        }
        "tool/code-dispatch" | "tool/code-dispatch-start" => {
            nonempty_fields(
                data,
                &["rootCallId", "parentCallId", "subCallId", "name"],
                &label,
            )?;
            if kind == "tool/code-dispatch" {
                boolean_value(&data["isError"], &format!("{label} isError"))?;
                content_blocks_value(&data["content"], &format!("{label} content"), version)?;
            }
        }
        "tool/result" => {
            coordinates(data, &label)?;
            message_value(
                &data["message"],
                &format!("{label} message"),
                version,
                Some("tool"),
            )?;
            if has(data, "error") {
                let e = exact(
                    &data["error"],
                    &format!("{label} error"),
                    &["name", "code"],
                    &[],
                )?;
                nonempty_fields(e, &["name", "code"], &format!("{label} error"))?;
            }
        }
        "turn/end" => {
            count(&data["turn"], &format!("{label} turn"))?;
            turn_end_reason_value(&data["reason"], &format!("{label} reason"))?;
        }
        "turn/start" => {
            count(&data["turn"], &format!("{label} turn"))?;
        }
        "user/message" => message_value(data, &label, version, Some("user"))?,
        "web/deepseek-search-llm-request" => {
            nonempty_fields(data, &["endpoint", "apiVersion"], &label)?;
            deep_seek_search_body_value(&data["body"], &format!("{label} body"))?;
        }
        _ => {
            return Err(error(format!(
                "released payload validator is missing event {}",
                event
                    .get("type")
                    .map(js_json_stringify)
                    .unwrap_or_else(|| "undefined".into())
            )));
        }
    }
    Ok(())
}

fn exact<'a>(
    value: &'a Value,
    label: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Value> {
    keys(value, required, optional, label)?;
    Ok(value)
}
fn string_value(value: &Value, label: &str) -> Result {
    if !value.is_string() {
        return Err(error(format!("{label} must be a string")));
    }
    Ok(())
}
fn non_empty_string(value: &Value, label: &str) -> Result {
    if !value.as_str().is_some_and(|s| !s.is_empty()) {
        return Err(error(format!("{label} must be a non-empty string")));
    }
    Ok(())
}
fn boolean_value(value: &Value, label: &str) -> Result {
    if !value.is_boolean() {
        return Err(error(format!("{label} must be a boolean")));
    }
    Ok(())
}
fn count(value: &Value, label: &str) -> Result<u64> {
    session_format_count(value, label)
}
fn safe_integer(value: &Value, label: &str) -> Result<i64> {
    session_format_safe_integer(value, label)
}
fn positive(value: &Value, label: &str) -> Result<u64> {
    let n = count(value, label)?;
    if n == 0 {
        return Err(error(format!("{label} must be positive")));
    }
    Ok(n)
}
fn finite_number(value: &Value, label: &str) -> Result<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && !(*v == 0.0 && v.is_sign_negative()))
        .ok_or_else(|| error(format!("{label} must be a finite number")))
}
fn literal(value: &Value, allowed: &[&str], label: &str) -> Result {
    if !value.as_str().is_some_and(|s| allowed.contains(&s)) {
        return Err(error(format!(
            "{label} must be one of {}",
            allowed.join(", ")
        )));
    }
    Ok(())
}
fn number_literal(value: &Value, allowed: &[u64], label: &str) -> Result {
    if !allowed
        .iter()
        .any(|n| crate::validation::number_eq(value, *n))
    {
        return Err(error(format!(
            "{label} must be one of {}",
            allowed
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}
fn true_value(value: &Value, label: &str) -> Result {
    if value != true {
        return Err(error(format!("{label} must be one of true")));
    }
    Ok(())
}
fn optional<T>(
    data: &Value,
    key: &str,
    label: &str,
    validate: impl FnOnce(&Value, &str) -> Result<T>,
) -> Result {
    if has(data, key) {
        validate(&data[key], &format!("{label} {key}"))?;
    }
    Ok(())
}
fn nonempty_fields(data: &Value, names: &[&str], label: &str) -> Result {
    for key in names {
        non_empty_string(&data[*key], &format!("{label} {key}"))?;
    }
    Ok(())
}
fn array<'a>(
    value: &'a Value,
    label: &str,
    mut validate: impl FnMut(&Value, &str) -> Result,
) -> Result<&'a [Value]> {
    let members = value
        .as_array()
        .ok_or_else(|| error(format!("{label} must be an array")))?;
    for (i, member) in members.iter().enumerate() {
        validate(member, &format!("{label}[{i}]"))?;
    }
    Ok(members)
}
fn coordinates(data: &Value, label: &str) -> Result {
    count(&data["turn"], &format!("{label} turn"))?;
    count(&data["step"], &format!("{label} step"))?;
    Ok(())
}
fn earlier_seq(value: &Value, event_seq: f64, label: &str) -> Result<u64> {
    let n = count(value, label)?;
    if n as f64 >= event_seq {
        return Err(error(format!("{label} must identify an earlier event")));
    }
    Ok(n)
}
fn seq_array<'a>(
    value: &'a Value,
    event_seq: f64,
    label: &str,
    require_nonempty: bool,
) -> Result<&'a [Value]> {
    let mut seen = HashSet::new();
    let values = array(value, label, |v, l| {
        let n = earlier_seq(v, event_seq, l)?;
        if !seen.insert(n) {
            return Err(error(format!("{label} repeats seq {n}")));
        }
        Ok(())
    })?;
    if require_nonempty && values.is_empty() {
        return Err(error(format!("{label} must be non-empty")));
    }
    Ok(values)
}
fn llm_failure_value(value: &Value, label: &str) -> Result {
    let failure = exact(
        value,
        label,
        &["message", "code"],
        &["status", "providerRetryAfterMs", "requestId"],
    )?;
    nonempty_fields(failure, &["message", "code"], label)?;
    if has(failure, "status") {
        let s = safe_integer(&failure["status"], &format!("{label} status"))?;
        if !(100..=599).contains(&s) {
            return Err(error(format!("{label} status must be 100 through 599")));
        }
    }
    if has(failure, "providerRetryAfterMs")
        && finite_number(
            &failure["providerRetryAfterMs"],
            &format!("{label} providerRetryAfterMs"),
        )? <= 0.0
    {
        return Err(error(format!(
            "{label} providerRetryAfterMs must be positive"
        )));
    }
    optional(failure, "requestId", label, non_empty_string)
}
fn token_usage_value(value: &Value, label: &str) -> Result {
    let usage = exact(
        value,
        label,
        &["inputTokens", "outputTokens"],
        &[
            "totalTokens",
            "cacheReadTokens",
            "cacheWriteTokens",
            "reasoningTokens",
        ],
    )?;
    if let Value::Object(object) = usage {
        for (key, value) in object {
            count(value, &format!("{label} {key}"))?;
        }
    }
    Ok(())
}
fn content_blocks_value(value: &Value, label: &str, version: u64) -> Result {
    array(value, label, |v, l| content_block_value(v, l, version))?;
    Ok(())
}
fn content_block_value(value: &Value, label: &str, version: u64) -> Result {
    let block = record(value, label)?;
    match block["type"].as_str().unwrap_or("") {
        "text" | "reasoning" => {
            keys(block, &["type", "text"], &[], label)?;
            string_value(&block["text"], &format!("{label} text"))?;
        }
        "image" => {
            keys(block, &["type", "attachment"], &[], label)?;
            image_attachment_value(&block["attachment"], &format!("{label} attachment"))?;
        }
        "tool-call" => {
            keys(block, &["type", "id", "name", "arguments"], &[], label)?;
            nonempty_fields(block, &["id", "name"], label)?;
            string_value(&block["arguments"], &format!("{label} arguments"))?;
        }
        "tool-result" => {
            keys(
                block,
                &["type", "toolCallId", "content"],
                &["isError"],
                label,
            )?;
            non_empty_string(&block["toolCallId"], &format!("{label} toolCallId"))?;
            content_blocks_value(&block["content"], &format!("{label} content"), version)?;
            optional(block, "isError", label, boolean_value)?;
        }
        _ => non_empty_string(&block["type"], &format!("{label} type"))?,
    }
    Ok(())
}
fn image_attachment_value(value: &Value, label: &str) -> Result {
    let a = exact(
        value,
        label,
        &["attachmentId", "mediaType", "bytes", "width", "height"],
        &["name", "originalDimensions"],
    )?;
    non_empty_string(&a["attachmentId"], &format!("{label} attachmentId"))?;
    literal(
        &a["mediaType"],
        &["image/png", "image/jpeg", "image/webp", "image/gif"],
        &format!("{label} mediaType"),
    )?;
    count(&a["bytes"], &format!("{label} bytes"))?;
    positive(&a["width"], &format!("{label} width"))?;
    positive(&a["height"], &format!("{label} height"))?;
    optional(a, "name", label, string_value)?;
    if has(a, "originalDimensions") {
        let d = exact(
            &a["originalDimensions"],
            &format!("{label} originalDimensions"),
            &["width", "height"],
            &[],
        )?;
        positive(&d["width"], &format!("{label} original width"))?;
        positive(&d["height"], &format!("{label} original height"))?;
    }
    Ok(())
}
fn message_value(value: &Value, label: &str, version: u64, expected: Option<&str>) -> Result {
    let message = exact(value, label, &["id", "role", "content", "source"], &[])?;
    non_empty_string(&message["id"], &format!("{label} id"))?;
    let roles: &[&str] = match expected {
        Some("assistant") => &["assistant"],
        Some("user" | "tool") => &["user"],
        _ => &["system", "user", "assistant"],
    };
    literal(&message["role"], roles, &format!("{label} role"))?;
    content_blocks_value(&message["content"], &format!("{label} content"), version)?;
    let source = record(&message["source"], &format!("{label} source"))?;
    if version < 2 && expected == Some("user") && source["kind"] == "goal" && has(source, "change")
    {
        legacy_goal_message_value(message, source, label)?;
    } else {
        message_source_value(source, &format!("{label} source"), version, expected)?;
    }
    if expected == Some("tool") {
        let content = message["content"].as_array();
        let block = if let Some(c) = content.filter(|c| c.len() == 1) {
            Some(record(&c[0], &format!("{label} tool result"))?)
        } else {
            None
        };
        let source = record(&message["source"], &format!("{label} source"))?;
        if !block.is_some_and(|b| b["type"] == "tool-result" && b["toolCallId"] == source["callId"])
        {
            return Err(error(format!(
                "{label} must contain exactly one tool-result block"
            )));
        }
    }
    Ok(())
}
fn legacy_goal_message_value(message: &Value, source: &Value, label: &str) -> Result {
    keys(
        source,
        &["kind", "goalId", "revision", "round", "change"],
        &[],
        &format!("{label} source"),
    )?;
    non_empty_string(&source["goalId"], &format!("{label} source goalId"))?;
    positive(&source["revision"], &format!("{label} source revision"))?;
    if !crate::validation::number_eq(&source["round"], 0) {
        return Err(error(format!("{label} legacy goal source round must be 0")));
    }
    let change = record(&source["change"], &format!("{label} source change"))?;
    goal_change_value(change, &format!("{label} source change"))?;
    let reference = record(
        if change["operation"] == "clear" {
            &change["cleared"]
        } else {
            &change["goal"]
        },
        &format!("{label} source change ref"),
    )?;
    if source["goalId"] != reference["id"]
        || !deep_equal_json(&source["revision"], &reference["revision"])
    {
        return Err(error(format!(
            "{label} legacy goal source does not match its change"
        )));
    }
    let payload = if change["operation"] == "clear" {
        json!({"cleared":change["cleared"],"clearedAt":change["clearedAt"]})
    } else {
        json!({"goal":change["goal"],"roundsStarted":change["roundsStarted"],"createdAt":change["createdAt"],"updatedAt":change["updatedAt"]})
    };
    let expected = json!([{"type":"text","text":format!("<goal_state>{}</goal_state>",js_json_stringify(&payload))}]);
    if !deep_equal_json(&message["content"], &expected) {
        return Err(error(format!(
            "{label} legacy goal content does not match its change"
        )));
    }
    Ok(())
}

fn message_source_value(
    value: &Value,
    label: &str,
    version: u64,
    expected: Option<&str>,
) -> Result {
    let s = record(value, label)?;
    if expected == Some("assistant") && s["kind"] != "model" {
        return Err(error(format!("{label} must be model source")));
    }
    if expected == Some("tool") && s["kind"] != "tool" {
        return Err(error(format!("{label} must be tool source")));
    }
    match s["kind"].as_str().unwrap_or("") {
        "user" => {
            keys(s, &["kind"], &["rpcId", "clientTimeZone"], label)?;
            optional(s, "rpcId", label, non_empty_string)?;
            optional(s, "clientTimeZone", label, non_empty_string)?;
        }
        "plugin" => plugin_source_value(s, label)?,
        "model" => {
            keys(s, &["kind", "provider", "model"], &["replayState"], label)?;
            nonempty_fields(s, &["provider", "model"], label)?;
        }
        "tool" => {
            keys(s, &["kind", "callId"], &[], label)?;
            non_empty_string(&s["callId"], &format!("{label} callId"))?;
        }
        "agent-instructions" => {
            keys(
                s,
                &["kind", "form", "changes"],
                &["baseline", "baselineIdentity"],
                label,
            )?;
            literal(&s["form"], &["instructions"], &format!("{label} form"))?;
            optional(s, "baseline", label, true_value)?;
            optional(s, "baselineIdentity", label, non_empty_string)?;
            array(&s["changes"], &format!("{label} changes"), |v, l| {
                let c = exact(v, l, &["action", "scope", "path"], &["digest"])?;
                literal(
                    &c["action"],
                    &["set", "replace", "remove"],
                    &format!("{l} action"),
                )?;
                string_value(&c["scope"], &format!("{l} scope"))?;
                string_value(&c["path"], &format!("{l} path"))?;
                optional(c, "digest", l, string_value)
            })?;
        }
        "session-reference" => session_reference_source_value(s, label, version)?,
        "team-message" => {
            keys(
                s,
                &["kind", "teamId", "messageId", "senderId", "senderName"],
                &[],
                label,
            )?;
            nonempty_fields(s, &["teamId", "messageId", "senderId"], label)?;
            string_value(&s["senderName"], &format!("{label} senderName"))?;
        }
        "goal" => {
            keys(s, &["kind", "goalId", "revision", "round"], &[], label)?;
            non_empty_string(&s["goalId"], &format!("{label} goalId"))?;
            positive(&s["revision"], &format!("{label} revision"))?;
            positive(&s["round"], &format!("{label} round"))?;
        }
        "skill-invocation" => {
            keys(s, &["kind", "name", "form"], &[], label)?;
            non_empty_string(&s["name"], &format!("{label} name"))?;
            literal(&s["form"], &["instructions"], &format!("{label} form"))?;
        }
        "skill-catalog" => {
            keys(s, &["kind", "form", "entries"], &["update"], label)?;
            literal(&s["form"], &["catalog"], &format!("{label} form"))?;
            optional(s, "update", label, true_value)?;
            array(&s["entries"], &format!("{label} entries"), |v, l| {
                let e = exact(v, l, &["name", "description"], &[])?;
                non_empty_string(&e["name"], &format!("{l} name"))?;
                string_value(&e["description"], &format!("{l} description"))
            })?;
        }
        "coordinator" | "subagent-report" => {
            keys(s, &["kind", "form", "senderSessionId"], &[], label)?;
            literal(&s["form"], &["relay"], &format!("{label} form"))?;
            non_empty_string(&s["senderSessionId"], &format!("{label} senderSessionId"))?;
        }
        "subagent-settled" => {
            keys(
                s,
                &["kind", "form", "summary", "senderSessionId"],
                &[],
                label,
            )?;
            literal(&s["form"], &["notice"], &format!("{label} form"))?;
            string_value(&s["summary"], &format!("{label} summary"))?;
            non_empty_string(&s["senderSessionId"], &format!("{label} senderSessionId"))?;
        }
        "webhook" => {
            keys(
                s,
                &[
                    "kind",
                    "provider",
                    "source",
                    "deliveryId",
                    "ruleId",
                    "form",
                    "summary",
                ],
                &[],
                label,
            )?;
            nonempty_fields(s, &["provider", "source", "deliveryId", "ruleId"], label)?;
            literal(&s["form"], &["notice"], &format!("{label} form"))?;
            string_value(&s["summary"], &format!("{label} summary"))?;
        }
        _ => non_empty_string(&s["kind"], &format!("{label} kind"))?,
    }
    Ok(())
}
fn plugin_source_value(s: &Value, label: &str) -> Result {
    let mut optional_keys = vec!["form", "sections", "summary"];
    if s["plugin"] == "compact" {
        optional_keys.extend(["compactionId", "sourceCommandId"]);
    }
    keys(s, &["kind", "plugin"], &optional_keys, label)?;
    non_empty_string(&s["plugin"], &format!("{label} plugin"))?;
    if s["plugin"] == "compact" {
        non_empty_string(&s["compactionId"], &format!("{label} compactionId"))?;
        optional(s, "sourceCommandId", label, non_empty_string)?;
    }
    if !has(s, "form") {
        return Ok(());
    }
    literal(
        &s["form"],
        &[
            "instructions",
            "catalog",
            "snapshot",
            "notice",
            "relay",
            "recall",
        ],
        &format!("{label} form"),
    )?;
    if s["form"] == "snapshot" {
        array(&s["sections"], &format!("{label} sections"), |v, l| {
            let section = exact(v, l, &["name", "text"], &[])?;
            non_empty_string(&section["name"], &format!("{l} name"))?;
            string_value(&section["text"], &format!("{l} text"))
        })?;
    } else if has(s, "sections") {
        return Err(error(format!("{label} sections require snapshot form")));
    }
    if s["form"] == "notice" {
        string_value(&s["summary"], &format!("{label} summary"))?;
    } else if has(s, "summary") {
        return Err(error(format!("{label} summary requires notice form")));
    }
    Ok(())
}
fn session_reference_source_value(s: &Value, label: &str, version: u64) -> Result {
    keys(s, &["kind", "form", "version", "references"], &[], label)?;
    literal(&s["form"], &["recall"], &format!("{label} form"))?;
    number_literal(&s["version"], &[1], &format!("{label} version"))?;
    let mut expected_index = 0;
    let mut session_ids = HashSet::new();
    let refs = array(&s["references"], &format!("{label} references"), |v, l| {
        let r = exact(
            v,
            l,
            &[
                "sessionId",
                "label",
                "capturedThroughSeq",
                "compacted",
                "originalMessages",
                "retainedMessages",
                "omittedMessages",
                "omittedBytes",
                "truncated",
                "inputIndex",
            ],
            if version >= 1 {
                &["capturedFormatVersion"]
            } else {
                &[]
            },
        )?;
        non_empty_string(&r["sessionId"], &format!("{l} sessionId"))?;
        string_value(&r["label"], &format!("{l} label"))?;
        if r["capturedThroughSeq"] != Value::Null {
            count(&r["capturedThroughSeq"], &format!("{l} capturedThroughSeq"))?;
        }
        if has(r, "capturedFormatVersion") {
            let captured = count(
                &r["capturedFormatVersion"],
                &format!("{l} capturedFormatVersion"),
            )?;
            if captured < 1 || captured > version {
                return Err(error(format!(
                    "{l} capturedFormatVersion must be between 1 and {version}"
                )));
            }
        }
        boolean_value(&r["compacted"], &format!("{l} compacted"))?;
        let original = count(&r["originalMessages"], &format!("{l} originalMessages"))?;
        let retained = count(&r["retainedMessages"], &format!("{l} retainedMessages"))?;
        let omitted = count(&r["omittedMessages"], &format!("{l} omittedMessages"))?;
        let bytes = count(&r["omittedBytes"], &format!("{l} omittedBytes"))?;
        let input = count(&r["inputIndex"], &format!("{l} inputIndex"))?;
        boolean_value(&r["truncated"], &format!("{l} truncated"))?;
        if retained > original || omitted != original - retained {
            return Err(error(format!("{l} message counts are inconsistent")));
        }
        if r["truncated"] != (omitted > 0 || bytes > 0) {
            return Err(error(format!(
                "{l} truncated disagrees with omitted content"
            )));
        }
        if input != expected_index {
            return Err(error(format!(
                "{label} inputIndex must match reference position"
            )));
        }
        expected_index += 1;
        let id = r["sessionId"].as_str().unwrap_or("");
        if !session_ids.insert(id.to_owned()) {
            return Err(error(format!("{label} repeats sessionId {id}")));
        }
        Ok(())
    })?;
    if refs.is_empty() {
        return Err(error(format!("{label} references must be non-empty")));
    }
    Ok(())
}
fn stream_chunk_value(value: &Value, label: &str) -> Result {
    let c = record(value, label)?;
    match c["type"].as_str().unwrap_or("") {
        "block-start" => {
            keys(c, &["type", "index", "blockType"], &[], label)?;
            count(&c["index"], &format!("{label} index"))?;
            non_empty_string(&c["blockType"], &format!("{label} blockType"))?;
        }
        "text-delta" | "reasoning-delta" => {
            keys(c, &["type", "index", "text"], &[], label)?;
            count(&c["index"], &format!("{label} index"))?;
            string_value(&c["text"], &format!("{label} text"))?;
        }
        "tool-call-delta" => {
            keys(
                c,
                &["type", "index", "id", "argumentsDelta"],
                &["name"],
                label,
            )?;
            count(&c["index"], &format!("{label} index"))?;
            non_empty_string(&c["id"], &format!("{label} id"))?;
            optional(c, "name", label, string_value)?;
            string_value(&c["argumentsDelta"], &format!("{label} argumentsDelta"))?;
        }
        "block-end" => {
            keys(c, &["type", "index", "block"], &[], label)?;
            count(&c["index"], &format!("{label} index"))?;
            content_block_value(&c["block"], &format!("{label} block"), 1)?;
        }
        "usage" => {
            keys(c, &["type", "usage"], &[], label)?;
            token_usage_value(&c["usage"], &format!("{label} usage"))?;
        }
        "finish" => {
            keys(c, &["type", "reason"], &["replayState"], label)?;
            finish_reason_value(&c["reason"], &format!("{label} reason"))?;
            if has(c, "replayState") {
                let r = exact(
                    &c["replayState"],
                    &format!("{label} replayState"),
                    &["response"],
                    &["blocks"],
                )?;
                if has(r, "blocks") && !r["blocks"].is_array() {
                    return Err(error(format!(
                        "{label} replayState blocks must be an array"
                    )));
                }
            }
        }
        _ => {
            return Err(error(format!(
                "{label} has unknown stream chunk type {}",
                c.get("type")
                    .map(js_json_stringify)
                    .unwrap_or_else(|| "undefined".into())
            )));
        }
    }
    Ok(())
}
fn finish_reason_value(value: &Value, label: &str) -> Result {
    let r = record(value, label)?;
    if r["kind"] == "aborted" || r["kind"] == "error" {
        keys(r, &["kind", "failure"], &[], label)?;
        return llm_failure_value(&r["failure"], &format!("{label} failure"));
    }
    if matches!(
        r["kind"].as_str(),
        Some("stop" | "tool-calls" | "max-tokens")
    ) {
        keys(r, &["kind"], &[], label)?;
    }
    non_empty_string(&r["kind"], &format!("{label} kind"))
}
fn turn_end_reason_value(value: &Value, label: &str) -> Result {
    let r = record(value, label)?;
    match r["kind"].as_str().unwrap_or("") {
        "completed" | "blocked" | "max-tokens" | "interrupted" => keys(r, &["kind"], &[], label)?,
        "aborted" => {
            keys(r, &["kind", "reason"], &[], label)?;
            let c = record(&r["reason"], &format!("{label} abort cause"))?;
            if c["kind"] == "hook" {
                keys(c, &["kind", "reason"], &[], &format!("{label} abort cause"))?;
                string_value(&c["reason"], &format!("{label} abort reason"))?;
            } else {
                keys(c, &["kind"], &[], &format!("{label} abort cause"))?;
                literal(
                    &c["kind"],
                    &["user", "parent", "disposed", "legacy"],
                    &format!("{label} abort kind"),
                )?;
            }
        }
        "error" => {
            keys(r, &["kind", "error"], &[], label)?;
            llm_failure_value(&r["error"], &format!("{label} error"))?;
        }
        _ => non_empty_string(&r["kind"], &format!("{label} kind"))?,
    }
    Ok(())
}
fn request_header_value(value: &Value, label: &str) -> Result {
    let h = exact(
        value,
        label,
        &["config"],
        &["adapterDefaults", "system", "tools"],
    )?;
    let c = exact(
        &h["config"],
        &format!("{label} config"),
        &["provider", "model"],
        &["reasoningEffort", "temperature", "maxTokens", "stop"],
    )?;
    nonempty_fields(c, &["provider", "model"], label)?;
    optional(c, "reasoningEffort", label, non_empty_string)?;
    optional(c, "temperature", label, finite_number)?;
    optional(c, "maxTokens", label, positive)?;
    if has(c, "stop") {
        array(&c["stop"], &format!("{label} stop"), string_value)?;
    }
    if has(h, "adapterDefaults") {
        let d = exact(
            &h["adapterDefaults"],
            &format!("{label} adapterDefaults"),
            &[],
            &["reasoningEffort", "maxTokens"],
        )?;
        if let Value::Object(object) = d {
            for (key, marker) in object {
                true_value(marker, &format!("{label} adapterDefaults {key}"))?;
                if !has(c, key) {
                    return Err(error(format!(
                        "{label} adapter default {key} lacks config value"
                    )));
                }
            }
        }
    }
    optional(h, "system", label, string_value)?;
    if has(h, "tools") {
        array(&h["tools"], &format!("{label} tools"), |v, l| {
            let s = exact(v, l, &["name", "description", "parameters"], &[])?;
            non_empty_string(&s["name"], &format!("{l} name"))?;
            string_value(&s["description"], &format!("{l} description"))?;
            record(&s["parameters"], &format!("{l} parameters"))?;
            Ok(())
        })?;
    }
    Ok(())
}
fn shadowed_value(data: &Value, seq: f64, label: &str) -> Result {
    let r = exact(
        &data["shadowedRange"],
        &format!("{label} shadowedRange"),
        &["start", "end"],
        &[],
    )?;
    let start = earlier_seq(&r["start"], seq, &format!("{label} shadowedRange start"))?;
    let end = earlier_seq(&r["end"], seq, &format!("{label} shadowedRange end"))?;
    let seqs = seq_array(
        &data["shadowedSeqs"],
        seq,
        &format!("{label} shadowedSeqs"),
        true,
    )?;
    if !seqs
        .first()
        .is_some_and(|v| crate::validation::number_eq(v, start))
        || !seqs
            .last()
            .is_some_and(|v| crate::validation::number_eq(v, end))
    {
        return Err(error(format!(
            "{label} shadowedRange must match shadowedSeqs endpoints"
        )));
    }
    count(
        &data["shadowedTokenCount"],
        &format!("{label} shadowedTokenCount"),
    )?;
    Ok(())
}
fn goal_change_value(data: &Value, label: &str) -> Result {
    literal(&data["kind"], &["goal/change"], &format!("{label} kind"))?;
    number_literal(&data["version"], &[1], &format!("{label} version"))?;
    if data["operation"] == "clear" {
        keys(
            data,
            &["kind", "version", "operation", "cleared", "clearedAt"],
            &[],
            &format!("{label} data"),
        )?;
        let r = exact(
            &data["cleared"],
            &format!("{label} cleared"),
            &["id", "revision"],
            &[],
        )?;
        non_empty_string(&r["id"], &format!("{label} cleared id"))?;
        positive(&r["revision"], &format!("{label} cleared revision"))?;
        count(&data["clearedAt"], &format!("{label} clearedAt"))?;
        return Ok(());
    }
    keys(
        data,
        &[
            "kind",
            "version",
            "operation",
            "goal",
            "roundsStarted",
            "createdAt",
            "updatedAt",
        ],
        &[],
        &format!("{label} data"),
    )?;
    literal(
        &data["operation"],
        &["create", "edit", "pause", "resume", "complete", "block"],
        &format!("{label} operation"),
    )?;
    let l = format!("{label} goal");
    let g = exact(
        &data["goal"],
        &l,
        &["id", "revision", "objective", "phase", "maxGoalRounds"],
        &["blockedReason"],
    )?;
    non_empty_string(&g["id"], &format!("{l} id"))?;
    positive(&g["revision"], &format!("{l} revision"))?;
    non_empty_string(&g["objective"], &format!("{l} objective"))?;
    literal(
        &g["phase"],
        &["active", "paused", "blocked", "complete"],
        &format!("{l} phase"),
    )?;
    positive(&g["maxGoalRounds"], &format!("{l} maxGoalRounds"))?;
    if g["phase"] == "blocked" {
        let r = exact(
            &g["blockedReason"],
            &format!("{l} blockedReason"),
            &["code", "message"],
            &[],
        )?;
        non_empty_string(&r["code"], &format!("{l} blocked code"))?;
        non_empty_string(&r["message"], &format!("{l} blocked message"))?;
    } else if has(g, "blockedReason") {
        return Err(error(format!("{l} blockedReason requires blocked phase")));
    }
    for key in ["roundsStarted", "createdAt", "updatedAt"] {
        count(&data[key], &format!("{label} {key}"))?;
    }
    Ok(())
}
fn schedule_change_value(data: &Value, label: &str) -> Result {
    number_literal(&data["version"], &[1], &format!("{label} version"))?;
    if data["operation"] == "create" {
        keys(
            data,
            &["version", "operation", "schedule"],
            &[],
            &format!("{label} data"),
        )?;
        let l = format!("{label} schedule");
        let s = record(&data["schedule"], &l)?;
        match s["kind"].as_str().unwrap_or("") {
            "after" => {
                keys(
                    s,
                    &["id", "kind", "prompt", "afterSeconds", "scheduledAt"],
                    &[],
                    &l,
                )?;
                positive(&s["afterSeconds"], &format!("{l} afterSeconds"))?;
            }
            "at" => keys(s, &["id", "kind", "prompt", "scheduledAt"], &[], &l)?,
            "every" => {
                keys(
                    s,
                    &["id", "kind", "prompt", "everySeconds", "scheduledAt"],
                    &[],
                    &l,
                )?;
                if positive(&s["everySeconds"], &format!("{l} everySeconds"))? < 300 {
                    return Err(error(format!("{l} everySeconds must be at least 300")));
                }
            }
            _ => return Err(error(format!("{l} has unknown schedule kind"))),
        }
        schedule_id_value(&s["id"], &format!("{l} id"))?;
        non_empty_string(&s["prompt"], &format!("{l} prompt"))?;
        instant_value(&s["scheduledAt"], &format!("{l} scheduledAt"))?;
        return Ok(());
    }
    keys(
        data,
        &["version", "operation", "id"],
        if data["operation"] == "dispatch" {
            &["acceptedAt"]
        } else {
            &[]
        },
        &format!("{label} data"),
    )?;
    literal(
        &data["operation"],
        &["delete", "dispatch"],
        &format!("{label} operation"),
    )?;
    schedule_id_value(&data["id"], &format!("{label} id"))?;
    optional(data, "acceptedAt", label, instant_value)
}
fn schedule_id_value(value: &Value, label: &str) -> Result {
    non_empty_string(value, label)?;
    if value
        .as_str()
        .is_some_and(|s| s.trim_matches(js_whitespace) != s)
    {
        return Err(error(format!(
            "{label} must not have surrounding whitespace"
        )));
    }
    Ok(())
}
fn js_whitespace(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn instant_value(value: &Value, label: &str) -> Result {
    if !value
        .as_str()
        .is_some_and(|s| matches!(four_digit_year_epoch(s), InstantResult::Accepted { .. }))
    {
        return Err(error(format!("{label} must be a canonical UTC instant")));
    }
    Ok(())
}
fn title_source_value(value: &Value, label: &str) -> Result {
    let s = record(value, label)?;
    if s["kind"] == "provider" {
        keys(s, &["kind", "provider"], &["model"], label)?;
        non_empty_string(&s["provider"], &format!("{label} provider"))?;
        optional(s, "model", label, model_route_value)?;
        return Ok(());
    }
    keys(s, &["kind"], &[], label)?;
    literal(&s["kind"], &["fallback", "user"], &format!("{label} kind"))
}
fn model_route_value(value: &Value, label: &str) -> Result {
    let r = exact(value, label, &["provider", "model"], &[])?;
    nonempty_fields(r, &["provider", "model"], label)
}

pub const SUBAGENT_DESCRIPTOR_VERSIONS: &[u64] = &[1, 2, 3];
fn subagent_descriptor_value(data: &Value, label: &str) -> Result {
    number_literal(
        &data["version"],
        SUBAGENT_DESCRIPTOR_VERSIONS,
        &format!("{label} version"),
    )?;
    non_empty_string(&data["provider"], &format!("{label} provider"))?;
    let mut composition = vec!["agentProvider", "agentModel", "persona", "toolFilter"];
    if crate::validation::number_eq(&data["version"], 1) {
        keys(
            data,
            &["version", "provider", "label"],
            &composition,
            &format!("{label} data"),
        )?;
    } else if data["mode"] == "one-shot" {
        keys(
            data,
            &["mode", "version", "provider"],
            &["label"],
            &format!("{label} data"),
        )?;
        return optional(data, "label", label, string_value);
    } else {
        literal(&data["mode"], &["continuable"], &format!("{label} mode"))?;
        if crate::validation::number_eq(&data["version"], 3) {
            composition.push("agentReasoningEffort");
        }
        keys(
            data,
            &["mode", "version", "provider", "label"],
            &composition,
            &format!("{label} data"),
        )?;
    }
    let strict = crate::validation::number_eq(&data["version"], 3);
    let validate = if strict {
        non_empty_string
    } else {
        string_value
    };
    validate(&data["label"], &format!("{label} label"))?;
    for key in [
        "agentProvider",
        "agentModel",
        "agentReasoningEffort",
        "persona",
    ] {
        optional(data, key, label, validate)?;
    }
    if strict && has(data, "agentProvider") != has(data, "agentModel") {
        return Err(error(format!(
            "{label} agentProvider and agentModel must be paired"
        )));
    }
    if has(data, "toolFilter") {
        let f = exact(
            &data["toolFilter"],
            &format!("{label} toolFilter"),
            &[],
            &["allow", "deny"],
        )?;
        if !has(f, "allow") && !has(f, "deny") {
            return Err(error(format!("{label} toolFilter requires allow or deny")));
        }
        for key in ["allow", "deny"] {
            if has(f, key) {
                array(&f[key], &format!("{label} {key}"), validate)?;
            }
        }
    }
    Ok(())
}
fn allowed_models_value(value: &Value, label: &str) -> Result {
    let mut seen = HashSet::new();
    let routes = array(value, label, |v, l| {
        let r = exact(v, l, &["provider", "model"], &[])?;
        nonempty_fields(r, &["provider", "model"], l)?;
        let key = format!(
            "{}\0{}",
            r["provider"].as_str().unwrap_or(""),
            r["model"].as_str().unwrap_or("")
        );
        if !seen.insert(key.clone()) {
            return Err(error(format!("{label} repeats route {key}")));
        }
        Ok(())
    })?;
    if routes.is_empty() {
        return Err(error(format!("{label} must be non-empty")));
    }
    Ok(())
}
fn team_selector(data: &Value, label: &str) -> Result {
    number_literal(&data["version"], &[1], &format!("{label} version"))?;
    non_empty_string(&data["teamId"], &format!("{label} teamId"))
}
fn team_member_value(value: &Value, label: &str) -> Result {
    let m = exact(
        value,
        label,
        &["id", "name", "description", "provider", "context", "phase"],
        &["error"],
    )?;
    non_empty_string(&m["id"], &format!("{label} id"))?;
    for key in ["name", "description", "provider"] {
        string_value(&m[key], &format!("{label} {key}"))?;
    }
    literal(
        &m["context"],
        &["fresh", "fork"],
        &format!("{label} context"),
    )?;
    literal(
        &m["phase"],
        &["provisioning", "active", "failed"],
        &format!("{label} phase"),
    )?;
    optional(m, "error", label, string_value)
}
fn team_task_value(value: &Value, label: &str) -> Result {
    let t = exact(
        value,
        label,
        &[
            "id",
            "revision",
            "subject",
            "description",
            "status",
            "blockedBy",
            "writeScopes",
        ],
        &["ownerId"],
    )?;
    non_empty_string(&t["id"], &format!("{label} id"))?;
    positive(&t["revision"], &format!("{label} revision"))?;
    string_value(&t["subject"], &format!("{label} subject"))?;
    string_value(&t["description"], &format!("{label} description"))?;
    literal(
        &t["status"],
        &["pending", "in_progress", "completed", "deleted"],
        &format!("{label} status"),
    )?;
    optional(t, "ownerId", label, non_empty_string)?;
    array(
        &t["blockedBy"],
        &format!("{label} blockedBy"),
        non_empty_string,
    )?;
    array(
        &t["writeScopes"],
        &format!("{label} writeScopes"),
        string_value,
    )?;
    Ok(())
}
fn team_message_value(value: &Value, label: &str, version: u64) -> Result {
    let m = exact(
        value,
        label,
        &[
            "id",
            "senderId",
            "senderName",
            "targetId",
            "delivery",
            "content",
        ],
        &[],
    )?;
    nonempty_fields(m, &["id", "senderId", "targetId"], label)?;
    string_value(&m["senderName"], &format!("{label} senderName"))?;
    literal(
        &m["delivery"],
        &["quiet", "wakeup"],
        &format!("{label} delivery"),
    )?;
    content_blocks_value(&m["content"], &format!("{label} content"), version)
}
fn workflow_identity(data: &Value, label: &str) -> Result {
    non_empty_string(&data["runId"], &format!("{label} runId"))?;
    positive(&data["seq"], &format!("{label} seq"))?;
    Ok(())
}
fn deep_seek_search_body_value(value: &Value, label: &str) -> Result {
    let b = exact(
        value,
        label,
        &["model", "max_tokens", "messages", "tools"],
        &[],
    )?;
    non_empty_string(&b["model"], &format!("{label} model"))?;
    positive(&b["max_tokens"], &format!("{label} max_tokens"))?;
    let messages = array(&b["messages"], &format!("{label} messages"), |v, l| {
        let m = exact(v, l, &["role", "content"], &[])?;
        literal(&m["role"], &["user"], &format!("{l} role"))?;
        let c = array(&m["content"], &format!("{l} content"), |v, l| {
            let t = exact(v, l, &["type", "text"], &[])?;
            literal(&t["type"], &["text"], &format!("{l} type"))?;
            string_value(&t["text"], &format!("{l} text"))
        })?;
        if c.len() != 1 {
            return Err(error(format!("{l} content must contain one text block")));
        }
        Ok(())
    })?;
    if messages.len() != 1 {
        return Err(error(format!(
            "{label} messages must contain one user message"
        )));
    }
    let tools = array(&b["tools"], &format!("{label} tools"), |v, l| {
        let t = exact(v, l, &["type", "name", "max_uses"], &[])?;
        literal(&t["type"], &["web_search_20250305"], &format!("{l} type"))?;
        literal(&t["name"], &["web_search"], &format!("{l} name"))?;
        positive(&t["max_uses"], &format!("{l} max_uses"))?;
        Ok(())
    })?;
    if tools.len() != 1 {
        return Err(error(format!(
            "{label} tools must contain one web search tool"
        )));
    }
    Ok(())
}
