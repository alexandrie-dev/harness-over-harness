use crate::validation::{assert_released_session_format_header, number_eq};
use crate::validation_helpers::{
    assert_released_v0_keys, field, js_display, normalize_json_layout, released_v0_record,
};
use hoh_session_format::{
    SessionFormatArtifactDecoder, SessionFormatCodec, SessionFormatError, SessionFormatEvent,
    SessionFormatEventRun, SessionFormatHeader, SessionFormatJsonObject,
    SessionFormatMigrationContext, SessionFormatRecovery, session_format_count,
    session_format_safe_integer, snapshot_session_format_json_labeled,
};
use serde_json::{Value, json};

/// A codec-owned packed Assistant row; expansion consumes it and stays lazy.
#[derive(Debug, Clone)]
pub struct ReleasedAssistantChunkRun {
    pub first_seq: u64,
    pub event_count: u64,
    pub turn: u64,
    pub step: u64,
    pub last_seq: u64,
    pub last_time: i64,
    pub stream: SessionFormatJsonObject,
}

pub fn is_released_assistant_chunk_run(run: &dyn SessionFormatEventRun) -> bool {
    run.run_type() == "released-assistant-chunks"
}

impl SessionFormatEventRun for ReleasedAssistantChunkRun {
    fn run_type(&self) -> &str {
        "released-assistant-chunks"
    }
    fn first_seq(&self) -> u64 {
        self.first_seq
    }
    fn event_count(&self) -> u64 {
        self.event_count
    }
    fn expand(self: Box<Self>) -> Box<dyn Iterator<Item = SessionFormatEvent>> {
        let integer = |v: &Value| {
            v.as_i64()
                .or_else(|| v.as_f64().map(|f| f as i64))
                .unwrap_or(0)
        };
        let mut time = integer(field(&self.stream, "time0"));
        Box::new((0..self.event_count).map(move |index| {
            if index > 0 { time += integer(&field(&self.stream,"dt")[index as usize - 1]); }
            let tool = field(&self.stream,"type") == "tool-call-chunks";
            let member = field(&self.stream,if tool { "args" } else { "texts" })[index as usize].clone();
            let chunk = if !tool {
                json!({"type":if field(&self.stream,"type") == "text-chunks" { "text-delta" } else { "reasoning-delta" },"index":field(&self.stream,"index"),"text":member})
            } else {
                let mut c = SessionFormatJsonObject::new();
                c.insert("type".into(),"tool-call-delta".into()); c.insert("index".into(),field(&self.stream,"index").clone()); c.insert("id".into(),field(&self.stream,"id").clone());
                if let Some(name) = self.stream.get("name") { c.insert("name".into(),name.clone()); }
                c.insert("argumentsDelta".into(),member); Value::Object(c)
            };
            let mut event = SessionFormatEvent::new();
            event.insert("type".into(),"assistant/chunk".into()); event.insert("seq".into(),(self.first_seq + index).into()); event.insert("time".into(),time.into());
            event.insert("data".into(),json!({"turn":self.turn,"step":self.step,"chunk":chunk})); event
        }))
    }
}

/// Stateless released physical codec, instantiated by the two released constants.
#[derive(Debug, Clone, Copy)]
pub struct ReleasedSessionFormatCodec {
    version: u64,
}
pub const RELEASED_V0_SESSION_FORMAT_CODEC: ReleasedSessionFormatCodec =
    ReleasedSessionFormatCodec { version: 0 };
pub const RELEASED_V1_SESSION_FORMAT_CODEC: ReleasedSessionFormatCodec =
    ReleasedSessionFormatCodec { version: 1 };

