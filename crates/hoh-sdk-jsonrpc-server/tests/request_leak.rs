//! transport.ts keeps nothing for a request once it is answered; so must the transport here.
//! Per-request kernel effects (one `Effect` + task handle each) used to stay registered until
//! the plugin was unloaded. Resident memory is the observable: the workspace forbids the unsafe
//! code a counting allocator would need, and the kernel exposes no effect count.
use hoh_kernel::Kernel;
use hoh_sdk_jsonrpc_server::transport::JsonRpcLineTransport;
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};

/// Resident memory of this test process, in bytes (`/proc/self/statm`, second field, in pages).
fn resident_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    statm
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 4096
}

/// Discards the output but counts the answered requests (one line each).
#[derive(Clone)]
struct CountLines(Arc<AtomicUsize>);
impl AsyncWrite for CountLines {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0.fetch_add(
            bytes.iter().filter(|b| **b == b'\n').count(),
            Ordering::SeqCst,
        );
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn answer_requests(
    input: &mut tokio::io::DuplexStream,
    answered: &AtomicUsize,
    count: usize,
    first_id: usize,
) {
    let mut batch = String::new();
    for id in first_id..first_id + count {
        batch.push_str(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"nope/unknown\"}}\n"
        ));
    }
    let target = answered.load(Ordering::SeqCst) + count;
    input.write_all(batch.as_bytes()).await.unwrap();
    input.flush().await.unwrap();
    while answered.load(Ordering::SeqCst) < target {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    // let the last tasks finish and drop what they hold
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn answered_requests_leave_nothing_registered_in_the_kernel() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let (mut client, server) = tokio::io::duplex(8 << 20);
    let answered = Arc::new(AtomicUsize::new(0));
    let transport =
        JsonRpcLineTransport::start(&ctx, BufReader::new(server), CountLines(answered.clone()))
            .unwrap();
    transport.on_request(Arc::new(|method, _| {
        Box::pin(async move { Err(format!("unknown: {method}").into()) })
    }));
    // Batches keep the number of simultaneously running requests (and so the high-water mark
    // of memory the allocator keeps) the same in the warm-up and in the measured part.
    const BATCH: usize = 500;
    for batch in 0..4 {
        answer_requests(&mut client, &answered, BATCH, batch * BATCH).await;
    }
    let before = resident_bytes();
    for batch in 4..124 {
        answer_requests(&mut client, &answered, BATCH, batch * BATCH).await;
    }
    let growth = resident_bytes().saturating_sub(before);
    // A retained effect costs several hundred bytes: 60 000 of them are tens of megabytes.
    assert!(
        growth < 8 << 20,
        "resident memory grew by {growth} bytes over 60000 answered requests"
    );
    drop(client);
    kernel.shutdown().await;
}
