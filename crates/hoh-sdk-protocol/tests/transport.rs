use hoh_error::{BoxError, SharedError};
use hoh_sdk_protocol::{JsonRpcLineTransport, JsonRpcResponseError, JsonRpcTransportPeer};
use hoh_timeout::AbortController;
use hoh_util_values::{js_json_parse, js_json_stringify, js_trim};
use serde_json::{Value, json};
use std::{
    future, io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{
        AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufWriter, DuplexStream, ReadBuf,
        duplex,
    },
    sync::mpsc,
};

fn boxed(message: &'static str) -> BoxError {
    Box::new(io::Error::other(message))
}

async fn pair() -> (JsonRpcLineTransport, JsonRpcLineTransport) {
    let (a_output, b_input) = duplex(16 * 1024);
    let (b_output, a_input) = duplex(16 * 1024);
    let a = JsonRpcLineTransport::new(a_input, a_output).unwrap();
    let b = JsonRpcLineTransport::new(b_input, b_output).unwrap();
    a.start().unwrap();
    b.start().unwrap();
    (a, b)
}

fn raw_transport(capacity: usize) -> (JsonRpcLineTransport, DuplexStream, DuplexStream) {
    let (input_writer, input) = duplex(capacity);
    let (output, output_reader) = duplex(capacity);
    (
        JsonRpcLineTransport::new(input, output).unwrap(),
        input_writer,
        output_reader,
    )
}

async fn read_frame(reader: &mut DuplexStream) -> Value {
    let mut bytes = Vec::new();
    loop {
        let byte = reader.read_u8().await.unwrap();
        bytes.push(byte);
        if byte == b'\n' {
            break;
        }
    }
    js_json_parse(js_trim(&String::from_utf8(bytes).unwrap())).unwrap()
}

async fn write_frame(writer: &mut DuplexStream, value: Value) {
    let mut bytes = js_json_stringify(&value).into_bytes();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}

#[tokio::test]
async fn bidirectional_requests_and_notifications() {
    let (a, b) = pair().await;
    a.on_request(|method, params| async move {
        Ok(json!({ "side": "a", "method": method, "params": params }))
    });
    b.on_request(|method, params| async move {
        Ok(json!({ "side": "b", "method": method, "params": params }))
    });
    let (notice_tx, mut notice_rx) = mpsc::unbounded_channel();
    b.on_notification(move |method, params| {
        notice_tx.send((method, params)).unwrap();
    });
    let from_b = a.request("right", json!({ "n": 1 }), None).await.unwrap();
    let from_a = b.request("left", json!({ "n": 2 }), None).await.unwrap();
    a.notify("tick", Some(json!({ "ok": true }))).unwrap();
    assert_eq!(
        from_b,
        json!({ "side": "b", "method": "right", "params": { "n": 1 } })
    );
    assert_eq!(
        from_a,
        json!({ "side": "a", "method": "left", "params": { "n": 2 } })
    );
    assert_eq!(
        notice_rx.recv().await,
        Some(("tick".into(), json!({ "ok": true })))
    );
}

#[tokio::test]
async fn remote_handler_error_preserves_wire_code() {
    let (a, b) = pair().await;
    b.on_request(|_, _| async { Err(boxed("handler boom")) });
    let cause = a.request("fail", json!({}), None).await.unwrap_err();
    let response = cause.downcast_ref::<JsonRpcResponseError>().unwrap();
    assert_eq!(response.code, Some(-32603.0));
    assert_eq!(response.message, "handler boom");
    assert_eq!(response.data, None);
}

#[tokio::test]
async fn panicking_handler_becomes_internal_error_response() {
    let (a, b) = pair().await;
    b.on_request(|_, _| async move {
        panic!("boom");
        #[allow(unreachable_code)]
        Ok(Value::Null)
    });

    let cause = tokio::time::timeout(
        Duration::from_millis(100),
        a.request("panic", json!({}), None),
    )
    .await
    .expect("panicking handler must still answer")
    .unwrap_err();
    let response = cause.downcast_ref::<JsonRpcResponseError>().unwrap();
    assert_eq!(response.code, Some(-32603.0));
    assert_eq!(response.message, "boom");
}

#[tokio::test]
async fn synchronously_panicking_handler_becomes_internal_error_response() {
    let (a, b) = pair().await;
    b.on_request(|_, _| {
        panic!("sync boom");
        #[allow(unreachable_code)]
        future::ready(Ok(Value::Null))
    });

    let cause = tokio::time::timeout(
        Duration::from_millis(100),
        a.request("sync-panic", json!({}), None),
    )
    .await
    .expect("synchronously panicking handler must still answer")
    .unwrap_err();
    let response = cause.downcast_ref::<JsonRpcResponseError>().unwrap();
    assert_eq!(response.code, Some(-32603.0));
    assert_eq!(response.message, "sync boom");
}

