//! Scriptable HTTP/SSE server for exercising LLM clients.

pub mod cli;

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

/// Largest millisecond delay accepted by Node timers without truncation.
pub const MAX_MOCK_LLM_TIMER_DELAY_MS: u64 = 2_147_483_647;

/// Default stress profile used by [`MockLlmBehavior::Random`].
pub static DEFAULT_MOCK_LLM_RANDOM_WEIGHTS: LazyLock<IndexMap<String, f64>> = LazyLock::new(|| {
    [
        ("success", 48.0),
        ("slow_success", 10.0),
        ("max_tokens", 2.0),
        ("connection_reset", 5.0),
        ("stream_disconnect", 5.0),
        ("partial_disconnect", 10.0),
        ("empty", 5.0),
        ("stall", 2.0),
        ("rate_limit", 5.0),
        ("server_error", 4.0),
        ("service_unavailable", 2.0),
        ("partial_eof", 1.0),
        ("malformed_json", 1.0),
    ]
    .into_iter()
    .map(|(name, weight)| (name.to_owned(), weight))
    .collect()
});

/// All request-scoped behavior names accepted by the TypeScript package.
pub const MOCK_LLM_BEHAVIORS: &[&str] = &[
    "connection_reset",
    "stream_disconnect",
    "empty",
    "empty_body",
    "stream_eof",
    "partial_eof",
    "partial_disconnect",
    "stall",
    "malformed_json",
    "malformed_event",
    "wrong_content_type",
    "rate_limit",
    "server_error",
    "service_unavailable",
    "auth_error",
    "invalid_request",
    "context_overflow",
    "quota_exceeded",
    "success",
    "reasoning_success",
    "tool_call_success",
    "max_tokens",
    "slow_success",
    "random",
];

/// One scripted mock behavior.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MockLlmBehavior {
    ConnectionReset,
    StreamDisconnect,
    Empty,
    EmptyBody,
    StreamEof,
    PartialEof,
    PartialDisconnect,
    Stall,
    MalformedJson,
    MalformedEvent,
    WrongContentType,
    RateLimit,
    ServerError,
    ServiceUnavailable,
    AuthError,
    InvalidRequest,
    ContextOverflow,
    QuotaExceeded,
    Success,
    ReasoningSuccess,
    ToolCallSuccess,
    MaxTokens,
    SlowSuccess,
    Random,
}

impl MockLlmBehavior {
    /// Wire name used by the JavaScript package.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionReset => "connection_reset",
            Self::StreamDisconnect => "stream_disconnect",
            Self::Empty => "empty",
            Self::EmptyBody => "empty_body",
            Self::StreamEof => "stream_eof",
            Self::PartialEof => "partial_eof",
            Self::PartialDisconnect => "partial_disconnect",
            Self::Stall => "stall",
            Self::MalformedJson => "malformed_json",
            Self::MalformedEvent => "malformed_event",
            Self::WrongContentType => "wrong_content_type",
            Self::RateLimit => "rate_limit",
            Self::ServerError => "server_error",
            Self::ServiceUnavailable => "service_unavailable",
            Self::AuthError => "auth_error",
            Self::InvalidRequest => "invalid_request",
            Self::ContextOverflow => "context_overflow",
            Self::QuotaExceeded => "quota_exceeded",
            Self::Success => "success",
            Self::ReasoningSuccess => "reasoning_success",
            Self::ToolCallSuccess => "tool_call_success",
            Self::MaxTokens => "max_tokens",
            Self::SlowSuccess => "slow_success",
            Self::Random => "random",
        }
    }

    /// Parse a behavior's wire name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        serde_json::from_value(Value::String(value.to_owned())).ok()
    }

    const fn concrete(self) -> bool {
        !matches!(self, Self::Random)
    }
}

/// Script entry captured before random resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MockLlmScriptBehavior {
    Behavior(MockLlmBehavior),
    ScriptExhausted,
}

impl MockLlmScriptBehavior {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Behavior(value) => value.as_str(),
            Self::ScriptExhausted => "script_exhausted",
        }
    }
}

/// Concrete behavior selected for a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MockLlmSelectedBehavior {
    Behavior(MockLlmBehavior),
    ScriptExhausted,
}

impl MockLlmSelectedBehavior {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Behavior(value) => value.as_str(),
            Self::ScriptExhausted => "script_exhausted",
        }
    }
}

/// How one accepted request ended at the mock boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MockLlmRequestOutcome {
    Completed,
    Reset,
    Stalled,
    ClientClosed,
    ServerError,
}

impl MockLlmRequestOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Reset => "reset",
            Self::Stalled => "stalled",
            Self::ClientClosed => "client_closed",
            Self::ServerError => "server_error",
        }
    }
}

/// Immutable telemetry emitted as a request starts and finishes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MockLlmServerEvent {
    Request {
        attempt: usize,
        script_behavior: MockLlmScriptBehavior,
        behavior: MockLlmSelectedBehavior,
        path: String,
    },
    Result {
        attempt: usize,
        script_behavior: MockLlmScriptBehavior,
        behavior: MockLlmSelectedBehavior,
        outcome: MockLlmRequestOutcome,
        chunks_sent: usize,
    },
}

