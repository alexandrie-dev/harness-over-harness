//! A tiny HTTP/1.1 server on 127.0.0.1 that replays recorded replies, and helpers.
#![allow(dead_code)]
use futures_util::StreamExt;
use hoh_llm::{
    AssistantProviderMetadata, BlockAssembler, ContentBlock, GenerateOptions, LlmAdapter,
    MessageSource, NewAssistantMessage, NewUserMessage, RequestMessage, StreamChunk, ToolCallBlock,
    ToolResultMessageInput, ToolSchema, create_assistant_message, create_tool_result_message,
    create_user_message,
};
use hoh_llm_openai::{OpenAiAdapter, OpenAiConfig};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type Routes = Arc<Mutex<HashMap<String, (Vec<Reply>, usize)>>>;

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    /// Written one by one with a flush between them, so tests choose where chunks split.
    pub pieces: Vec<Vec<u8>>,
    /// Keep the connection open and silent after the pieces.
    pub stall: bool,
}

impl Reply {
    pub fn sse(pieces: Vec<Vec<u8>>) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream",
            headers: vec![],
            pieces,
            stall: false,
        }
    }
    pub fn sse_text(text: &str) -> Self {
        Self::sse(vec![text.as_bytes().to_vec()])
    }
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "application/json",
            headers: vec![],
            pieces: vec![body.as_bytes().to_vec()],
            stall: false,
        }
    }
    pub fn stalled(mut self) -> Self {
        self.stall = true;
        self
    }
    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }
}

#[derive(Clone, Debug)]
pub struct Captured {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Captured {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("request body is JSON")
    }
}

pub struct Server {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<Captured>>>,
}

impl Server {
    pub fn captured(&self) -> Vec<Captured> {
        self.requests.lock().unwrap().clone()
    }
    pub fn last_body(&self) -> Value {
        self.captured().last().expect("a request").json()
    }
}

/// `routes`: `"POST /v1/chat/completions"` → replies in order; the last one repeats.
pub async fn serve(routes: Vec<(&str, Vec<Reply>)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let table: Routes = Arc::new(Mutex::new(
        routes
            .into_iter()
            .map(|(k, v)| (k.to_owned(), (v, 0)))
            .collect(),
    ));
    let log = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let (table, log) = (table.clone(), log.clone());
            tokio::spawn(async move {
                let _ = handle(socket, table, log).await;
            });
        }
    });
    Server { base_url, requests }
}