#[tokio::test]
async fn pre_aborted_signal_creates_no_pending_request() {
    let (transport, _input, mut output) = raw_transport(4096);
    let controller = AbortController::new();
    controller.abort_with(io::Error::other("already stopped"));
    let reason = controller.signal().reason().unwrap();
    let cause = transport
        .request("never", json!({}), Some(controller.signal()))
        .await
        .unwrap_err();
    assert!(cause.downcast_ref::<SharedError>().unwrap().ptr_eq(&reason));
    assert_eq!(transport.pending_len(), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), output.read_u8())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn abort_removes_pending_and_preserves_reason_identity() {
    let (transport, _input, _output) = raw_transport(4096);
    let controller = AbortController::new();
    let request = transport.request("wait", json!({}), Some(controller.signal()));
    assert_eq!(transport.pending_len(), 1);
    controller.abort_with(io::Error::other("plain-string-reason"));
    let reason = controller.signal().reason().unwrap();
    tokio::time::timeout(Duration::from_millis(100), async {
        while transport.pending_len() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("abort must remove an unpolled request eagerly");
    let cause = request.await.unwrap_err();
    assert!(cause.downcast_ref::<SharedError>().unwrap().ptr_eq(&reason));
    assert_eq!(transport.pending_len(), 0);
}

#[tokio::test]
async fn response_settled_before_abort_remains_successful_when_polled_later() {
    let (transport, mut input, mut output) = raw_transport(4096);
    transport.start().unwrap();
    let controller = AbortController::new();
    let request = transport.request("race", json!({}), Some(controller.signal()));
    let outbound = read_frame(&mut output).await;
    write_frame(
        &mut input,
        json!({ "jsonrpc": "2.0", "id": outbound["id"], "result": 42 }),
    )
    .await;
    tokio::time::timeout(Duration::from_millis(100), async {
        while transport.pending_len() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("response must settle the request before abort");

    controller.abort_with(io::Error::other("too late"));
    assert_eq!(request.await.unwrap(), json!(42));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn request_handlers_start_in_line_order_before_following_notification() {
    let (transport, mut input, _output) = raw_transport(4096);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let request_tx = tx.clone();
    transport.on_request(move |method, _| {
        request_tx.send(method).unwrap();
        async move { future::pending::<Result<Value, BoxError>>().await }
    });
    transport.on_notification(move |method, _| {
        tx.send(method).unwrap();
    });
    transport.start().unwrap();

    let mut frames = Vec::new();
    for frame in [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "session/prompt" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "session/wait" }),
        json!({ "jsonrpc": "2.0", "method": "session.status" }),
    ] {
        frames.extend_from_slice(js_json_stringify(&frame).as_bytes());
        frames.push(b'\n');
    }
    input.write_all(&frames).await.unwrap();

    let observed = tokio::time::timeout(Duration::from_millis(100), async {
        vec![
            rx.recv().await.unwrap(),
            rx.recv().await.unwrap(),
            rx.recv().await.unwrap(),
        ]
    })
    .await
    .unwrap();
    assert_eq!(
        observed,
        ["session/prompt", "session/wait", "session.status"]
    );
}

#[tokio::test]
async fn structured_error_data_is_preserved() {
    let (transport, mut input, mut output) = raw_transport(4096);
    transport.start().unwrap();
    let request = transport.request("structured", json!({}), None);
    let outbound = read_frame(&mut output).await;
    let id = outbound["id"].as_str().unwrap();
    assert_eq!(id.len(), 36);
    assert!(id.starts_with("req_"));
    assert!(id[4..].bytes().all(|byte| byte.is_ascii_hexdigit()));
    write_frame(
        &mut input,
        json!({
            "jsonrpc": "2.0",
            "id": outbound["id"],
            "error": { "code": 91.5, "message": "structured", "data": { "retry": false } }
        }),
    )
    .await;
    let cause = request.await.unwrap_err();
    let response = cause.downcast_ref::<JsonRpcResponseError>().unwrap();
    assert_eq!(response.code, Some(91.5));
    assert_eq!(response.data, Some(json!({ "retry": false })));
}

#[tokio::test]
async fn handler_failure_display_is_sent_like_foreign_throw_stringification() {
    let (a, b) = pair().await;
    b.on_request(|_, _| async { Err(boxed("plain failure")) });
    let cause = a.request("fail", json!({}), None).await.unwrap_err();
    assert_eq!(cause.to_string(), "plain failure");
}

#[tokio::test]
async fn missing_handler_returns_method_not_found() {
    let (a, _b) = pair().await;
    let cause = a.request("absent", json!({}), None).await.unwrap_err();
    let response = cause.downcast_ref::<JsonRpcResponseError>().unwrap();
    assert_eq!(response.code, Some(-32601.0));
    assert_eq!(response.message, "method not found: absent");
}

#[tokio::test]
async fn non_object_params_normalize_and_unhandled_notification_is_ignored() {
    let (transport, mut input, mut output) = raw_transport(4096);
    let (tx, mut rx) = mpsc::unbounded_channel();
    transport.on_request(move |_, params| {
        let tx = tx.clone();
        async move {
            tx.send(params).unwrap();
            Ok(Value::Null)
        }
    });
    transport.start().unwrap();
    write_frame(
        &mut input,
        json!({ "jsonrpc": "2.0", "method": "ignored", "params": [1] }),
    )
    .await;
    write_frame(
        &mut input,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "one", "params": [1] }),
    )
    .await;
    assert_eq!(rx.recv().await, Some(json!({})));
    assert_eq!(read_frame(&mut output).await["result"], Value::Null);
    write_frame(
        &mut input,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "two", "params": 5 }),
    )
    .await;
    assert_eq!(rx.recv().await, Some(json!({})));
    assert_eq!(read_frame(&mut output).await["id"], 2);
}

