//! A deterministic, file-scripted OpenAI Chat Completions mock.
//!
//! Non-streaming completions are probe requests and do not advance the script.
//! Every streaming completion consumes one response. Requests are retained in
//! memory and, when configured, appended to a JSONL journal without headers.

use std::fmt;
use std::io::{Seek, SeekFrom, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// A complete deterministic model script.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub model: String,
    pub responses: Vec<ScenarioResponse>,
}

impl Scenario {
    /// Read and validate a scenario JSON file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, MockError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|error| {
            MockError::new(format!("cannot read scenario {}: {error}", path.display()))
        })?;
        let scenario: Self = serde_json::from_slice(&bytes).map_err(|error| {
            MockError::new(format!("invalid scenario {}: {error}", path.display()))
        })?;
        scenario.validate()?;
        Ok(scenario)
    }

    fn validate(&self) -> Result<(), MockError> {
        if self.model.trim().is_empty() {
            return Err(MockError::new("scenario model must not be empty"));
        }
        if self.responses.is_empty() {
            return Err(MockError::new(
                "scenario must contain at least one response",
            ));
        }
        for (response_index, response) in self.responses.iter().enumerate() {
            for (call_index, call) in response.tool_calls.iter().enumerate() {
                if call.id.trim().is_empty() || call.name.trim().is_empty() {
                    return Err(MockError::new(format!(
                        "response {response_index} tool call {call_index} has an empty id or name"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// One response consumed by one streaming completion.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioResponse {
    /// Separate SSE text deltas, in order.
    #[serde(default)]
    pub text: Vec<String>,
    /// Function calls emitted after the text deltas.
    #[serde(default)]
    pub tool_calls: Vec<ScenarioToolCall>,
    #[serde(default)]
    pub usage: ScenarioUsage,
}

/// One OpenAI function tool call.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioToolCall {
    pub id: String,
    pub name: String,
    /// Serialized as the string expected in `function.arguments`.
    #[serde(default = "empty_object")]
    pub arguments: Value,
}

fn empty_object() -> Value {
    json!({})
}

/// Token counts returned in the terminal usage chunk.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
}

/// One journal entry. Headers, including authorization, are intentionally absent.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecord {
    pub sequence: usize,
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// True when `MockOptions::api_key` is set and the request carried exactly that bearer token.
    /// The token itself is never recorded.
    #[serde(default)]
    pub authorized: bool,
}

/// How the configured pauses are taken. `Recorded` lets a test see them without waiting.
#[derive(Clone, Debug, Default)]
pub enum Sleeper {
    /// Sleep for real.
    #[default]
    Real,
    /// Do not sleep; append `(response index, milliseconds)` for every pause that was due.
    Recorded(Arc<std::sync::Mutex<Vec<(usize, u64)>>>),
}

impl Sleeper {
    async fn pause(&self, index: usize, milliseconds: u64) {
        match self {
            Self::Real => {
                tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
            }
            Self::Recorded(log) => log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((index, milliseconds)),
        }
    }
}

/// Listener and journal configuration.
#[derive(Clone)]
pub struct MockOptions {
    pub host: String,
    pub port: u16,
    pub scenario: Scenario,
    pub journal_path: Option<PathBuf>,
    /// Expected bearer token; only decides `RequestRecord::authorized`, never rejects a request.
    pub api_key: Option<String>,
    /// Response index -> pause (ms) after the opening chunk and before the first delta, so a
    /// test can cancel a turn that is still being answered.
    pub delays_ms: std::collections::HashMap<usize, u64>,
    /// Response index -> pause (ms) before every text delta of that response, so a streamed
    /// answer lasts long enough to be stopped by hand. Kept here and not in `ScenarioResponse`
    /// so that existing struct literals of the scenario types keep compiling.
    pub part_delays_ms: std::collections::HashMap<usize, u64>,
    /// How `delays_ms` and `part_delays_ms` are slept; real by default.
    pub sleeper: Sleeper,
}

impl fmt::Debug for MockOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MockOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("scenario", &self.scenario)
            .field("journal_path", &self.journal_path)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("delays_ms", &self.delays_ms)
            .field("part_delays_ms", &self.part_delays_ms)
            .field("sleeper", &self.sleeper)
            .finish()
    }
}

impl MockOptions {
    #[must_use]
    pub fn new(scenario: Scenario) -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 0,
            scenario,
            journal_path: None,
            api_key: None,
            delays_ms: std::collections::HashMap::new(),
            part_delays_ms: std::collections::HashMap::new(),
            sleeper: Sleeper::Real,
        }
    }
}

