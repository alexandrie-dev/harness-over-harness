//! Minimal HTTP/1.1 server on 127.0.0.1 that replays canned replies and records requests.
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[derive(Clone)]
pub struct Reply {
    pub content_type: &'static str,
    pub body: String,
    /// Never answer: hold the connection until the client closes it.
    pub stall: bool,
    /// Answer 400 "unsupported parameter" to a request body containing this text.
    pub reject_body_containing: Option<&'static str>,
}

impl Reply {
    fn new(content_type: &'static str, body: String) -> Self {
        Self {
            content_type,
            body,
            stall: false,
            reject_body_containing: None,
        }
    }
    pub fn json(body: &str) -> Self {
        Self::new("application/json", body.into())
    }
    pub fn sse(parts: &[String]) -> Self {
        Self::new("text/event-stream", parts.concat())
    }
    pub fn stalled() -> Self {
        Self {
            stall: true,
            ..Self::json("")
        }
    }
    pub fn rejecting(mut self, text: &'static str) -> Self {
        self.reject_body_containing = Some(text);
        self
    }
}

#[derive(Clone, Debug)]
pub struct Captured {
    pub request_line: String,
    pub authorization: Option<String>,
    pub body: String,
}

pub struct Server {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<Captured>>>,
    /// Connections the client closed while the server was holding them (`Reply::stalled`).
    pub closed_stalled: Arc<AtomicUsize>,
}

impl Server {
    pub fn captured(&self) -> Vec<Captured> {
        self.requests.lock().unwrap().clone()
    }
}

/// `routes`: `"POST /v1/chat/completions"` → reply (repeated for every matching request).
pub async fn serve(routes: Vec<(&str, Reply)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let table: Arc<HashMap<String, Reply>> = Arc::new(
        routes
            .into_iter()
            .map(|(key, reply)| (key.to_owned(), reply))
            .collect(),
    );
    let log = requests.clone();
    let closed_stalled = Arc::new(AtomicUsize::new(0));
    let closed = closed_stalled.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let (table, log, closed) = (table.clone(), log.clone(), closed.clone());
            tokio::spawn(async move {
                let _ = handle(socket, &table, &log, &closed).await;
            });
        }
    });
    Server {
        base_url,
        requests,
        closed_stalled,
    }
}

async fn handle(
    mut socket: TcpStream,
    table: &HashMap<String, Reply>,
    log: &Mutex<Vec<Captured>>,
    closed: &AtomicUsize,
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
    let mut parts = lines.next().unwrap_or("").split(' ');
    let request_line = format!(
        "{} {}",
        parts.next().unwrap_or(""),
        parts.next().unwrap_or("")
    );
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
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
        request_line: request_line.clone(),
        authorization: headers.get("authorization").cloned(),
        body: body.clone(),
    });
    let Some(reply) = table.get(&request_line) else {
        socket
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    };
    if reply.stall {
        while socket.read(&mut chunk).await? > 0 {}
        closed.fetch_add(1, Ordering::SeqCst);
        return Ok(());
    }
    let (status, content_type, payload) = match reply.reject_body_containing {
        Some(text) if body.contains(text) => (
            "400 Bad Request",
            "application/json",
            r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.","type":"invalid_request_error","param":"max_tokens","code":"unsupported_parameter"}}"#,
        ),
        _ => ("200 OK", reply.content_type, reply.body.as_str()),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(payload.as_bytes()).await?;
    socket.shutdown().await
}
