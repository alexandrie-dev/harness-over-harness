#![allow(dead_code)]
use futures_util::TryStreamExt;
use hoh_kernel::{Context, Kernel};
use hoh_llm::*;
use hoh_llm_replay::*;
use hoh_session::SessionEvent;
use serde_json::{Value, json};
pub fn chunks(text: &str) -> Vec<StreamChunk> {
    serde_json::from_value(json!([
        {"type":"block-start","index":0,"blockType":"text"},
        {"type":"text-delta","index":0,"text":text},
        {"type":"block-end","index":0,"block":{"type":"text","text":text}},
        {"type":"usage","usage":{"inputTokens":1,"outputTokens":1}},
        {"type":"finish","reason":{"kind":"stop"}}
    ]))
    .unwrap()
}
pub fn stream_event(seq: u64, turn: u64, step: u64, chunks: &[StreamChunk]) -> SessionEvent {
    let mut accumulator = AssistantStreamAccumulator::new();
    let mut assembler = BlockAssembler::new();
    for (i, chunk) in chunks.iter().enumerate() {
        accumulator
            .push(TimedStreamChunk {
                time: 1000 + i as i64 * 7,
                chunk: chunk.clone(),
            })
            .unwrap();
        assembler.push(chunk.clone());
    }
    let mut value = json!({"type":"assistant/attempt","seq":seq,"time":1000+chunks.len().saturating_sub(1)*7,"data":{"turn":turn,"step":step,"stream":accumulator.snapshot()}});
    if chunks
        .last()
        .is_some_and(|c| matches!(c,StreamChunk::Finish{reason,..} if reason.kind!="error"))
    {
        value["type"] = json!("assistant/message");
        value["surfaceOp"] = json!("append");
        value["data"]["message"] = json!({"id":format!("assistant-{seq}"),"role":"assistant","source":{"kind":"model","provider":"mock","model":"mock"},"content":assembler.blocks().unwrap()});
    }
    SessionEvent::from_value(value)
}
pub fn jsonl(rows: &[Value]) -> String {
    rows.iter()
        .map(|v| hoh_util_values::js_json_stringify(v) + "\n")
        .collect()
}
pub fn header(id: &str, at: i64) -> Value {
    json!({"type":"session","version":4,"id":id,"createdAt":at,"cwd":"{{cwd}}","isSeeded":false,"delegationDepth":0})
}
pub fn fixture(id: &str, at: i64, calls: &[Vec<StreamChunk>]) -> String {
    let mut rows = vec![
        header(id, at),
        json!({"type":"turn/start","data":{"turn":1}}),
    ];
    for (i, chunks) in calls.iter().enumerate() {
        rows.push(json!({"type":"step/start","data":{"turn":1,"step":i+1}}));
        let mut row = stream_event(rows.len() as u64 - 1, 1, i as u64 + 1, chunks)
            .as_value()
            .clone();
        row.as_object_mut().unwrap().shift_remove("seq");
        row.as_object_mut().unwrap().shift_remove("time");
        rows.push(row);
        rows.push(json!({"type":"step/end","data":{"turn":1,"step":i+1}}));
    }
    rows.push(json!({"type":"turn/end","data":{"turn":1,"reason":{"kind":"completed"}}}));
    jsonl(&rows)
}
pub struct Files {
    pub root: tempfile::TempDir,
    pub config: ReplayConfig,
}
impl Files {
    pub fn new(calls: &[Vec<StreamChunk>]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("session.v4.jsonl");
        std::fs::write(&file, fixture("p", 100, calls)).unwrap();
        Self {
            root,
            config: ReplayConfig {
                file: file.to_string_lossy().into_owned(),
                ..Default::default()
            },
        }
    }
    pub fn override_doc(&mut self, value: Value) {
        let file = self.root.path().join("override.json");
        std::fs::write(&file, hoh_util_values::js_json_stringify(&value)).unwrap();
        self.config.override_file = Some(file.to_string_lossy().into_owned());
    }
    pub fn child(&mut self, id: &str, at: i64, calls: &[Vec<StreamChunk>]) -> String {
        let file = self.root.path().join(format!("{id}.jsonl"));
        std::fs::write(&file, fixture(id, at, calls)).unwrap();
        let file = file.to_string_lossy().into_owned();
        self.config
            .child_files
            .get_or_insert_default()
            .push(file.clone());
        file
    }
}
pub fn options(id: Option<&str>) -> GenerateOptions {
    let mut result: GenerateOptions =
        serde_json::from_value(json!({"provider":"m","model":"m","messages":[]})).unwrap();
    result.session_id = id.map(Into::into);
    result
}
pub async fn runtime() -> (Kernel, Context, hoh_kernel::Service<Llm>) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<LlmPlugin>(Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let llm = ctx.get::<Llm>().unwrap();
    (kernel, ctx, llm)
}
pub async fn drain(stream: LlmStream) -> Result<Vec<StreamChunk>> {
    stream.try_collect().await
}
pub fn message(text: &str) -> Vec<RequestMessage> {
    serde_json::from_value(json!([{"role":"user","content":[{"type":"text","text":text}]}]))
        .unwrap()
}