/// Configuration, protocol, or I/O error.
#[derive(Debug)]
pub struct MockError(String);

impl MockError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for MockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MockError {}

impl From<std::io::Error> for MockError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

struct State {
    cursor: usize,
    records: Vec<RequestRecord>,
    journal: Option<std::fs::File>,
}

struct Shared {
    scenario: Scenario,
    api_key: Option<String>,
    delays_ms: std::collections::HashMap<usize, u64>,
    part_delays_ms: std::collections::HashMap<usize, u64>,
    sleeper: Sleeper,
    state: Mutex<State>,
}

impl Shared {
    async fn accept(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        authorized: bool,
        consume_response: bool,
    ) -> Result<Option<(usize, ScenarioResponse)>, MockError> {
        let mut state = self.state.lock().await;
        let record = RequestRecord {
            sequence: state.records.len() + 1,
            method: method.to_owned(),
            path: path.to_owned(),
            body,
            authorized,
        };
        if let Some(file) = &mut state.journal {
            let length = file.metadata().map(|metadata| metadata.len()).ok();
            if let Err(error) = append_journal_line(file, &record) {
                // A refused write may have taken part of the line: cut it off, so that the next
                // record never lands on a torn one.
                if let Some(length) = length {
                    let _ = cut_journal(file, length);
                }
                return Err(error);
            }
        }
        state.records.push(record);
        if !consume_response {
            return Ok(None);
        }
        let index = state.cursor;
        let response = self.scenario.responses.get(index).cloned();
        if response.is_some() {
            state.cursor += 1;
        }
        Ok(response.map(|response| (index, response)))
    }
}

/// Cuts the journal back to `length` bytes and moves the write position there: `set_len` alone
/// leaves the position past the end, and the next line would land after a gap of zero bytes.
fn cut_journal(file: &mut std::fs::File, length: u64) -> std::io::Result<()> {
    file.set_len(length)?;
    file.seek(SeekFrom::Start(length)).map(|_| ())
}

/// One journal record as one complete line, written with a single `write_all`.
fn append_journal_line(writer: &mut impl Write, record: &RequestRecord) -> Result<(), MockError> {
    let mut line = serde_json::to_vec(record)
        .map_err(|error| MockError::new(format!("cannot serialize journal: {error}")))?;
    line.push(b'\n');
    writer
        .write_all(&line)
        .and_then(|()| writer.flush())
        .map_err(|error| MockError::new(format!("cannot write journal: {error}")))
}

/// Whether an `accept` failure ends the listener. Resource exhaustion and aborted handshakes are
/// transient: the next `accept` may succeed.
fn accept_error_is_fatal(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    const ENOMEM: i32 = 12;
    const ENFILE: i32 = 23;
    const EMFILE: i32 = 24;
    const ENOBUFS: i32 = 105;
    if matches!(
        error.kind(),
        ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::Interrupted
            | ErrorKind::WouldBlock
            | ErrorKind::TimedOut
            | ErrorKind::OutOfMemory
    ) {
        return false;
    }
    !matches!(
        error.raw_os_error(),
        Some(ENOMEM | ENFILE | EMFILE | ENOBUFS)
    )
}

