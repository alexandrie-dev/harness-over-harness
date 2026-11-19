use hoh_kernel::{Context, Error, Event, Kernel, Waterfall};
use std::{
    future::{Future, poll_fn},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
};
use tokio::sync::oneshot;

struct Question;
impl Event for Question {
    const NAME: &'static str = "test/waterfall-start";
    type Mode = Waterfall<u32>;
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap()
}
async fn assert_pending<F: Future>(future: std::pin::Pin<&mut F>) {
    let mut future = future;
    assert!(poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await);
}

async fn pending_pass(delegates: usize) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let trace = Arc::new(Mutex::new(Vec::new()));
    for _ in 0..delegates {
        let trace = trace.clone();
        ctx.on_waterfall::<Question, u32, _>(move |_, next| {
            let trace = trace.clone();
            async move {
                lock(&trace).push("delegate");
                next.run().await
            }
        })
        .unwrap();
    }
    let (release, wait) = oneshot::channel();
    let slot = Arc::new(Mutex::new(Some(wait)));
    let prefix = Arc::new(AtomicBool::new(false));
    let observed = prefix.clone();
    let observed_trace = trace.clone();
    ctx.on_waterfall::<Question, u32, _>(move |_, _| {
        lock(&observed_trace).push("factory");
        let wait = lock(&slot).take().unwrap();
        let trace = observed_trace.clone();
        let observed = observed.clone();
        async move {
            lock(&trace).push("prefix");
            observed.store(true, Ordering::Release);
            Ok(wait.await.unwrap())
        }
    })
    .unwrap();
    let (started, answer) =
        ctx.waterfall_with_start::<Question, u32, _>(Question, |_| async { Ok(99) });
    let mut answer = Box::pin(answer);
    started.await;
    assert!(prefix.load(Ordering::Acquire));
    let mut expected = vec!["delegate"; delegates];
    expected.extend(["factory", "prefix"]);
    assert_eq!(*lock(&trace), expected);
    assert_pending(answer.as_mut()).await;
    release.send(7).unwrap();
    assert_eq!(answer.await.unwrap(), 7);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn startup_covers_initially_delegated_prefixes_without_waiting_for_the_answer() {
    for delegates in [0, 1, 8, 128] {
        pending_pass(delegates).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_is_a_happens_before_barrier_across_workers() {
    for _ in 0..16 {
        pending_pass(8).await;
    }
}

#[tokio::test(start_paused = true)]
async fn empty_chain_reports_the_pending_tails_initial_poll() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let (send, receive) = oneshot::channel();
    let entered = Arc::new(AtomicBool::new(false));
    let capture = entered.clone();
    let (started, answer) =
        ctx.waterfall_with_start::<Question, u32, _>(Question, move |_| async move {
            capture.store(true, Ordering::Release);
            Ok(receive.await.unwrap())
        });
    let mut answer = Box::pin(answer);
    started.await;
    assert!(entered.load(Ordering::Acquire));
    assert_pending(answer.as_mut()).await;
    send.send(3).unwrap();
    assert_eq!(answer.await.unwrap(), 3);
    kernel.shutdown().await;
}

fn failing(ctx: &Context, kind: u8) {
    match kind {
        0 => {
            ctx.on_waterfall::<Question, u32, _>(|_, _| async { Err("answer failed".into()) })
                .unwrap();
        }
        1 => {
            ctx.on_waterfall::<Question, u32, _>(
                |_, _| -> std::future::Ready<Result<u32, Error>> { panic!("factory failed") },
            )
            .unwrap();
        }
        2 => {
            ctx.on_waterfall::<Question, u32, _>(|_, _| async { panic!("poll failed") })
                .unwrap();
        }
        _ => panic!("unknown fixture"),
    }
}
#[tokio::test(start_paused = true)]
async fn errors_and_panics_release_startup_and_remain_answer_errors() {
    for kind in 0..3 {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        failing(&ctx, kind);
        let (started, answer) =
            ctx.waterfall_with_start::<Question, u32, _>(Question, |_| async { Ok(0) });
        started.await;
        assert!(answer.await.is_err());
        kernel.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_continuation_polled_after_an_await_is_not_part_of_startup() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let (send, wait) = oneshot::channel();
    let slot = Arc::new(Mutex::new(Some(wait)));
    ctx.on_waterfall::<Question, u32, _>(move |_, next| {
        let wait = lock(&slot).take().unwrap();
        async move {
            wait.await.unwrap();
            next.run().await
        }
    })
    .unwrap();
    let second = Arc::new(AtomicBool::new(false));
    let capture = second.clone();
    ctx.on_waterfall::<Question, u32, _>(move |_, _| {
        capture.store(true, Ordering::Release);
        async { Ok(4) }
    })
    .unwrap();
    let (started, answer) =
        ctx.waterfall_with_start::<Question, u32, _>(Question, |_| async { Ok(0) });
    started.await;
    assert!(!second.load(Ordering::Acquire));
    let mut answer = Box::pin(answer);
    assert_pending(answer.as_mut()).await;
    send.send(()).unwrap();
    assert_eq!(answer.await.unwrap(), 4);
    assert!(second.load(Ordering::Acquire));
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn dropping_either_observer_does_not_cancel_dispatch() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let (started, answer) =
        ctx.waterfall_with_start::<Question, u32, _>(Question, |_| async { Ok(1) });
    drop(started);
    assert_eq!(answer.await.unwrap(), 1);
    let (started, answer) =
        ctx.waterfall_with_start::<Question, u32, _>(Question, |_| async { Ok(2) });
    drop(answer);
    started.await;
    kernel.shutdown().await;
}