impl Serialize for MockLlmServerEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let value = match self {
            Self::Request {
                attempt,
                script_behavior,
                behavior,
                path,
            } => json!({
                "type": "request", "attempt": attempt,
                "scriptBehavior": script_behavior.as_str(), "behavior": behavior.as_str(), "path": path,
            }),
            Self::Result {
                attempt,
                script_behavior,
                behavior,
                outcome,
                chunks_sent,
            } => json!({
                "type": "result", "attempt": attempt,
                "scriptBehavior": script_behavior.as_str(), "behavior": behavior.as_str(),
                "outcome": outcome.as_str(), "chunksSent": chunks_sent,
            }),
        };
        value.serialize(serializer)
    }
}

/// Captured request and its final server-side outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct MockLlmRequestRecord {
    pub attempt: usize,
    pub script_behavior: MockLlmScriptBehavior,
    pub behavior: MockLlmSelectedBehavior,
    pub path: String,
    pub headers: IndexMap<String, String>,
    pub body: Option<Value>,
    pub chunks_sent: usize,
    pub outcome: Option<MockLlmRequestOutcome>,
}

/// Configuration for one mock server.
#[derive(Clone, Default)]
pub struct MockLlmServerOptions {
    pub host: Option<String>,
    pub port: Option<f64>,
    pub api_key: Option<String>,
    pub sequence: Vec<MockLlmBehavior>,
    pub repeat_last: Option<bool>,
    pub random_seed: Option<f64>,
    pub random_weights: Option<IndexMap<String, f64>>,
    pub success_text: Option<String>,
    pub partial_text: Option<String>,
    pub reasoning_text: Option<String>,
    pub chunk_size: Option<f64>,
    pub chunk_delay_ms: Option<f64>,
    pub disconnect_delay_ms: Option<f64>,
    pub retry_after_ms: Option<f64>,
    pub request_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_arguments: Option<String>,
    pub on_event: Option<Arc<dyn Fn(MockLlmServerEvent) + Send + Sync>>,
}

impl fmt::Debug for MockLlmServerOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MockLlmServerOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("sequence", &self.sequence)
            .field("repeat_last", &self.repeat_last)
            .finish_non_exhaustive()
    }
}

/// Configuration or listener error.
#[derive(Debug)]
pub struct MockLlmServerError(String);

impl MockLlmServerError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for MockLlmServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MockLlmServerError {}

impl From<std::io::Error> for MockLlmServerError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Clone)]
struct ResolvedOptions {
    host: String,
    port: u16,
    api_key: Option<String>,
    sequence: Vec<MockLlmBehavior>,
    last_behavior: MockLlmBehavior,
    repeat_last: bool,
    random_seed: u32,
    random_weights: Vec<(MockLlmBehavior, f64)>,
    success_text: String,
    partial_text: String,
    reasoning_text: String,
    chunk_size: usize,
    chunk_delay_ms: u64,
    disconnect_delay_ms: u64,
    retry_after_ms: u64,
    request_id: Option<String>,
    tool_name: String,
    tool_arguments: String,
    on_event: Option<Arc<dyn Fn(MockLlmServerEvent) + Send + Sync>>,
}

fn bounded_integer(name: &str, value: f64, min: u64, max: u64) -> Result<u64, MockLlmServerError> {
    if !value.is_finite() || value.fract() != 0.0 || value < min as f64 || value > max as f64 {
        return Err(MockLlmServerError::new(format!(
            "llm-mock-server: {name} must be an integer between {min} and {max}"
        )));
    }
    Ok(value as u64)
}

fn default_weights() -> IndexMap<String, f64> {
    DEFAULT_MOCK_LLM_RANDOM_WEIGHTS.clone()
}

