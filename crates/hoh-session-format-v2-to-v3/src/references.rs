//! Explicit remapping of audited local coordinates.
use crate::{helpers::*, payload::record};
use hoh_session_format::{SessionFormatEvent, session_format_count};
use serde_json::{Value, json};
struct Mapper<'a> {
    event: &'a SessionFormatEvent,
    mapping: &'a [u64],
}
impl Mapper<'_> {
    fn one(&self, value: &Value) -> Result<Value> {
        let source = session_format_count(value, "source event reference")?;
        let target = usize::try_from(source)
            .ok()
            .and_then(|i| self.mapping.get(i));
        if field(self.event, "seq")
            .as_f64()
            .is_none_or(|seq| source as f64 >= seq)
            || target.is_none()
        {
            return Err(error("reference must name an earlier source event"));
        }
        // The check above establishes the mapping entry.
        Ok((*target.expect("mapped source")).into())
    }
    fn list(&self, value: &Value) -> Result<Value> {
        let values = value
            .as_array()
            .ok_or_else(|| error("sequence references must be an array"))?;
        Ok(Value::Array(
            values
                .iter()
                .map(|v| self.one(v))
                .collect::<Result<Vec<_>>>()?,
        ))
    }
    fn range(&self, value: &Value) -> Result<Value> {
        let mut range = record(value, "sequence range")?.clone();
        let start = self.one(field(&range, "start"))?;
        let end = self.one(field(&range, "end"))?;
        range.insert("start".into(), start);
        range.insert("end".into(), end);
        Ok(Value::Object(range))
    }
}
pub fn remap_event(
    event: SessionFormatEvent,
    seq: u64,
    mapping: &[u64],
) -> Result<SessionFormatEvent> {
    let mapper = Mapper {
        event: &event,
        mapping,
    };
    let mut data = record(field(&event, "data"), kind(&event))?.clone();
    match kind(&event) {
        "command/done" => {
            if let Some(source) = data.get("sourceEventSeq") {
                let target = mapper.one(source)?;
                data.insert("sourceEventSeq".into(), target);
            }
        }
        "compaction/summary" | "compaction/prune" => {
            let range = mapper.range(field(&data, "shadowedRange"))?;
            data.insert("shadowedRange".into(), range);
            let seqs = mapper.list(field(&data, "shadowedSeqs"))?;
            data.insert("shadowedSeqs".into(), seqs);
        }
        "session/title" | "session/title-llm-request" => {
            let seqs = mapper.list(field(&data, "messageSeqs"))?;
            data.insert("messageSeqs".into(), seqs);
        }
        _ => {}
    }
    let sources = event
        .get("sourceEventSeqs")
        .map(|v| mapper.list(v))
        .transpose()?;
    let op = event
        .get("surfaceOp")
        .filter(|op| *op != "append")
        .map(|v| mapper.range(v))
        .transpose()?;
    let mut output = event;
    output.insert("seq".into(), json!(seq));
    output.insert("data".into(), Value::Object(data));
    if let Some(sources) = sources {
        output.insert("sourceEventSeqs".into(), sources);
    }
    if let Some(op) = op {
        output.insert("surfaceOp".into(), op);
    }
    Ok(output)
}
