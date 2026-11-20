//! Bounded newline-delimited projection of one Agent's durable Session events.

use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_session::{SessionEvent, SessionEventEmitted};
use hoh_util_values::{js_json_parse, js_json_stringify};
use serde_json::{Map, Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Default byte limit for every string and object key.
pub const MAX_STRING_BYTES: usize = 8 * 1024;
/// Default byte limit for a serialized event including its newline.
pub const MAX_EVENT_BYTES: usize = 32 * 1024;
const MAX_DEPTH: usize = 64;

/// Process-facing sink used by JSON projection and runtime diagnostics.
pub trait JsonSink: Send + Sync + 'static {
    fn write(&self, chunk: &str);
}

impl<F> JsonSink for F
where
    F: Fn(&str) + Send + Sync + 'static,
{
    fn write(&self, chunk: &str) {
        self(chunk);
    }
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut_split_a_scalar = end != max_bytes;
    let mut truncated = text[..end].to_owned();
    if !cut_split_a_scalar && truncated.ends_with('\u{fffd}') {
        truncated.pop();
    }
    truncated
}

fn bound_value(value: &Value, max_bytes: usize, truncated: &mut bool, depth: usize) -> Value {
    match value {
        Value::String(text) => {
            if text.len() > max_bytes {
                *truncated = true;
                Value::String(truncate_utf8(text, max_bytes))
            } else {
                value.clone()
            }
        }
        Value::Array(items) => {
            if depth >= MAX_DEPTH {
                *truncated = true;
                return Value::String("[truncated: depth]".into());
            }
            Value::Array(
                items
                    .iter()
                    .map(|item| bound_value(item, max_bytes, truncated, depth + 1))
                    .collect(),
            )
        }
        Value::Object(object) => {
            if depth >= MAX_DEPTH {
                *truncated = true;
                return Value::String("[truncated: depth]".into());
            }
            let mut bounded = Map::new();
            for (key, item) in object {
                let key = if key.len() > max_bytes {
                    *truncated = true;
                    truncate_utf8(key, max_bytes)
                } else {
                    key.clone()
                };
                bounded.insert(key, bound_value(item, max_bytes, truncated, depth + 1));
            }
            Value::Object(bounded)
        }
        _ => value.clone(),
    }
}

/// Serialize a projected event under per-string and whole-record byte limits.
pub fn bound_json_line(event: &Value, max_string_bytes: usize, max_event_bytes: usize) -> String {
    let mut truncated = false;
    let mut bounded = bound_value(event, max_string_bytes, &mut truncated, 0);
    if truncated && let Some(object) = bounded.as_object_mut() {
        object.insert("truncated".into(), Value::Bool(true));
    }
    let limit = max_event_bytes.saturating_sub(1);
    let line = js_json_stringify(&bounded);
    if line.len() <= limit {
        return line;
    }
    let mut scalars = Map::new();
    if let Some(object) = bounded.as_object() {
        for (key, value) in object {
            if !value.is_array() && !value.is_object() {
                scalars.insert(key.clone(), value.clone());
            }
        }
    }
    scalars.insert("truncated".into(), Value::Bool(true));
    let short = js_json_stringify(&Value::Object(scalars));
    if short.len() <= limit {
        return short;
    }
    let mut identity = Map::new();
    if let Some(event_type) = bounded.get("type") {
        identity.insert("type".into(), event_type.clone());
    }
    identity.insert("truncated".into(), Value::Bool(true));
    js_json_stringify(&Value::Object(identity))
}

/// Source defaults for [`bound_json_line`].
pub fn bound_json_event_line(event: &Value) -> String {
    bound_json_line(event, MAX_STRING_BYTES, MAX_EVENT_BYTES)
}

fn parse_arguments(raw: &str) -> Value {
    if raw.is_empty() {
        return json!({});
    }
    let Ok(parsed) = js_json_parse(raw) else {
        return Value::String(raw.into());
    };
    if contains_non_finite_number(raw) {
        Value::String(raw.into())
    } else {
        parsed
    }
}

fn contains_non_finite_number(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut in_string = false;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                in_string = !in_string;
                index += 1;
            }
            b'\\' if in_string => index = (index + 2).min(bytes.len()),
            b'-' | b'0'..=b'9' if !in_string => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && matches!(bytes[index], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    index += 1;
                }
                if raw[start..index]
                    .parse::<f64>()
                    .is_ok_and(|number| !number.is_finite())
                {
                    return true;
                }
            }
            _ => index += 1,
        }
    }
    false
}

fn result_text(blocks: Option<&Vec<Value>>) -> String {
    blocks
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect()
}

