//! Loopback HTTP fixtures of the web scenarios (`snapshots/session/loopback-fixture-server.mjs`
//! and the four `*fetch*-fixture*.mjs` files that use it).
use super::SNAPSHOTS;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_loader::PluginRegistry;
use hoh_timeout::AbortSignal;
use hoh_web::{Web, WebFetchProvider, WebFetchRequest, WebFetchResult, WebFuture, WebProviderInfo};
use hoh_web_fetch_http::{HttpFetchLimits, HttpFetchProvider};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What the loopback server answers to one request.
pub enum Reply {
    /// Status, headers and body; the connection closes after the response.
    Respond(u16, Vec<(&'static str, String)>, Vec<u8>),
    /// Close the connection as soon as the request has arrived (`req.socket.end()`).
    Close,
}

/// One parsed request head.
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
}

type Handler = Arc<dyn Fn(&Request) -> Reply + Send + Sync>;

/// A loopback listener (`applyLoopbackServerEffect`): ephemeral port unless `port` is given, stopped
/// by the effect that owns it.
pub struct Loopback {
    pub port: u16,
}

impl Loopback {
    pub async fn start(
        ctx: &Context,
        label: &str,
        port: u16,
        handler: Handler,
    ) -> Result<Self, BoxError> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| format!("{label}: {e}"))?;
        let port = listener.local_addr()?.port();
        // The connections belong to the accept loop: aborting it on unload drops the set, which
        // aborts every connection task and closes its socket (`server.closeAllConnections()`).
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                while connections.try_join_next().is_some() {}
                let handler = handler.clone();
                connections.spawn(async move {
                    let _ = serve(stream, handler).await;
                });
            }
        });
        let abort = task.abort_handle();
        ctx.effect(label.to_owned(), move || Undo::sync(move || abort.abort()))?;
        Ok(Self { port })
    }
}

