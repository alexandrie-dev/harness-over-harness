#![allow(dead_code)]
use hoh_error::{BoxError, HarnessError};
use hoh_web::{WebFetchBody, WebFetchRequest, WebFetchResult};
use hoh_web_fetch_http::{HttpFetchLimits, HttpFetchProvider};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Debug)]
pub struct Request {
    pub target: String,
    pub headers: String,
}
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub open: bool,
    pub silent: bool,
}
impl Reply {
    pub fn text(text: impl AsRef<[u8]>) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".into(), "text/plain".into())],
            body: text.as_ref().to_vec(),
            open: false,
            silent: false,
        }
    }
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name.into(), value.into()));
        self
    }
    pub fn without_type(mut self) -> Self {
        self.headers.clear();
        self
    }
    pub fn redirect(status: u16, target: Option<&str>) -> Self {
        let mut reply = Self::text("").without_type();
        reply.status = status;
        if let Some(target) = target {
            reply = reply.header("location", target);
        }
        reply
    }
    pub fn unfinished(mut self) -> Self {
        self.open = true;
        self
    }
    pub fn closed() -> Self {
        Self {
            silent: true,
            ..Self::text("")
        }
    }
}
pub struct Server {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    pub arrived: Arc<Notify>,
    closed: Arc<AtomicUsize>,
    closed_notify: Arc<Notify>,
    task: JoinHandle<()>,
    clock: JoinHandle<()>,
}
impl Server {
    pub async fn new(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let closed = Arc::new(AtomicUsize::new(0));
        let count = closed.clone();
        let closed_notify = Arc::new(Notify::new());
        let closed_signal = closed_notify.clone();
        let arrived = Arc::new(Notify::new());
        let arrival = arrived.clone();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    connection=listener.accept()=> {
                        let Ok((mut stream,_))=connection else {break;};
                        let handler=handler.clone();let seen=seen.clone();let count=count.clone();let closed_signal=closed_signal.clone();let arrival=arrival.clone();
                        connections.spawn(async move {
                            let mut bytes=Vec::new();let mut byte=[0;1];
                            loop {match stream.read(&mut byte).await {Ok(1)=>bytes.push(byte[0]),_=>return,} if bytes.ends_with(b"\r\n\r\n") {break;} assert!(bytes.len()<64_000);}
                            let headers: String=bytes.into_iter().map(char::from).collect();
                            let target=headers.split_whitespace().nth(1).unwrap().to_string();
                            let request=Request {target,headers};seen.lock().unwrap().push(request.clone());
                            let reply=handler(&request);arrival.notify_one();
                            if reply.silent && reply.open {
                                let mut extra=[0;64];
                                loop {match stream.read(&mut extra).await {Ok(0)|Err(_)=>break,Ok(_)=>{}}}
                            } else if !reply.silent {
                                let mut headers=format!("HTTP/1.1 {} Fixture\r\nConnection: close\r\n",reply.status);
                                for (name,value) in reply.headers {headers.push_str(&format!("{name}: {value}\r\n"));}
                                headers.push_str("\r\n");
                                if stream.write_all(headers.as_bytes()).await.is_ok() {let _=stream.write_all(&reply.body).await;let _=stream.flush().await;}
                                if reply.open {
                                    let mut extra=[0;64];
                                    loop {match stream.read(&mut extra).await {Ok(0)|Err(_)=>break,Ok(_)=>{}}}
                                }
                            }
                            let _=stream.shutdown().await;count.fetch_add(1,Ordering::SeqCst);closed_signal.notify_waiters();
                        });
                    }
                    Some(result)=connections.join_next(),if !connections.is_empty()=>{result.unwrap();}
                }
            }
        });
        // Keep paused Tokio time from auto-advancing while a real loopback socket is pending.
        // Timeout tests advance explicitly after observing their request/response boundary.
        let clock = tokio::spawn(async {
            loop {
                tokio::task::yield_now().await;
            }
        });
        Self {
            base,
            requests,
            arrived,
            closed,
            closed_notify,
            task,
            clock,
        }
    }
    pub async fn wait_closed(&self, expected: usize) {
        loop {
            let notified = self.closed_notify.notified();
            if self.closed.load(Ordering::SeqCst) >= expected {
                return;
            }
            notified.await;
        }
    }
    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        self.clock.abort();
    }
}
pub fn provider() -> HttpFetchProvider {
    HttpFetchProvider::new(limits()).unwrap()
}
pub fn limits() -> HttpFetchLimits {
    HttpFetchLimits {
        timeout_ms: 5_000.0,
        user_agent: "test-agent/1.0".into(),
        ..Default::default()
    }
}
pub async fn fetch(provider: &HttpFetchProvider, url: &str) -> Result<WebFetchResult, BoxError> {
    provider
        .fetch(WebFetchRequest { url: url.into() }, None)
        .await
}
pub fn content(result: &WebFetchResult) -> &str {
    match &result.body {
        WebFetchBody::Html { content } | WebFetchBody::Text { content } => content,
    }
}
pub fn failure(error: BoxError, code: &str, message: Option<&str>) {
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.name(), "WebError");
    assert_eq!(error.code(), code);
    if let Some(message) = message {
        assert_eq!(error.message(), message);
    }
}