fn stream_usage(stream: &Value) -> Option<Value> {
    stream.as_array()?.iter().rev().find_map(|record| {
        let chunk = record.get("chunk").unwrap_or(record);
        (chunk.get("type").and_then(Value::as_str) == Some("usage"))
            .then(|| chunk.get("usage").cloned())
            .flatten()
    })
}

fn add_optional_numbers(total: &mut Map<String, Value>, next: &Map<String, Value>, key: &str) {
    match (
        total.get(key).and_then(Value::as_u64),
        next.get(key).and_then(Value::as_u64),
    ) {
        (Some(left), Some(right)) => {
            total.insert(key.into(), Value::from(left.saturating_add(right)));
        }
        _ => {
            total.remove(key);
        }
    }
}

fn add_usage(total: &mut Option<Value>, complete: &mut bool, next: Option<Value>) {
    let Some(next) = next else {
        *complete = false;
        return;
    };
    let Some(next_object) = next.as_object() else {
        *complete = false;
        return;
    };
    let Some(current) = total else {
        *total = Some(next);
        return;
    };
    let Some(current_object) = current.as_object_mut() else {
        *complete = false;
        return;
    };
    for key in ["inputTokens", "outputTokens"] {
        let Some(left) = current_object.get(key).and_then(Value::as_u64) else {
            *complete = false;
            continue;
        };
        let Some(right) = next_object.get(key).and_then(Value::as_u64) else {
            *complete = false;
            continue;
        };
        current_object.insert(key.into(), Value::from(left.saturating_add(right)));
    }
    for key in [
        "totalTokens",
        "cacheReadTokens",
        "cacheWriteTokens",
        "reasoningTokens",
    ] {
        add_optional_numbers(current_object, next_object, key);
    }
}

fn project_event(
    event: &SessionEvent,
    usage: &mut Option<Value>,
    complete: &mut bool,
) -> Vec<Value> {
    let data = event.data();
    match event.event_type() {
        "turn/start" => vec![json!({"type":"status", "phase":"turn_start", "turn":data["turn"]})],
        "step/start" => vec![
            json!({"type":"status", "phase":"step_start", "turn":data["turn"], "step":data["step"]}),
        ],
        "assistant/attempt" => {
            add_usage(usage, complete, stream_usage(&data["stream"]));
            Vec::new()
        }
        "assistant/message" => {
            add_usage(
                usage,
                complete,
                data.get("usage")
                    .cloned()
                    .or_else(|| stream_usage(&data["stream"])),
            );
            data.pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|block| match block.get("type").and_then(Value::as_str) {
                    Some("reasoning") => Some(json!({"type":"thinking", "text":block["text"]})),
                    Some("text") => Some(json!({"type":"text", "text":block["text"]})),
                    _ => None,
                })
                .collect()
        }
        "step/end" => {
            let mut projected = json!({"type":"status", "phase":"step_end", "turn":data["turn"], "step":data["step"]});
            if *complete && let Some(value) = usage.take() {
                projected["usage"] = value;
            }
            *usage = None;
            *complete = true;
            vec![projected]
        }
        "turn/end" => vec![
            json!({"type":"status", "phase":"turn_end", "turn":data["turn"], "reason":data["reason"]}),
        ],
        "tool/call" => vec![json!({
            "type":"tool_call", "callId":data["callId"], "tool":data["name"],
            "input":parse_arguments(data["arguments"].as_str().unwrap_or_default()),
        })],
        "tool/result" if event.surface_op() == Some(&Value::String("append".into())) => {
            let message = &data["message"];
            let mut projected = json!({
                "type":"tool_result",
                "callId":message["toolCallId"],
                "status":if message["isError"] == true { "error" } else { "completed" },
                "result":result_text(message["content"].as_array()),
            });
            if let Some(error) = data.get("error") {
                projected["error"] = error.clone();
            }
            vec![projected]
        }
        _ => Vec::new(),
    }
}

/// Active projection handle. `final` is deliberately lossless and unbounded.
pub struct JsonProjection {
    session_id: String,
    sink: Arc<dyn JsonSink>,
    active: Arc<AtomicBool>,
}

impl JsonProjection {
    pub fn finish(&self, text: &str, reason: Option<&Value>) {
        if self.active.load(Ordering::Acquire) {
            self.sink.write(&format!(
                "{}\n",
                js_json_stringify(&json!({
                    "type":"final", "sessionId":self.session_id,
                    "reason":reason.cloned().unwrap_or(Value::Null), "text":text,
                }))
            ));
        }
    }

    pub fn dispose(&self) {
        self.active.store(false, Ordering::Release);
    }
}