async fn serve(mut stream: TcpStream, handler: Handler) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let request = Request {
        method: first.next().unwrap_or("").to_owned(),
        target: first.next().unwrap_or("").to_owned(),
        headers: lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect(),
    };
    // Drain the request body so a reply does not reset a connection that still has unread bytes.
    let length: usize = request
        .headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut have = buffer.len() - head_end - 4;
    while have < length {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        have += n;
    }
    match handler(&request) {
        Reply::Close => stream.shutdown().await,
        Reply::Respond(status, headers, body) => {
            let mut out = format!(
                "HTTP/1.1 {status} {}\r\n",
                if status == 200 { "OK" } else { "Error" }
            );
            for (name, value) in headers {
                out.push_str(&format!("{name}: {value}\r\n"));
            }
            out.push_str(&format!(
                "content-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ));
            stream.write_all(out.as_bytes()).await?;
            stream.write_all(&body).await?;
            stream.shutdown().await
        }
    }
}

/// Model-visible URL retained by the recorded session.
const RECORDED_URL: &str = "http://public.test:43117/menu.html";

const PAGE: &str = "<!doctype html>
<html><head><title>Menu</title><style>.x{color:red}</style><script>ignored()</script></head>
<body>
<h1>Café menu</h1>
<p>Prices include <strong>service &amp; <em>tax</em></strong> &mdash; updated daily.</p>
<ul><li>Espresso</li><li>Flat white</li></ul>
<table><thead><tr><th>Drink</th><th>Price</th></tr></thead><tbody><tr><td>Espresso</td><td>€2</td></tr><tr><td>Flat white</td><td>€3</td></tr></tbody></table>
<p>See <a href=\"https://fixture.invalid/specials\">today&rsquo;s specials</a>.</p>
</body></html>
";

fn limits() -> HttpFetchLimits {
    HttpFetchLimits {
        max_response_bytes: 5_000_000,
        max_body_chars: 100_000,
        timeout_ms: 30_000.0,
        max_redirects: 5,
        user_agent: "deepseek-harness-snapshot/1.0".into(),
    }
}

/// The real HTTP provider behind the recorded URL: every fetch of `RECORDED_URL` goes to the
/// loopback listener and the result keeps the recorded URL.
struct RecordedProvider {
    inner: HttpFetchProvider,
    transport: String,
}
impl WebFetchProvider for RecordedProvider {
    fn info(&self) -> WebProviderInfo {
        self.inner.info()
    }
    fn available(&self) -> Result<bool, BoxError> {
        self.inner.available()
    }
    fn fetch<'a>(
        &'a self,
        request: WebFetchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebFetchResult> {
        Box::pin(async move {
            if request.url != RECORDED_URL {
                return Err(format!("unexpected snapshot URL: {}", request.url).into());
            }
            let mut result = self
                .inner
                .fetch(
                    WebFetchRequest {
                        url: self.transport.clone(),
                    },
                    signal,
                )
                .await?;
            result.url = RECORDED_URL.into();
            Ok(result)
        })
    }
}

#[derive(Inject)]
pub struct WebDeps {
    web: Service<Web>,
}

async fn register_recorded(
    ctx: &Context,
    deps: &WebDeps,
    label: &str,
    handler: Handler,
) -> Result<(), BoxError> {
    let server = Loopback::start(ctx, label, 0, handler).await?;
    let provider = RecordedProvider {
        inner: HttpFetchProvider::new(limits())?,
        transport: RECORDED_URL.replace("public.test:43117", &format!("127.0.0.1:{}", server.port)),
    };
    deps.web.register_fetch_provider(ctx, Arc::new(provider))?;
    Ok(())
}

/// `web-fetch-fixture-server.mjs`: a UTF-8 page with a malformed charset declaration.
pub struct WebFetchFixtureServer;
impl Plugin for WebFetchFixtureServer {
    const NAME: &'static str = "web-fetch-fixture-server";
    type Config = ();
    type Deps = WebDeps;
    async fn apply(ctx: Context, deps: WebDeps, _: Cfg<()>) -> Result<(), BoxError> {
        register_recorded(
            &ctx,
            &deps,
            "web-fetch-fixture-server",
            Arc::new(|request| {
                if request.target == "/menu.html" {
                    Reply::Respond(
                        200,
                        vec![("content-type", "text/html; charset=gbk,utf-8".into())],
                        PAGE.as_bytes().to_vec(),
                    )
                } else {
                    not_found()
                }
            }),
        )
        .await
    }
}

fn not_found() -> Reply {
    Reply::Respond(
        404,
        vec![("content-type", "text/plain; charset=utf-8".into())],
        b"not found".to_vec(),
    )
}

/// `web-fetch-transport-failure-fixture.mjs`: the server closes each connection on arrival.
pub struct WebFetchTransportFailure;
impl Plugin for WebFetchTransportFailure {
    const NAME: &'static str = "web-fetch-transport-failure-fixture";
    type Config = ();
    type Deps = WebDeps;
    async fn apply(ctx: Context, deps: WebDeps, _: Cfg<()>) -> Result<(), BoxError> {
        register_recorded(
            &ctx,
            &deps,
            "web-fetch-transport-failure-fixture",
            Arc::new(|_| Reply::Close),
        )
        .await
    }
}

/// The recorded Fetch API endpoint; the fixture serves it on this very port.
pub const DEEPSEEK_FETCH_PORT: u16 = 43118;

/// `deepseek-fetch-fixture.mjs`: the dedicated Fetch API response. The TypeScript fixture listens
/// on an ephemeral port and rewrites the recorded endpoint inside `globalThis.fetch`; no such
/// transport seam exists here, so the listener takes the recorded port itself.
pub struct DeepseekFetchFixture;
impl Plugin for DeepseekFetchFixture {
    const NAME: &'static str = "deepseek-fetch-fixture";
    type Config = ();
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<()>) -> Result<(), BoxError> {
        Loopback::start(
            &ctx,
            "deepseek-fetch-fixture",
            DEEPSEEK_FETCH_PORT,
            Arc::new(|request| {
                if request.method == "POST" && request.target == "/api/v1/tools/fetch" {
                    if request.headers.get("authorization").map(String::as_str)
                        != Some("Bearer snapshot-key")
                    {
                        return Reply::Respond(401, vec![], vec![]);
                    }
                    return Reply::Respond(
                        200,
                        vec![("content-type", "application/json".into())],
                        serde_json::json!({"title": "Menu", "content": "# Menu\n\nTea — $2"})
                            .to_string()
                            .into_bytes(),
                    );
                }
                not_found()
            }),
        )
        .await?;
        Ok(())
    }
}

pub fn add(registry: &mut PluginRegistry) {
    registry
        .add::<WebFetchFixtureServer>(&format!(
            "{SNAPSHOTS}session/web-fetch/web-fetch-fixture-server.mjs"
        ))
        .add::<WebFetchTransportFailure>(&format!(
            "{SNAPSHOTS}session/web-fetch-transport-failure/web-fetch-transport-failure-fixture.mjs"
        ))
        .add::<DeepseekFetchFixture>(&format!(
            "{SNAPSHOTS}session/deepseek-fetch/deepseek-fetch-fixture.mjs"
        ));
}
