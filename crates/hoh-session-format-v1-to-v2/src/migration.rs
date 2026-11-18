use crate::helpers::*;
use crate::validation::assert_released_v2_header;
use hoh_llm::AssistantStreamAccumulator;
use hoh_session_format::{
    SessionFormatEvent, SessionFormatEventRun, SessionFormatHeader, SessionFormatMigration,
    SessionFormatMigrationContext, SessionFormatMigrationStage, SessionFormatMigrationStageInput,
    SessionFormatSourceKind, session_format_count, session_format_safe_integer,
};
use hoh_session_format_v0_to_v1::{
    RELEASED_V0_EVENT_DISPOSITIONS, assert_released_event_payload, assert_released_v1_header,
    into_released_assistant_chunk_run,
};
use hoh_util_values::deep_equal_json;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub struct SessionFormatV1ToV2;
pub const SESSION_FORMAT_V1_TO_V2: SessionFormatV1ToV2 = SessionFormatV1ToV2;
impl SessionFormatMigration for SessionFormatV1ToV2 {
    fn name(&self) -> &str {
        "@deepseek-ai/dsh-session-format-v1-to-v2"
    }
    fn from_version(&self) -> u64 {
        1
    }
    fn to_version(&self) -> u64 {
        2
    }
    fn migrate_header(&self, mut header: SessionFormatHeader) -> Result<SessionFormatHeader> {
        assert_released_v1_header(&header)?;
        header.insert("version".into(), 2.into());
        Ok(header)
    }
    fn validate_target_header(&self, header: &SessionFormatHeader) -> Result {
        assert_released_v2_header(header)
    }
    fn create_stage(
        &self,
        input: SessionFormatMigrationStageInput<'_>,
    ) -> Result<Box<dyn SessionFormatMigrationStage>> {
        assert_released_v1_header(input.source_header)?;
        let source_cut = session_format_count(
            &input
                .source_inherited_event_count
                .map(Value::from)
                .unwrap_or(Value::Null),
            "format v1 inherited event count",
        )?;
        Ok(Box::new(Stage {
            source_header: input.source_header.clone(),
            source_cut,
            decoded: input.source_kind == SessionFormatSourceKind::Decoded,
            mapping: HashMap::new(),
            legacy_turns: LegacyTurnState::default(),
            pending: None,
            target_seq: 0,
            target_cut: if field(input.source_header, "isSeeded") == true {
                None
            } else {
                Some(0)
            },
            last_time: field(input.source_header, "createdAt").clone(),
        }))
    }
}
#[derive(Default)]
struct LegacyTurnState {
    open_turn: Option<u64>,
    open_step: Option<u64>,
    previous: Option<SessionFormatEvent>,
}
struct Span {
    first_seq: u64,
    event_count: u64,
}
struct StreamEntry {
    record: Value,
    last_time: i64,
}
struct AttemptGroup {
    turn: Value,
    step: Value,
    spans: Vec<Span>,
    stream: Vec<StreamEntry>,
    accumulator: Option<AssistantStreamAccumulator>,
    chunk_count: u64,
    last_chunk_seq: Option<u64>,
    last_chunk_time: Option<Value>,
    terminal: bool,
}
impl AttemptGroup {
    fn new(turn: Value, step: Value) -> Self {
        Self {
            turn,
            step,
            spans: Vec::new(),
            stream: Vec::new(),
            accumulator: None,
            chunk_count: 0,
            last_chunk_seq: None,
            last_chunk_time: None,
            terminal: false,
        }
    }
    fn record_span(&mut self, first_seq: u64, event_count: u64, last_time: Value) {
        if let Some(previous) = self.spans.last_mut()
            && previous.first_seq + previous.event_count == first_seq
        {
            previous.event_count += event_count;
        } else {
            self.spans.push(Span {
                first_seq,
                event_count,
            });
        }
        self.chunk_count += event_count;
        self.last_chunk_seq = Some(first_seq + event_count - 1);
        self.last_chunk_time = Some(last_time);
    }
    fn matches_sources(&self, sources: &[Value]) -> bool {
        if sources.len() as u64 != self.chunk_count {
            return false;
        }
        let mut index = 0;
        for span in &self.spans {
            for offset in 0..span.event_count {
                if !number_eq(&sources[index], span.first_seq + offset) {
                    return false;
                }
                index += 1;
            }
        }
        true
    }
    fn append_record(&mut self, source: Value, last_time: i64) -> Result {
        if let Some(previous) = self.stream.last_mut()
            && source["type"] != "chunk"
            && previous.record["type"] == source["type"]
        {
            let time0 = session_format_safe_integer(&source["time0"], "Assistant stream time")?;
            let gap = time0 - previous.last_time;
            let same = deep_equal_json(&previous.record["index"], &source["index"])
                && (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&gap)
                && (source["type"] != "tool-call-chunks"
                    || (previous.record["id"] == source["id"]
                        && previous.record.get("name") == source.get("name")));
            if same {
                let mut gaps = previous.record["dt"]
                    .as_array()
                    .cloned()
                    .ok_or_else(|| error("Assistant stream dt must be an array"))?;
                gaps.push(gap.into());
                gaps.extend(
                    source["dt"]
                        .as_array()
                        .cloned()
                        .ok_or_else(|| error("Assistant stream dt must be an array"))?,
                );
                previous.record["dt"] = Value::Array(gaps);
                let key = if source["type"] == "tool-call-chunks" {
                    "args"
                } else {
                    "texts"
                };
                let mut members = previous.record[key]
                    .as_array()
                    .cloned()
                    .ok_or_else(|| error("Assistant stream members must be an array"))?;
                members.extend(
                    source[key]
                        .as_array()
                        .cloned()
                        .ok_or_else(|| error("Assistant stream members must be an array"))?,
                );
                previous.record[key] = Value::Array(members);
                previous.last_time = last_time;
                return Ok(());
            }
        }
        self.stream.push(StreamEntry {
            record: source,
            last_time,
        });
        Ok(())
    }
    fn flush_accumulator(&mut self) -> Result {
        if let Some(accumulator) = &self.accumulator {
            for record in accumulator.snapshot() {
                let value = serde_json::to_value(record)
                    .map_err(hoh_session_format::SessionFormatError::other)?;
                let last = record_last_time(&value)?;
                self.append_record(value, last)?;
            }
            self.accumulator = None;
        }
        Ok(())
    }
    fn stream_of(&mut self) -> Result<Value> {
        self.flush_accumulator()?;
        Ok(Value::Array(
            self.stream
                .iter()
                .map(|entry| entry.record.clone())
                .collect(),
        ))
    }
    fn attempt_event(&mut self) -> Result<SessionFormatEvent> {
        let seq = self
            .last_chunk_seq
            .ok_or_else(|| error("Assistant attempt lacks chunks"))?;
        let time = self
            .last_chunk_time
            .clone()
            .ok_or_else(|| error("Assistant attempt lacks chunks"))?;
        let stream = self.stream_of()?;
        object(
            json!({"type":"assistant/attempt","seq":seq,"time":time,"data":{"turn":self.turn,"step":self.step,"stream":stream}}),
        )
    }
}
struct Pending {
    group: AttemptGroup,
    after_last_chunk: Vec<SessionFormatEvent>,
}
struct Stage {
    source_header: SessionFormatHeader,
    source_cut: u64,
    decoded: bool,
    mapping: HashMap<u64, u64>,
    legacy_turns: LegacyTurnState,
    pending: Option<Pending>,
    target_seq: u64,
    target_cut: Option<u64>,
    last_time: Value,
}
impl SessionFormatMigrationStage for Stage {
    fn transform_event(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        if self.decoded
            && kind(&event) != "assistant/chunk"
            && RELEASED_V0_EVENT_DISPOSITIONS.get(kind(&event)).is_some()
        {
            assert_released_event_payload(&event, 1)?;
        }
        self.transform_released_event(event, context)
    }
    fn transform_run(
        &mut self,
        run: Box<dyn SessionFormatEventRun>,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        let run = match into_released_assistant_chunk_run(run) {
            Ok(run) => run,
            Err(run) => {
                for event in run.expand() {
                    self.transform_released_event(event, context)?;
                }
                return Ok(());
            }
        };
        self.legacy_turns.previous = None;
        self.last_time = run.last_time.into();
        if self.pending.as_ref().is_some_and(|p| {
            p.group.terminal
                || !number_eq(&p.group.turn, run.turn)
                || !number_eq(&p.group.step, run.step)
        }) {
            self.finish_attempt(context)?;
        } else if self.pending.is_some() {
            self.flush_pending_buffer(context)?;
        }
        let pending = self.pending.get_or_insert_with(|| Pending {
            group: AttemptGroup::new(run.turn.into(), run.step.into()),
            after_last_chunk: Vec::new(),
        });
        assert_attempt_range(self.source_cut, run.first_seq, run.last_seq)?;
        pending.group.flush_accumulator()?;
        pending
            .group
            .append_record(Value::Object(run.stream), run.last_time)?;
        pending
            .group
            .record_span(run.first_seq, run.event_count, run.last_time.into());
        Ok(())
    }
    fn finish(&mut self, context: &mut dyn SessionFormatMigrationContext) -> Result<u64> {
        self.finish_attempt(context)?;
        if field(&self.source_header, "isSeeded") == true && self.target_cut.is_none() {
            self.target_cut = Some(self.target_seq);
            context.emit_event(marker(self.target_seq, self.last_time.clone())?)?;
            self.target_seq += 1;
        }
        self.target_cut
            .ok_or_else(|| error("format v2 inherited event count is not settled"))
    }
}
impl Stage {
    fn transform_released_event(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        if kind(&event) == "assistant/chunk" {
            assert_chunk_envelope(&event)?;
        }
        if RELEASED_V0_EVENT_DISPOSITIONS.get(kind(&event)).is_none() {
            return Err(refusal(format!(
                "format v1 contains unknown event type {} at seq {}",
                quoted(event.get("type")),
                text(event.get("seq"))
            )));
        }
        let interrupted = legacy_interrupted_turn(&self.legacy_turns, &event)?;
        if kind(&event) == "turn/start"
            && self.legacy_turns.open_turn.is_some()
            && interrupted.is_none()
        {
            return Err(refusal(format!(
                "turn/start {} does not close the prior turn",
                quoted(field(&event, "data").get("turn"))
            )));
        }
        self.assert_delivery_marker(&event)?;
        observe_legacy_turn(&mut self.legacy_turns, &event)?;
        self.last_time = field(&event, "time").clone();
        if let Some(interrupted) = interrupted {
            self.finish_attempt(context)?;
            self.emit_generated(seq(&event)?, interrupted, context)?;
        }
        if let Some((change, message)) = split_legacy_goal_change(&event)? {
            self.emit_generated(seq(&event)?, change, context)?;
            self.emit_source(message, context)?;
            return Ok(());
        }
        if kind(&event) == "assistant/chunk" {
            return self.transform_chunk(event, context);
        }
        if kind(&event) == "assistant/message" {
            return self.transform_message(event, context);
        }
        if matches!(
            kind(&event),
            "turn/end" | "step/end" | "llm/retry" | "llm/retry-started"
        ) {
            self.finish_attempt(context)?;
            return self.emit_source(event, context);
        }
        if let Some(pending) = &mut self.pending {
            pending.after_last_chunk.push(event);
            return Ok(());
        }
        self.emit_source(event, context)
    }
    fn assert_delivery_marker(&self, event: &SessionFormatEvent) -> Result {
        if kind(event) != "session-log-deepseek/delivery-accepted" {
            return Ok(());
        }
        let data = record(field(event, "data"))?;
        let inherited =
            self.source_header.contains_key("parentSession") && seq(event)? < self.source_cut;
        if number_eq(field(data, "sessionFormatVersion"), 1)
            && !inherited
            && field(data, "sessionId") != field(&self.source_header, "id")
        {
            return Err(refusal(
                "current-generation delivery marker names the wrong Session",
            ));
        }
        Ok(())
    }
    fn transform_chunk(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        let data = field(&event, "data");
        let turn = property_member(data, "turn")?
            .unwrap_or(&Value::Null)
            .clone();
        let step = property_member(data, "step")?
            .unwrap_or(&Value::Null)
            .clone();
        let chunk = property_member(data, "chunk")?;
        if self.pending.as_ref().is_some_and(|p| {
            p.group.terminal
                || !same_coordinate(&p.group.turn, &turn)
                || !same_coordinate(&p.group.step, &step)
        }) {
            self.finish_attempt(context)?;
        } else if self.pending.is_some() {
            self.flush_pending_buffer(context)?;
        }
        let pending = self.pending.get_or_insert_with(|| Pending {
            group: AttemptGroup::new(turn, step),
            after_last_chunk: Vec::new(),
        });
        let member = seq(&event)?;
        let first = pending
            .group
            .spans
            .first()
            .map_or(member, |span| span.first_seq);
        assert_attempt_range(self.source_cut, first, member)?;
        let mut timed = json!({"time":field(&event,"time")});
        if let Some(chunk) = chunk {
            timed["chunk"] = chunk.clone();
        }
        pending
            .group
            .accumulator
            .get_or_insert_with(AssistantStreamAccumulator::new)
            .push_value(&timed)
            .map_err(hoh_session_format::SessionFormatError::other)?;
        pending
            .group
            .record_span(member, 1, field(&event, "time").clone());
        if chunk
            .and_then(|chunk| chunk.get("type"))
            .is_some_and(|kind| kind == "finish")
        {
            pending.group.terminal = true;
        }
        Ok(())
    }
    fn transform_message(
        &mut self,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        let data = record(field(&event, "data"))?;
        let turn = field(data, "turn").clone();
        let step = field(data, "step").clone();
        if self.pending.as_ref().is_some_and(|p| {
            !same_coordinate(&p.group.turn, &turn) || !same_coordinate(&p.group.step, &step)
        }) {
            self.finish_attempt(context)?;
            return self.emit_source(
                message_event(event, &mut AttemptGroup::new(turn, step))?,
                context,
            );
        }
        let sources = event.get("sourceEventSeqs").and_then(Value::as_array);
        let Some(sources) = sources else {
            if self.pending.is_some() {
                return Err(refusal(format!(
                    "assistant/message {} does not cite its complete v1 chunk attempt",
                    text(event.get("seq"))
                )));
            }
            return self.emit_source(
                message_event(event, &mut AttemptGroup::new(turn, step))?,
                context,
            );
        };
        if sources.is_empty() {
            self.finish_attempt(context)?;
            return self.emit_source(
                message_event(event, &mut AttemptGroup::new(turn, step))?,
                context,
            );
        }
        let Some(pending) = &self.pending else {
            return Err(refusal(format!(
                "assistant/message {} chunk references are not one complete ordered attempt",
                text(event.get("seq"))
            )));
        };
        if !pending.group.matches_sources(sources) {
            return Err(refusal(format!(
                "assistant/message {} chunk references are not one complete ordered attempt",
                text(event.get("seq"))
            )));
        }
        assert_attempt_range(
            self.source_cut,
            pending
                .group
                .spans
                .first()
                .map_or(seq(&event)?, |span| span.first_seq),
            seq(&event)?,
        )?;
        // The checks above establish the pending attempt for this message.
        let mut pending = self.pending.take().expect("checked pending attempt");
        pending.group.terminal = true;
        let result = (|| {
            self.flush_buffered(&pending, context)?;
            pending.after_last_chunk.clear();
            let message = message_event(event, &mut pending.group)?;
            self.emit_source(message, context)
        })();
        if result.is_err() {
            self.pending = Some(pending);
        }
        result
    }
    fn finish_attempt(&mut self, context: &mut dyn SessionFormatMigrationContext) -> Result {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        let result = (|| {
            let event = pending.group.attempt_event()?;
            self.emit_generated(seq(&event)?, event, context)?;
            self.flush_buffered(&pending, context)?;
            Ok(())
        })();
        if result.is_err() {
            self.pending = Some(pending);
        }
        result
    }
    fn flush_buffered(
        &mut self,
        pending: &Pending,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        for event in &pending.after_last_chunk {
            self.emit_source(event.clone(), context)?;
        }
        Ok(())
    }
    fn flush_pending_buffer(&mut self, context: &mut dyn SessionFormatMigrationContext) -> Result {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        let result = self.flush_buffered(&pending, context);
        if result.is_ok() {
            pending.after_last_chunk.clear();
        }
        self.pending = Some(pending);
        result
    }
    fn emit_source(
        &mut self,
        mut event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        let origin = seq(&event)?;
        if field(&self.source_header, "isSeeded") == true
            && origin == self.source_cut
            && kind(&event) == "session/end-seed"
        {
            event.insert("data".into(), json!({"inherited":true}));
        }
        self.ensure_target_cut(origin, field(&event, "time").clone(), kind(&event), context)?;
        self.mapping.insert(origin, self.target_seq);
        context.emit_event(remap_references(event, self.target_seq, &self.mapping)?)?;
        self.target_seq += 1;
        Ok(())
    }
    fn emit_generated(
        &mut self,
        origin: u64,
        event: SessionFormatEvent,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        self.ensure_target_cut(origin, field(&event, "time").clone(), kind(&event), context)?;
        context.emit_event(remap_references(event, self.target_seq, &self.mapping)?)?;
        self.target_seq += 1;
        Ok(())
    }
    fn ensure_target_cut(
        &mut self,
        origin: u64,
        time: Value,
        kind: &str,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        if field(&self.source_header, "isSeeded") != true
            || self.target_cut.is_some()
            || origin < self.source_cut
        {
            return Ok(());
        }
        self.target_cut = Some(self.target_seq);
        if origin == self.source_cut && kind == "session/end-seed" {
            return Ok(());
        }
        context.emit_event(marker(self.target_seq, time)?)?;
        self.target_seq += 1;
        Ok(())
    }
}
fn record(value: &Value) -> Result<&hoh_session_format::SessionFormatJsonObject> {
    value
        .as_object()
        .ok_or_else(|| error("format v1 value must be an object"))
}
fn coordinate(value: &Value) -> Result<u64> {
    session_format_count(value, "format v1 coordinate")
}
fn same_coordinate(left: &Value, right: &Value) -> bool {
    // Separately owned JSON objects/arrays cannot be the same JS reference.
    !matches!(left, Value::Object(_) | Value::Array(_)) && deep_equal_json(left, right)
}
fn marker(seq: u64, time: Value) -> Result<SessionFormatEvent> {
    object(json!({"type":"session/end-seed","seq":seq,"time":time,"data":{"inherited":true}}))
}
fn assert_attempt_range(cut: u64, first: u64, last: u64) -> Result {
    if (first < cut) != (last < cut) {
        return Err(refusal(format!(
            "inherited Session cut {cut} splits one Assistant attempt"
        )));
    }
    Ok(())
}
fn assert_chunk_envelope(event: &SessionFormatEvent) -> Result {
    if let Some(key) = ordered_keys(event).into_iter().find(|key| {
        ![
            "type",
            "seq",
            "time",
            "data",
            "ignorable",
            "sourceEventSeqs",
            "surfaceOp",
        ]
        .contains(&key.as_str())
    }) {
        return Err(refusal(format!(
            "assistant/chunk {} has unexpected member {key}",
            text(event.get("seq"))
        )));
    }
    if let Some(key) = ["type", "seq", "time", "data"]
        .iter()
        .find(|key| !event.contains_key(**key))
    {
        return Err(refusal(format!(
            "assistant/chunk {} lacks required member {key}",
            text(event.get("seq"))
        )));
    }
    if event.contains_key("ignorable") && field(event, "ignorable") != true {
        return Err(refusal(format!(
            "assistant/chunk {} ignorable must be true when present",
            text(event.get("seq"))
        )));
    }
    Ok(())
}
fn legacy_interrupted_turn(
    state: &LegacyTurnState,
    event: &SessionFormatEvent,
) -> Result<Option<SessionFormatEvent>> {
    if kind(event) != "turn/start" || state.open_turn.is_none() || state.open_step.is_some() {
        return Ok(None);
    }
    let data = record(field(event, "data"))?;
    if !state
        .open_turn
        .is_some_and(|turn| number_eq(field(data, "turn"), turn + 1))
    {
        return Ok(None);
    }
    let Some(previous) = state
        .previous
        .as_ref()
        .filter(|e| kind(e) == "agent/inbox/spliced")
    else {
        return Ok(None);
    };
    let splice = record(field(previous, "data"))?;
    if field(splice, "target") != "next-turn"
        || !field(splice, "inserted")
            .as_array()
            .is_some_and(|a| !a.is_empty())
    {
        return Ok(None);
    }
    Ok(Some(object(
        json!({"type":"turn/end","seq":field(event,"seq"),"time":field(event,"time"),"data":{"turn":state.open_turn,"reason":{"kind":"interrupted"}}}),
    )?))
}
fn observe_legacy_turn(state: &mut LegacyTurnState, event: &SessionFormatEvent) -> Result {
    match kind(event) {
        "turn/start" => {
            let data = record(field(event, "data"))?;
            state.open_turn = Some(coordinate(field(data, "turn"))?);
            state.open_step = None;
        }
        "turn/end" => {
            state.open_turn = None;
            state.open_step = None;
        }
        "step/start" => {
            state.open_step = Some(coordinate(field(record(field(event, "data"))?, "step"))?)
        }
        "step/end" => state.open_step = None,
        _ => {}
    }
    state.previous = Some(event.clone());
    Ok(())
}
fn split_legacy_goal_change(
    event: &SessionFormatEvent,
) -> Result<Option<(SessionFormatEvent, SessionFormatEvent)>> {
    if kind(event) != "user/message" {
        return Ok(None);
    }
    let data = record(field(event, "data"))?;
    let source = record(field(data, "source"))?;
    if field(source, "kind") != "goal" || !source.contains_key("change") {
        return Ok(None);
    }
    let change = object(
        json!({"type":"goal/change","seq":field(event,"seq"),"time":field(event,"time"),"data":field(source,"change")}),
    )?;
    let mut message = event.clone();
    let mut data = data.clone();
    data.insert("source".into(), json!({"kind":"plugin","plugin":"goal"}));
    message.insert("data".into(), Value::Object(data));
    Ok(Some((change, message)))
}
fn record_last_time(record: &Value) -> Result<i64> {
    if record["type"] == "chunk" {
        return session_format_safe_integer(&record["time"], "Assistant stream time");
    }
    let mut time = session_format_safe_integer(&record["time0"], "Assistant stream time")?;
    for gap in record["dt"]
        .as_array()
        .ok_or_else(|| error("Assistant stream dt must be an array"))?
    {
        time += session_format_safe_integer(gap, "Assistant stream gap")?;
    }
    Ok(time)
}
fn message_event(
    mut source: SessionFormatEvent,
    group: &mut AttemptGroup,
) -> Result<SessionFormatEvent> {
    let mut data = record(field(&source, "data"))?.clone();
    source.shift_remove("sourceEventSeqs");
    data.insert("stream".into(), group.stream_of()?);
    source.insert("data".into(), Value::Object(data));
    Ok(source)
}
fn remap_references(
    mut source: SessionFormatEvent,
    target: u64,
    mapping: &HashMap<u64, u64>,
) -> Result<SessionFormatEvent> {
    let label = label(&source);
    let sources = source.shift_remove("sourceEventSeqs");
    let operation = source.shift_remove("surfaceOp");
    let sources = if let Some(sources) = sources {
        Some(map_list(&sources, mapping, &format!("{label} sources"))?)
    } else {
        None
    };
    let operation = if let Some(operation) = operation {
        if operation == "append" {
            Some(operation)
        } else {
            let start = map_optional(
                property_member(&operation, "start")?,
                mapping,
                &format!("{label} surface start"),
            )?;
            let end = map_optional(
                property_member(&operation, "end")?,
                mapping,
                &format!("{label} surface end"),
            )?;
            Some(json!({"op":"replace","start":start,"end":end}))
        }
    } else {
        None
    };
    let data = remap_payload_references(&source, mapping)?;
    source.insert("seq".into(), target.into());
    source.insert("data".into(), data);
    if let Some(sources) = sources {
        source.insert("sourceEventSeqs".into(), sources);
    }
    if let Some(operation) = operation {
        source.insert("surfaceOp".into(), operation);
    }
    Ok(source)
}
fn remap_payload_references(
    event: &SessionFormatEvent,
    mapping: &HashMap<u64, u64>,
) -> Result<Value> {
    if !matches!(
        kind(event),
        "command/done"
            | "compaction/prune"
            | "compaction/summary"
            | "session/title"
            | "session/title-llm-request"
    ) {
        return Ok(field(event, "data").clone());
    }
    let mut data = record(field(event, "data"))?.clone();
    let label = label(event);
    match kind(event) {
        "command/done" => {
            if let Some(source) = data.get("sourceEventSeq") {
                let mapped = map_one(source, mapping, &format!("{label} sourceEventSeq"))?;
                data.insert("sourceEventSeq".into(), mapped.into());
            }
        }
        "compaction/prune" | "compaction/summary" => {
            let range = field(&data, "shadowedRange");
            let start = map_one(
                &range["start"],
                mapping,
                &format!("{label} shadowedRange start"),
            )?;
            let end = map_one(
                &range["end"],
                mapping,
                &format!("{label} shadowedRange end"),
            )?;
            data.insert("shadowedRange".into(), json!({"start":start,"end":end}));
            let mapped = map_list(
                field(&data, "shadowedSeqs"),
                mapping,
                &format!("{label} shadowedSeqs"),
            )?;
            data.insert("shadowedSeqs".into(), mapped);
        }
        "session/title" | "session/title-llm-request" => {
            let mapped = map_list(
                field(&data, "messageSeqs"),
                mapping,
                &format!("{label} messageSeqs"),
            )?;
            data.insert("messageSeqs".into(), mapped);
        }
        _ => {}
    }
    Ok(Value::Object(data))
}
fn map_list(values: &Value, mapping: &HashMap<u64, u64>, label: &str) -> Result<Value> {
    let values = values
        .as_array()
        .ok_or_else(|| error("format v1 references must be an array"))?;
    Ok(Value::Array(
        values
            .iter()
            .map(|value| map_one(value, mapping, label).map(Value::from))
            .collect::<Result<Vec<_>>>()?,
    ))
}
fn map_one(value: &Value, mapping: &HashMap<u64, u64>, label: &str) -> Result<u64> {
    map_optional(Some(value), mapping, label)
}
fn map_optional(value: Option<&Value>, mapping: &HashMap<u64, u64>, label: &str) -> Result<u64> {
    value
        .and_then(exact_coordinate)
        .and_then(|key| mapping.get(&key).copied())
        .ok_or_else(|| {
            refusal(format!(
                "{label} targets consumed assistant/chunk {}",
                text(value)
            ))
        })
}
#[derive(Debug, thiserror::Error)]
enum MigrationTypeError {
    #[error("Cannot read properties of null (reading '{0}')")]
    NullProperty(String),
}
fn property_member<'a>(value: &'a Value, key: &str) -> Result<Option<&'a Value>> {
    if value.is_null() {
        return Err(hoh_session_format::SessionFormatError::other(
            MigrationTypeError::NullProperty(key.to_owned()),
        ));
    }
    // JS boxes non-null primitives; only object members can have these named keys.
    Ok(value.as_object().and_then(|object| object.get(key)))
}
