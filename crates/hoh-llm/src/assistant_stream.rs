use crate::{assembler::BlockAssembler, error::ValueError, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
const SAFE: i64 = 9_007_199_254_740_991;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedStreamChunk {
    pub time: i64,
    pub chunk: StreamChunk,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AssistantStreamRecord {
    TextChunks {
        time0: i64,
        index: u64,
        dt: Vec<i64>,
        texts: Vec<String>,
    },
    ReasoningChunks {
        time0: i64,
        index: u64,
        dt: Vec<i64>,
        texts: Vec<String>,
    },
    ToolCallChunks {
        time0: i64,
        index: u64,
        dt: Vec<i64>,
        id: ToolCallId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        args: Vec<String>,
    },
    // Source raw records admit every lossless JSON object, including future chunk tags.
    Chunk {
        time: i64,
        chunk: Value,
    },
}
pub type AssistantStreamRun = AssistantStreamRecord;
#[derive(Clone, Copy, Debug)]
pub enum RawStreamChunkType {
    BlockStart,
    BlockEnd,
    Usage,
    Finish,
}
impl RawStreamChunkType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlockStart => "block-start",
            Self::BlockEnd => "block-end",
            Self::Usage => "usage",
            Self::Finish => "finish",
        }
    }
}
fn safe_time(time: i64) -> Result<i64, ValueError> {
    if !(-SAFE..=SAFE).contains(&time) {
        return Err(ValueError::Message(format!(
            "Assistant stream time must be a safe integer, got {time}"
        )));
    }
    Ok(time)
}
fn safe_index(index: u64, label: &str) -> Result<(), ValueError> {
    if index > SAFE as u64 {
        return Err(ValueError::Message(format!(
            "{label} index must be a non-negative safe integer"
        )));
    }
    Ok(())
}
#[derive(Default)]
pub struct AssistantStreamAccumulator {
    records: Vec<AssistantStreamRecord>,
    last_time: Option<i64>,
}
impl AssistantStreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, value: TimedStreamChunk) -> Result<TimedStreamChunk, ValueError> {
        safe_time(value.time)?;
        let serialized = serde_json::to_value(&value.chunk).map_err(|_| {
            ValueError::Message(
                "Assistant stream chunk must be losslessly JSON-serializable".into(),
            )
        })?;
        if hoh_util_values::snapshot_json_value(&serialized).is_none() {
            return Err(ValueError::Message(
                "Assistant stream chunk must be losslessly JSON-serializable".into(),
            ));
        }
        let gap = self
            .last_time
            .and_then(|t| value.time.checked_sub(t))
            .filter(|g| (-SAFE..=SAFE).contains(g));
        let time = value.time;
        let mut packed = false;
        let new = match &value.chunk {
            StreamChunk::TextDelta { index, text }
            | StreamChunk::ReasoningDelta { index, text } => {
                safe_index(*index, value.chunk.kind())?;
                let reasoning = matches!(value.chunk, StreamChunk::ReasoningDelta { .. });
                if let Some(previous) = self.records.last_mut() {
                    match previous {
                        AssistantStreamRecord::TextChunks {
                            index: i,
                            dt,
                            texts,
                            ..
                        } if !reasoning && i == index && gap.is_some() => {
                            dt.push(gap.unwrap_or_default());
                            texts.push(text.clone());
                            packed = true;
                        }
                        AssistantStreamRecord::ReasoningChunks {
                            index: i,
                            dt,
                            texts,
                            ..
                        } if reasoning && i == index && gap.is_some() => {
                            dt.push(gap.unwrap_or_default());
                            texts.push(text.clone());
                            packed = true;
                        }
                        _ => {}
                    }
                }
                if reasoning {
                    AssistantStreamRecord::ReasoningChunks {
                        time0: time,
                        index: *index,
                        dt: vec![],
                        texts: vec![text.clone()],
                    }
                } else {
                    AssistantStreamRecord::TextChunks {
                        time0: time,
                        index: *index,
                        dt: vec![],
                        texts: vec![text.clone()],
                    }
                }
            }
            StreamChunk::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                safe_index(*index, "tool-call-delta")?;
                if id.as_str().is_empty() || name.as_deref() == Some("") {
                    AssistantStreamRecord::Chunk {
                        time,
                        chunk: serialized,
                    }
                } else {
                    if let Some(AssistantStreamRecord::ToolCallChunks {
                        index: i,
                        id: old_id,
                        name: old_name,
                        dt,
                        args,
                        ..
                    }) = self.records.last_mut()
                        && i == index
                        && old_id == id
                        && old_name == name
                        && let Some(gap) = gap
                    {
                        dt.push(gap);
                        args.push(arguments_delta.clone());
                        packed = true;
                    }
                    AssistantStreamRecord::ToolCallChunks {
                        time0: time,
                        index: *index,
                        dt: vec![],
                        id: id.clone(),
                        name: name.clone(),
                        args: vec![arguments_delta.clone()],
                    }
                }
            }
            StreamChunk::BlockStart { .. }
            | StreamChunk::BlockEnd { .. }
            | StreamChunk::Usage { .. }
            | StreamChunk::Finish { .. } => AssistantStreamRecord::Chunk {
                time,
                chunk: serialized,
            },
        };
        if !packed {
            self.records.push(new);
        }
        self.last_time = Some(time);
        Ok(value)
    }
    pub fn snapshot(&self) -> Vec<AssistantStreamRecord> {
        self.records.clone()
    }
    pub fn push_value(&mut self, value: &Value) -> Result<TimedStreamChunk, ValueError> {
        let time = safe_time_value(value.get("time").unwrap_or(&Value::Null))?;
        let chunk = value.get("chunk").ok_or_else(|| {
            ValueError::Message(
                "Assistant stream chunk must be losslessly JSON-serializable".into(),
            )
        })?;
        let label = chunk.get("type").and_then(Value::as_str).unwrap_or("");
        match label {
            "text-delta" | "reasoning-delta" | "tool-call-delta" => {
                safe_index_value(chunk.get("index").unwrap_or(&Value::Null), label)?;
                for key in if label == "tool-call-delta" {
                    vec!["id", "argumentsDelta"]
                } else {
                    vec!["text"]
                } {
                    if !chunk.get(key).is_some_and(Value::is_string) {
                        return Err(ValueError::Message(format!(
                            "{label} {key} must be a string"
                        )));
                    }
                }
                if label == "tool-call-delta" && chunk.get("name").is_some_and(|v| !v.is_string()) {
                    return Err(ValueError::Message(
                        "tool-call-delta name must be a string".into(),
                    ));
                }
            }
            "block-start" | "block-end" | "usage" | "finish" => {}
            _ => {
                return Err(ValueError::Message(format!(
                    "unreachable variant in AssistantStreamAccumulator.push: {}",
                    hoh_util_values::js_json_stringify(chunk)
                )));
            }
        }
        let chunk = serde_json::from_value(chunk.clone()).map_err(|_| {
            ValueError::Message(
                "Assistant stream chunk must be losslessly JSON-serializable".into(),
            )
        })?;
        self.push(TimedStreamChunk { time, chunk })
    }
}
fn safe_time_value(value: &Value) -> Result<i64, ValueError> {
    let n = value.as_f64();
    n.filter(|n| n.is_finite() && n.fract() == 0.0 && n.abs() <= SAFE as f64)
        .map(|n| n as i64)
        .ok_or_else(|| {
            ValueError::Message(format!(
                "Assistant stream time must be a safe integer, got {}",
                hoh_util_values::js_string(value)
            ))
        })
}
fn safe_index_value(value: &Value, label: &str) -> Result<u64, ValueError> {
    value
        .as_f64()
        .filter(|n| {
            n.is_finite()
                && n.fract() == 0.0
                && *n >= 0.0
                && *n <= SAFE as f64
                && !(*n == 0.0 && n.is_sign_negative())
        })
        .map(|n| n as u64)
        .ok_or_else(|| {
            ValueError::Message(format!("{label} index must be a non-negative safe integer"))
        })
}
pub fn validate_record(value: &Value) -> Result<AssistantStreamRecord, ValueError> {
    let record = value
        .as_object()
        .ok_or_else(|| ValueError::Message("Assistant stream record must be an object".into()))?;
    let label = value.get("type").and_then(Value::as_str).unwrap_or("");
    let keys: &[&str] = match label {
        "text-chunks" | "reasoning-chunks" => &["type", "time0", "index", "dt", "texts"],
        "tool-call-chunks" if record.contains_key("name") => {
            &["type", "time0", "index", "dt", "id", "name", "args"]
        }
        "tool-call-chunks" => &["type", "time0", "index", "dt", "id", "args"],
        "chunk" => &["type", "time", "chunk"],
        _ => {
            return Err(ValueError::Message(format!(
                "Unsupported Assistant stream record {}",
                value
                    .get("type")
                    .map(hoh_util_values::js_json_stringify)
                    .unwrap_or_else(|| "undefined".into())
            )));
        }
    };
    if keys.len() != record.len() || keys.iter().any(|k| !record.contains_key(*k)) {
        return Err(ValueError::Message(format!(
            "{label} Assistant stream record must contain exactly {}",
            keys.join(", ")
        )));
    }
    if label == "chunk" {
        safe_time_value(&value["time"])?;
        if !value["chunk"].is_object()
            || hoh_util_values::snapshot_json_value(&value["chunk"]).is_none()
        {
            return Err(ValueError::Message(
                "Assistant stream raw chunk must be a lossless JSON object".into(),
            ));
        }
    } else {
        let field = if label == "tool-call-chunks" {
            "args"
        } else {
            "texts"
        };
        let fragments = value[field]
            .as_array()
            .filter(|a| a.iter().all(Value::is_string))
            .ok_or_else(|| {
                ValueError::Message(format!("{label} {field} must be a string array"))
            })?;
        if fragments.is_empty() {
            return Err(ValueError::Message(format!(
                "{label} {field} must be non-empty"
            )));
        }
        if label == "tool-call-chunks" {
            for field in ["id", "name"] {
                if (field == "id" || record.contains_key(field))
                    && value[field].as_str().is_none_or(str::is_empty)
                {
                    return Err(ValueError::Message(format!(
                        "tool-call-chunks {field} must be a non-empty string"
                    )));
                }
            }
        }
        let mut time = safe_time_value(&value["time0"])?;
        safe_index_value(&value["index"], label)?;
        let dt = value["dt"]
            .as_array()
            .filter(|dt| dt.iter().all(|v| safe_time_value(v).is_ok()))
            .ok_or_else(|| ValueError::Message(format!("{label} dt must contain safe integers")))?;
        if dt.len() != fragments.len() - 1 {
            return Err(ValueError::Message(format!(
                "{label} dt length must be one less than its members"
            )));
        }
        for gap in dt {
            time = time
                .checked_add(safe_time_value(gap)?)
                .filter(|n| (-SAFE..=SAFE).contains(n))
                .ok_or_else(|| {
                    ValueError::Message(format!("{label} member times must stay safe integers"))
                })?;
        }
    }
    // Normalize integer-valued JSON f64 after validation, as JS numbers have one numeric type.
    let mut normalized = value.clone();
    for key in ["time", "time0", "index"] {
        if record.contains_key(key) {
            normalized[key] = json!(safe_time_value(&value[key])?);
        }
    }
    if let Some(gaps) = value.get("dt").and_then(Value::as_array) {
        normalized["dt"] = json!(
            gaps.iter()
                .map(safe_time_value)
                .collect::<Result<Vec<_>, _>>()?
        );
    }
    serde_json::from_value(normalized).map_err(|e| ValueError::Message(e.to_string()))
}
pub fn expand_assistant_stream(
    stream: &[AssistantStreamRecord],
) -> Result<Vec<TimedStreamChunk>, ValueError> {
    let mut chunks = vec![];
    for record in stream {
        let value = serde_json::to_value(record).map_err(|e| ValueError::Message(e.to_string()))?;
        let record = validate_record(&value)?;
        match record {
            AssistantStreamRecord::Chunk { time, chunk } => {
                let chunk = serde_json::from_value(chunk)
                    .map_err(|e| ValueError::Message(e.to_string()))?;
                chunks.push(TimedStreamChunk { time, chunk });
            }
            AssistantStreamRecord::TextChunks {
                time0,
                index,
                dt,
                texts,
            } => expand_run(
                time0,
                &dt,
                texts,
                |text| StreamChunk::TextDelta { index, text },
                &mut chunks,
            ),
            AssistantStreamRecord::ReasoningChunks {
                time0,
                index,
                dt,
                texts,
            } => expand_run(
                time0,
                &dt,
                texts,
                |text| StreamChunk::ReasoningDelta { index, text },
                &mut chunks,
            ),
            AssistantStreamRecord::ToolCallChunks {
                time0,
                index,
                dt,
                id,
                name,
                args,
            } => expand_run(
                time0,
                &dt,
                args,
                |arguments_delta| StreamChunk::ToolCallDelta {
                    index,
                    id: id.clone(),
                    name: name.clone(),
                    arguments_delta,
                },
                &mut chunks,
            ),
        }
    }
    Ok(chunks)
}
fn expand_run(
    time: i64,
    dt: &[i64],
    fragments: Vec<String>,
    make: impl Fn(String) -> StreamChunk,
    out: &mut Vec<TimedStreamChunk>,
) {
    let mut time = time;
    for (i, fragment) in fragments.into_iter().enumerate() {
        if i > 0 {
            time += dt[i - 1];
        }
        out.push(TimedStreamChunk {
            time,
            chunk: make(fragment),
        });
    }
}
/// Validates raw durable records, retaining forward-compatible raw chunk objects.
pub fn expand_assistant_stream_value(stream: &Value) -> Result<Vec<Value>, ValueError> {
    let records = stream
        .as_array()
        .ok_or_else(|| ValueError::Message("Assistant stream must be an array".into()))?;
    let mut out = vec![];
    for value in records {
        let record = validate_record(value)?;
        if let AssistantStreamRecord::Chunk { time, chunk } = record {
            out.push(json!({"time":time,"chunk":chunk}));
        } else {
            for timed in expand_assistant_stream(&[record])? {
                out.push(
                    serde_json::to_value(timed).map_err(|e| ValueError::Message(e.to_string()))?,
                );
            }
        }
    }
    Ok(out)
}
fn visible(text: &str) -> bool {
    !hoh_util_values::js_trim(text).is_empty()
}
pub fn is_token_delta(chunk: &StreamChunk) -> bool {
    match chunk {
        StreamChunk::TextDelta { text, .. } | StreamChunk::ReasoningDelta { text, .. } => {
            !text.is_empty()
        }
        StreamChunk::ToolCallDelta {
            name,
            arguments_delta,
            ..
        } => name.is_some() || !arguments_delta.is_empty(),
        StreamChunk::BlockStart { .. }
        | StreamChunk::BlockEnd { .. }
        | StreamChunk::Usage { .. }
        | StreamChunk::Finish { .. } => false,
    }
}
pub fn is_visible_chunk(chunk: &StreamChunk) -> bool {
    match chunk {
        StreamChunk::TextDelta { text, .. } | StreamChunk::ReasoningDelta { text, .. } => {
            visible(text)
        }
        StreamChunk::BlockStart { block_type, .. } => {
            !matches!(block_type.as_str(), "text" | "reasoning" | "tool-call")
        }
        StreamChunk::BlockEnd { block, .. } => match block.r#type.as_str() {
            "tool-call" => false,
            "text" | "reasoning" => block.text_value().is_some_and(visible),
            _ => true,
        },
        StreamChunk::ToolCallDelta { .. }
        | StreamChunk::Usage { .. }
        | StreamChunk::Finish { .. } => false,
    }
}
pub fn chunk_has_visible_text(chunk: &StreamChunk) -> bool {
    match chunk {
        StreamChunk::TextDelta { text, .. } => visible(text),
        StreamChunk::BlockEnd { block, .. } => {
            block.r#type == "text" && block.text_value().is_some_and(visible)
        }
        _ => false,
    }
}
fn first_run_time(run: &AssistantStreamRun, test: impl Fn(&str) -> bool) -> Option<i64> {
    let (mut time, dt, members) = match run {
        AssistantStreamRecord::TextChunks {
            time0, dt, texts, ..
        }
        | AssistantStreamRecord::ReasoningChunks {
            time0, dt, texts, ..
        } => (*time0, dt, texts),
        AssistantStreamRecord::ToolCallChunks {
            time0, dt, args, ..
        } => (*time0, dt, args),
        AssistantStreamRecord::Chunk { .. } => return None,
    };
    for (i, fragment) in members.iter().enumerate() {
        if i > 0 {
            time += dt[i - 1];
        }
        if test(fragment) {
            return Some(time);
        }
    }
    None
}
pub fn run_first_token_time(run: &AssistantStreamRun) -> Option<i64> {
    if let AssistantStreamRecord::ToolCallChunks {
        time0,
        name: Some(_),
        ..
    } = run
    {
        return Some(*time0);
    }
    first_run_time(run, |s| !s.is_empty())
}
pub fn run_first_visible_time(run: &AssistantStreamRun) -> Option<i64> {
    if matches!(run, AssistantStreamRecord::ToolCallChunks { .. }) {
        None
    } else {
        first_run_time(run, visible)
    }
}
fn parsed(chunk: &Value) -> Option<StreamChunk> {
    serde_json::from_value(chunk.clone()).ok()
}
pub fn assistant_stream_first_token_time(stream: &[AssistantStreamRecord]) -> Option<i64> {
    stream.iter().find_map(|r| match r {
        AssistantStreamRecord::Chunk { time, chunk } => {
            parsed(chunk).filter(is_token_delta).map(|_| *time)
        }
        run => run_first_token_time(run),
    })
}
pub fn assistant_stream_has_visible_content(stream: &[AssistantStreamRecord]) -> bool {
    stream.iter().any(|r| match r {
        AssistantStreamRecord::Chunk { chunk, .. } => {
            parsed(chunk).is_some_and(|c| is_visible_chunk(&c))
        }
        run => run_first_visible_time(run).is_some(),
    })
}
pub fn assistant_stream_has_visible_text(stream: &[AssistantStreamRecord]) -> bool {
    stream.iter().any(|r| match r {
        AssistantStreamRecord::TextChunks { texts, .. } => texts.iter().any(|t| visible(t)),
        AssistantStreamRecord::Chunk { chunk, .. } => {
            parsed(chunk).is_some_and(|c| chunk_has_visible_text(&c))
        }
        _ => false,
    })
}
pub fn last_assistant_stream_chunk(
    stream: &[AssistantStreamRecord],
    kind: RawStreamChunkType,
) -> Option<StreamChunk> {
    stream.iter().rev().find_map(|r| match r {
        AssistantStreamRecord::Chunk { chunk, .. } if chunk["type"] == kind.as_str() => {
            parsed(chunk)
        }
        _ => None,
    })
}
pub fn assistant_stream_chunks(
    stream: &[AssistantStreamRecord],
    kind: RawStreamChunkType,
) -> Vec<StreamChunk> {
    stream
        .iter()
        .filter_map(|r| match r {
            AssistantStreamRecord::Chunk { chunk, .. } if chunk["type"] == kind.as_str() => {
                parsed(chunk)
            }
            _ => None,
        })
        .collect()
}
pub fn join_assistant_stream_text(stream: &[AssistantStreamRecord]) -> String {
    let mut text = String::new();
    for r in stream {
        match r {
            AssistantStreamRecord::TextChunks { texts, .. } => {
                for t in texts {
                    text += t;
                }
            }
            AssistantStreamRecord::Chunk { chunk, .. } if chunk["type"] == "text-delta" => {
                text += chunk["text"].as_str().unwrap_or_default()
            }
            _ => {}
        }
    }
    text
}
pub fn assemble_assistant_stream(
    stream: &[AssistantStreamRecord],
    assembler: &mut BlockAssembler,
) -> Result<(), ValueError> {
    for r in stream {
        let chunk = match r {
            AssistantStreamRecord::Chunk { chunk, .. } => {
                assembler.push_value(chunk.clone())?;
                continue;
            }
            AssistantStreamRecord::TextChunks { index, texts, .. } => StreamChunk::TextDelta {
                index: *index,
                text: texts.join(""),
            },
            AssistantStreamRecord::ReasoningChunks { index, texts, .. } => {
                StreamChunk::ReasoningDelta {
                    index: *index,
                    text: texts.join(""),
                }
            }
            AssistantStreamRecord::ToolCallChunks {
                index,
                id,
                name,
                args,
                ..
            } => StreamChunk::ToolCallDelta {
                index: *index,
                id: id.clone(),
                name: name.clone(),
                arguments_delta: args.join(""),
            },
        };
        assembler.push(chunk);
    }
    Ok(())
}
