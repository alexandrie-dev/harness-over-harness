use crate::helpers::*;
use crate::validation::assert_released_v2_header;
use hoh_session_format::{
    SessionFormatArtifactDecoder, SessionFormatCodec, SessionFormatCurrentEncoder,
    SessionFormatEvent, SessionFormatHeader, SessionFormatJsonObject,
    SessionFormatMigrationContext, SessionFormatRecovery, session_format_count,
    session_format_count_u64, session_format_safe_integer, snapshot_session_format_json_labeled,
};
use serde_json::{Map, Value};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy)]
pub struct ReleasedV2SessionFormatCodec;
pub const RELEASED_V2_SESSION_FORMAT_CODEC: ReleasedV2SessionFormatCodec =
    ReleasedV2SessionFormatCodec;
impl SessionFormatCodec for ReleasedV2SessionFormatCodec {
    fn version(&self) -> u64 {
        2
    }
    fn decode_header(&self, value: &Value) -> Result<SessionFormatHeader> {
        decode_physical_header(value)
    }
    fn create_decoder(
        &self,
        value: &Value,
        recovery: SessionFormatRecovery,
    ) -> Result<Box<dyn SessionFormatArtifactDecoder>> {
        Ok(Box::new(Decoder {
            header: decode_physical_header(value)?,
            recovery,
            row_index: 0,
            event_count: 0,
            inherited: None,
            issue: None,
        }))
    }
}
impl SessionFormatCurrentEncoder for ReleasedV2SessionFormatCodec {
    fn encode_header(
        &self,
        header: &SessionFormatHeader,
        inherited_event_count: u64,
    ) -> Result<SessionFormatJsonObject> {
        assert_released_v2_header(header)?;
        let cut =
            session_format_count_u64(inherited_event_count, "format v2 inherited event count")?;
        if field(header, "isSeeded") != true && cut != 0 {
            return Err(error("unseeded format v2 Session has inherited events"));
        }
        let mut output = Map::new();
        output.insert("type".into(), "session".into());
        output.insert("version".into(), 2.into());
        for key in [
            "id",
            "createdAt",
            "cwd",
            "parentSession",
            "isSeeded",
            "origin",
            "delegationDepth",
            "agentPreset",
        ] {
            if let Some(value) = header.get(key) {
                output.insert(key.into(), value.clone());
            }
        }
        Ok(output)
    }
    fn encode_event(&self, event: &SessionFormatEvent) -> Result<SessionFormatJsonObject> {
        let mut output = event.clone();
        if let Some(sources) = event.get("sourceEventSeqs") {
            let sources = sources
                .as_array()
                .ok_or_else(|| error("sourceEventSeqs must be an array"))?;
            output.insert("sourceEventSeqs".into(), encode_seq_ranges(sources));
        }
        Ok(output)
    }
}
fn decode_physical_header(value: &Value) -> Result<SessionFormatHeader> {
    let snapshot = snapshot_session_format_json_labeled(value, "released v2 physical header")?;
    let record = json_record(&snapshot, "released v2 physical header")?;
    exact_keys(
        record,
        &[
            "type",
            "version",
            "id",
            "createdAt",
            "isSeeded",
            "delegationDepth",
        ],
        &["cwd", "parentSession", "origin", "agentPreset"],
        "released v2 physical header",
    )?;
    if field(record, "type") != "session" || !number_eq(field(record, "version"), 2) {
        return Err(error("expected released v2 physical Session header"));
    }
    if !field(record, "id").is_string() {
        return Err(error("released v2 header id must be a string"));
    }
    let created = session_format_count(field(record, "createdAt"), "released v2 header createdAt")?;
    let depth = session_format_count(
        field(record, "delegationDepth"),
        "released v2 header delegationDepth",
    )?;
    if !field(record, "isSeeded").is_boolean() {
        return Err(error("released v2 header isSeeded must be boolean"));
    }
    for key in ["cwd", "parentSession", "agentPreset"] {
        if record.contains_key(key) && !field(record, key).is_string() {
            return Err(error(format!("released v2 header {key} must be a string")));
        }
    }
    if record.contains_key("origin") && field(record, "origin") != "subagent" {
        return Err(error("released v2 header origin must be \"subagent\""));
    }
    let mut header = Map::new();
    header.insert("version".into(), 2.into());
    header.insert("id".into(), field(record, "id").clone());
    header.insert("createdAt".into(), created.into());
    for key in ["cwd", "parentSession", "isSeeded", "origin"] {
        if let Some(value) = record.get(key) {
            header.insert(key.into(), value.clone());
        }
    }
    header.insert("delegationDepth".into(), depth.into());
    if let Some(value) = record.get("agentPreset") {
        header.insert("agentPreset".into(), value.clone());
    }
    let header = object(snapshot_session_format_json_labeled(
        &Value::Object(header),
        "released v2 logical header",
    )?)?;
    assert_released_v2_header(&header)?;
    Ok(header)
}
struct Decoder {
    header: SessionFormatHeader,
    recovery: SessionFormatRecovery,
    row_index: u64,
    event_count: u64,
    inherited: Option<u64>,
    issue: Option<String>,
}
impl SessionFormatArtifactDecoder for Decoder {
    fn header(&self) -> &SessionFormatHeader {
        &self.header
    }
    fn decode_row(
        &mut self,
        value: Value,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        let index = self.row_index;
        self.row_index += 1;
        let event = match decode_event(value, index) {
            Ok(e) => e,
            Err(e) => {
                if self.recovery == SessionFormatRecovery::Strict {
                    return Err(e);
                }
                if self.issue.is_none() {
                    self.issue = Some(e.to_string());
                }
                return Ok(());
            }
        };
        if let Some(issue) = &self.issue {
            return if kind(&event) == "turn/end" {
                Err(error(issue.clone()))
            } else {
                Ok(())
            };
        }
        if !number_eq(field(&event, "seq"), self.event_count) {
            let message = format!(
                "released v2 row {index} has seq gap (expected {}, got {})",
                self.event_count,
                text(event.get("seq"))
            );
            if self.recovery == SessionFormatRecovery::Strict {
                return Err(error(message));
            }
            self.issue = Some(message.clone());
            if kind(&event) == "turn/end" {
                return Err(error(message));
            }
            return Ok(());
        }
        self.event_count += 1;
        if kind(&event) == "session/end-seed" {
            let data = json_record(
                field(&event, "data"),
                &format!("session/end-seed {} data", text(event.get("seq"))),
            )?;
            if field(data, "inherited") == true {
                self.inherited = Some(seq(&event)?);
            }
        }
        context.emit_event(event)
    }
    fn finish(&mut self, _context: &mut dyn SessionFormatMigrationContext) -> Result<u64> {
        let seeded = field(&self.header, "isSeeded") == true;
        if seeded && self.inherited.is_none() {
            return Err(error(
                "released v2 seeded Session lacks an inherited end-seed marker",
            ));
        }
        if !seeded && self.inherited.is_some() {
            return Err(error(
                "released v2 unseeded Session contains an inherited end-seed marker",
            ));
        }
        Ok(self.inherited.unwrap_or(0))
    }
}
fn decode_event(value: Value, index: u64) -> Result<SessionFormatEvent> {
    let record = json_record(&value, &format!("released v2 row {index}"))?;
    for key in ["type", "seq", "time", "data"] {
        if !record.contains_key(key) {
            return Err(error(format!(
                "released v2 row {index} lacks required field {key}"
            )));
        }
    }
    if let Some(key) = ordered_keys(record).into_iter().find(|key| {
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
        return Err(error(format!(
            "released v2 row {index} has unexpected field {key}"
        )));
    }
    if !field(record, "type").is_string() {
        return Err(error(format!(
            "released v2 row {index} type must be a string"
        )));
    }
    session_format_safe_integer(
        field(record, "time"),
        &format!("released v2 row {index} time"),
    )?;
    if record.contains_key("ignorable") && field(record, "ignorable") != true {
        return Err(error(format!(
            "released v2 row {index} ignorable must be true when present"
        )));
    }
    let sources = if let Some(sources) = record.get("sourceEventSeqs") {
        let seq = session_format_count(
            field(record, "seq"),
            &format!("released v2 row {index} seq"),
        )?;
        Some(decode_seq_ranges(sources, seq)?)
    } else {
        None
    };
    let mut record = object(value)?;
    if let Some(sources) = sources {
        record.insert("sourceEventSeqs".into(), sources);
    }
    Ok(record)
}
fn decode_seq_ranges(value: &Value, max: u64) -> Result<Value> {
    let entries = value
        .as_array()
        .ok_or_else(|| error("sourceEventSeqs must be an array"))?;
    let mut output = Vec::new();
    let mut has_range = false;
    for entry in entries {
        let Some(pair) = entry.as_array() else {
            output.push(session_format_count(entry, "sourceEventSeqs member")?);
            continue;
        };
        if pair.len() != 2 {
            return Err(error("sourceEventSeqs range must be a [start, end] pair"));
        }
        let start = session_format_count(&pair[0], "sourceEventSeqs range start")?;
        let end = session_format_count(&pair[1], "sourceEventSeqs range end")?;
        if start > end || end >= max || end - start + 1 > max.saturating_sub(output.len() as u64) {
            return Err(error("sourceEventSeqs range exceeds its event seq"));
        }
        output.extend(start..=end);
        has_range = true;
    }
    let mut seen = HashSet::new();
    for source in &output {
        if *source >= max || !seen.insert(*source) {
            return Err(error(
                "sourceEventSeqs ranges must contain unique earlier seqs",
            ));
        }
    }
    if has_range && output.windows(2).any(|w| w[1] <= w[0]) {
        return Err(error("sourceEventSeqs ranges must be strictly increasing"));
    }
    Ok(Value::Array(output.into_iter().map(Value::from).collect()))
}
fn encode_seq_ranges(values: &[Value]) -> Value {
    if values.windows(2).any(|w| {
        w[1].as_f64()
            .zip(w[0].as_f64())
            .is_some_and(|(a, b)| a <= b)
    }) {
        return Value::Array(values.to_vec());
    }
    let mut output = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let start = values[index].clone();
        let mut end = start.clone();
        let mut length = 1;
        while index + 1 < values.len()
            && end
                .as_f64()
                .zip(values[index + 1].as_f64())
                .is_some_and(|(a, b)| b == a + 1.0)
        {
            index += 1;
            end = values[index].clone();
            length += 1;
        }
        if length >= 3 {
            output.push(Value::Array(vec![start, end]));
        } else {
            output.push(start);
            if length == 2 {
                output.push(end);
            }
        }
        index += 1;
    }
    Value::Array(output)
}
fn json_record<'a>(value: &'a Value, label: &str) -> Result<&'a SessionFormatJsonObject> {
    value
        .as_object()
        .ok_or_else(|| error(format!("{label} must be an object")))
}
fn exact_keys(
    record: &SessionFormatJsonObject,
    required: &[&str],
    optional: &[&str],
    label: &str,
) -> Result {
    if let Some(key) = required.iter().find(|key| !record.contains_key(**key)) {
        return Err(error(format!("{label} lacks {key}")));
    }
    if let Some(key) = ordered_keys(record)
        .into_iter()
        .find(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(error(format!("{label} has unexpected field {key}")));
    }
    Ok(())
}