impl Drop for JsonProjection {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

/// Project durable events for exactly `agent` to `sink`.
pub fn project_json_run(
    ctx: &Context,
    agent: Arc<dyn Agent>,
    sink: Arc<dyn JsonSink>,
    cwd: &str,
    max_string_bytes: usize,
) -> Result<JsonProjection, BoxError> {
    let session_id = agent.id().to_string();
    sink.write(&format!(
        "{}\n",
        bound_json_line(
            &json!({"type":"session", "sessionId":session_id, "cwd":cwd}),
            max_string_bytes,
            MAX_EVENT_BYTES,
        )
    ));
    let active = Arc::new(AtomicBool::new(true));
    let state = Arc::new(Mutex::new((None, true)));
    let expected = agent.session().clone();
    let listener_active = active.clone();
    let listener_sink = sink.clone();
    ctx.on_contained::<SessionEventEmitted>(move |emitted| {
        if !listener_active.load(Ordering::Acquire) || emitted.session != expected {
            return Ok(());
        }
        let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
        let (usage, complete) = &mut *state;
        for projected in project_event(&emitted.event, usage, complete) {
            listener_sink.write(&format!(
                "{}\n",
                bound_json_line(&projected, max_string_bytes, MAX_EVENT_BYTES)
            ));
        }
        Ok(())
    })?;
    Ok(JsonProjection {
        session_id,
        sink,
        active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_utf8_strings_and_marks_payload() {
        let line = bound_json_line(&json!({"type":"text", "text":"ééé"}), 3, 100);
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({
                "typ":"tex", "tex":"é", "truncated":true
            })
        );
    }

    #[test]
    fn whole_line_falls_back_to_scalars_then_identity() {
        let line = bound_json_line(&json!({"type":"tool", "nested":[1,2,3]}), 100, 25);
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"type":"tool", "truncated":true})
        );
        let tiny = bound_json_line(&json!({"type":"very-long"}), 100, 2);
        assert_eq!(
            serde_json::from_str::<Value>(&tiny).unwrap(),
            json!({"type":"very-long", "truncated":true})
        );
    }

    #[test]
    fn arguments_match_executor_contract() {
        assert_eq!(parse_arguments(""), json!({}));
        assert_eq!(parse_arguments("{\"ok\":true}"), json!({"ok":true}));
        assert_eq!(parse_arguments("{"), Value::String("{".into()));
        assert_eq!(parse_arguments("1e400"), Value::String("1e400".into()));
    }

    #[test]
    fn arguments_use_javascript_number_semantics() {
        for (raw, expected) in [
            ("{\"x\":1.0}", "{\"x\":1}"),
            ("{\"x\":1e2}", "{\"x\":100}"),
            (
                "{\"x\":12345678901234567890}",
                "{\"x\":12345678901234567000}",
            ),
            ("{\"x\":-0}", "{\"x\":0}"),
        ] {
            assert_eq!(js_json_stringify(&parse_arguments(raw)), expected);
        }
    }

    #[test]
    fn truncation_drops_a_boundary_replacement_character_like_node_buffer() {
        assert_eq!(
            bound_json_line(&json!({"text":"a\u{fffd}b"}), 4, 100),
            "{\"text\":\"a\",\"truncated\":true}"
        );
    }

    #[test]
    fn truncation_keeps_a_real_replacement_character_before_a_split_scalar() {
        assert_eq!(
            bound_json_line(&json!({"text":"a\u{fffd}é"}), 5, 100),
            "{\"text\":\"a�\",\"truncated\":true}"
        );
    }

    #[test]
    fn identity_fallback_omits_an_absent_type() {
        assert_eq!(
            bound_json_line(&json!({"nested":[1, 2, 3]}), 100, 2),
            "{\"truncated\":true}"
        );
    }

    #[test]
    fn incomplete_attempt_usage_is_omitted() {
        let mut usage = None;
        let mut complete = true;
        let attempt = SessionEvent::from_value(json!({
            "type":"assistant/attempt", "seq":0, "time":0, "data":{"stream":[]}
        }));
        assert!(project_event(&attempt, &mut usage, &mut complete).is_empty());
        let end = SessionEvent::from_value(json!({
            "type":"step/end", "seq":1, "time":0, "data":{"turn":1,"step":1}
        }));
        assert!(
            project_event(&end, &mut usage, &mut complete)[0]
                .get("usage")
                .is_none()
        );
    }

    #[test]
    fn projects_turn_and_step_boundaries_with_terminal_reason() {
        let mut usage = None;
        let mut complete = true;
        for (event, expected) in [
            (
                json!({"type":"turn/start","seq":0,"time":0,"data":{"turn":2}}),
                json!({"type":"status","phase":"turn_start","turn":2}),
            ),
            (
                json!({"type":"step/start","seq":1,"time":0,"data":{"turn":2,"step":3}}),
                json!({"type":"status","phase":"step_start","turn":2,"step":3}),
            ),
            (
                json!({"type":"turn/end","seq":2,"time":0,"data":{"turn":2,"reason":{"kind":"completed"}}}),
                json!({"type":"status","phase":"turn_end","turn":2,"reason":{"kind":"completed"}}),
            ),
        ] {
            let event = SessionEvent::from_value(event);
            assert_eq!(project_event(&event, &mut usage, &mut complete), [expected]);
        }
    }

    #[test]
    fn depth_is_bounded_without_recursing_forever() {
        let mut value = Value::String("tail".into());
        for _ in 0..70 {
            value = Value::Array(vec![value]);
        }
        let line = bound_json_line(&json!({"type":"deep", "value":value}), 100, 10_000);
        let bounded: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(bounded["truncated"], true);
        assert!(line.contains("[truncated: depth]"));
    }

    #[test]
    fn long_keys_are_bounded_as_utf8_and_collide_last_wins() {
        let line = bound_json_line(
            &json!({"type":"keys", "éé-first":1, "éé-second":2}),
            4,
            1000,
        );
        let bounded: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(bounded["éé"], 2);
        assert_eq!(bounded["truncated"], true);
    }

    #[test]
    fn usage_sums_required_and_only_shared_optional_buckets() {
        let mut usage = None;
        let mut complete = true;
        add_usage(
            &mut usage,
            &mut complete,
            Some(json!({"inputTokens":2,"outputTokens":3,"totalTokens":5,"cacheReadTokens":1})),
        );
        add_usage(
            &mut usage,
            &mut complete,
            Some(json!({"inputTokens":7,"outputTokens":11,"totalTokens":18})),
        );
        assert!(complete);
        assert_eq!(
            usage,
            Some(json!({"inputTokens":9,"outputTokens":14,"totalTokens":23}))
        );
    }

    #[test]
    fn committed_message_projects_reasoning_before_text() {
        let event = SessionEvent::from_value(json!({
            "type":"assistant/message", "seq":0, "time":0,
            "data":{"message":{"content":[
                {"type":"reasoning","text":"think"},
                {"type":"text","text":"answer"}
            ]},"stream":[],"usage":{"inputTokens":1,"outputTokens":2}}
        }));
        let mut usage = None;
        let mut complete = true;
        assert_eq!(
            project_event(&event, &mut usage, &mut complete),
            [
                json!({"type":"thinking","text":"think"}),
                json!({"type":"text","text":"answer"})
            ]
        );
    }

    #[test]
    fn replaced_tool_results_are_not_replayed() {
        let event = SessionEvent::from_value(json!({
            "type":"tool/result", "seq":0, "time":0,
            "surfaceOp":{"op":"replace","startSeq":0,"endSeq":1},
            "data":{"message":{"toolCallId":"call","isError":false,"content":[{"type":"text","text":"old"}]}}
        }));
        let mut usage = None;
        let mut complete = true;
        assert!(project_event(&event, &mut usage, &mut complete).is_empty());
    }

    #[test]
    fn append_tool_result_keeps_structured_error_and_joins_text() {
        let event = SessionEvent::from_value(json!({
            "type":"tool/result", "seq":0, "time":0, "surfaceOp":"append",
            "data":{"message":{"toolCallId":"call","isError":true,"content":[
                {"type":"text","text":"one"},{"type":"image"},{"type":"text","text":"two"}
            ]},"error":{"code":"BROKEN"}}
        }));
        let mut usage = None;
        let mut complete = true;
        assert_eq!(
            project_event(&event, &mut usage, &mut complete),
            [json!({
                "type":"tool_result","callId":"call","status":"error","result":"onetwo",
                "error":{"code":"BROKEN"}
            })]
        );
    }

    #[test]
    fn dropping_projection_stops_all_later_output() {
        let active = Arc::new(AtomicBool::new(true));
        let projection = JsonProjection {
            session_id: "session-test".into(),
            sink: Arc::new(|_: &str| {}),
            active: active.clone(),
        };
        drop(projection);
        assert!(!active.load(Ordering::Acquire));
    }

    #[test]
    fn final_is_lossless_keeps_identity_and_stops_after_dispose() {
        let output = Arc::new(Mutex::new(String::new()));
        let observed = output.clone();
        let projection = JsonProjection {
            session_id: "session-exact".into(),
            sink: Arc::new(move |text: &str| {
                observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(text);
            }),
            active: Arc::new(AtomicBool::new(true)),
        };
        let answer = "x".repeat(MAX_STRING_BYTES + 1);
        projection.finish(&answer, Some(&json!({"kind":"interrupted"})));
        let line = output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let final_event: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(final_event["sessionId"], "session-exact");
        assert_eq!(final_event["reason"], json!({"kind":"interrupted"}));
        assert_eq!(final_event["text"], answer);
        projection.dispose();
        projection.finish("late", None);
        assert_eq!(
            *output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            line
        );
    }
}