/// A running local mock server.
pub struct MockServer {
    pub base_url: String,
    pub port: u16,
    shared: Arc<Shared>,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl MockServer {
    /// Start a server. Port zero selects an unused local port.
    pub async fn start(options: MockOptions) -> Result<Self, MockError> {
        options.scenario.validate()?;
        if options.host.trim().is_empty() {
            return Err(MockError::new("mock host must not be empty"));
        }
        let journal = options
            .journal_path
            .as_ref()
            .map(|path| {
                std::fs::File::create(path).map_err(|error| {
                    MockError::new(format!("cannot create journal {}: {error}", path.display()))
                })
            })
            .transpose()?;
        let listener = TcpListener::bind((options.host.as_str(), options.port)).await?;
        let address = listener.local_addr()?;
        let advertised_host = if options
            .host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_ipv6())
        {
            format!("[{}]", options.host)
        } else {
            options.host
        };
        let shared = Arc::new(Shared {
            scenario: options.scenario,
            api_key: options.api_key,
            delays_ms: options.delays_ms,
            part_delays_ms: options.part_delays_ms,
            sleeper: options.sleeper,
            state: Mutex::new(State {
                cursor: 0,
                records: Vec::new(),
                journal,
            }),
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
                            let shared = Arc::clone(&task_shared);
                            connections.spawn(async move {
                                let _ = serve_connection(stream, shared).await;
                            });
                        }
                        Err(error) if accept_error_is_fatal(&error) => break,
                        Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                    },
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        let port = address.port();
        Ok(Self {
            base_url: format!("http://{advertised_host}:{port}/v1"),
            port,
            shared,
            shutdown,
            task: Some(task),
        })
    }

    /// Snapshot the journal retained in memory.
    pub async fn requests(&self) -> Vec<RequestRecord> {
        self.shared.state.lock().await.records.clone()
    }

    /// Stop accepting requests. Idempotent.
    pub async fn close(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct HttpRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Option<Value>,
}

async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, MockError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    let header_end = loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(MockError::new("connection closed before request headers"));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(MockError::new("request exceeds size limit"));
        }
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| MockError::new("request headers are not UTF-8"))?;
    let mut lines = headers.split("\r\n");
    let mut request_line = lines
        .next()
        .ok_or_else(|| MockError::new("missing request line"))?
        .split_whitespace();
    let method = request_line
        .next()
        .ok_or_else(|| MockError::new("missing request method"))?
        .to_owned();
    let path = request_line
        .next()
        .ok_or_else(|| MockError::new("missing request path"))?
        .to_owned();
    let fields = lines
        .filter_map(|line| line.split_once(':'))
        .collect::<Vec<_>>();
    let authorization = fields
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.trim().to_owned());
    let content_length = fields
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>())
        .transpose()
        .map_err(|_| MockError::new("invalid content-length"))?
        .unwrap_or(0);
    let request_end = header_end
        .checked_add(content_length)
        .ok_or_else(|| MockError::new("content-length overflows request size"))?;
    if request_end > MAX_REQUEST_BYTES {
        return Err(MockError::new("request exceeds size limit"));
    }
    while bytes.len() < request_end {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(MockError::new("connection closed before request body"));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = if content_length == 0 {
        None
    } else {
        Some(
            serde_json::from_slice(&bytes[header_end..request_end])
                .map_err(|error| MockError::new(format!("request body is not JSON: {error}")))?,
        )
    };
    Ok(HttpRequest {
        method,
        path,
        authorization,
        body,
    })
}