async fn handle(
    mut socket: TcpStream,
    table: Routes,
    log: Arc<Mutex<Vec<Captured>>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or("").split(' ');
    let (method, path) = (
        request_line.next().unwrap_or("").to_owned(),
        request_line.next().unwrap_or("").to_owned(),
    );
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while buffer.len() < head_end + length {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buffer[head_end..]).into_owned();
    log.lock().unwrap().push(Captured {
        method: method.clone(),
        path: path.clone(),
        headers,
        body,
    });
    let reply = {
        let mut table = table.lock().unwrap();
        table
            .get_mut(&format!("{method} {path}"))
            .map(|(replies, next)| {
                let reply = replies[(*next).min(replies.len() - 1)].clone();
                *next += 1;
                reply
            })
    };
    let Some(reply) = reply else {
        socket
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    };
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nConnection: close\r\n",
        reply.status, reply.content_type
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    socket.write_all(head.as_bytes()).await?;
    for piece in &reply.pieces {
        socket.write_all(piece).await?;
        socket.flush().await?;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    if reply.stall {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
    socket.shutdown().await
}

pub const CHAT: &str = "POST /v1/chat/completions";

pub fn adapter(server: &Server, edit: impl FnOnce(&mut OpenAiConfig)) -> OpenAiAdapter {
    let mut config = OpenAiConfig::new(server.base_url.clone());
    config.stream_idle_timeout_ms = 5_000;
    edit(&mut config);
    OpenAiAdapter::new(config).unwrap()
}

pub fn user(text: &str) -> RequestMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
    .into()
}

pub fn assistant(content: Vec<ContentBlock>) -> RequestMessage {
    create_assistant_message(NewAssistantMessage {
        content,
        source: AssistantProviderMetadata {
            provider: "local".into(),
            model: "qwen".into(),
            replay_state: None,
        },
    })
    .into()
}

pub fn call(id: &str, name: &str, arguments: &str) -> ContentBlock {
    ToolCallBlock {
        id: id.into(),
        name: name.into(),
        arguments: arguments.into(),
    }
    .into()
}

pub fn tool_result(id: &str, text: &str) -> RequestMessage {
    create_tool_result_message(ToolResultMessageInput {
        call_id: id.into(),
        content: vec![ContentBlock::text(text)],
        is_error: false,
    })
    .into()
}

pub fn tool(name: &str, required: &[&str]) -> ToolSchema {
    let mut parameters = Map::new();
    parameters.insert("type".into(), json!("object"));
    parameters.insert("properties".into(), json!({"city": {"type": "string"}}));
    parameters.insert("required".into(), json!(required));
    ToolSchema {
        name: name.into(),
        description: format!("Tool {name}."),
        parameters,
        defer_loading: None,
    }
}

pub fn options(messages: Vec<RequestMessage>) -> GenerateOptions {
    GenerateOptions {
        provider: "local".into(),
        model: "qwen".into(),
        messages,
        ..Default::default()
    }
}

pub struct Run {
    pub chunks: Vec<StreamChunk>,
    pub error: Option<hoh_error::HarnessError>,
}

impl Run {
    pub fn code(&self) -> &str {
        self.error.as_ref().expect("an error").code()
    }
    pub fn assembled(&self) -> BlockAssembler {
        let mut assembler = BlockAssembler::new();
        for chunk in &self.chunks {
            assembler.push(chunk.clone());
        }
        assembler
    }
    pub fn kinds(&self) -> Vec<&'static str> {
        self.chunks.iter().map(StreamChunk::kind).collect()
    }
    pub fn blocks(&self) -> Vec<(String, String)> {
        self.assembled()
            .blocks()
            .unwrap()
            .iter()
            .map(|b| {
                let value = b.text_value().map(str::to_owned).unwrap_or_else(|| {
                    format!(
                        "{}({})",
                        b.data["name"].as_str().unwrap_or(""),
                        b.data["arguments"].as_str().unwrap_or("")
                    )
                });
                (b.r#type.clone(), value)
            })
            .collect()
    }
    pub fn finish_kind(&self) -> String {
        match self.chunks.last() {
            Some(StreamChunk::Finish { reason, .. }) => reason.kind.clone(),
            other => panic!("stream did not end with a finish chunk: {other:?}"),
        }
    }
    pub fn usage(&self) -> Option<hoh_llm::TokenUsage> {
        self.chunks.iter().find_map(|c| match c {
            StreamChunk::Usage { usage, .. } => Some(usage.clone()),
            _ => None,
        })
    }
}

pub async fn run(adapter: &OpenAiAdapter, options: GenerateOptions) -> Run {
    let mut stream = adapter.stream(options).unwrap();
    let mut chunks = Vec::new();
    let mut error = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => chunks.push(chunk),
            Err(e) => {
                let e: &(dyn std::error::Error + 'static) = &*e;
                error = Some(
                    hoh_error::HarnessError::of(e)
                        .expect("a HarnessError")
                        .clone(),
                );
                break;
            }
        }
    }
    Run { chunks, error }
}

pub async fn run_simple(server: &Server, edit: impl FnOnce(&mut OpenAiConfig)) -> Run {
    run(&adapter(server, edit), options(vec![user("hi")])).await
}

/// One `data:` frame.
pub fn data(value: Value) -> String {
    format!("data: {value}\n\n")
}

pub fn done() -> &'static str {
    "data: [DONE]\n\n"
}

pub fn delta(fields: Value) -> String {
    data(
        json!({"id": "c1", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": fields, "finish_reason": null}]}),
    )
}

pub fn finish(reason: &str) -> String {
    data(
        json!({"id": "c1", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}),
    )
}

pub fn usage_chunk(prompt: u64, completion: u64) -> String {
    data(
        json!({"id": "c1", "object": "chat.completion.chunk", "choices": [],
        "usage": {"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion}}),
    )
}
