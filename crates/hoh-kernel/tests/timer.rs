mod support;
use futures_util::{FutureExt, StreamExt};
use serde_json::json;
use support::*;
#[tokio::test(start_paused = true)]
async fn completes_concurrent_interval_reads_in_order_one_per_tick() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    // Stream::poll_next needs exclusive access: the Rust contract permits one reader.
    let mut stream = Box::pin(t.context().interval(Duration::from_millis(10)));
    let mut completed = vec![];
    let start = tokio::time::Instant::now();
    for i in 0..3 {
        assert!(stream.next().now_or_never().is_none());
        tokio::time::advance(Duration::from_millis(9)).await;
        assert!(stream.next().now_or_never().is_none());
        assert_eq!(completed.len(), i);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(stream.next().now_or_never(), Some(Some(())));
        completed.push(i);
        assert_eq!(start.elapsed(), Duration::from_millis(10 * (i as u64 + 1)));
    }
    assert_eq!(completed, [0, 1, 2]);
    drop(stream);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn settles_all_pending_interval_reads_on_return_throw_dispose() {
    for operation in 0..3 {
        let (k, r, t) = harness();
        let f = r.plugin::<Recorded>(json!({})).unwrap();
        f.settled().await.unwrap();
        let ctx = t.context();
        let mut stream = Box::pin(ctx.interval(Duration::from_millis(10)));
        assert!(stream.next().now_or_never().is_none());
        tokio::time::advance(Duration::from_millis(9)).await;
        assert!(stream.next().now_or_never().is_none());
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(stream.next().now_or_never(), Some(Some(())));
        // AsyncIterator.return/throw are consumer cancellation/error in the Stream API.
        // A read is pending before either operation; dropping its stream frees the timer.
        // Context disposal terminates the stream with None.
        assert!(stream.next().now_or_never().is_none());
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<Option<()>, &'static str>>();
        let read = tokio::spawn(async move {
            tokio::select! {biased;item=stream.next()=>Ok(item),command=rx=>command.unwrap()}
        });
        turn().await;
        assert!(!read.is_finished());
        let before = tokio::time::Instant::now();
        match operation {
            0 => tx.send(Ok(None)).unwrap(),
            1 => tx.send(Err("stopped")).unwrap(),
            _ => {
                f.dispose().await;
                drop(tx);
            }
        }
        assert_eq!(
            read.await.unwrap(),
            if operation == 1 {
                Err("stopped")
            } else {
                Ok(None)
            }
        );
        assert_eq!(before.elapsed(), Duration::ZERO);
        if operation == 2 {
            let mut closed = Box::pin(ctx.interval(Duration::from_millis(10)));
            assert_eq!(closed.next().await, None);
            assert_eq!(closed.next().await, None);
        } else {
            assert!(ctx.ensure_active().is_ok());
        }
        f.dispose().await;
        k.shutdown().await;
    }
}
