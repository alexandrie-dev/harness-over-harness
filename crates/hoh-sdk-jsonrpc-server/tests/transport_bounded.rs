use hoh_kernel::Kernel;
use hoh_sdk_jsonrpc_server::transport::{
    JsonRpcLineTransport, JsonRpcTransportPeer, OUTPUT_QUEUE_MAX_BYTES, OUTPUT_QUEUE_MAX_FRAMES,
};
use serde_json::{Value, json};
use std::{
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};

#[derive(Clone, Default)]
struct SlowCapture(Arc<Mutex<CaptureState>>);

#[derive(Default)]
struct CaptureState {
    open: bool,
    bytes: Vec<u8>,
    waker: Option<Waker>,
}

impl SlowCapture {
    fn open(&self) {
        let mut state = self.0.lock().unwrap();
        state.open = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    fn frames(&self) -> Vec<Value> {
        let state = self.0.lock().unwrap();
        String::from_utf8(state.bytes.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

impl AsyncWrite for SlowCapture {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = self.0.lock().unwrap();
        if !state.open {
            state.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        state.bytes.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct FailingOutput;

impl AsyncWrite for FailingOutput {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "test output failed",
        )))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "test flush failed",
        )))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn stream_frame(kind: &str, index: u64) -> Value {
    match kind {
        "start" => json!({
            "sessionId":"slow", "frame":{
                "type":"start", "attemptId":"slow:1", "revision":1, "turn":1, "step":1
            }
        }),
        "chunk" => json!({
            "sessionId":"slow", "frame":{
                "type":"chunk", "attemptId":"slow:1", "revision":1, "index":index,
                "time":1, "chunk":{"type":"text-delta", "text":"part"}
            }
        }),
        "end" => json!({
            "sessionId":"slow", "frame":{
                "type":"end", "attemptId":"slow:1", "revision":1, "index":index,
                "outcome":"committed"
            }
        }),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn slow_client_gets_gap_response_durable_message_and_end_from_a_bounded_queue() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let (mut client, server) = tokio::io::duplex(4096);
    let output = SlowCapture::default();
    let transport =
        JsonRpcLineTransport::start(&ctx, BufReader::new(server), output.clone()).unwrap();
    transport.on_request(Arc::new(|method, _| {
        Box::pin(async move { Ok(json!(method)) })
    }));
    let answered = Arc::new(AtomicBool::new(false));
    let answer_ready = Arc::new(tokio::sync::Notify::new());
    transport.after_response({
        let answered = answered.clone();
        let answer_ready = answer_ready.clone();
        Arc::new(move |_, _| {
            answered.store(true, Ordering::SeqCst);
            answer_ready.notify_one();
        })
    });

    transport
        .notify("session.assistant-stream", Some(stream_frame("start", 0)))
        .unwrap();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    for index in 0..(OUTPUT_QUEUE_MAX_FRAMES as u64 * 4) {
        transport
            .notify(
                "session.assistant-stream",
                Some(stream_frame("chunk", index)),
            )
            .unwrap();
    }
    transport
        .notify(
            "session.event",
            Some(json!({
                "sessionId":"slow",
                "event":{"type":"assistant/message","seq":3,"data":{"message":{"id":"m","content":[{"type":"text","text":"complete"}]}}}
            })),
        )
        .unwrap();
    transport
        .notify("session.assistant-stream", Some(stream_frame("end", 1024)))
        .unwrap();
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\",\"params\":{}}\n")
        .await
        .unwrap();
    while !answered.load(Ordering::SeqCst) {
        answer_ready.notified().await;
    }

    output.open();
    transport.flush().await.unwrap();
    let frames = output.frames();
    assert!(
        frames.len() < 16,
        "queue did not coalesce: {}",
        frames.len()
    );
    let position = |predicate: &dyn Fn(&Value) -> bool| {
        frames
            .iter()
            .position(predicate)
            .expect("required output frame")
    };
    let gap = position(&|frame| {
        frame["method"] == "session.assistant-stream"
            && frame["params"]["frame"]["type"] == "gap"
            && frame["params"]["frame"]["index"] == 0
    });
    let message = position(&|frame| {
        frame["method"] == "session.event"
            && frame["params"]["event"]["type"] == "assistant/message"
    });
    let end = position(&|frame| {
        frame["method"] == "session.assistant-stream" && frame["params"]["frame"]["type"] == "end"
    });
    let response = position(&|frame| frame["id"] == 7 && frame["result"] == "ping");
    assert!(gap < message && message < end && end < response);
    assert!(!frames.iter().any(|frame| {
        frame["method"] == "session.assistant-stream" && frame["params"]["frame"]["type"] == "chunk"
    }));

    transport.close();
    kernel.shutdown().await;
}

#[tokio::test]
async fn oversized_request_fails_without_deadlocking_the_pending_request_lock() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let output = SlowCapture::default();
    let transport = JsonRpcLineTransport::prepare(&ctx, output).unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        transport.request(
            "too-large",
            json!({"value":"x".repeat(OUTPUT_QUEUE_MAX_BYTES + 1)}),
            None,
        ),
    )
    .await
    .expect("request admission deadlocked")
    .unwrap_err();
    assert!(result.to_string().contains("output queue overflow"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn output_overflow_rejects_an_already_pending_host_request() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let output = SlowCapture::default();
    let transport = JsonRpcLineTransport::prepare(&ctx, output.clone()).unwrap();
    let pending = tokio::spawn({
        let transport = transport.clone();
        async move { transport.request("pending", json!({}), None).await }
    });
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    let overflow = transport
        .notify(
            "critical",
            Some(json!({"value":"x".repeat(OUTPUT_QUEUE_MAX_BYTES + 1)})),
        )
        .unwrap_err();
    assert!(overflow.to_string().contains("output queue overflow"));
    let pending = tokio::time::timeout(std::time::Duration::from_secs(1), pending)
        .await
        .expect("pending request leaked after output overflow")
        .unwrap()
        .unwrap_err();
    assert!(pending.to_string().contains("output queue overflow"));

    output.open();
    kernel.shutdown().await;
}

#[tokio::test]
async fn write_failure_releases_a_queued_flush_with_an_error() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let transport = JsonRpcLineTransport::prepare(&ctx, FailingOutput).unwrap();
    transport.notify("first", None).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), transport.flush())
        .await
        .expect("flush waiter leaked")
        .unwrap_err();
    assert!(
        result.to_string().contains("test output failed")
            || result.to_string().contains("transport closed")
    );
    kernel.shutdown().await;
}