fn resolve_options(options: MockLlmServerOptions) -> Result<ResolvedOptions, MockLlmServerError> {
    let host = options.host.unwrap_or_else(|| "127.0.0.1".to_owned());
    let port = bounded_integer("port", options.port.unwrap_or(0.0), 0, 65_535)? as u16;
    let chunk_size = bounded_integer(
        "chunkSize",
        options.chunk_size.unwrap_or(8.0),
        1,
        9_007_199_254_740_991,
    )? as usize;
    let chunk_delay_ms = bounded_integer(
        "chunkDelayMs",
        options.chunk_delay_ms.unwrap_or(25.0),
        0,
        MAX_MOCK_LLM_TIMER_DELAY_MS,
    )?;
    let disconnect_delay_ms = bounded_integer(
        "disconnectDelayMs",
        options.disconnect_delay_ms.unwrap_or(10.0),
        0,
        MAX_MOCK_LLM_TIMER_DELAY_MS,
    )?;
    let retry_after_ms = bounded_integer(
        "retryAfterMs",
        options.retry_after_ms.unwrap_or(1_000.0),
        1,
        MAX_MOCK_LLM_TIMER_DELAY_MS,
    )?;
    let random_seed = match options.random_seed {
        Some(seed) => bounded_integer("randomSeed", seed, 0, u32::MAX.into())? as u32,
        None => {
            let mut bytes = [0_u8; 4];
            getrandom::fill(&mut bytes)
                .map_err(|error| MockLlmServerError::new(error.to_string()))?;
            u32::from_le_bytes(bytes)
        }
    };
    let success_text = options
        .success_text
        .unwrap_or_else(|| "mock response recovered".to_owned());
    let partial_text = options
        .partial_text
        .unwrap_or_else(|| "discarded partial response".to_owned());
    let reasoning_text = options
        .reasoning_text
        .unwrap_or_else(|| "mock reasoning".to_owned());
    let tool_name = options.tool_name.unwrap_or_else(|| "mock_tool".to_owned());
    let tool_arguments = options
        .tool_arguments
        .unwrap_or_else(|| r#"{"value":"mock"}"#.to_owned());

    if host.is_empty() {
        return Err(MockLlmServerError::new(
            "llm-mock-server: host must not be empty",
        ));
    }
    if options.sequence.is_empty() {
        return Err(MockLlmServerError::new(
            "llm-mock-server: sequence must not be empty",
        ));
    }
    let last_behavior = *options.sequence.last().expect("non-empty checked above");
    if options.api_key.as_deref() == Some("") {
        return Err(MockLlmServerError::new(
            "llm-mock-server: apiKey must not be empty",
        ));
    }
    for (value, name) in [
        (&success_text, "successText"),
        (&partial_text, "partialText"),
        (&reasoning_text, "reasoningText"),
        (&tool_name, "toolName"),
    ] {
        if value.is_empty() {
            return Err(MockLlmServerError::new(format!(
                "llm-mock-server: {name} must not be empty"
            )));
        }
    }
    if options.request_id.as_deref() == Some("") {
        return Err(MockLlmServerError::new(
            "llm-mock-server: requestId must not be empty",
        ));
    }
    if hoh_util_values::js_json_parse(&tool_arguments).is_err() {
        return Err(MockLlmServerError::new(
            "llm-mock-server: toolArguments must be valid JSON",
        ));
    }

    let weights = options.random_weights.unwrap_or_else(default_weights);
    let mut random_weights = Vec::new();
    for (name, weight) in weights {
        let Some(behavior) = MockLlmBehavior::parse(&name).filter(|value| value.concrete()) else {
            let quoted = hoh_util_values::js_json_stringify(&Value::String(name.clone()));
            return Err(MockLlmServerError::new(format!(
                "llm-mock-server: randomWeights contains unknown concrete behavior {quoted}"
            )));
        };
        if !weight.is_finite() || weight < 0.0 {
            return Err(MockLlmServerError::new(format!(
                "llm-mock-server: random weight for {name} must be a non-negative finite number"
            )));
        }
        if weight > 0.0 {
            random_weights.push((behavior, weight));
        }
    }
    if random_weights.is_empty() {
        return Err(MockLlmServerError::new(
            "llm-mock-server: randomWeights must contain at least one positive weight",
        ));
    }
    Ok(ResolvedOptions {
        host,
        port,
        api_key: options.api_key,
        sequence: options.sequence,
        last_behavior,
        repeat_last: options.repeat_last.unwrap_or(false),
        random_seed,
        random_weights,
        success_text,
        partial_text,
        reasoning_text,
        chunk_size,
        chunk_delay_ms,
        disconnect_delay_ms,
        retry_after_ms,
        request_id: options.request_id,
        tool_name,
        tool_arguments,
        on_event: options.on_event,
    })
}

#[derive(Clone, Copy)]
struct Mulberry32 {
    state: u32,
}

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 15)).wrapping_mul(mixed | 1);
        mixed ^= mixed.wrapping_add((mixed ^ (mixed >> 7)).wrapping_mul(mixed | 61));
        f64::from(mixed ^ (mixed >> 14)) / 4_294_967_296.0
    }
}

struct ServerState {
    cursor: usize,
    random: Mulberry32,
    requests: Vec<MockLlmRequestRecord>,
}

struct Shared {
    options: ResolvedOptions,
    state: Mutex<ServerState>,
}

impl Shared {
    fn emit(&self, event: MockLlmServerEvent) {
        if let Some(observer) = &self.options.on_event {
            let observer = Arc::clone(observer);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observer(event)));
        }
    }

    async fn accept_request(
        &self,
        path: String,
        headers: IndexMap<String, String>,
        body: Option<Value>,
    ) -> MockLlmRequestRecord {
        let mut state = self.state.lock().await;
        let selected = self.options.sequence.get(state.cursor).copied();
        state.cursor += 1;
        let script = match selected {
            Some(script) => script,
            None if self.options.repeat_last => self.options.last_behavior,
            None => {
                let record = MockLlmRequestRecord {
                    attempt: state.requests.len() + 1,
                    script_behavior: MockLlmScriptBehavior::ScriptExhausted,
                    behavior: MockLlmSelectedBehavior::ScriptExhausted,
                    path,
                    headers,
                    body,
                    chunks_sent: 0,
                    outcome: None,
                };
                state.requests.push(record.clone());
                return record;
            }
        };
        let behavior = if script == MockLlmBehavior::Random {
            choose_random(&self.options.random_weights, &mut state.random)
        } else {
            script
        };
        let record = MockLlmRequestRecord {
            attempt: state.requests.len() + 1,
            script_behavior: MockLlmScriptBehavior::Behavior(script),
            behavior: MockLlmSelectedBehavior::Behavior(behavior),
            path,
            headers,
            body,
            chunks_sent: 0,
            outcome: None,
        };
        state.requests.push(record.clone());
        record
    }

    async fn add_chunk(&self, attempt: usize) {
        if let Some(record) = self.state.lock().await.requests.get_mut(attempt - 1) {
            record.chunks_sent += 1;
        }
    }

    async fn finish(&self, attempt: usize, outcome: MockLlmRequestOutcome) {
        let event = {
            let mut state = self.state.lock().await;
            let Some(record) = state.requests.get_mut(attempt - 1) else {
                return;
            };
            if record.outcome.is_some() {
                return;
            }
            record.outcome = Some(outcome);
            MockLlmServerEvent::Result {
                attempt,
                script_behavior: record.script_behavior,
                behavior: record.behavior,
                outcome,
                chunks_sent: record.chunks_sent,
            }
        };
        self.emit(event);
    }

    async fn finish_open_as_client_closed(&self) {
        let events = {
            let mut state = self.state.lock().await;
            state
                .requests
                .iter_mut()
                .filter_map(|record| {
                    if record.outcome.is_some() {
                        return None;
                    }
                    record.outcome = Some(MockLlmRequestOutcome::ClientClosed);
                    Some(MockLlmServerEvent::Result {
                        attempt: record.attempt,
                        script_behavior: record.script_behavior,
                        behavior: record.behavior,
                        outcome: MockLlmRequestOutcome::ClientClosed,
                        chunks_sent: record.chunks_sent,
                    })
                })
                .collect::<Vec<_>>()
        };
        for event in events {
            self.emit(event);
        }
    }
}

