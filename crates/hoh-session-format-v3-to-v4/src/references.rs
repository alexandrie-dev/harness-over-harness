use crate::helpers::*;
use hoh_session_format::{SessionFormatEvent, SessionFormatJsonObject};
use serde_json::Value;
fn object(value: Option<&Value>) -> Result<SessionFormatJsonObject> {
    value
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| error("V3 event reference container must be an object"))
}
pub fn remap_v3_references(
    mut event: SessionFormatEvent,
    seq: u64,
    mapping: &[u64],
) -> Result<SessionFormatEvent> {
    if number_eq(event.get("seq"), seq) {
        return Ok(event);
    }
    let reference = |value: Option<&Value>| -> Result<Value> {
        let source = count(value, "V3 source event reference")?;
        let target = usize::try_from(source).ok().and_then(|i| mapping.get(i));
        if event
            .get("seq")
            .and_then(Value::as_f64)
            .is_none_or(|n| source as f64 >= n)
            || target.is_none()
        {
            return Err(error("V3 reference must name an earlier source event"));
        }
        // The preceding bounds check proves the coordinate exists.
        Ok((*target.expect("checked source reference")).into())
    };
    let references = |value: Option<&Value>| -> Result<Value> {
        let values = value
            .and_then(Value::as_array)
            .ok_or_else(|| error("V3 event references must be an array"))?;
        Ok(values
            .iter()
            .map(|v| reference(Some(v)))
            .collect::<Result<Vec<_>>>()?
            .into())
    };
    let mut data = field(&event, "data").clone();
    match kind(&event) {
        "command/done" => {
            let mut source = object(Some(&data))?;
            if source.contains_key("sourceEventSeq") {
                let n = reference(source.get("sourceEventSeq"))?;
                source.insert("sourceEventSeq".into(), n);
            }
            data = source.into();
        }
        "compaction/summary" | "compaction/prune" => {
            let mut source = object(Some(&data))?;
            let mut range = object(source.get("shadowedRange"))?;
            range.insert("start".into(), reference(range.get("start"))?);
            range.insert("end".into(), reference(range.get("end"))?);
            source.insert("shadowedRange".into(), range.into());
            source.insert(
                "shadowedSeqs".into(),
                references(source.get("shadowedSeqs"))?,
            );
            data = source.into();
        }
        "session/title" | "session/title-llm-request" => {
            let mut source = object(Some(&data))?;
            source.insert("messageSeqs".into(), references(source.get("messageSeqs"))?);
            data = source.into();
        }
        "image/offload" => {
            let mut source = object(Some(&data))?;
            let targets = field(&source, "targets")
                .as_array()
                .ok_or_else(|| error("V3 image offload targets must be an array"))?;
            let converted = targets
                .iter()
                .map(|t| {
                    let mut target = object(Some(t))?;
                    target.insert("seq".into(), reference(target.get("seq"))?);
                    Ok(Value::Object(target))
                })
                .collect::<Result<Vec<_>>>()?;
            source.insert("targets".into(), converted.into());
            data = source.into();
        }
        _ => {}
    }
    let surface = event.get("surfaceOp");
    let mut range = if surface.is_none() || surface.is_some_and(|s| s == "append") {
        None
    } else {
        Some(object(surface)?)
    };
    let source_refs = event
        .get("sourceEventSeqs")
        .map(|v| references(Some(v)))
        .transpose()?;
    if let Some(r) = range.as_mut() {
        r.insert("startSeq".into(), reference(r.get("startSeq"))?);
        r.insert("endSeq".into(), reference(r.get("endSeq"))?);
    }
    event.insert("seq".into(), seq.into());
    event.insert("data".into(), data);
    if let Some(refs) = source_refs {
        event.insert("sourceEventSeqs".into(), refs);
    }
    if let Some(range) = range {
        event.insert("surfaceOp".into(), range.into());
    }
    Ok(event)
}