async fn serve_connection(mut stream: TcpStream, shared: Arc<Shared>) -> Result<(), MockError> {
    let request = match read_request(&mut stream).await {
        Ok(request) => request,
        Err(error) => {
            write_json_error(&mut stream, 400, &error.to_string()).await?;
            return Ok(());
        }
    };
    let consume_response = request.method == "POST"
        && request.path == "/v1/chat/completions"
        && request
            .body
            .as_ref()
            .and_then(|body| body.get("stream"))
            .and_then(Value::as_bool)
            == Some(true);
    let authorized = shared.api_key.as_ref().is_some_and(|key| {
        request.authorization.as_deref() == Some(format!("Bearer {key}").as_str())
    });
    let scripted = match shared
        .accept(
            &request.method,
            &request.path,
            request.body.clone(),
            authorized,
            consume_response,
        )
        .await
    {
        Ok(scripted) => scripted,
        Err(error) => {
            write_json_error(&mut stream, 500, &error.to_string()).await?;
            return Ok(());
        }
    };
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/models") => write_models(&mut stream, &shared.scenario.model).await?,
        ("POST", "/v1/chat/completions") => {
            let Some(body) = request.body else {
                write_json_error(&mut stream, 400, "missing JSON body").await?;
                return Ok(());
            };
            if body.get("stream").and_then(Value::as_bool) == Some(true) {
                match scripted {
                    Some((index, response)) => {
                        write_stream(
                            &mut stream,
                            &shared.scenario.model,
                            index,
                            &response,
                            shared.delays_ms.get(&index).copied(),
                            shared.part_delays_ms.get(&index).copied(),
                            &shared.sleeper,
                        )
                        .await?
                    }
                    None => write_json_error(&mut stream, 409, "scenario exhausted").await?,
                }
            } else {
                write_probe(&mut stream, &shared.scenario.model).await?;
            }
        }
        _ => write_json_error(&mut stream, 404, "route not found").await?,
    }
    Ok(())
}

async fn write_models(stream: &mut TcpStream, model: &str) -> Result<(), MockError> {
    write_json(
        stream,
        200,
        &json!({"object":"list","data":[{"id":model,"object":"model","created":0,"owned_by":"hoh"}]}),
    )
    .await
}

async fn write_probe(stream: &mut TcpStream, model: &str) -> Result<(), MockError> {
    write_json(
        stream,
        200,
        &json!({
            "id":"chatcmpl-probe","object":"chat.completion","created":0,"model":model,
            "choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}
        }),
    )
    .await
}