fn choose_random(weights: &[(MockLlmBehavior, f64)], random: &mut Mulberry32) -> MockLlmBehavior {
    let mut draw = random.next() * weights.iter().map(|entry| entry.1).sum::<f64>();
    for &(behavior, weight) in weights {
        if draw < weight {
            return behavior;
        }
        draw -= weight;
    }
    weights.last().expect("positive weight validated").0
}

struct HttpRequest {
    method: String,
    target: String,
    headers: IndexMap<String, String>,
    body: Vec<u8>,
    close: bool,
}

struct Connection {
    stream: TcpStream,
    buffered: Vec<u8>,
}

impl Connection {
    async fn read_more(&mut self) -> std::io::Result<bool> {
        let mut bytes = [0_u8; 8_192];
        let count = self.stream.read(&mut bytes).await?;
        self.buffered.extend_from_slice(&bytes[..count]);
        Ok(count != 0)
    }

    async fn request(&mut self) -> std::io::Result<Option<HttpRequest>> {
        let header_end = loop {
            if let Some(index) = find_bytes(&self.buffered, b"\r\n\r\n") {
                break index + 4;
            }
            if self.buffered.len() > 1024 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "headers too large",
                ));
            }
            if !self.read_more().await? {
                return if self.buffered.is_empty() {
                    Ok(None)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "partial headers",
                    ))
                };
            }
        };
        let head = String::from_utf8(self.buffered[..header_end - 4].to_vec())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid headers"))?;
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap_or_default().to_owned();
        let target = request_parts.next().unwrap_or_default().to_owned();
        let version = request_parts.next().unwrap_or_default();
        let mut headers = IndexMap::new();
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid header",
                ));
            };
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
        self.buffered.drain(..header_end);
        let body = if headers.get("transfer-encoding").is_some_and(|value| {
            value
                .split(',')
                .any(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
        }) {
            self.chunked_body().await?
        } else {
            let size = headers.get("content-length").map_or(Ok(0), |value| {
                value.parse::<usize>().map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid content-length")
                })
            })?;
            while self.buffered.len() < size {
                if !self.read_more().await? {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "partial body",
                    ));
                }
            }
            self.buffered.drain(..size).collect()
        };
        let close = headers
            .get("connection")
            .is_some_and(|value| value.eq_ignore_ascii_case("close"))
            || (version == "HTTP/1.0"
                && !headers
                    .get("connection")
                    .is_some_and(|value| value.eq_ignore_ascii_case("keep-alive")));
        Ok(Some(HttpRequest {
            method,
            target,
            headers,
            body,
            close,
        }))
    }

    async fn chunked_body(&mut self) -> std::io::Result<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            let line_end = loop {
                if let Some(index) = find_bytes(&self.buffered, b"\r\n") {
                    break index;
                }
                if !self.read_more().await? {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "partial chunk",
                    ));
                }
            };
            let line = std::str::from_utf8(&self.buffered[..line_end]).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid chunk size")
            })?;
            let size = usize::from_str_radix(line.split(';').next().unwrap_or_default().trim(), 16)
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid chunk size")
                })?;
            self.buffered.drain(..line_end + 2);
            if size == 0 {
                loop {
                    if let Some(end) = find_bytes(&self.buffered, b"\r\n") {
                        let empty = end == 0;
                        self.buffered.drain(..end + 2);
                        if empty {
                            return Ok(body);
                        }
                    } else if !self.read_more().await? {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "partial trailers",
                        ));
                    }
                }
            }
            while self.buffered.len() < size + 2 {
                if !self.read_more().await? {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "partial chunk",
                    ));
                }
            }
            body.extend(self.buffered.drain(..size));
            if self.buffered.drain(..2).collect::<Vec<_>>() != b"\r\n" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid chunk ending",
                ));
            }
        }
    }

    async fn fixed(
        &mut self,
        status: u16,
        reason: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> std::io::Result<()> {
        let mut response = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        self.stream.write_all(response.as_bytes()).await?;
        self.stream.write_all(body).await
    }

    async fn open_sse(&mut self, content_type: &str) -> std::io::Result<()> {
        self.stream.write_all(format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncache-control: no-cache\r\nconnection: keep-alive\r\ntransfer-encoding: chunked\r\n\r\n"
        ).as_bytes()).await
    }

    async fn sse(&mut self, payload: SsePayload<'_>) -> std::io::Result<()> {
        let data = match payload {
            SsePayload::Raw(value) => format!("data: {value}\n\n"),
            SsePayload::Json(value) => format!(
                "event: {}\ndata: {}\n\n",
                value["type"].as_str().unwrap_or_default(),
                hoh_util_values::js_json_stringify(&value),
            ),
        };
        let chunk = format!("{:x}\r\n{data}\r\n", data.len());
        self.stream.write_all(chunk.as_bytes()).await
    }

    async fn end_chunks(&mut self) -> std::io::Result<()> {
        self.stream.write_all(b"0\r\n\r\n").await
    }
}