#[tokio::test]
async fn malformed_lines_are_ignored_notification_defaults_params_and_start_is_idempotent() {
    let (transport, mut input, _output) = raw_transport(4096);
    let (tx, mut rx) = mpsc::unbounded_channel();
    transport.on_notification(move |method, params| {
        tx.send((method, params)).unwrap();
    });
    transport.start().unwrap();
    transport.start().unwrap();
    input
        .write_all(b"\n {bad}\nnull\n[]\n{\"jsonrpc\":\"2.0\",\"method\":\"notice\"}\n")
        .await
        .unwrap();
    assert_eq!(rx.recv().await, Some(("notice".into(), json!({}))));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn split_multibyte_utf8_is_decoded_as_one_method() {
    let (transport, mut input, _output) = raw_transport(4096);
    let (tx, mut rx) = mpsc::unbounded_channel();
    transport.on_notification(move |method, _| {
        tx.send(method).unwrap();
    });
    transport.start().unwrap();
    let bytes = "{\"jsonrpc\":\"2.0\",\"method\":\"привет\"}\n".as_bytes();
    let split = bytes.iter().position(|byte| *byte >= 0x80).unwrap() + 1;
    input.write_all(&bytes[..split]).await.unwrap();
    tokio::task::yield_now().await;
    input.write_all(&bytes[split..]).await.unwrap();
    assert_eq!(rx.recv().await.as_deref(), Some("привет"));
}

#[tokio::test]
async fn flush_waits_for_earlier_frame_write() {
    let (transport, _input, mut output) = raw_transport(64);
    transport
        .notify("large", Some(json!({ "text": "x".repeat(4096) })))
        .unwrap();
    let mut flush = Box::pin(transport.flush());
    assert!(
        tokio::time::timeout(Duration::from_millis(10), flush.as_mut())
            .await
            .is_err()
    );
    let frame = read_frame(&mut output).await;
    assert_eq!(frame["method"], "large");
    flush.await.unwrap();
}

#[tokio::test]
async fn every_frame_flushes_a_buffered_output() {
    let (_input_writer, input) = duplex(4096);
    let (output, mut output_reader) = duplex(4096);
    let transport = JsonRpcLineTransport::new(input, BufWriter::new(output)).unwrap();

    transport.notify("visible", None).unwrap();
    let frame = tokio::time::timeout(Duration::from_millis(100), read_frame(&mut output_reader))
        .await
        .expect("notify frame must not remain in BufWriter");
    assert_eq!(frame, json!({ "jsonrpc": "2.0", "method": "visible" }));
}

#[tokio::test]
async fn notify_without_params_omits_the_params_member_on_the_wire() {
    let (transport, _input, mut output) = raw_transport(4096);
    transport.notify("heartbeat", None).unwrap();
    assert_eq!(
        read_frame(&mut output).await,
        json!({ "jsonrpc": "2.0", "method": "heartbeat" })
    );
}

struct FlushFailure;

impl AsyncWrite for FlushFailure {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other("flush failed")))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn flush_reports_output_callback_failure() {
    let (_writer, input) = duplex(64);
    let transport = JsonRpcLineTransport::new(input, FlushFailure).unwrap();
    assert_eq!(
        transport.flush().await.unwrap_err().to_string(),
        "flush failed"
    );
}

