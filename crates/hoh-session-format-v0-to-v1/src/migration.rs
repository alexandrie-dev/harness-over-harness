use crate::codec::is_released_assistant_chunk_run;
use crate::validation::{assert_released_event_payload, assert_released_v1_header, number_eq};
use crate::validation_helpers::{
    event_label, event_seq, event_seq_text, event_type, exact_u64, field, has, keys, record,
};
use hoh_session_format::{
    SessionFormatError, SessionFormatEvent, SessionFormatEventRun, SessionFormatHeader,
    SessionFormatMigration, SessionFormatMigrationContext, SessionFormatMigrationStage,
    SessionFormatMigrationStageInput, session_format_count,
};
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub struct SessionFormatV0ToV1;
pub const SESSION_FORMAT_V0_TO_V1: SessionFormatV0ToV1 = SessionFormatV0ToV1;
impl SessionFormatMigration for SessionFormatV0ToV1 {
    fn name(&self) -> &str {
        "@deepseek-ai/dsh-session-format-v0-to-v1"
    }
    fn from_version(&self) -> u64 {
        0
    }
    fn to_version(&self) -> u64 {
        1
    }
    fn migrate_header(
        &self,
        mut header: SessionFormatHeader,
    ) -> Result<SessionFormatHeader, SessionFormatError> {
        assert_header_version(&header, 0)?;
        header.insert("version".into(), 1.into());
        Ok(header)
    }
    fn create_stage(
        &self,
        input: SessionFormatMigrationStageInput<'_>,
    ) -> Result<Box<dyn SessionFormatMigrationStage>, SessionFormatError> {
        assert_header_version(input.source_header, 0)?;
        let cut = session_format_count(
            &input
                .source_inherited_event_count
                .map(Value::from)
                .unwrap_or(Value::Null),
            "format v0 inherited event count",
        )?;
        Ok(Box::new(ReleasedV0ToV1Stage {
            source_header: input.source_header.clone(),
            cut,
            state: LegacyNormalizationState::default(),
        }))
    }
    fn validate_target_header(
        &self,
        header: &SessionFormatHeader,
    ) -> Result<(), SessionFormatError> {
        assert_released_v1_header(header)
    }
}
struct ReleasedV0ToV1Stage {
    source_header: SessionFormatHeader,
    cut: u64,
    state: LegacyNormalizationState,
}
impl SessionFormatMigrationStage for ReleasedV0ToV1Stage {
    fn header_inherited_event_count(&self) -> Option<u64> {
        Some(self.cut)
    }
    fn transform_event(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<(), SessionFormatError> {
        let id = field(&self.source_header, "id").as_str().unwrap_or("");
        let event = normalize_released_v0_event(event, id, &mut self.state)?;
        if event_type(&event) == "session-log-deepseek/delivery-accepted" {
            let d = record(
                field(&event, "data"),
                &format!("{} data", event_label(&event)),
            )?;
            let accepted = d
                .get("sessionFormatVersion")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or(0.into());
            let inherited = self.source_header.contains_key("parentSession")
                && event_seq(&event).is_some_and(|seq| seq < self.cut);
            if number_eq(&accepted, 0)
                && !inherited
                && d["sessionId"] != *field(&self.source_header, "id")
            {
                return Err(SessionFormatError::new(
                    "current-generation delivery marker names the wrong Session",
                ));
            }
        }
        context.emit_event(event)
    }
    fn transform_run(
        &mut self,
        run: Box<dyn SessionFormatEventRun>,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<(), SessionFormatError> {
        if is_released_assistant_chunk_run(run.as_ref()) {
            return context.emit_run(run);
        }
        for event in run.expand() {
            self.transform_event(event, context)?;
        }
        Ok(())
    }
    fn finish(
        &mut self,
        _context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<u64, SessionFormatError> {
        Ok(self.cut)
    }
}
fn assert_header_version(
    header: &SessionFormatHeader,
    version: u64,
) -> Result<(), SessionFormatError> {
    if !number_eq(field(header, "version"), version) {
        return Err(SessionFormatError::new(format!(
            "expected format v{version} header"
        )));
    }
    Ok(())
}
#[derive(Default)]
struct LegacyNormalizationState {
    message_ids: HashMap<u64, String>,
    retry_ids: HashMap<String, String>,
    compaction_id: Option<String>,
}
fn malformed(id: &str, kind: &str, seq: &str) -> SessionFormatError {
    SessionFormatError::new(format!(
        "session {} contains malformed pre-react-loop {kind} at seq {seq}",
        js_json_stringify(&Value::from(id))
    ))
}
fn data_of(event: &SessionFormatEvent) -> Result<Value, SessionFormatError> {
    Ok(record(
        field(event, "data"),
        &format!("{} data", event_label(event)),
    )?
    .clone())
}
fn legacy_id(id: &str, seq: &str) -> String {
    format!("legacy-message:{id}:{seq}")
}
fn normalize_released_v0_event(
    mut event: SessionFormatEvent,
    id: &str,
    state: &mut LegacyNormalizationState,
) -> Result<SessionFormatEvent, SessionFormatError> {
    let renamed = match event_type(&event) {
        "compact/start" => Some("compaction/start"),
        "compact/summary" => Some("compaction/summary"),
        "compact/end" => Some("compaction/end"),
        "compact/prune" => Some("compaction/prune"),
        _ => None,
    };
    if let Some(kind) = renamed {
        event.insert("type".into(), kind.into());
    }
    let kind = event_type(&event).to_owned();
    let seq = event_seq_text(&event);
    if matches!(kind.as_str(), "request/header-delta" | "mode/set") {
        return Err(SessionFormatError::unsupported_migration(format!(
            "session {} contains unsupported legacy {kind} event at seq {seq}",
            js_json_stringify(&Value::from(id))
        )));
    }
    if kind == "request/header" && data_of(&event)?["reason"] == "fallback" {
        return Err(SessionFormatError::unsupported_migration(format!(
            "session {} contains unsupported request/header reason \"fallback\" at seq {seq}",
            js_json_stringify(&Value::from(id))
        )));
    }
    if kind == "turn/start" {
        let d = data_of(&event)?;
        if has(&d, "trigger") {
            keys(
                &d,
                &["turn", "trigger"],
                &[],
                &format!("turn/start {seq} data"),
            )?;
            let turn = session_format_count(&d["turn"], &format!("turn/start {seq} turn"))?;
            let trigger = record(&d["trigger"], &format!("turn/start {seq} trigger"))?;
            if turn < 1 || !trigger["kind"].as_str().is_some_and(|s| !s.is_empty()) {
                return Err(malformed(id, "turn/start", &seq));
            }
            event.insert("data".into(), json!({"turn":turn}));
        }
    }
    if kind == "turn/end" {
        let mut d = data_of(&event)?;
        keys(
            &d,
            &["turn", "reason"],
            &[],
            &format!("turn/end {seq} data"),
        )?;
        if session_format_count(&d["turn"], &format!("turn/end {seq} turn"))? < 1 {
            return Err(malformed(id, "turn/end", &seq));
        }
        let r = record(&d["reason"], &format!("turn/end {seq} reason"))?;
        let Some(reason_kind) = r["kind"].as_str() else {
            return Err(malformed(id, "turn/end", &seq));
        };
        let current = match reason_kind {
            "completed" | "blocked" | "max-tokens" | "interrupted" => {
                keys(r, &["kind"], &[], &format!("turn/end {seq} reason"))?;
                None
            }
            "aborted" if !has(r, "reason") => {
                keys(r, &["kind"], &[], &format!("turn/end {seq} reason"))?;
                Some(json!({"kind":"aborted","reason":{"kind":"legacy"}}))
            }
            "disposed" => {
                keys(r, &["kind"], &[], &format!("turn/end {seq} reason"))?;
                Some(json!({"kind":"aborted","reason":{"kind":"disposed"}}))
            }
            "error" if !has(r, "error") => Some(normalize_legacy_error_reason(r, &seq, id)?),
            _ => None,
        };
        if let Some(r) = current {
            d["reason"] = r;
            event.insert("data".into(), d);
        }
    }
    if kind == "request/header" {
        let mut d = data_of(&event)?;
        let h = record(&d["header"], &format!("request/header {seq} header"))?;
        if has(h, "messagePrefix") {
            if !h["messagePrefix"].is_array() {
                return Err(SessionFormatError::new(format!(
                    "session {} contains malformed request/header messagePrefix at seq {seq}",
                    js_json_stringify(&Value::from(id))
                )));
            }
            let mut h = h.clone();
            remove(&mut h, "messagePrefix");
            d["header"] = h;
            event.insert("data".into(), d);
        }
    }
    if kind == "steering/message" {
        let d = data_of(&event)?;
        let l = format!("steering/message {seq}");
        let data = if has(&d, "message") {
            keys(&d, &["turn", "message"], &[], &format!("{l} data"))?;
            session_format_count(&d["turn"], &format!("{l} turn"))?;
            d["message"].clone()
        } else {
            keys(
                &d,
                &["turn", "content", "source"],
                &[],
                &format!("{l} data"),
            )?;
            session_format_count(&d["turn"], &format!("{l} turn"))?;
            let mut m = d;
            remove(&mut m, "turn");
            m["id"] = legacy_id(id, &seq).into();
            m["role"] = "user".into();
            m
        };
        event.insert("type".into(), "user/message".into());
        event.insert("data".into(), data);
    }
    normalize_legacy_retry(&mut event, id, &mut state.retry_ids)?;
    normalize_legacy_compaction(&mut event, id, state)?;
    normalize_legacy_message(&mut event, id, &state.message_ids)?;
    if event_type(&event) != "assistant/chunk" {
        assert_released_event_payload(&event, 0)?;
    }
    let d = data_of(&event)?;
    let message = if event_type(&event) == "user/message" {
        &d
    } else {
        &d["message"]
    };
    if let Some(message_id) = message["id"].as_str()
        && let Some(seq) = event_seq(&event)
    {
        state.message_ids.insert(seq, message_id.to_owned());
    }
    Ok(event)
}
fn remove(value: &mut Value, key: &str) {
    if let Some(object) = value.as_object_mut() {
        object.shift_remove(key);
    }
}
fn normalize_legacy_error_reason(
    r: &Value,
    seq: &str,
    id: &str,
) -> Result<Value, SessionFormatError> {
    session_format_count(&r["step"], &format!("turn/end {seq} error step"))?;
    if has(r, "failure") {
        keys(
            r,
            &["kind", "step", "failure"],
            &[],
            &format!("turn/end {seq} reason"),
        )?;
        let f = record(&r["failure"], &format!("turn/end {seq} failure"))?;
        keys(
            f,
            &["message", "code"],
            &["status", "providerRetryAfterMs", "requestId"],
            &format!("turn/end {seq} failure"),
        )?;
        if !f["message"].is_string() || !f["code"].is_string() {
            return Err(malformed(id, "turn/end", seq));
        }
        return Ok(json!({"kind":"error","error":f}));
    }
    keys(
        r,
        &["kind", "step", "message"],
        &["code"],
        &format!("turn/end {seq} reason"),
    )?;
    if !r["message"].is_string() || (has(r, "code") && !r["code"].is_string()) {
        return Err(malformed(id, "turn/end", seq));
    }
    Ok(
        json!({"kind":"error","error":{"message":r["message"],"code":r["code"].as_str().unwrap_or("UNKNOWN")}}),
    )
}
fn normalize_legacy_retry(
    event: &mut SessionFormatEvent,
    id: &str,
    ids: &mut HashMap<String, String>,
) -> Result<(), SessionFormatError> {
    if event_type(event) != "llm/retry" {
        return Ok(());
    }
    let mut d = data_of(event)?;
    let seq = event_seq_text(event);
    let chain = ["turn", "step", "provider", "policyKey"]
        .map(|key| d.get(key).map(js_json_stringify).unwrap_or_default())
        .join("\0");
    if let Some(retry_id) = d["retryId"].as_str().filter(|s| !s.is_empty()) {
        ids.insert(chain, retry_id.to_owned());
        return Ok(());
    }
    if has(&d, "retryId") {
        return Ok(());
    }
    let retry_id = ids
        .entry(chain)
        .or_insert_with(|| format!("legacy-retry:{id}:{seq}"))
        .clone();
    d["retryId"] = retry_id.into();
    event.insert("data".into(), d);
    Ok(())
}
fn normalize_legacy_compaction(
    event: &mut SessionFormatEvent,
    id: &str,
    state: &mut LegacyNormalizationState,
) -> Result<(), SessionFormatError> {
    let kind = event_type(event);
    let seq = event_seq_text(event);
    if kind == "session/end-seed" {
        state.compaction_id = None;
        return Ok(());
    }
    if kind == "compaction/start" {
        let mut d = data_of(event)?;
        if let Some(existing) = d["compactionId"].as_str().filter(|s| !s.is_empty()) {
            state.compaction_id = Some(existing.to_owned());
            return Ok(());
        }
        if has(&d, "compactionId") {
            return Ok(());
        }
        let value = format!("legacy-compaction:{id}:{seq}");
        state.compaction_id = Some(value.clone());
        d["compactionId"] = value.into();
        event.insert("data".into(), d);
        return Ok(());
    }
    let Some(compaction_id) = state.compaction_id.clone() else {
        return Ok(());
    };
    if matches!(kind, "compaction/summary" | "compaction/end") {
        let end = kind == "compaction/end";
        let mut d = data_of(event)?;
        if !has(&d, "compactionId") {
            d["compactionId"] = compaction_id.into();
            event.insert("data".into(), d);
        }
        if end {
            state.compaction_id = None;
        }
        return Ok(());
    }
    if kind != "user/message" {
        return Ok(());
    }
    let mut d = data_of(event)?;
    let s = &d["source"];
    if !s.is_object() || s["kind"] != "plugin" || s["plugin"] != "compact" || has(s, "compactionId")
    {
        return Ok(());
    }
    d["source"]["compactionId"] = compaction_id.into();
    event.insert("data".into(), d);
    Ok(())
}
fn normalize_legacy_message(
    event: &mut SessionFormatEvent,
    id: &str,
    ids: &HashMap<u64, String>,
) -> Result<(), SessionFormatError> {
    let mut d = data_of(event)?;
    let seq = event_seq_text(event);
    match event_type(event) {
        "user/message" => {
            if has(&d, "id")
                || has(&d, "role")
                || has(&d, "message")
                || !has(&d, "content")
                || !has(&d, "source")
            {
                return Ok(());
            }
            d["id"] = legacy_id(id, &seq).into();
            d["role"] = "user".into();
        }
        "assistant/message" => {
            if has(&d, "message") || !has(&d, "content") || !has(&d, "provenance") {
                return Ok(());
            }
            let content = d["content"].clone();
            remove(&mut d, "content");
            let mut source = record(
                &d["provenance"],
                &format!("assistant/message {seq} legacy source"),
            )?
            .clone();
            remove(&mut d, "provenance");
            source["kind"] = "model".into();
            d["message"] = json!({"id":legacy_id(id,&seq),"role":"assistant","content":content,"source":source});
        }
        "tool/result" => {
            if has(&d, "message")
                || !has(&d, "callId")
                || !has(&d, "content")
                || !has(&d, "isError")
            {
                return Ok(());
            }
            let call_id = d["callId"].clone();
            let content = d["content"].clone();
            let is_error = d["isError"].clone();
            if !call_id.is_string() || !is_error.is_boolean() {
                return Ok(());
            }
            let operation = field(event, "surfaceOp");
            let message_id = if operation.is_object() && operation["op"] == "replace" {
                let start = exact_u64(&operation["start"]);
                start.and_then(|n| ids.get(&n).cloned()).ok_or_else(|| {
                    SessionFormatError::new(format!(
                        "tool/result {seq} replacement cites a message without identity"
                    ))
                })?
            } else {
                legacy_id(id, &seq)
            };
            for key in ["callId", "content", "isError"] {
                remove(&mut d, key);
            }
            d["message"] = json!({"id":message_id,"role":"user","content":[{"type":"tool-result","toolCallId":call_id,"content":content,"isError":is_error}],"source":{"kind":"tool","callId":call_id}});
        }
        _ => return Ok(()),
    }
    event.insert("data".into(), d);
    Ok(())
}