enum SsePayload<'a> {
    Raw(&'a str),
    Json(Value),
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[allow(deprecated)]
fn reset_on_drop(stream: &TcpStream) -> std::io::Result<()> {
    // This intentionally models Node's `socket.destroy()` transport reset. The
    // zero timeout cannot block; Tokio's deprecation concerns non-zero linger.
    stream.set_linger(Some(Duration::ZERO))
}

fn path_from_target(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

async fn send_plain_error(
    connection: &mut Connection,
    status: u16,
    reason: &str,
) -> std::io::Result<()> {
    connection.fixed(status, reason, &[], b"").await
}

async fn send_json_error(
    connection: &mut Connection,
    status: u16,
    reason: &str,
    message: &str,
    code: &str,
) -> std::io::Result<()> {
    let body = hoh_util_values::js_json_stringify(
        &json!({ "error": { "message": message, "code": code } }),
    )
    .into_bytes();
    connection
        .fixed(
            status,
            reason,
            &[("content-type", "application/json".to_owned())],
            &body,
        )
        .await
}

async fn send_event(
    shared: &Shared,
    record: &mut MockLlmRequestRecord,
    connection: &mut Connection,
    payload: SsePayload<'_>,
) -> std::io::Result<()> {
    connection.sse(payload).await?;
    record.chunks_sent += 1;
    shared.add_chunk(record.attempt).await;
    Ok(())
}

async fn start_message(
    shared: &Shared,
    record: &mut MockLlmRequestRecord,
    connection: &mut Connection,
) -> std::io::Result<()> {
    send_event(shared, record, connection, SsePayload::Json(json!({
        "type":"message_start",
        "message":{"id":"mock-message","type":"message","role":"assistant","model":"mock-model","content":[],"usage":{"input_tokens":3,"output_tokens":0}}
    }))).await
}

async fn start_text(
    shared: &Shared,
    record: &mut MockLlmRequestRecord,
    connection: &mut Connection,
    index: usize,
) -> std::io::Result<()> {
    send_event(
        shared,
        record,
        connection,
        SsePayload::Json(json!({
            "type":"content_block_start","index":index,"content_block":{"type":"text","text":""}
        })),
    )
    .await
}

fn split_text(text: &str, size: usize) -> Vec<String> {
    let points: Vec<char> = text.chars().collect();
    points
        .chunks(size)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

async fn pause_or_closed(milliseconds: u64, stream: &TcpStream) -> bool {
    if milliseconds == 0 {
        return true;
    }
    tokio::select! {
        () = tokio::time::sleep(Duration::from_millis(milliseconds)) => true,
        ready = stream.readable() => {
            if ready.is_err() { return false; }
            let mut byte = [0_u8; 1];
            match stream.peek(&mut byte).await {
                Ok(0) | Err(_) => false,
                Ok(_) => {
                    tokio::time::sleep(Duration::from_millis(milliseconds)).await;
                    true
                }
            }
        }
    }
}

async fn stream_text(
    shared: &Shared,
    record: &mut MockLlmRequestRecord,
    connection: &mut Connection,
    text: &str,
    delay: u64,
    index: usize,
) -> std::io::Result<()> {
    for chunk in split_text(text, shared.options.chunk_size) {
        send_event(shared, record, connection, SsePayload::Json(json!({
            "type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":chunk}
        }))).await?;
        if !pause_or_closed(delay, &connection.stream).await {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "client closed during stream delay",
            ));
        }
    }
    Ok(())
}

async fn done(
    shared: &Shared,
    record: &mut MockLlmRequestRecord,
    connection: &mut Connection,
) -> std::io::Result<()> {
    send_event(
        shared,
        record,
        connection,
        SsePayload::Json(json!({"type":"message_stop"})),
    )
    .await
}

async fn complete_text(
    shared: &Shared,
    record: &mut MockLlmRequestRecord,
    connection: &mut Connection,
    reason: &str,
    delay: u64,
    index: usize,
) -> std::io::Result<()> {
    start_text(shared, record, connection, index).await?;
    let text = shared.options.success_text.clone();
    stream_text(shared, record, connection, &text, delay, index).await?;
    send_event(
        shared,
        record,
        connection,
        SsePayload::Json(json!({
            "type":"content_block_stop","index":index
        })),
    )
    .await?;
    send_event(
        shared,
        record,
        connection,
        SsePayload::Json(json!({
            "type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},
            "usage":{"output_tokens":text.chars().count()}
        })),
    )
    .await?;
    done(shared, record, connection).await?;
    connection.end_chunks().await
}

