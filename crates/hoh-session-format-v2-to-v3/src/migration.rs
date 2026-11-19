use crate::{
    helpers::*,
    payload::{SURFACE_TYPES, assert_event, canonicalize_transformed_event, record},
    references::remap_event,
    validation::assert_released_v3_header,
};
use hoh_session_format::{
    SessionFormatEvent, SessionFormatEventRun, SessionFormatHeader, SessionFormatMigration,
    SessionFormatMigrationContext, SessionFormatMigrationStage, SessionFormatMigrationStageInput,
    session_format_count,
};
use hoh_session_format_v1_to_v2::assert_released_v2_header;
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
#[derive(Debug, Clone, Copy)]
pub struct SessionFormatV2ToV3;
pub const SESSION_FORMAT_V2_TO_V3: SessionFormatV2ToV3 = SessionFormatV2ToV3;
impl SessionFormatMigration for SessionFormatV2ToV3 {
    fn name(&self) -> &str {
        "@deepseek-ai/dsh-session-format-v2-to-v3"
    }
    fn from_version(&self) -> u64 {
        2
    }
    fn to_version(&self) -> u64 {
        3
    }
    fn migrate_header(&self, mut header: SessionFormatHeader) -> Result<SessionFormatHeader> {
        assert_released_v2_header(&header)?;
        header.insert("version".into(), 3.into());
        if field(&header, "agentPreset") == "code" {
            header.insert("agentPreset".into(), "ptc".into());
        }
        Ok(header)
    }
    fn validate_target_header(&self, header: &SessionFormatHeader) -> Result {
        assert_released_v3_header(header)
    }
    fn create_stage(
        &self,
        input: SessionFormatMigrationStageInput<'_>,
    ) -> Result<Box<dyn SessionFormatMigrationStage>> {
        assert_released_v2_header(input.source_header)?;
        let cut = if field(input.source_header, "isSeeded") == true {
            None
        } else {
            Some(0)
        };
        Ok(Box::new(Stage {
            source_header: input.source_header.clone(),
            declared_cut: input.source_inherited_event_count,
            mapping: Vec::new(),
            original_ids: HashSet::new(),
            generated_ids: HashSet::new(),
            target_seq: 0,
            source_cut: cut,
            target_cut: cut,
            last_foreign_delivery_seq: None,
            step: None,
            head: None,
            prompt: String::new(),
        }))
    }
}
struct Stage {
    source_header: SessionFormatHeader,
    declared_cut: Option<u64>,
    mapping: Vec<u64>,
    original_ids: HashSet<String>,
    generated_ids: HashSet<String>,
    target_seq: u64,
    source_cut: Option<u64>,
    target_cut: Option<u64>,
    last_foreign_delivery_seq: Option<u64>,
    step: Option<(Value, Value)>,
    head: Option<u64>,
    prompt: String,
}
impl SessionFormatMigrationStage for Stage {
    fn header_inherited_event_count(&self) -> Option<u64> {
        if field(&self.source_header, "isSeeded") == true {
            None
        } else {
            Some(0)
        }
    }
    fn transform_event(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        if !number_eq(field(&event, "seq"), self.mapping.len() as u64) {
            return Err(error("format v2 source events must be dense"));
        }
        assert_event(&event, 2)?;
        self.observe_message_ids(&event)?;
        let data = record(field(&event, "data"), kind(&event))?;
        let mut source = event.clone();
        if kind(&event) == "request/header" {
            let mut header = record(field(data, "header"), "request header")?.clone();
            let system = header.shift_remove("system");
            let prompt = system.as_ref().and_then(Value::as_str).unwrap_or("");
            if prompt != self.prompt {
                self.emit_system(prompt, &event, context)?;
            }
            let mut data = data.clone();
            data.insert("header".into(), Value::Object(header));
            source.insert("data".into(), Value::Object(data));
        }
        if SURFACE_TYPES.contains(&kind(&event)) && self.head.is_none() {
            return Err(refusal(
                "format v2 surface before first step cannot acquire a system head without changing chronology",
            ));
        }
        let seq = session_format_count(field(&event, "seq"), "event seq")?;
        if kind(&event) == "session/end-seed" && field(data, "inherited") == true {
            if field(&self.source_header, "isSeeded") != true {
                return Err(error(
                    "format v2 unseeded Session contains an inherited end-seed marker",
                ));
            }
            self.source_cut = Some(seq);
            self.target_cut = Some(self.target_seq);
        }
        if kind(&event) == "session-log-deepseek/delivery-accepted" {
            if number_eq(field(data, "sessionFormatVersion"), 3) {
                return Err(error("format v2 delivery marker claims target format v3"));
            }
            if number_eq(field(data, "sessionFormatVersion"), 2)
                && field(data, "sessionId") != field(&self.source_header, "id")
            {
                self.last_foreign_delivery_seq = Some(seq);
            }
        }
        let target = remap_event(source, self.target_seq, &self.mapping)?;
        self.mapping.push(self.target_seq);
        self.target_seq += 1;
        context.emit_event(canonicalize_transformed_event(rename_ptc_event(target)?)?)?;
        if kind(&event) == "step/start" {
            self.step = Some((field(data, "turn").clone(), field(data, "step").clone()));
            if self.head.is_none() {
                self.emit_system("", &event, context)?;
            }
        } else if matches!(kind(&event), "step/end" | "turn/end") {
            self.step = None;
        }
        Ok(())
    }
    fn transform_run(
        &mut self,
        run: Box<dyn SessionFormatEventRun>,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        for event in run.expand() {
            self.transform_event(event, context)?;
        }
        Ok(())
    }
    fn finish(&mut self, _context: &mut dyn SessionFormatMigrationContext) -> Result<u64> {
        let cut = session_format_count(
            &self.source_cut.map(Value::from).unwrap_or(Value::Null),
            "format v2 inherited end-seed marker",
        )?;
        if self.declared_cut.is_some_and(|n| n != cut) {
            return Err(error(
                "format v2 inherited end-seed marker disagrees with its source cut",
            ));
        }
        if self
            .last_foreign_delivery_seq
            .is_some_and(|seq| !self.source_header.contains_key("parentSession") || seq >= cut)
        {
            return Err(error(
                "current-generation delivery marker names the wrong Session",
            ));
        }
        session_format_count(
            &self.target_cut.map(Value::from).unwrap_or(Value::Null),
            "format v3 inherited event count",
        )
    }
}
impl Stage {
    fn observe_message_ids(&mut self, event: &SessionFormatEvent) -> Result {
        let data = record(field(event, "data"), kind(event))?;
        let messages: Vec<&Value> = match kind(event) {
            "user/message" => vec![field(event, "data")],
            "assistant/message" | "tool/result" => vec![field(data, "message")],
            "agent/inbox/spliced" | "session/title-llm-request" => field(
                data,
                if kind(event) == "agent/inbox/spliced" {
                    "inserted"
                } else {
                    "messages"
                },
            )
            .as_array()
            .map(|v| v.iter().collect())
            .unwrap_or_default(),
            _ => Vec::new(),
        };
        for message in messages {
            let message = record(message, "message")?;
            let id = field(message, "id").as_str().unwrap_or("");
            if self.generated_ids.contains(id) {
                return Err(refusal(
                    "source message id collides with a generated system message id",
                ));
            }
            self.original_ids.insert(id.to_owned());
        }
        Ok(())
    }
    fn emit_system(
        &mut self,
        prompt: &str,
        anchor: &SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        let Some((turn, step)) = &self.step else {
            return Err(refusal(
                "format v2 changed request prompt outside an open step cannot retain source chronology",
            ));
        };
        let identity = js_json_stringify(&json!([
            "session-format-v2-to-v3",
            field(&self.source_header, "id"),
            field(anchor, "seq"),
            field(anchor, "type")
        ]));
        let digest = Sha256::digest(identity.as_bytes());
        let mut id = String::from("v2-to-v3-system-");
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(&mut id, "{byte:02x}");
        }
        if self.original_ids.contains(&id) || self.generated_ids.contains(&id) {
            return Err(refusal(
                "generated system message id collides with an existing message id",
            ));
        }
        self.generated_ids.insert(id.clone());
        let seq = self.target_seq;
        self.target_seq += 1;
        let mut event = object(
            json!({"type":"system/message","seq":seq,"time":field(anchor,"time"),"data":{"turn":turn,"step":step,"message":{"id":id,"role":"system","source":{"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"},"content":if prompt.is_empty() {json!([])}else{json!([{"type":"text","text":prompt}])}}}}),
        );
        if let Some(head) = self.head {
            event.insert(
                "surfaceOp".into(),
                json!({"op":"replace","start":head,"end":head}),
            );
            event.insert("sourceEventSeqs".into(), json!([head]));
        } else {
            event.insert("surfaceOp".into(), "append".into());
        }
        context.emit_event(canonicalize_transformed_event(event)?)?;
        self.head = Some(seq);
        self.prompt = prompt.to_owned();
        Ok(())
    }
}
fn rename_ptc_event(mut event: SessionFormatEvent) -> Result<SessionFormatEvent> {
    match kind(&event) {
        "agent-preset/selected" => {
            let mut data = record(field(&event, "data"), "agent-preset/selected")?.clone();
            if field(&data, "agentPreset") == "code" {
                data.insert("agentPreset".into(), "ptc".into());
                event.insert("data".into(), Value::Object(data));
            }
        }
        "tool/code-dispatch-start" => {
            event.insert("type".into(), "tool/ptc-dispatch-start".into());
        }
        "tool/code-dispatch" => {
            event.insert("type".into(), "tool/ptc-dispatch".into());
        }
        "user/message" => {
            let data = rename_message_source(field(&event, "data"))?;
            event.insert("data".into(), data);
        }
        "agent/inbox/spliced" | "session/title-llm-request" => {
            let key = if kind(&event) == "agent/inbox/spliced" {
                "inserted"
            } else {
                "messages"
            };
            let mut data = record(field(&event, "data"), kind(&event))?.clone();
            let messages = field(&data, key)
                .as_array()
                .ok_or_else(|| error("messages must be an array"))?;
            let renamed = messages
                .iter()
                .map(rename_message_source)
                .collect::<Result<Vec<_>>>()?;
            data.insert(key.into(), Value::Array(renamed));
            event.insert("data".into(), Value::Object(data));
        }
        _ => {}
    }
    Ok(event)
}
fn rename_message_source(message: &Value) -> Result<Value> {
    let mut message = record(message, "message")?.clone();
    let mut source = record(field(&message, "source"), "message source")?.clone();
    if field(&source, "kind") == "plugin" && field(&source, "plugin") == "tools-code-mode" {
        source.insert("plugin".into(), "tools-ptc".into());
        message.insert("source".into(), Value::Object(source));
    }
    Ok(Value::Object(message))
}
