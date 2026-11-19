//! Audited v2 migration admission and v3 payload validation.
use crate::helpers::*;
use hoh_session_format::{
    SessionFormatEvent, SessionFormatJsonObject, session_format_count, session_format_safe_integer,
};
use hoh_session_format_v0_to_v1::{
    assert_released_payload_semantics, assert_released_surface_metadata,
};
use hoh_session_format_v1_to_v2::{
    RELEASED_V2_EVENT_DISPOSITIONS,
    validation::{assert_released_v2_keys, released_v2_record},
};
use serde_json::{Value, json};
use std::collections::HashSet;
pub const SURFACE_TYPES: &[&str] = &[
    "system/message",
    "user/message",
    "assistant/message",
    "tool/result",
];
const SOURCE_KINDS: &[&str] = &[
    "user",
    "plugin",
    "model",
    "tool",
    "agent-instructions",
    "session-reference",
    "team-message",
    "goal",
    "skill-invocation",
    "skill-catalog",
    "coordinator",
    "subagent-report",
    "subagent-settled",
    "webhook",
    "agent-message",
];
const CONTENT_KINDS: &[&str] = &[
    "text",
    "reasoning",
    "image",
    "file",
    "tool-call",
    "tool-result",
];
pub fn record<'a>(value: &'a Value, label: &str) -> Result<&'a SessionFormatJsonObject> {
    released_v2_record(value, label)
}
pub fn keys(
    value: &SessionFormatJsonObject,
    required: &[&str],
    optional: &[&str],
    label: &str,
) -> Result {
    assert_released_v2_keys(value, required, optional, label)
}
pub fn assert_event(event: &SessionFormatEvent, version: u64) -> Result {
    if version == 3 {
        return assert_v3_event(event, None);
    }
    let disposition = RELEASED_V2_EVENT_DISPOSITIONS.get(kind(event));
    let feedback = matches!(
        kind(event),
        "feedback/message-put" | "feedback/message-delete"
    );
    if disposition.is_none() && !feedback {
        return Err(refusal(format!(
            "format v2 to v3 cannot safely transform unclassified event {}",
            text(event.get("type"))
        )));
    }
    let surface = SURFACE_TYPES.contains(&kind(event));
    keys(
        event,
        &["type", "seq", "time", "data"],
        if surface {
            &["ignorable", "sourceEventSeqs", "surfaceOp"]
        } else {
            &["ignorable"]
        },
        &text(event.get("type")),
    )?;
    let seq = session_format_count(field(event, "seq"), "event seq")?;
    session_format_safe_integer(field(event, "time"), "event time")?;
    if event.contains_key("ignorable") && field(event, "ignorable") != true {
        return Err(error("ignorable must be true"));
    }
    if surface {
        assert_released_surface_metadata(event, seq, kind(event), "forbid-assistant")?;
        if !event.contains_key("surfaceOp") {
            return Err(error(format!("{} requires surfaceOp", kind(event))));
        }
    }
    let data = record(field(event, "data"), &format!("{} data", kind(event)))?;
    if feedback {
        return assert_feedback(kind(event), data);
    }
    // Non-inventory feedback has returned; the admission check proves this disposition.
    let disposition = disposition.expect("admitted inventory");
    let required: Vec<_> = disposition.required.iter().map(String::as_str).collect();
    let optional: Vec<_> = disposition.optional.iter().map(String::as_str).collect();
    keys(data, &required, &optional, &format!("{} data", kind(event)))?;
    assert_owned_content(event, data)?;
    if kind(event) != "assistant/attempt" {
        assert_released_payload_semantics(event, version)?;
    }
    if matches!(kind(event), "assistant/message" | "assistant/attempt") {
        for coordinate in ["turn", "step"] {
            if session_format_count(field(data, coordinate), coordinate)? == 0 {
                return Err(error(format!("{coordinate} must be positive")));
            }
        }
    }
    if kind(event) == "session/end-seed"
        && data.contains_key("inherited")
        && field(data, "inherited") != true
    {
        return Err(error("session/end-seed inherited must be true"));
    }
    if kind(event) == "user/message" {
        assert_source(data)?;
    }
    if matches!(kind(event), "assistant/message" | "tool/result") {
        assert_source(record(field(data, "message"), "message")?)?;
    }
    if kind(event) == "tool/result"
        && field(data, "error")
            .as_object()
            .is_some_and(|e| field(e, "code") == "TOOL_NOT_STARTED")
    {
        let message = record(field(data, "message"), "tool result message")?;
        let source = record(field(message, "source"), "tool result source")?;
        if !is_repair_identity(message.get("id"), source.get("callId")) {
            return Err(error(
                "TOOL_NOT_STARTED repair requires its canonical historical message id",
            ));
        }
    }
    if matches!(
        kind(event),
        "agent/inbox/spliced" | "session/title-llm-request"
    ) {
        let messages = field(
            data,
            if kind(event) == "agent/inbox/spliced" {
                "inserted"
            } else {
                "messages"
            },
        );
        for message in content_array(messages, "messages")? {
            assert_source(record(message, "message")?)?;
        }
    }
    Ok(())
}
pub fn is_repair_identity(id: Option<&Value>, call_id: Option<&Value>) -> bool {
    let Some(call) = call_id.and_then(Value::as_str) else {
        return false;
    };
    let prefix = format!("interrupted-tool-result-{call}-");
    let Some(suffix) = id
        .and_then(Value::as_str)
        .and_then(|id| id.strip_prefix(&prefix))
    else {
        return false;
    };
    (suffix == "0"
        || (!suffix.starts_with('0')
            && !suffix.is_empty()
            && suffix.bytes().all(|b| b.is_ascii_digit())))
        && suffix
            .parse::<u64>()
            .is_ok_and(|n| n <= 9_007_199_254_740_991)
}
fn assert_source(message: &SessionFormatJsonObject) -> Result {
    let source = record(field(message, "source"), "message source")?;
    if !field(source, "kind")
        .as_str()
        .is_some_and(|s| SOURCE_KINDS.contains(&s))
    {
        return Err(refusal(
            "cannot safely transform unclassified message source",
        ));
    }
    if field(source, "kind") == "agent-message" {
        keys(
            source,
            &["kind", "form", "senderSessionId"],
            &[],
            "agent-message source",
        )?;
        if field(source, "form") != "relay"
            || !field(source, "senderSessionId")
                .as_str()
                .is_some_and(|s| !s.is_empty())
        {
            return Err(error(
                "agent-message source requires relay form and senderSessionId",
            ));
        }
    }
    Ok(())
}
fn content_array<'a>(value: &'a Value, label: &str) -> Result<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| error(format!("{label}: content must be an array")))
}
fn assert_owned_content(event: &SessionFormatEvent, data: &SessionFormatJsonObject) -> Result {
    let label = format!(
        "format v2 {} at seq {} data",
        text(event.get("type")),
        text(event.get("seq"))
    );
    match kind(event) {
        "user/message" | "tool/code-dispatch" => {
            assert_content_kinds(field(data, "content"), &format!("{label}.content"))?
        }
        "assistant/message" | "tool/result" | "team/message/queued" => assert_content_kinds(
            field(
                record(field(data, "message"), &format!("{label}.message"))?,
                "content",
            ),
            &format!("{label}.message.content"),
        )?,
        "agent/inbox/spliced" | "session/title-llm-request" => {
            let key = if kind(event) == "agent/inbox/spliced" {
                "inserted"
            } else {
                "messages"
            };
            let path = format!("{label}.{key}");
            for (index, value) in content_array(field(data, key), &path)?.iter().enumerate() {
                let path = format!("{path}[{index}]");
                assert_content_kinds(
                    field(record(value, &path)?, "content"),
                    &format!("{path}.content"),
                )?;
            }
        }
        "compaction/summary" => {
            assert_content_kinds(field(data, "summary"), &format!("{label}.summary"))?;
            if let Some(raw) = data.get("rawOutput") {
                assert_content_kinds(raw, &format!("{label}.rawOutput"))?;
            }
        }
        _ => {}
    }
    if matches!(kind(event), "assistant/message" | "assistant/attempt") {
        let path = format!("{label}.stream");
        for (index, value) in content_array(field(data, "stream"), &path)?
            .iter()
            .enumerate()
        {
            let path = format!("{path}[{index}]");
            let entry = record(value, &path)?;
            if field(entry, "type") != "chunk" {
                continue;
            }
            let chunk = record(field(entry, "chunk"), &format!("{path}.chunk"))?;
            if field(chunk, "type") == "block-end" {
                assert_content_block(field(chunk, "block"), &format!("{path}.chunk.block"))?;
            }
            if field(chunk, "type") == "block-start" {
                assert_content_kind(chunk.get("blockType"), &format!("{path}.chunk.blockType"))?;
            }
        }
    }
    Ok(())
}
fn assert_content_kind(kind: Option<&Value>, label: &str) -> Result {
    if !kind
        .and_then(Value::as_str)
        .is_some_and(|s| CONTENT_KINDS.contains(&s))
    {
        return Err(refusal(format!(
            "{label}: cannot safely transform unclassified message content kind {}",
            quoted(kind)
        )));
    }
    Ok(())
}
fn assert_content_kinds(content: &Value, label: &str) -> Result {
    for (index, value) in content_array(content, label)?.iter().enumerate() {
        assert_content_block(value, &format!("{label}[{index}]"))?;
    }
    Ok(())
}
fn assert_content_block(value: &Value, label: &str) -> Result {
    let block = record(value, label)?;
    assert_content_kind(block.get("type"), label)?;
    if field(block, "type") == "tool-result" {
        if !field(block, "content").is_array() {
            return Err(error(format!(
                "{label}.content: invalid message content kind \"tool-result\": content must be an array"
            )));
        }
        assert_content_kinds(field(block, "content"), &format!("{label}.content"))?;
    }
    if field(block, "type") == "file" {
        let path = format!("{label} kind \"file\"");
        keys(block, &["type", "attachment"], &[], &path)?;
        let attachment = record(field(block, "attachment"), &format!("{path} attachment"))?;
        keys(
            attachment,
            &["attachmentId", "name", "bytes"],
            &[],
            &format!("{path} attachment"),
        )?;
        if !field(attachment, "attachmentId")
            .as_str()
            .is_some_and(|s| !s.is_empty())
            || !field(attachment, "name").is_string()
        {
            return Err(error(format!(
                "{path}: file attachment requires attachmentId and name"
            )));
        }
        session_format_count(
            field(attachment, "bytes"),
            &format!("{path} attachment bytes"),
        )?;
        return Ok(());
    }
    let mut leaf = block.clone();
    if field(block, "type") == "tool-result" {
        leaf.insert("content".into(), json!([]));
    }
    let probe = object(
        json!({"type":"user/message","seq":0,"time":0,"data":{"id":"content-admission","role":"user","source":{"kind":"user"},"content":[leaf]}}),
    );
    assert_released_payload_semantics(&probe, 2).map_err(|cause| {
        let name = if cause.is_unsupported_migration() {
            "SessionFormatUnsupportedMigrationError"
        } else {
            "SessionFormatError"
        };
        error(format!(
            "{label}: invalid message content kind {}: {name}: {cause}",
            quoted(block.get("type"))
        ))
    })
}
pub fn assert_v3_structural_row(value: &Value) -> Result {
    let Some(row) = value.as_object() else {
        return Ok(());
    };
    if field(row, "type") == "request/header" {
        let data = record(field(row, "data"), "request/header data")?;
        if record(field(data, "header"), "request header")?.contains_key("system") {
            return Err(refusal(
                "format v3 request/header rejects retired header.system",
            ));
        }
    } else if field(row, "type") == "system/message" {
        let data = record(field(row, "data"), "system/message data")?;
        assert_system(
            &object(json!({"type":"system/message","seq":0,"time":0,"data":data})),
            data,
        )?;
    }
    Ok(())
}
fn assert_system(event: &SessionFormatEvent, data: &SessionFormatJsonObject) -> Result {
    keys(
        data,
        &["turn", "step", "message"],
        &[],
        "system/message data",
    )?;
    for coordinate in ["turn", "step"] {
        if session_format_count(field(data, coordinate), coordinate)? == 0 {
            return Err(error(format!("{coordinate} must be positive")));
        }
    }
    let message = record(field(data, "message"), "system message")?;
    keys(
        message,
        &["id", "role", "source", "content"],
        &[],
        "system message",
    )?;
    if !field(message, "id").as_str().is_some_and(|s| !s.is_empty())
        || field(message, "role") != "system"
    {
        return Err(error("system message requires an id and system role"));
    }
    let source = record(field(message, "source"), "system source")?;
    if field(source, "kind") != "plugin"
        || !field(source, "plugin")
            .as_str()
            .is_some_and(|s| !s.is_empty())
    {
        return Err(error("system message requires plugin source"));
    }
    let mut probe = event.clone();
    probe.insert("type".into(), "user/message".into());
    let mut message = message.clone();
    message.insert("role".into(), "user".into());
    probe.insert("data".into(), Value::Object(message));
    assert_released_payload_semantics(&probe, 3)
}
fn assert_feedback(kind: &str, data: &SessionFormatJsonObject) -> Result {
    keys(
        data,
        if kind == "feedback/message-put" {
            &["sessionId", "item"]
        } else {
            &["sessionId", "messageId"]
        },
        &[],
        kind,
    )?;
    if !field(data, "sessionId").is_string() {
        return Err(error("feedback sessionId must be a string"));
    }
    if kind == "feedback/message-delete" {
        if !field(data, "messageId").is_string() {
            return Err(error("feedback messageId must be a string"));
        }
        return Ok(());
    }
    let item = record(field(data, "item"), "feedback item")?;
    keys(
        item,
        &["messageId", "rating", "version", "createdAt", "updatedAt"],
        &["note"],
        "feedback item",
    )?;
    for key in ["messageId", "version"] {
        if !field(item, key).is_string() {
            return Err(error(format!("feedback {key} must be a string")));
        }
    }
    if field(item, "rating") != "positive" && field(item, "rating") != "negative" {
        return Err(error("invalid feedback rating"));
    }
    if item.contains_key("note") && !field(item, "note").is_string() {
        return Err(error("feedback note must be a string"));
    }
    session_format_count(field(item, "createdAt"), "feedback createdAt")?;
    session_format_count(field(item, "updatedAt"), "feedback updatedAt")?;
    Ok(())
}
pub fn assert_v3_event(
    event: &SessionFormatEvent,
    known_event_types: Option<&HashSet<String>>,
) -> Result {
    let subject = subject(event);
    let obsolete = matches!(
        kind(event),
        "tool/code-dispatch-start" | "tool/code-dispatch"
    );
    let known = !obsolete
        && (SURFACE_TYPES.contains(&kind(event))
            || RELEASED_V2_EVENT_DISPOSITIONS.get(kind(event)).is_some()
            || matches!(
                kind(event),
                "tool/ptc-dispatch-start"
                    | "tool/ptc-dispatch"
                    | "feedback/message-put"
                    | "feedback/message-delete"
            )
            || known_event_types.is_some_and(|s| s.contains(kind(event))));
    keys(
        event,
        &["type", "seq", "time", "data"],
        if SURFACE_TYPES.contains(&kind(event)) || !known {
            &["ignorable", "surfaceOp", "sourceEventSeqs"]
        } else {
            &["ignorable"]
        },
        &subject,
    )?;
    if !field(event, "type").is_string() {
        return Err(error(format!("{subject} type must be a string")));
    }
    let seq = session_format_count(field(event, "seq"), &format!("{subject} seq"))?;
    session_format_safe_integer(field(event, "time"), &format!("{subject} time"))?;
    if event.contains_key("ignorable") && field(event, "ignorable") != true {
        return Err(error(format!(
            "{subject} ignorable must be true when present"
        )));
    }
    if SURFACE_TYPES.contains(&kind(event)) {
        let Some(op) = event.get("surfaceOp") else {
            return Err(error(format!("{subject} requires a surfaceOp marker")));
        };
        if op != "append" {
            let replace = record(op, &format!("{subject} surfaceOp"))?;
            if replace.len() != 3
                || field(replace, "op") != "replace"
                || !replace.contains_key("startSeq")
                || !replace.contains_key("endSeq")
            {
                return Err(error(format!(
                    "{subject} requires exact replace fields op/startSeq/endSeq"
                )));
            }
            for key in ["startSeq", "endSeq"] {
                if session_format_count(field(replace, key), &format!("{subject} surfaceOp {key}"))?
                    >= seq
                {
                    return Err(error(format!(
                        "{subject} replacement endpoints must reference earlier events"
                    )));
                }
            }
        }
        if let Some(sources) = event.get("sourceEventSeqs") {
            if kind(event) == "assistant/message" {
                return Err(error(format!(
                    "{subject} embeds its stream and cannot carry sourceEventSeqs"
                )));
            }
            let sources = sources
                .as_array()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    error(format!(
                        "{subject} sourceEventSeqs must be a non-empty array"
                    ))
                })?;
            let mut seen = HashSet::new();
            for source in sources {
                let source =
                    session_format_count(source, &format!("{subject} sourceEventSeqs member"))?;
                if source >= seq || !seen.insert(source) {
                    return Err(error(format!(
                        "{subject} sourceEventSeqs must be unique earlier seqs"
                    )));
                }
            }
        }
    }
    assert_v3_structural_row(&Value::Object(event.clone()))?;
    assert_canonical_payload(event)
}
fn assert_canonical_payload(event: &SessionFormatEvent) -> Result {
    let subject = subject(event);
    if kind(event) == "request/header" {
        let data = record(field(event, "data"), &format!("{subject} data"))?;
        let header = record(field(data, "header"), &format!("{subject} header"))?;
        if field(header, "tools").as_array().is_some_and(Vec::is_empty)
            || field(header, "adapterDefaults")
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        {
            return Err(error(format!(
                "{subject} empty optional header fields must be omitted"
            )));
        }
    }
    if kind(event) != "tool/result" {
        return Ok(());
    }
    let data = record(field(event, "data"), &format!("{subject} data"))?;
    if !data.contains_key("error") {
        return Ok(());
    }
    let message = record(field(data, "message"), &format!("{subject} message"))?;
    let valid = field(message, "content")
        .as_array()
        .filter(|a| a.len() == 1)
        .and_then(|a| a[0].as_object())
        .is_some_and(|b| field(b, "type") == "tool-result" && field(b, "isError") == true);
    if !valid {
        return Err(error(format!(
            "{subject} carries error metadata for a non-error tool result"
        )));
    }
    Ok(())
}
pub fn canonicalize_transformed_event(mut event: SessionFormatEvent) -> Result<SessionFormatEvent> {
    if let Some(op) = event.get("surfaceOp")
        && op != "append"
    {
        let subject = format!(
            "format v2 {} at seq {}",
            text(event.get("type")),
            text(event.get("seq"))
        );
        let replace = record(op, &format!("{subject} surfaceOp"))?;
        if replace.len() != 3
            || field(replace, "op") != "replace"
            || !replace.contains_key("start")
            || !replace.contains_key("end")
        {
            return Err(error(format!(
                "{subject} requires exact replace fields op/start/end"
            )));
        }
        let start =
            session_format_count(field(replace, "start"), &format!("{subject} replace start"))?;
        let end = session_format_count(field(replace, "end"), &format!("{subject} replace end"))?;
        event.insert(
            "surfaceOp".into(),
            json!({"op":"replace","startSeq":start,"endSeq":end}),
        );
    }
    if kind(&event) == "request/header" {
        let label = format!("format v2 request/header at seq {}", text(event.get("seq")));
        let mut data = record(field(&event, "data"), &format!("{label} data"))?.clone();
        let mut header = record(field(&data, "header"), &format!("{label} header"))?.clone();
        if field(&header, "tools")
            .as_array()
            .is_some_and(Vec::is_empty)
        {
            header.shift_remove("tools");
        }
        if field(&header, "adapterDefaults")
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            header.shift_remove("adapterDefaults");
        }
        data.insert("header".into(), Value::Object(header));
        event.insert("data".into(), Value::Object(data));
    }
    assert_v3_event(&event, None)?;
    Ok(event)
}