struct HttpBehaviorError<'a> {
    status: u16,
    reason: &'a str,
    message: &'a str,
    code: &'a str,
    kind: &'a str,
}

async fn http_behavior_error(
    shared: &Shared,
    record: &MockLlmRequestRecord,
    connection: &mut Connection,
    error: HttpBehaviorError<'_>,
) -> std::io::Result<()> {
    let body = hoh_util_values::js_json_stringify(&json!({
        "error": { "message": error.message, "type": error.kind, "code": error.code }
    }))
    .into_bytes();
    let mut headers = vec![("content-type", "application/json".to_owned())];
    if record.behavior == MockLlmSelectedBehavior::Behavior(MockLlmBehavior::RateLimit) {
        headers.push((
            "retry-after",
            shared.options.retry_after_ms.div_ceil(1_000).to_string(),
        ));
    }
    if let Some(request_id) = &shared.options.request_id {
        headers.push(("x-request-id", request_id.clone()));
    }
    connection
        .fixed(error.status, error.reason, &headers, &body)
        .await
}

async fn run_behavior(
    shared: &Shared,
    record: &mut MockLlmRequestRecord,
    connection: &mut Connection,
) -> std::io::Result<bool> {
    let selected = record.behavior;
    if selected == MockLlmSelectedBehavior::ScriptExhausted {
        http_behavior_error(
            shared,
            record,
            connection,
            HttpBehaviorError {
                status: 500,
                reason: "Internal Server Error",
                message: "mock script exhausted",
                code: "MOCK_SCRIPT_EXHAUSTED",
                kind: "mock_error",
            },
        )
        .await?;
        shared
            .finish(record.attempt, MockLlmRequestOutcome::Completed)
            .await;
        return Ok(true);
    }
    let MockLlmSelectedBehavior::Behavior(behavior) = selected else {
        unreachable!()
    };
    match behavior {
        MockLlmBehavior::ConnectionReset => {
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Reset)
                .await;
            reset_on_drop(&connection.stream)?;
            Ok(false)
        }
        MockLlmBehavior::StreamDisconnect => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            if !pause_or_closed(shared.options.disconnect_delay_ms, &connection.stream).await {
                shared
                    .finish(record.attempt, MockLlmRequestOutcome::ClientClosed)
                    .await;
                return Ok(false);
            }
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Reset)
                .await;
            reset_on_drop(&connection.stream)?;
            Ok(false)
        }
        MockLlmBehavior::Empty => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            start_message(shared, record, connection).await?;
            send_event(shared, record, connection, SsePayload::Json(json!({
                "type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":0}
            }))).await?;
            done(shared, record, connection).await?;
            connection.end_chunks().await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::EmptyBody => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            connection.end_chunks().await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::StreamEof => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            start_message(shared, record, connection).await?;
            connection.end_chunks().await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::PartialEof => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            start_message(shared, record, connection).await?;
            start_text(shared, record, connection, 0).await?;
            let text = shared.options.partial_text.clone();
            stream_text(shared, record, connection, &text, 0, 0).await?;
            connection.end_chunks().await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::PartialDisconnect => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            start_message(shared, record, connection).await?;
            start_text(shared, record, connection, 0).await?;
            let text = shared.options.partial_text.clone();
            stream_text(
                shared,
                record,
                connection,
                &text,
                shared.options.chunk_delay_ms,
                0,
            )
            .await?;
            if !pause_or_closed(shared.options.disconnect_delay_ms, &connection.stream).await {
                shared
                    .finish(record.attempt, MockLlmRequestOutcome::ClientClosed)
                    .await;
                return Ok(false);
            }
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Reset)
                .await;
            reset_on_drop(&connection.stream)?;
            Ok(false)
        }
        MockLlmBehavior::Stall => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Stalled)
                .await;
            std::future::pending::<()>().await;
            Ok(false)
        }
        MockLlmBehavior::MalformedJson => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            send_event(shared, record, connection, SsePayload::Raw("{not-json")).await?;
            done(shared, record, connection).await?;
            connection.end_chunks().await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::MalformedEvent => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            send_event(
                shared,
                record,
                connection,
                SsePayload::Json(json!({
                    "type":"content_block_start","index":0,"content_block":null
                })),
            )
            .await?;
            done(shared, record, connection).await?;
            connection.end_chunks().await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::WrongContentType
        | MockLlmBehavior::Success
        | MockLlmBehavior::MaxTokens
        | MockLlmBehavior::SlowSuccess => {
            let content_type = if behavior == MockLlmBehavior::WrongContentType {
                "application/json"
            } else {
                "text/event-stream; charset=utf-8"
            };
            connection.open_sse(content_type).await?;
            start_message(shared, record, connection).await?;
            let reason = if behavior == MockLlmBehavior::MaxTokens {
                "max_tokens"
            } else {
                "end_turn"
            };
            let delay = if behavior == MockLlmBehavior::SlowSuccess {
                shared.options.chunk_delay_ms
            } else {
                0
            };
            complete_text(shared, record, connection, reason, delay, 0).await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::ReasoningSuccess => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            start_message(shared, record, connection).await?;
            send_event(shared, record, connection, SsePayload::Json(json!({
                "type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}
            }))).await?;
            let reasoning = shared.options.reasoning_text.clone();
            for chunk in split_text(&reasoning, shared.options.chunk_size) {
                send_event(shared, record, connection, SsePayload::Json(json!({
                    "type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":chunk}
                }))).await?;
            }
            send_event(
                shared,
                record,
                connection,
                SsePayload::Json(json!({"type":"content_block_stop","index":0})),
            )
            .await?;
            complete_text(shared, record, connection, "end_turn", 0, 1).await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::ToolCallSuccess => {
            connection
                .open_sse("text/event-stream; charset=utf-8")
                .await?;
            start_message(shared, record, connection).await?;
            send_event(shared, record, connection, SsePayload::Json(json!({
                "type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"mock-call-1","name":shared.options.tool_name,"input":{}}
            }))).await?;
            let args = &shared.options.tool_arguments;
            let middle_units = (args.encode_utf16().count() / 2).max(1);
            let midpoint = args
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(args.len()))
                .find(|&index| args[..index].encode_utf16().count() >= middle_units)
                .unwrap_or(args.len());
            for partial in [&args[..midpoint], &args[midpoint..]] {
                send_event(shared, record, connection, SsePayload::Json(json!({
                    "type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":partial}
                }))).await?;
            }
            send_event(
                shared,
                record,
                connection,
                SsePayload::Json(json!({"type":"content_block_stop","index":0})),
            )
            .await?;
            send_event(shared, record, connection, SsePayload::Json(json!({
                "type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":2}
            }))).await?;
            done(shared, record, connection).await?;
            connection.end_chunks().await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::RateLimit
        | MockLlmBehavior::ServerError
        | MockLlmBehavior::ServiceUnavailable
        | MockLlmBehavior::AuthError
        | MockLlmBehavior::InvalidRequest
        | MockLlmBehavior::ContextOverflow
        | MockLlmBehavior::QuotaExceeded => {
            let (status, reason, message, code, kind) = match behavior {
                MockLlmBehavior::RateLimit => (
                    429,
                    "Too Many Requests",
                    "mock rate limit",
                    "rate_limit",
                    "mock_error",
                ),
                MockLlmBehavior::ServerError => (
                    500,
                    "Internal Server Error",
                    "mock server error",
                    "server_error",
                    "mock_error",
                ),
                MockLlmBehavior::ServiceUnavailable => (
                    503,
                    "Service Unavailable",
                    "mock service unavailable",
                    "service_unavailable",
                    "mock_error",
                ),
                MockLlmBehavior::AuthError => (
                    401,
                    "Unauthorized",
                    "mock authentication failed",
                    "invalid_api_key",
                    "mock_error",
                ),
                MockLlmBehavior::InvalidRequest => (
                    400,
                    "Bad Request",
                    "mock invalid request",
                    "invalid_request",
                    "mock_error",
                ),
                MockLlmBehavior::ContextOverflow => (
                    400,
                    "Bad Request",
                    "mock input exceeds the model context window",
                    "context_length_exceeded",
                    "invalid_request_error",
                ),
                MockLlmBehavior::QuotaExceeded => (
                    429,
                    "Too Many Requests",
                    "mock insufficient quota",
                    "insufficient_quota",
                    "mock_error",
                ),
                _ => unreachable!(),
            };
            http_behavior_error(
                shared,
                record,
                connection,
                HttpBehaviorError {
                    status,
                    reason,
                    message,
                    code,
                    kind,
                },
            )
            .await?;
            shared
                .finish(record.attempt, MockLlmRequestOutcome::Completed)
                .await;
            Ok(true)
        }
        MockLlmBehavior::Random => unreachable!("random is resolved before dispatch"),
    }
}

