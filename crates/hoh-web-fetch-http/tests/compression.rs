mod support;
use async_compression::tokio::bufread::{
    BrotliEncoder, DeflateEncoder, GzipEncoder, ZlibEncoder, ZstdEncoder,
};
use hoh_web_fetch_http::HttpFetchProvider;
use std::pin::Pin;
use support::*;
use tokio::io::{AsyncRead, AsyncReadExt};

async fn encode(kind: &str, bytes: &[u8]) -> Vec<u8> {
    let mut reader: Pin<Box<dyn AsyncRead + '_>> = match kind {
        "gzip" | "x-gzip" => Box::pin(GzipEncoder::new(bytes)),
        "deflate" => Box::pin(ZlibEncoder::new(bytes)),
        "raw" => Box::pin(DeflateEncoder::new(bytes)),
        "br" => Box::pin(BrotliEncoder::new(bytes)),
        "zstd" => Box::pin(ZstdEncoder::new(bytes)),
        _ => panic!("test encoding"),
    };
    let mut output = Vec::new();
    reader.read_to_end(&mut output).await.unwrap();
    output
}

#[tokio::test(start_paused = true)]
async fn decodes_each_supported_content_encoding_and_raw_deflate() {
    for encoding in ["gzip", "x-gzip", "deflate", "raw", "br", "zstd"] {
        let bytes = encode(encoding, "Привет 🐈".as_bytes()).await;
        let coding = if encoding == "raw" {
            "deflate"
        } else {
            encoding
        };
        let length = bytes.len().to_string();
        let server = Server::new(move |_| {
            Reply::text(&bytes)
                .header("content-encoding", coding)
                .header("content-length", &length)
        })
        .await;
        let result = fetch(&provider(), &server.base).await.unwrap();
        assert_eq!(content(&result), "Привет 🐈", "{encoding}");
        assert!(!result.truncated);
        assert!(
            server.requests.lock().unwrap()[0]
                .headers
                .contains("accept-encoding: gzip, deflate")
        );
    }
}

#[tokio::test(start_paused = true)]
async fn decodes_stacked_encodings_in_reverse_order_and_preserves_unknown_chains() {
    let gzip = encode("gzip", b"stacked").await;
    let stacked = encode("br", &gzip).await;
    let server =
        Server::new(move |_| Reply::text(&stacked).header("content-encoding", "GZIP, br")).await;
    assert_eq!(
        content(&fetch(&provider(), &server.base).await.unwrap()),
        "stacked"
    );
    for encoding in ["unknown", "gzip, unknown", "identity"] {
        let server =
            Server::new(move |_| Reply::text("raw").header("content-encoding", encoding)).await;
        assert_eq!(
            content(&fetch(&provider(), &server.base).await.unwrap()),
            "raw"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn applies_the_byte_cap_to_decoded_bytes_and_detects_exact_eof() {
    for size in [100, 101] {
        let bytes = encode("gzip", &vec![b'a'; size]).await;
        let length = bytes.len().to_string();
        let server = Server::new(move |_| {
            Reply::text(&bytes)
                .header("content-encoding", "gzip")
                .header("content-length", &length)
        })
        .await;
        let mut caps = limits();
        caps.max_response_bytes = 100;
        let result = fetch(&HttpFetchProvider::new(caps).unwrap(), &server.base)
            .await
            .unwrap();
        assert_eq!(content(&result), "a".repeat(100));
        assert_eq!(result.truncated, size == 101);
    }
}

#[tokio::test(start_paused = true)]
async fn accepts_concatenated_gzip_members_and_a_missing_gzip_trailer() {
    let mut members = encode("gzip", b"one").await;
    members.extend(encode("gzip", b"two").await);
    let server =
        Server::new(move |_| Reply::text(&members).header("content-encoding", "gzip")).await;
    assert_eq!(
        content(&fetch(&provider(), &server.base).await.unwrap()),
        "onetwo"
    );
    let mut bytes = encode("gzip", b"without trailer").await;
    bytes.truncate(bytes.len() - 8);
    let server = Server::new(move |_| Reply::text(&bytes).header("content-encoding", "gzip")).await;
    assert_eq!(
        content(&fetch(&provider(), &server.base).await.unwrap()),
        "without trailer"
    );
}

#[tokio::test(start_paused = true)]
async fn rejects_malformed_compressed_bodies_and_cancels_an_unfinished_decoder() {
    let server =
        Server::new(|_| Reply::text("invalid gzip").header("content-encoding", "gzip")).await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_FETCH_NETWORK_ERROR",
        None,
    );
    let server = Server::new(|_| {
        Reply::text([0x1f, 0x8b, 8, 0])
            .header("content-encoding", "gzip")
            .unfinished()
    })
    .await;
    let controller = hoh_timeout::AbortController::new();
    let signal = controller.signal();
    let url = server.base.clone();
    let task = tokio::spawn(async move {
        provider()
            .fetch(hoh_web::WebFetchRequest { url }, Some(&signal))
            .await
    });
    server.arrived.notified().await;
    controller.abort();
    failure(
        task.await.unwrap().unwrap_err(),
        "WEB_ABORTED",
        Some("web fetch aborted"),
    );
    server.wait_closed(1).await;
}

#[tokio::test(start_paused = true)]
async fn combines_repeated_headers_as_fetch_does_before_classification() {
    let server = Server::new(|_| {
        let mut reply = Reply::text("binary").header("content-type", "application/octet-stream");
        reply
            .headers
            .push(("content-type".into(), "text/plain".into()));
        reply
    })
    .await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_UNSUPPORTED_CONTENT_TYPE",
        Some("unsupported content type \"application/octet-stream, text/plain\""),
    );
}

#[tokio::test(start_paused = true)]
async fn sends_byte_string_user_agents_and_rejects_non_byte_string_values_before_connecting() {
    let server = Server::new(|_| Reply::text("ok")).await;
    let mut caps = limits();
    caps.user_agent = " café ".into();
    fetch(&HttpFetchProvider::new(caps).unwrap(), &server.base)
        .await
        .unwrap();
    assert!(
        server.requests.lock().unwrap()[0]
            .headers
            .contains("user-agent: café\r\n")
    );
    let mut caps = limits();
    caps.user_agent = "🐈".into();
    failure(
        fetch(
            &HttpFetchProvider::new(caps).unwrap(),
            "http://127.0.0.1:9999",
        )
        .await
        .unwrap_err(),
        "WEB_FETCH_NETWORK_ERROR",
        Some(
            "web fetch failed: the network request failed: Cannot convert argument to a ByteString because the character at index 0 has a value of 55357 which is greater than 255.",
        ),
    );
}

#[tokio::test(start_paused = true)]
async fn decoded_character_caps_count_utf16_units_and_preserve_whole_scalars() {
    for (cap, expected) in [(3, "a😀"), (2, "a")] {
        let server = Server::new(|_| Reply::text("a😀b")).await;
        let mut caps = limits();
        caps.max_body_chars = cap;
        let result = fetch(&HttpFetchProvider::new(caps).unwrap(), &server.base)
            .await
            .unwrap();
        assert_eq!(content(&result), expected);
        assert!(result.truncated);
    }
}
