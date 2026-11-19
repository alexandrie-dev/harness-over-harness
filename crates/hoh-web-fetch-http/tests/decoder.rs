//! In-memory decoder checks supplement the unchanged loopback integration fixtures.
use async_compression::tokio::bufread::{
    BrotliEncoder, DeflateEncoder, GzipEncoder, ZlibEncoder, ZstdEncoder,
};
use hoh_error::HarnessError;
use hoh_web::{WebFetchRequest, WebFuture};
use hoh_web_fetch_http::{HttpFetchLimits, HttpFetchProvider, provider::HttpFetchTransport};
use std::{pin::Pin, sync::Arc};
use tokio::io::{AsyncRead, AsyncReadExt};
use url::Url;

struct Memory {
    encoding: String,
    bytes: Vec<u8>,
    declared: Option<u64>,
}
struct Fault(Option<i32>);
impl HttpFetchTransport for Fault {
    fn get<'a>(&'a self, _: &'a Url, _: &'a str) -> WebFuture<'a, reqwest::Response> {
        let error = self.0.map_or_else(
            || std::io::Error::from(std::io::ErrorKind::TimedOut),
            std::io::Error::from_raw_os_error,
        );
        Box::pin(async move {
            let body = reqwest::Body::wrap_stream(futures_util::stream::once(async move {
                Err::<Vec<u8>, _>(error)
            }));
            Ok(http::Response::builder()
                .header("content-type", "text/plain")
                .body(body)
                .unwrap()
                .into())
        })
    }
}
impl HttpFetchTransport for Memory {
    fn get<'a>(&'a self, _: &'a Url, _: &'a str) -> WebFuture<'a, reqwest::Response> {
        Box::pin(async move {
            let mut response = http::Response::builder()
                .header("content-type", "text/plain")
                .header("content-encoding", &self.encoding);
            if let Some(length) = self.declared {
                response = response.header("content-length", length);
            }
            Ok(response
                .body(reqwest::Body::from(self.bytes.clone()))
                .unwrap()
                .into())
        })
    }
}
async fn encoded(kind: &str, bytes: &[u8]) -> Vec<u8> {
    let mut reader: Pin<Box<dyn AsyncRead + '_>> = match kind {
        "gzip" | "x-gzip" => Box::pin(GzipEncoder::new(bytes)),
        "deflate" => Box::pin(ZlibEncoder::new(bytes)),
        "raw" => Box::pin(DeflateEncoder::new(bytes)),
        "br" => Box::pin(BrotliEncoder::new(bytes)),
        "zstd" => Box::pin(ZstdEncoder::new(bytes)),
        _ => panic!("test codec"),
    };
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.unwrap();
    bytes
}
async fn fetch(
    encoding: &str,
    bytes: Vec<u8>,
    cap: u64,
    declared: Option<u64>,
) -> Result<hoh_web::WebFetchResult, hoh_error::BoxError> {
    HttpFetchProvider::with_transport(
        HttpFetchLimits {
            max_response_bytes: cap,
            ..Default::default()
        },
        Arc::new(Memory {
            encoding: encoding.into(),
            bytes,
            declared,
        }),
    )
    .fetch(
        WebFetchRequest {
            url: "http://127.0.0.1/".into(),
        },
        None,
    )
    .await
}
fn content(result: &hoh_web::WebFetchResult) -> &str {
    match &result.body {
        hoh_web::WebFetchBody::Html { content } | hoh_web::WebFetchBody::Text { content } => {
            content
        }
    }
}

#[tokio::test(start_paused = true)]
async fn decodes_all_supported_codecs_and_stacked_codings_without_a_socket() {
    for kind in ["gzip", "x-gzip", "deflate", "raw", "br", "zstd"] {
        let bytes = encoded(kind, "Привет 🐈".as_bytes()).await;
        let coding = if kind == "raw" { "deflate" } else { kind };
        let result = fetch(coding, bytes, 1000, None).await.unwrap();
        assert_eq!(content(&result), "Привет 🐈", "{kind}");
        assert!(!result.truncated);
    }
    let gzip = encoded("gzip", b"stacked").await;
    let result = fetch("gzip, br", encoded("br", &gzip).await, 1000, None)
        .await
        .unwrap();
    assert_eq!(content(&result), "stacked");
    assert_eq!(
        content(
            &fetch("gzip, unknown", b"raw".to_vec(), 1000, None)
                .await
                .unwrap()
        ),
        "raw"
    );
}

#[tokio::test(start_paused = true)]
async fn rejects_the_original_content_length_and_caps_decoded_bytes() {
    let bytes = encoded("gzip", b"hello").await;
    let length = bytes.len() as u64;
    let error = fetch("gzip", bytes, 10, Some(length)).await.unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "WEB_FETCH_TOO_LARGE"
    );
    for size in [100, 101] {
        let bytes = encoded("gzip", &vec![b'a'; size]).await;
        let length = bytes.len() as u64;
        let result = fetch("gzip", bytes, 100, Some(length)).await.unwrap();
        assert_eq!(content(&result), "a".repeat(100));
        assert_eq!(result.truncated, size > 100);
    }
}

#[tokio::test(start_paused = true)]
async fn accepts_multiple_gzip_members_and_a_missing_trailer_without_a_socket() {
    let mut members = encoded("gzip", b"one").await;
    members.extend(encoded("gzip", b"two").await);
    assert_eq!(
        content(&fetch("gzip", members, 1000, None).await.unwrap()),
        "onetwo"
    );
    let mut bytes = encoded("gzip", b"without trailer").await;
    bytes.truncate(bytes.len() - 8);
    assert_eq!(
        content(&fetch("gzip", bytes, 1000, None).await.unwrap()),
        "without trailer"
    );
}

#[tokio::test(start_paused = true)]
async fn malformed_compression_keeps_the_network_error_class() {
    let error = fetch("gzip", b"invalid gzip".to_vec(), 1000, None)
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "WEB_FETCH_NETWORK_ERROR"
    );
}

#[tokio::test(start_paused = true)]
async fn a_client_body_timeout_uses_the_undici_body_timeout_code_and_detail() {
    let p = HttpFetchProvider::with_transport(HttpFetchLimits::default(), Arc::new(Fault(None)));
    let error = p
        .fetch(
            WebFetchRequest {
                url: "http://127.0.0.1/".into(),
            },
            None,
        )
        .await
        .unwrap_err();
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "WEB_FETCH_NETWORK_ERROR");
    assert_eq!(
        error.message(),
        "web fetch failed: the network request failed (UND_ERR_BODY_TIMEOUT): Body Timeout Error"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn operating_system_read_faults_keep_their_codes_without_a_connect_address() {
    for (errno, code, message) in [
        (
            104,
            "WEB_FETCH_CONNECTION_INTERRUPTED",
            "web fetch failed: the connection was interrupted (ECONNRESET): read ECONNRESET",
        ),
        (
            110,
            "WEB_FETCH_CONNECTION_TIMEOUT",
            "web fetch failed: the connection timed out (ETIMEDOUT): read ETIMEDOUT",
        ),
    ] {
        let p = HttpFetchProvider::with_transport(
            HttpFetchLimits::default(),
            Arc::new(Fault(Some(errno))),
        );
        let error = p
            .fetch(
                WebFetchRequest {
                    url: "http://127.0.0.1/".into(),
                },
                None,
            )
            .await
            .unwrap_err();
        let error = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), code);
        assert_eq!(error.message(), message);
    }
}