async fn serve_connection(stream: TcpStream, shared: Arc<Shared>) {
    let mut connection = Connection {
        stream,
        buffered: Vec::new(),
    };
    loop {
        let request = match connection.request().await {
            Ok(Some(request)) => request,
            Ok(None) | Err(_) => return,
        };
        let path = path_from_target(&request.target).to_owned();
        if request.method != "POST" {
            let _ = connection
                .fixed(
                    405,
                    "Method Not Allowed",
                    &[("allow", "POST".to_owned())],
                    b"",
                )
                .await;
            if request.close {
                return;
            }
            continue;
        }
        if !path.ends_with("/v1/messages") {
            let _ = send_plain_error(&mut connection, 404, "Not Found").await;
            if request.close {
                return;
            }
            continue;
        }
        if shared
            .options
            .api_key
            .as_ref()
            .is_some_and(|expected| request.headers.get("x-api-key") != Some(expected))
        {
            let _ = send_json_error(
                &mut connection,
                401,
                "Unauthorized",
                "invalid mock API key",
                "invalid_api_key",
            )
            .await;
            if request.close {
                return;
            }
            continue;
        }
        let body = if request.body.is_empty() {
            None
        } else {
            let text = String::from_utf8_lossy(&request.body);
            match hoh_util_values::js_json_parse(&text) {
                Ok(body) => Some(body),
                Err(_) => {
                    let _ = send_json_error(
                        &mut connection,
                        400,
                        "Bad Request",
                        "request body must be valid JSON",
                        "invalid_json",
                    )
                    .await;
                    if request.close {
                        return;
                    }
                    continue;
                }
            }
        };
        let mut record = shared
            .accept_request(path.clone(), request.headers, body)
            .await;
        shared.emit(MockLlmServerEvent::Request {
            attempt: record.attempt,
            script_behavior: record.script_behavior,
            behavior: record.behavior,
            path,
        });
        match run_behavior(&shared, &mut record, &mut connection).await {
            Ok(keep_alive) => {
                if !keep_alive || request.close {
                    return;
                }
            }
            Err(_) => {
                shared
                    .finish(record.attempt, MockLlmRequestOutcome::ClientClosed)
                    .await;
                return;
            }
        }
    }
}

