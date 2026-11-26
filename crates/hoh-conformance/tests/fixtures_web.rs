//! The loopback web fixtures (`web-fetch-fixture-server`, `web-fetch-transport-failure-fixture`,
//! `deepseek-fetch-fixture`) run behind the real HTTP provider, as the TypeScript files do.
use hoh_conformance::fixtures::web::{
    DeepseekFetchFixture, WebFetchFixtureServer, WebFetchTransportFailure,
};
use hoh_kernel::{Kernel, Plugin};
use hoh_web::{Web, WebFetchBody, WebFetchRequest, WebRuntime};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const URL: &str = "http://public.test:43117/menu.html";

async fn kernel_with<P: Plugin>() -> Kernel {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<WebRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<P>(serde_json::Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    kernel
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_recorded_url_is_served_by_the_loopback_page_and_keeps_its_name() {
    let kernel = kernel_with::<WebFetchFixtureServer>().await;
    let web = kernel.root().get::<Web>().unwrap();
    let result = web
        .fetch(WebFetchRequest { url: URL.into() }, None)
        .await
        .unwrap();
    assert_eq!(result.url, URL);
    assert_eq!(result.status_code, 200);
    let WebFetchBody::Html { content } = &result.body else {
        panic!("not html: {:?}", result.body);
    };
    assert!(content.contains("<h1>Café menu</h1>"), "{content}");
    let error = web
        .fetch(
            WebFetchRequest {
                url: "http://public.test:43117/other.html".into(),
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unexpected snapshot URL"),
        "{error}"
    );
    kernel.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_transport_failure_fixture_closes_every_connection() {
    let kernel = kernel_with::<WebFetchTransportFailure>().await;
    let web = kernel.root().get::<Web>().unwrap();
    let error = web
        .fetch(WebFetchRequest { url: URL.into() }, None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains("fetch"),
        "a transport failure is reported as a fetch error: {error}"
    );
    kernel.shutdown().await;
}

async fn post(auth: Option<&str>) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", 43118))
        .await
        .unwrap();
    let auth = auth
        .map(|a| format!("authorization: {a}\r\n"))
        .unwrap_or_default();
    stream
        .write_all(
            format!(
                "POST /api/v1/tools/fetch HTTP/1.1\r\nhost: x\r\n{auth}content-length: 2\r\n\r\n{{}}"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).await.unwrap();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_deepseek_fetch_fixture_answers_only_a_bearer_snapshot_key() {
    let kernel = Kernel::start();
    kernel
        .root()
        .plugin::<DeepseekFetchFixture>(serde_json::Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(post(None).await.starts_with("HTTP/1.1 401"));
    let ok = post(Some("Bearer snapshot-key")).await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(
        ok.contains("\"title\":\"Menu\"") && ok.contains("Tea — $2"),
        "{ok}"
    );
    kernel.shutdown().await;
}

/// `closeAllConnections()` of the TypeScript helper: a connection that never sent its request is
/// closed when the plugin is unloaded (the listener is the one every loopback fixture uses).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unloading_the_fixture_closes_a_connection_that_sent_no_request() {
    use hoh_conformance::fixtures::web::{Loopback, Reply};
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let server = Loopback::start(&ctx, "idle-test", 0, std::sync::Arc::new(|_| Reply::Close))
        .await
        .unwrap();
    let mut idle = tokio::net::TcpStream::connect(("127.0.0.1", server.port))
        .await
        .unwrap();
    kernel.shutdown().await;
    let mut buffer = [0u8; 8];
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), idle.read(&mut buffer))
        .await
        .expect("the connection must be closed on unload");
    match read {
        Ok(bytes) => assert_eq!(bytes, 0),
        // Shutdown can close the listening socket before it accepts the connection.
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset),
    }
}