#[tokio::test]
async fn input_eof_rejects_pending_and_future_requests_with_first_failure() {
    let (transport, input, _output) = raw_transport(64);
    let pending = transport.request("pending", json!({ "text": "x".repeat(4096) }), None);
    transport.start().unwrap();
    drop(input);
    let first = tokio::time::timeout(Duration::from_millis(100), pending)
        .await
        .expect("EOF must reject while the output write is blocked")
        .unwrap_err();
    assert_eq!(first.to_string(), "JSON-RPC input closed");
    let second = transport
        .request("later", json!({}), None)
        .await
        .unwrap_err();
    let first = first.downcast_ref::<SharedError>().unwrap();
    let second = second.downcast_ref::<SharedError>().unwrap();
    assert!(first.ptr_eq(second));
}

struct ReadFailure;

impl AsyncRead for ReadFailure {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other("input failed")))
    }
}

#[tokio::test]
async fn input_error_rejects_pending_and_future_requests() {
    let (output, _output_reader) = duplex(64);
    let transport = JsonRpcLineTransport::new(ReadFailure, output).unwrap();
    let pending = transport.request("pending", json!({ "text": "x".repeat(4096) }), None);
    transport.start().unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), pending)
            .await
            .expect("input error must reject while the output write is blocked")
            .unwrap_err()
            .to_string(),
        "input failed"
    );
    assert_eq!(
        transport
            .request("later", json!({}), None)
            .await
            .unwrap_err()
            .to_string(),
        "input failed"
    );
}

#[tokio::test]
async fn close_rejects_pending_and_future_but_start_after_early_close_reads() {
    let (transport, mut input, _output) = raw_transport(64);
    let (tx, mut rx) = mpsc::unbounded_channel();
    transport.on_notification(move |method, _| {
        tx.send(method).unwrap();
    });
    let pending = transport.request("pending", json!({ "text": "x".repeat(4096) }), None);
    transport.close();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), pending)
            .await
            .expect("close must reject while the output write is blocked")
            .unwrap_err()
            .to_string(),
        "JSON-RPC transport closed"
    );
    assert_eq!(
        transport
            .request("later", json!({}), None)
            .await
            .unwrap_err()
            .to_string(),
        "JSON-RPC transport closed"
    );
    transport.start().unwrap();
    write_frame(
        &mut input,
        json!({ "jsonrpc": "2.0", "method": "still-reads" }),
    )
    .await;
    assert_eq!(rx.recv().await.as_deref(), Some("still-reads"));
}

struct WriteFailure(&'static str);

impl AsyncWrite for WriteFailure {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other(self.0)))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn request_frame_write_failure_rejects_and_clears_pending() {
    let (_writer, input) = duplex(64);
    let transport = JsonRpcLineTransport::new(input, WriteFailure("write failed")).unwrap();
    assert_eq!(
        transport
            .request("fail", json!({}), None)
            .await
            .unwrap_err()
            .to_string(),
        "write failed"
    );
    assert_eq!(transport.pending_len(), 0);
}

#[tokio::test]
async fn foreign_style_write_failure_uses_display_text() {
    let (_writer, input) = duplex(64);
    let transport = JsonRpcLineTransport::new(input, WriteFailure("string write failure")).unwrap();
    assert_eq!(
        transport
            .request("fail", json!({}), None)
            .await
            .unwrap_err()
            .to_string(),
        "string write failure"
    );
}

#[tokio::test]
async fn malformed_error_uses_fallback_message() {
    let (transport, mut input, mut output) = raw_transport(4096);
    transport.start().unwrap();
    let request = transport.request("fallback", json!({}), None);
    let outbound = read_frame(&mut output).await;
    write_frame(
        &mut input,
        json!({ "jsonrpc": "2.0", "id": outbound["id"], "error": [] }),
    )
    .await;
    let cause = request.await.unwrap_err();
    let response = cause.downcast_ref::<JsonRpcResponseError>().unwrap();
    assert_eq!(response.code, None);
    assert_eq!(response.message, "JSON-RPC error");
    assert_eq!(response.data, None);
}

#[tokio::test]
async fn unmatched_response_is_ignored() {
    let (transport, mut input, mut output) = raw_transport(4096);
    transport.start().unwrap();
    write_frame(
        &mut input,
        json!({ "jsonrpc": "2.0", "id": "unknown", "result": 1 }),
    )
    .await;
    tokio::task::yield_now().await;
    assert_eq!(transport.pending_len(), 0);
    let request = transport.request("after", json!({}), None);
    let outbound = read_frame(&mut output).await;
    write_frame(
        &mut input,
        json!({ "jsonrpc": "2.0", "id": outbound["id"], "result": 2 }),
    )
    .await;
    assert_eq!(request.await.unwrap(), json!(2));
}