/// Running mock server and captured request state.
pub struct MockLlmServer {
    pub base_url: String,
    pub port: u16,
    pub random_seed: u32,
    shared: Arc<Shared>,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl MockLlmServer {
    /// A snapshot of request records in arrival order.
    pub async fn requests(&self) -> Vec<MockLlmRequestRecord> {
        self.shared.state.lock().await.requests.clone()
    }

    /// Stop accepting requests and force-close active streams. Idempotent.
    pub async fn close(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for MockLlmServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Start a local Messages server consuming one behavior per accepted valid request.
pub async fn start_mock_llm_server(
    options: MockLlmServerOptions,
) -> Result<MockLlmServer, MockLlmServerError> {
    let options = resolve_options(options)?;
    let listener = TcpListener::bind((options.host.as_str(), options.port)).await?;
    let address = listener.local_addr()?;
    let advertised_host = if options.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv6()) {
        format!("[{}]", options.host)
    } else {
        options.host.clone()
    };
    let shared = Arc::new(Shared {
        state: Mutex::new(ServerState {
            cursor: 0,
            random: Mulberry32 {
                state: options.random_seed,
            },
            requests: Vec::new(),
        }),
        options,
    });
    let (shutdown, mut shutdown_rx) = watch::channel(false);
    let task_shared = Arc::clone(&shared);
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() { break; }
                }
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let connection_shared = Arc::clone(&task_shared);
                        connections.spawn(async move { serve_connection(stream, connection_shared).await });
                    }
                    Err(_) => break,
                },
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        task_shared.finish_open_as_client_closed().await;
    });
    Ok(MockLlmServer {
        base_url: format!("http://{advertised_host}:{}", address.port()),
        port: address.port(),
        random_seed: shared.options.random_seed,
        shared,
        shutdown,
        task: Some(task),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_MOCK_LLM_RANDOM_WEIGHTS, MockLlmBehavior, MockLlmScriptBehavior,
        MockLlmSelectedBehavior, MockLlmServerOptions, Mulberry32, ServerState, Shared,
        resolve_options,
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn mulberry32_matches_javascript_reference() {
        let mut random = Mulberry32 { state: 1 };
        let values = [random.next(), random.next(), random.next()];
        let expected = [
            0.627_073_940_588_161_3,
            0.002_735_721_180_215_478,
            0.527_447_039_959_952_2,
        ];
        for (actual, expected) in values.into_iter().zip(expected) {
            assert!((actual - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn public_default_weights_preserve_source_order_and_values() {
        let entries: Vec<_> = DEFAULT_MOCK_LLM_RANDOM_WEIGHTS.iter().collect();
        assert_eq!(
            entries
                .first()
                .map(|(name, value)| (name.as_str(), **value)),
            Some(("success", 48.0))
        );
        assert_eq!(
            entries.last().map(|(name, value)| (name.as_str(), **value)),
            Some(("malformed_json", 1.0))
        );
        assert_eq!(entries.len(), 13);
    }

    #[test]
    fn tool_arguments_use_stack_safe_javascript_json_parser() {
        let deeply_nested = format!("{}0{}", "[".repeat(200), "]".repeat(200));
        let resolved = resolve_options(MockLlmServerOptions {
            sequence: vec![MockLlmBehavior::Success],
            tool_arguments: Some(deeply_nested.clone()),
            random_seed: Some(1.0),
            ..Default::default()
        })
        .expect("JSON.parse accepts nesting beyond serde_json's default recursion limit");
        assert_eq!(resolved.tool_arguments, deeply_nested);
    }

    #[tokio::test]
    async fn scripted_sequence_uses_entries_before_exhaustion() {
        let options = resolve_options(MockLlmServerOptions {
            sequence: vec![MockLlmBehavior::Success],
            random_seed: Some(1.0),
            ..Default::default()
        })
        .expect("valid options");
        let random = Mulberry32 {
            state: options.random_seed,
        };
        let shared = Arc::new(Shared {
            options,
            state: Mutex::new(ServerState {
                cursor: 0,
                random,
                requests: Vec::new(),
            }),
        });

        let first = shared
            .accept_request("/v1/messages".into(), Default::default(), None)
            .await;
        let second = shared
            .accept_request("/v1/messages".into(), Default::default(), None)
            .await;

        assert_eq!(
            first.script_behavior,
            MockLlmScriptBehavior::Behavior(MockLlmBehavior::Success)
        );
        assert_eq!(
            first.behavior,
            MockLlmSelectedBehavior::Behavior(MockLlmBehavior::Success)
        );
        assert_eq!(
            second.script_behavior,
            MockLlmScriptBehavior::ScriptExhausted
        );
        assert_eq!(second.behavior, MockLlmSelectedBehavior::ScriptExhausted);
    }
}