impl SessionFormatCodec for ReleasedSessionFormatCodec {
    fn version(&self) -> u64 {
        self.version
    }
    fn decode_header(&self, value: &Value) -> Result<SessionFormatHeader, SessionFormatError> {
        Ok(decode_physical_header(value, self.version)?.0)
    }
    fn create_decoder(
        &self,
        header_value: &Value,
        recovery: SessionFormatRecovery,
    ) -> Result<Box<dyn SessionFormatArtifactDecoder>, SessionFormatError> {
        let (header, inherited) = decode_physical_header(header_value, self.version)?;
        Ok(Box::new(ReleasedDecoder {
            header,
            inherited,
            recoverable: recovery == SessionFormatRecovery::Recoverable,
            row_index: 0,
            event_count: 0,
            issue: None,
        }))
    }
}
struct ReleasedDecoder {
    header: SessionFormatHeader,
    inherited: u64,
    recoverable: bool,
    row_index: u64,
    event_count: u64,
    issue: Option<String>,
}
enum Decoded {
    Event(SessionFormatEvent),
    Run(ReleasedAssistantChunkRun),
}
impl SessionFormatArtifactDecoder for ReleasedDecoder {
    fn header(&self) -> &SessionFormatHeader {
        &self.header
    }
    fn header_inherited_event_count(&self) -> Option<u64> {
        Some(self.inherited)
    }
    fn decode_row(
        &mut self,
        row_value: Value,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<(), SessionFormatError> {
        let index = self.row_index;
        self.row_index += 1;
        let decoded = (|| {
            let row = released_v0_record(&row_value, &format!("released Session row {index}"))?;
            let kind = field(row, "type").as_str().unwrap_or("");
            if matches!(
                kind,
                "text-chunks" | "reasoning-chunks" | "tool-call-chunks"
            ) {
                Ok(Decoded::Run(decode_packed_run(row, kind, index)?))
            } else {
                Ok(Decoded::Event(decode_event(row.clone(), index)?))
            }
        })();
        let decoded = match decoded {
            Ok(value) => value,
            Err(e) => {
                if !self.recoverable {
                    return Err(e);
                }
                if self.issue.is_none() {
                    self.issue = Some(e.to_string());
                }
                return Ok(());
            }
        };
        let terminal = matches!(&decoded,Decoded::Event(e) if field(e,"type") == "turn/end");
        if let Some(issue) = &self.issue {
            return if terminal {
                Err(SessionFormatError::new(issue.clone()))
            } else {
                Ok(())
            };
        }
        let (seq, seq_text) = match &decoded {
            Decoded::Run(r) => (Value::from(r.first_seq), r.first_seq.to_string()),
            Decoded::Event(e) => (field(e, "seq").clone(), js_display(e.get("seq"))),
        };
        if !number_eq(&seq, self.event_count) {
            let message = format!(
                "released Session row {index} has seq gap (expected {}, got {seq_text})",
                self.event_count
            );
            if !self.recoverable || terminal {
                return Err(SessionFormatError::new(message));
            }
            self.issue = Some(message);
            return Ok(());
        }
        match decoded {
            Decoded::Run(run) => {
                self.event_count += run.event_count;
                context.emit_run(Box::new(run))
            }
            Decoded::Event(event) => {
                self.event_count += 1;
                context.emit_event(event)
            }
        }
    }
    fn finish(
        &mut self,
        _context: &mut dyn SessionFormatMigrationContext,
    ) -> Result<u64, SessionFormatError> {
        if self.inherited > self.event_count {
            return Err(SessionFormatError::new(
                "Session inheritedEventCount exceeds its event count",
            ));
        }
        Ok(self.inherited)
    }
}
fn decode_physical_header(
    value: &Value,
    version: u64,
) -> Result<(SessionFormatHeader, u64), SessionFormatError> {
    let label = format!("released v{version} physical header");
    let source = snapshot_session_format_json_labeled(value, &label)?;
    let r = released_v0_record(&source, &label)?;
    assert_released_v0_keys(
        r,
        &["type", "version", "id", "createdAt", "delegationDepth"],
        &[
            "cwd",
            "parentSession",
            "seedLength",
            "origin",
            "agentPreset",
        ],
        &label,
    )?;
    if field(r, "type") != "session" || !number_eq(field(r, "version"), version) {
        return Err(SessionFormatError::new(format!(
            "expected released v{version} physical Session header"
        )));
    }
    if !field(r, "id").is_string() {
        return Err(SessionFormatError::new(format!(
            "released v{version} header id must be a string"
        )));
    }
    let created = session_format_count(
        field(r, "createdAt"),
        &format!("released v{version} header createdAt"),
    )?;
    let depth = session_format_count(
        field(r, "delegationDepth"),
        &format!("released v{version} header delegationDepth"),
    )?;
    let seed = if r.contains_key("seedLength") {
        session_format_count(
            field(r, "seedLength"),
            &format!("released v{version} header seedLength"),
        )?
    } else {
        0
    };
    for key in ["cwd", "parentSession", "agentPreset"] {
        if r.contains_key(key) && !field(r, key).is_string() {
            return Err(SessionFormatError::new(format!(
                "released v{version} header {key} must be a string"
            )));
        }
    }
    if r.contains_key("origin") && field(r, "origin") != "subagent" {
        return Err(SessionFormatError::new(format!(
            "released v{version} header origin must be \"subagent\""
        )));
    }
    let mut h = SessionFormatHeader::new();
    h.insert("version".into(), version.into());
    h.insert("id".into(), field(r, "id").clone());
    h.insert("createdAt".into(), created.into());
    for key in ["cwd", "parentSession"] {
        if let Some(v) = r.get(key) {
            h.insert(key.into(), v.clone());
        }
    }
    h.insert("isSeeded".into(), r.contains_key("seedLength").into());
    if let Some(v) = r.get("origin") {
        h.insert("origin".into(), v.clone());
    }
    h.insert("delegationDepth".into(), depth.into());
    if let Some(v) = r.get("agentPreset") {
        h.insert("agentPreset".into(), v.clone());
    }
    assert_released_session_format_header(&h, version)?;
    Ok((h, seed))
}
fn decode_event(
    mut record: SessionFormatEvent,
    index: u64,
) -> Result<SessionFormatEvent, SessionFormatError> {
    if let Some(sources) = record.get("sourceEventSeqs") {
        let seq = session_format_count(
            field(&record, "seq"),
            &format!("released Session row {index} seq"),
        )?;
        let decoded = decode_seq_ranges(sources, seq)?;
        record.insert("sourceEventSeqs".into(), Value::Array(decoded));
    }
    for value in record.values_mut() {
        normalize_json_layout(value);
    }
    Ok(record)
}
fn decode_packed_run(
    row: &SessionFormatJsonObject,
    kind: &str,
    index: u64,
) -> Result<ReleasedAssistantChunkRun, SessionFormatError> {
    let l = format!("released {kind} row {index}");
    assert_released_v0_keys(row, &["type", "seq0", "time0", "data"], &[], &l)?;
    let seq0 = session_format_count(field(row, "seq0"), &format!("{l} seq0"))?;
    let time0 = session_format_safe_integer(field(row, "time0"), &format!("{l} time0"))?;
    let d = released_v0_record(field(row, "data"), &format!("{l} data"))?;
    let tool = kind == "tool-call-chunks";
    assert_released_v0_keys(
        d,
        if tool {
            &["turn", "step", "index", "id", "dt", "args"]
        } else {
            &["turn", "step", "index", "dt", "texts"]
        },
        if tool { &["name"] } else { &[] },
        &format!("{l} data"),
    )?;
    let payload = field(d, if tool { "args" } else { "texts" });
    let Some(members) = payload
        .as_array()
        .filter(|a| !a.is_empty() && a.iter().all(Value::is_string))
    else {
        return Err(SessionFormatError::new(format!(
            "{l} payload must be a non-empty string array"
        )));
    };
    let Some(gaps) = field(d, "dt")
        .as_array()
        .filter(|a| a.len() == members.len() - 1)
    else {
        return Err(SessionFormatError::new(format!(
            "{l} dt length must match its payload"
        )));
    };
    let mut last_time = time0;
    for gap in gaps {
        let valid = session_format_safe_integer(gap, &format!("{l} dt member"))?;
        last_time = session_format_safe_integer(
            &Value::from(last_time + valid),
            &format!("{l} member time"),
        )?;
    }
    let turn = session_format_count(field(d, "turn"), &format!("{l} turn"))?;
    let step = session_format_count(field(d, "step"), &format!("{l} step"))?;
    let chunk_index = session_format_count(field(d, "index"), &format!("{l} index"))?;
    if tool
        && (!field(d, "id").as_str().is_some_and(|s| !s.is_empty())
            || (d.contains_key("name") && !field(d, "name").is_string()))
    {
        return Err(SessionFormatError::new(format!(
            "{l} id and optional name must be strings"
        )));
    }
    let last_seq = session_format_count(
        &Value::from(seq0 + members.len() as u64 - 1),
        &format!("{l} final seq"),
    )?;
    let mut stream = SessionFormatJsonObject::new();
    stream.insert("type".into(), kind.into());
    stream.insert("time0".into(), time0.into());
    stream.insert("index".into(), chunk_index.into());
    stream.insert("dt".into(), Value::Array(gaps.clone()));
    if tool {
        stream.insert("id".into(), field(d, "id").clone());
        if let Some(v) = d.get("name") {
            stream.insert("name".into(), v.clone());
        }
    }
    stream.insert(if tool { "args" } else { "texts" }.into(), payload.clone());
    for value in stream.values_mut() {
        normalize_json_layout(value);
    }
    Ok(ReleasedAssistantChunkRun {
        first_seq: seq0,
        event_count: members.len() as u64,
        turn,
        step,
        last_seq,
        last_time,
        stream,
    })
}
fn decode_seq_ranges(value: &Value, max_entries: u64) -> Result<Vec<Value>, SessionFormatError> {
    let Some(entries) = value.as_array() else {
        return Err(SessionFormatError::new("sourceEventSeqs must be an array"));
    };
    let mut output = Vec::new();
    let mut has_range = false;
    for entry in entries {
        if entry.is_number() {
            if output.len() as u64 >= max_entries {
                return Err(SessionFormatError::new(
                    "sourceEventSeqs exceeds its event seq",
                ));
            }
            output.push(Value::from(session_format_count(
                entry,
                "sourceEventSeqs member",
            )?));
            continue;
        }
        let Some(pair) = entry.as_array().filter(|a| a.len() == 2) else {
            return Err(SessionFormatError::new(
                "sourceEventSeqs range must be a [start, end] pair",
            ));
        };
        let start = session_format_count(&pair[0], "sourceEventSeqs range start")?;
        let end = session_format_count(&pair[1], "sourceEventSeqs range end")?;
        if end < start || end - start + 1 > max_entries.saturating_sub(output.len() as u64) {
            return Err(SessionFormatError::new(
                "sourceEventSeqs range exceeds its event seq",
            ));
        }
        output.extend((start..=end).map(Value::from));
        has_range = true;
    }
    if has_range && output.windows(2).any(|w| w[1].as_u64() <= w[0].as_u64()) {
        return Err(SessionFormatError::new(
            "sourceEventSeqs ranges must be strictly increasing",
        ));
    }
    Ok(output)
}
