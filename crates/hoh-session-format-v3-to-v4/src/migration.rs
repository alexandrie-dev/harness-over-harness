use crate::{
    content::migrate_v3_event_content,
    extension_identities::{RELEASED_V3_EVENT_TYPES, namespace_v3_opaque_event},
    facts::*,
    helpers::*,
    references::remap_v3_references,
    sources::{map_event_messages, rewrite_v3_message_source},
    tool_role::lift_tool_result,
    validation::{assert_released_v4_header, validate_delivery_accepted},
};
use hoh_session_format::*;
use hoh_session_format_v2_to_v3::assert_released_v3_header;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Debug, Clone, Copy)]
pub struct SessionFormatV3ToV4;
pub const SESSION_FORMAT_V3_TO_V4: SessionFormatV3ToV4 = SessionFormatV3ToV4;
impl SessionFormatMigration for SessionFormatV3ToV4 {
    fn name(&self) -> &str {
        "@deepseek-ai/dsh-session-format-v3-to-v4"
    }
    fn from_version(&self) -> u64 {
        3
    }
    fn to_version(&self) -> u64 {
        4
    }
    fn migrate_header(&self, mut header: SessionFormatHeader) -> Result<SessionFormatHeader> {
        assert_released_v3_header(&header)?;
        header.insert("version".into(), 4.into());
        Ok(header)
    }
    fn create_stage(
        &self,
        _input: SessionFormatMigrationStageInput<'_>,
    ) -> Result<Box<dyn SessionFormatMigrationStage>> {
        Err(refusal(
            "V3 catalog migration requires explicit historical child facts, including an empty array for a parent without children",
        ))
    }
    fn validate_target_header(&self, header: &SessionFormatHeader) -> Result {
        assert_released_v4_header(header)
    }
}
struct WithChildren {
    children: Arc<[Value]>,
}
/// Bind complete immutable historical child evidence, including an explicitly empty set.
pub fn create_session_format_v3_to_v4(children: Vec<Value>) -> Arc<dyn SessionFormatMigration> {
    Arc::new(WithChildren {
        children: children.into(),
    })
}
impl SessionFormatMigration for WithChildren {
    fn name(&self) -> &str {
        SESSION_FORMAT_V3_TO_V4.name()
    }
    fn from_version(&self) -> u64 {
        3
    }
    fn to_version(&self) -> u64 {
        4
    }
    fn migrate_header(&self, header: SessionFormatHeader) -> Result<SessionFormatHeader> {
        SESSION_FORMAT_V3_TO_V4.migrate_header(header)
    }
    fn validate_target_header(&self, header: &SessionFormatHeader) -> Result {
        assert_released_v4_header(header)
    }
    fn create_stage(
        &self,
        input: SessionFormatMigrationStageInput<'_>,
    ) -> Result<Box<dyn SessionFormatMigrationStage>> {
        Ok(Box::new(Stage::new(input, &self.children)?))
    }
}
struct Stage {
    header: SessionFormatHeader,
    source_inherited_event_count: Option<u64>,
    candidates: Vec<SessionFormatJsonObject>,
    catalogs: Vec<Value>,
    cut: Option<u64>,
    source_cut: Option<u64>,
    mapping: Vec<u64>,
    turn: Option<Value>,
    step_open: bool,
    next_turn_spliced: bool,
    next_seq: u64,
    time: Value,
    foreign_delivery_seq: Option<u64>,
}
impl Stage {
    fn new(input: SessionFormatMigrationStageInput<'_>, children: &[Value]) -> Result<Self> {
        let mut candidates = children
            .iter()
            .map(|v| child_catalog_source(v).cloned())
            .collect::<Result<Vec<_>>>()?;
        candidates.sort_by(|a, b| {
            let time = field(a, "childCreatedAt")
                .as_f64()
                .partial_cmp(&field(b, "childCreatedAt").as_f64())
                .unwrap_or(std::cmp::Ordering::Equal);
            time.then_with(|| {
                field(a, "childId")
                    .as_str()
                    .unwrap_or("")
                    .encode_utf16()
                    .cmp(field(b, "childId").as_str().unwrap_or("").encode_utf16())
            })
        });
        let cut = (field(input.source_header, "isSeeded") != true).then_some(0);
        Ok(Self {
            header: input.source_header.clone(),
            source_inherited_event_count: input.source_inherited_event_count,
            candidates,
            catalogs: vec![],
            cut,
            source_cut: cut,
            mapping: vec![],
            turn: None,
            step_open: false,
            next_turn_spliced: false,
            next_seq: 0,
            time: field(input.source_header, "createdAt").clone(),
            foreign_delivery_seq: None,
        })
    }
    fn observe_restart(&mut self, event: &SessionFormatEvent) -> Option<Value> {
        let data = field(event, "data");
        let interrupted = if kind(event) == "turn/start"
            && !self.step_open
            && self.next_turn_spliced
            && self
                .turn
                .as_ref()
                .and_then(Value::as_f64)
                .is_some_and(|n| data.get("turn").and_then(Value::as_f64) == Some(n + 1.))
        {
            self.turn.clone()
        } else {
            None
        };
        self.next_turn_spliced = kind(event) == "agent/inbox/spliced"
            && data.get("target").is_some_and(|t| t == "next-turn")
            && data
                .get("inserted")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty());
        match kind(event) {
            "turn/start" => {
                if let Some(turn) = data.get("turn").filter(|v| v.is_number()) {
                    self.turn = Some(turn.clone());
                }
            }
            "turn/end" => self.turn = None,
            "step/start" => self.step_open = true,
            "step/end" => self.step_open = false,
            _ => {}
        }
        interrupted
    }
}
impl SessionFormatMigrationStage for Stage {
    fn header_inherited_event_count(&self) -> Option<u64> {
        (field(&self.header, "isSeeded") != true).then_some(0)
    }
    fn transform_event(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        if !number_eq(event.get("seq"), self.mapping.len() as u64) {
            return Err(error("V3 source events must be dense"));
        }
        let source_seq = self.mapping.len() as u64;
        if let Some(turn) = self.observe_restart(&event) {
            let mut data = SessionFormatJsonObject::new();
            data.insert("turn".into(), turn);
            data.insert("reason".into(), json!({"kind":"interrupted"}));
            let mut end = SessionFormatJsonObject::new();
            end.insert("type".into(), "turn/end".into());
            end.insert("seq".into(), self.next_seq.into());
            self.next_seq += 1;
            end.insert("time".into(), field(&event, "time").clone());
            end.insert("data".into(), data.into());
            context.emit_event(end)?;
        }
        let target_seq = self.next_seq;
        self.next_seq += 1;
        self.time = field(&event, "time").clone();
        if kind(&event) == "session/end-seed"
            && field(&event, "data")
                .get("inherited")
                .is_some_and(|v| v == true)
        {
            if field(&self.header, "isSeeded") != true {
                return Err(error(
                    "unseeded format v3 Session contains an inherited end-seed marker",
                ));
            }
            self.source_cut = Some(source_seq);
            self.cut = Some(target_seq);
            self.catalogs.clear();
        } else if kind(&event) == "subagent/catalog" {
            self.catalogs.push(field(&event, "data").clone());
        }
        let delivery_id = validate_delivery_accepted(&event, 3)?;
        if kind(&event) == "session-log-deepseek/delivery-accepted" {
            if field(&event, "data")
                .get("sessionFormatVersion")
                .is_some_and(|v| number_eq(Some(v), 4))
            {
                return Err(refusal("format v3 delivery marker claims target format v4"));
            }
            if delivery_id.is_some_and(|id| field(&self.header, "id") != id) {
                self.foreign_delivery_seq = Some(source_seq);
            }
        }
        if field(&event, "ignorable") == true && !RELEASED_V3_EVENT_TYPES.contains(&kind(&event)) {
            let mut opaque = namespace_v3_opaque_event(event);
            self.mapping.push(target_seq);
            opaque.insert("seq".into(), target_seq.into());
            return context.emit_event(opaque);
        }
        if !RELEASED_V3_EVENT_TYPES.contains(&kind(&event)) {
            return Err(refusal(format!(
                "format v3 contains unknown event type {} at seq {}",
                quoted(event.get("type")),
                text(event.get("seq"))
            )));
        }
        let remapped = remap_v3_references(event, target_seq, &self.mapping)?;
        self.mapping.push(target_seq);
        let rewritten = map_event_messages(remapped, |mut message| {
            if let Some(source) = field(&message, "source").as_object() {
                let converted =
                    rewrite_v3_message_source(source.clone(), source_seq, message.get("role"))?;
                message.insert("source".into(), converted.into());
            }
            Ok(message)
        })?;
        context.emit_event(migrate_v3_event_content(lift_tool_result(rewritten)?)?)
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
    fn finish(&mut self, context: &mut dyn SessionFormatMigrationContext) -> Result<u64> {
        let cut = self
            .cut
            .ok_or_else(|| error("V3 inherited event count must be a non-negative safe integer"))?;
        let source_cut = self.source_cut.ok_or_else(|| {
            error("V3 source inherited event count must be a non-negative safe integer")
        })?;
        let mut existing_catalogs: IndexMap<String, SessionFormatJsonObject> = IndexMap::new();
        for data in &self.catalogs {
            let fact = catalog_fact(data)?;
            let id = text(fact.get("childId"));
            if existing_catalogs.contains_key(&id) {
                return Err(refusal(format!("duplicate catalog child {id}")));
            }
            existing_catalogs.insert(id, fact.clone());
        }
        if self
            .source_inherited_event_count
            .is_some_and(|n| n != source_cut)
        {
            return Err(error(
                "format v3 inherited cut disagrees with its source marker",
            ));
        }
        if self
            .foreign_delivery_seq
            .is_some_and(|n| !self.header.contains_key("parentSession") || n >= source_cut)
        {
            return Err(error(
                "current-generation delivery marker names the wrong Session",
            ));
        }
        for source in &self.candidates {
            let id = text(source.get("childId"));
            let fact = child_catalog_fact(source)?;
            if let Some(existing) = existing_catalogs.get(&id) {
                if !strict_eq(existing.get("childCreatedAt"), source.get("childCreatedAt"))
                    || fact.as_ref().is_some_and(|fact| {
                        ["mode", "label"]
                            .iter()
                            .any(|k| !strict_eq(existing.get(*k), fact.get(*k)))
                    })
                {
                    return Err(refusal(format!(
                        "{} conflicts with its parent catalog",
                        child_catalog_subject(source)
                    )));
                }
                continue;
            }
            let entry = fact.unwrap_or_else(|| {
                let mut e = SessionFormatJsonObject::new();
                e.insert("version".into(), 1.into());
                e.insert("childId".into(), id.clone().into());
                e.insert(
                    "childCreatedAt".into(),
                    field(source, "childCreatedAt").clone(),
                );
                e.insert("mode".into(), "unknown".into());
                e
            });
            existing_catalogs.insert(id, entry.clone());
            let mut event = SessionFormatJsonObject::new();
            event.insert("type".into(), "subagent/catalog".into());
            event.insert("seq".into(), self.next_seq.into());
            self.next_seq += 1;
            event.insert("time".into(), self.time.clone());
            event.insert("data".into(), entry.into());
            context.emit_event(event)?;
        }
        Ok(cut)
    }
}
