//! A write that arrives while the writer is being stopped is an error, not a silent drop
//! (transport.ts never loses a frame without telling the caller).
use hoh_kernel::Kernel;
use hoh_sdk_jsonrpc_server::transport::{JsonRpcLineTransport, JsonRpcTransportPeer};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};
use tokio::io::{AsyncWrite, BufReader};

/// An output whose writes block until `open` is called, so the writer task stays alive with a
/// frame in its hands while the stop is queued behind it.
#[derive(Clone, Default)]
struct Gate(Arc<Mutex<(bool, Option<Waker>)>>);
impl Gate {
    fn open(&self) {
        let mut state = self.0.lock().unwrap();
        state.0 = true;
        if let Some(waker) = state.1.take() {
            waker.wake();
        }
    }
}
impl AsyncWrite for Gate {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = self.0.lock().unwrap();
        if state.0 {
            Poll::Ready(Ok(bytes.len()))
        } else {
            state.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn notify_fails_once_the_writer_has_been_told_to_stop() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let (_client, server) = tokio::io::duplex(1024);
    let gate = Gate::default();
    let transport =
        JsonRpcLineTransport::start(&ctx, BufReader::new(server), gate.clone()).unwrap();
    transport.notify("first", None).unwrap();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    // unload: the writer is stuck on "first", the stop queues behind it
    let unload = tokio::spawn(async move { kernel.shutdown().await });
    let mut refused = false;
    for _ in 0..1000 {
        if transport.notify("late", None).is_err() {
            refused = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    gate.open();
    unload.await.unwrap();
    assert!(
        refused,
        "a frame queued behind the stop was reported as written"
    );
}
