mod support;
use futures_util::StreamExt;
use hoh_kernel::*;
use serde_json::json;
use support::*;
#[tokio::test(start_paused = true)]
async fn settles_all_pending_interval_reads_on_return_throw_dispose() {
    for operation in 0..3 {
        let (k, r, t) = harness();
        let f = r.plugin::<Recorded>(json!({})).unwrap();
        f.settled().await.unwrap();
        let ctx = t.context();
        let mut stream = Box::pin(ctx.interval(Duration::from_millis(100)));
        assert_eq!(stream.next().await, Some(()));
        if operation == 0 {
            drop(stream);
        } else if operation == 1 {
            let sleep = ctx.sleep(Duration::from_secs(5));
            f.restart().await.unwrap();
            assert!(matches!(sleep.await, Err(KernelError::Inactive)));
            assert_eq!(stream.next().await, None);
        } else {
            f.dispose().await;
            assert_eq!(stream.next().await, None);
            assert_eq!(stream.next().await, None);
        }
        k.shutdown().await;
    }
}