async fn write_stream(
    stream: &mut TcpStream,
    model: &str,
    index: usize,
    response: &ScenarioResponse,
    delay_ms: Option<u64>,
    part_delay_ms: Option<u64>,
    sleeper: &Sleeper,
) -> Result<(), MockError> {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n",
        )
        .await?;
    let id = format!("chatcmpl-mock-{}", index + 1);
    write_sse(
        stream,
        &json!({"id":id,"object":"chat.completion.chunk","created":0,"model":model,
            "choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}),
    )
    .await?;
    if let Some(delay) = delay_ms {
        sleeper.pause(index, delay).await;
    }
    for text in &response.text {
        if let Some(delay) = part_delay_ms {
            sleeper.pause(index, delay).await;
        }
        write_sse(
            stream,
            &json!({"id":id,"object":"chat.completion.chunk","created":0,"model":model,
                "choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]}),
        )
        .await?;
    }
    for (call_index, call) in response.tool_calls.iter().enumerate() {
        let arguments = arguments_text(&call.arguments)?;
        write_sse(
            stream,
            &json!({"id":id,"object":"chat.completion.chunk","created":0,"model":model,
                "choices":[{"index":0,"delta":{"tool_calls":[{
                    "index":call_index,"id":call.id,"type":"function",
                    "function":{"name":call.name,"arguments":arguments}
                }]},"finish_reason":null}]}),
        )
        .await?;
    }
    let finish_reason = if response.tool_calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    write_sse(
        stream,
        &json!({"id":id,"object":"chat.completion.chunk","created":0,"model":model,
            "choices":[{"index":0,"delta":{},"finish_reason":finish_reason}]}),
    )
    .await?;
    let prompt = response.usage.prompt_tokens;
    let completion = response.usage.completion_tokens;
    write_sse(
        stream,
        &json!({"id":id,"object":"chat.completion.chunk","created":0,"model":model,
            "choices":[],"usage":{"prompt_tokens":prompt,"completion_tokens":completion,
            "total_tokens":prompt.saturating_add(completion)}}),
    )
    .await?;
    stream.write_all(b"data: [DONE]\n\n").await?;
    stream.flush().await?;
    Ok(())
}

/// A string `arguments` is sent verbatim (so a scenario can carry malformed arguments); any other
/// JSON value is serialized.
fn arguments_text(arguments: &Value) -> Result<String, MockError> {
    match arguments {
        Value::String(text) => Ok(text.clone()),
        other => serde_json::to_string(other).map_err(|error| MockError::new(error.to_string())),
    }
}

async fn write_sse(stream: &mut TcpStream, value: &Value) -> Result<(), MockError> {
    stream
        .write_all(format!("data: {value}\n\n").as_bytes())
        .await?;
    stream.flush().await?;
    Ok(())
}

async fn write_json_error(
    stream: &mut TcpStream,
    status: u16,
    message: &str,
) -> Result<(), MockError> {
    write_json(
        stream,
        status,
        &json!({"error":{"message":message,"type":"mock_error"}}),
    )
    .await
}

async fn write_json(stream: &mut TcpStream, status: u16, value: &Value) -> Result<(), MockError> {
    let body = serde_json::to_vec(value).map_err(|error| MockError::new(error.to_string()))?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        MockOptions, RequestRecord, Scenario, ScenarioResponse, ScenarioToolCall, ScenarioUsage,
        accept_error_is_fatal, append_journal_line, arguments_text, cut_journal,
    };
    use serde_json::json;
    use std::io::{Error, ErrorKind, Write};

    fn response() -> ScenarioResponse {
        ScenarioResponse {
            text: vec![],
            tool_calls: vec![],
            usage: ScenarioUsage::default(),
        }
    }

    fn scenario(responses: Vec<ScenarioResponse>) -> Scenario {
        Scenario {
            model: "m".to_owned(),
            responses,
        }
    }

    #[test]
    fn scenario_rejects_empty_model() {
        let scenario = Scenario {
            model: String::new(),
            responses: vec![response()],
        };
        assert_eq!(
            scenario.validate().unwrap_err().to_string(),
            "scenario model must not be empty"
        );
    }

    #[test]
    fn scenario_rejects_no_responses_and_blank_call_ids_with_their_position() {
        assert_eq!(
            scenario(vec![]).validate().unwrap_err().to_string(),
            "scenario must contain at least one response"
        );
        let call = |id: &str, name: &str| ScenarioToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: json!({}),
        };
        let mut blank_id = response();
        blank_id.tool_calls = vec![call("ok", "bash"), call(" ", "bash")];
        let mut blank_name = response();
        blank_name.tool_calls = vec![call("ok", "")];
        assert_eq!(
            scenario(vec![response(), blank_id])
                .validate()
                .unwrap_err()
                .to_string(),
            "response 1 tool call 1 has an empty id or name"
        );
        assert_eq!(
            scenario(vec![blank_name])
                .validate()
                .unwrap_err()
                .to_string(),
            "response 0 tool call 0 has an empty id or name"
        );
    }

    #[test]
    fn scenario_file_errors_name_the_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let missing = directory.path().join("absent.json");
        let message = Scenario::from_path(&missing).unwrap_err().to_string();
        assert!(
            message.starts_with(&format!("cannot read scenario {}", missing.display())),
            "{message}"
        );
        let broken = directory.path().join("broken.json");
        std::fs::write(&broken, r#"{"model":"m","responses":[],"extra":1}"#).expect("write");
        let message = Scenario::from_path(&broken).unwrap_err().to_string();
        assert!(
            message.starts_with(&format!("invalid scenario {}", broken.display()))
                && message.contains("unknown field `extra`"),
            "{message}"
        );
    }

    #[test]
    fn arguments_default_to_object_and_round_trip() {
        let call: ScenarioToolCall =
            serde_json::from_value(json!({"id":"c1","name":"bash"})).expect("deserialize call");
        assert_eq!(call.arguments, json!({}));
        let back = serde_json::to_value(&call).expect("serialize call");
        assert_eq!(back, json!({"id":"c1","name":"bash","arguments":{}}));
        let again: ScenarioToolCall = serde_json::from_value(back).expect("deserialize again");
        assert_eq!(again.arguments, call.arguments);
    }

    #[test]
    fn string_arguments_are_sent_verbatim_and_other_values_serialized() {
        assert_eq!(arguments_text(&json!("{broken")).unwrap(), "{broken");
        assert_eq!(arguments_text(&json!({"a":1})).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn the_api_key_is_redacted_in_debug_output() {
        let mut options = MockOptions::new(scenario(vec![response()]));
        options.api_key = Some("do-not-record".to_owned());
        let text = format!("{options:?}");
        assert!(!text.contains("do-not-record"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
        assert!(
            format!("{:?}", MockOptions::new(scenario(vec![response()]))).contains("api_key: None")
        );
    }

    /// Takes a whole `write` call or refuses it, like a disk with no room left for it.
    struct Disk {
        room: usize,
        content: Vec<u8>,
    }

    impl Write for Disk {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if buffer.len() > self.room {
                return Err(Error::other("no space left on device"));
            }
            self.room -= buffer.len();
            self.content.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn record(sequence: usize) -> RequestRecord {
        RequestRecord {
            sequence,
            method: "GET".to_owned(),
            path: "/v1/models".to_owned(),
            body: None,
            authorized: false,
        }
    }

    #[test]
    fn a_journal_line_is_written_whole_or_not_at_all() {
        let line_length = serde_json::to_vec(&record(1)).expect("serialize").len() + 1;
        let mut disk = Disk {
            room: line_length + line_length / 2,
            content: Vec::new(),
        };
        append_journal_line(&mut disk, &record(1)).expect("the first line fits");
        append_journal_line(&mut disk, &record(2)).expect_err("the second line does not fit");
        let text = String::from_utf8(disk.content).expect("UTF-8");
        assert!(
            text.ends_with('\n'),
            "a torn line was left behind: {text:?}"
        );
        assert_eq!(text.lines().count(), 1, "{text:?}");
        serde_json::from_str::<RequestRecord>(text.trim_end()).expect("the line parses");
    }

    #[test]
    fn a_line_after_a_cut_journal_follows_the_last_whole_line_without_a_gap() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("journal.jsonl");
        let mut file = std::fs::File::create(&path).expect("journal");
        append_journal_line(&mut file, &record(1)).expect("first line");
        let length = file.metadata().expect("metadata").len();
        file.write_all(b"{\"torn").expect("a partial line");
        cut_journal(&mut file, length).expect("cut");
        append_journal_line(&mut file, &record(3)).expect("line after the cut");
        let bytes = std::fs::read(&path).expect("journal content");
        assert!(!bytes.contains(&0), "zero bytes in the journal: {bytes:?}");
        let text = String::from_utf8(bytes).expect("UTF-8");
        let sequences = text
            .lines()
            .map(|line| {
                serde_json::from_str::<RequestRecord>(line)
                    .expect("every line parses")
                    .sequence
            })
            .collect::<Vec<_>>();
        assert_eq!(sequences, [1, 3], "{text:?}");
    }

    #[test]
    fn only_a_dead_listener_ends_the_accept_loop() {
        for transient in [
            Error::from(ErrorKind::ConnectionAborted),
            Error::from(ErrorKind::ConnectionReset),
            Error::from(ErrorKind::Interrupted),
            Error::from_raw_os_error(24),
            Error::from_raw_os_error(23),
            Error::from_raw_os_error(105),
        ] {
            assert!(!accept_error_is_fatal(&transient), "{transient:?}");
        }
        for fatal in [
            Error::from(ErrorKind::PermissionDenied),
            Error::from(ErrorKind::InvalidInput),
            Error::from_raw_os_error(9),
        ] {
            assert!(accept_error_is_fatal(&fatal), "{fatal:?}");
        }
    }
}
