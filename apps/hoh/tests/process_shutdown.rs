use hoh::process_shutdown::*;
use hoh_error::BoxError;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
#[derive(Default)]
struct Seen {
    forced: Mutex<Vec<i32>>,
    completed: Mutex<Vec<i32>>,
}
fn controller(dispose: Disposal, timeout: Option<Duration>) -> (ProcessShutdown, Arc<Seen>) {
    let s = Arc::new(Seen::default());
    let f = s.clone();
    let c = s.clone();
    (
        create_process_shutdown(
            dispose,
            Arc::new(move |code| lock(&f.forced).push(code)),
            Arc::new(move |code| lock(&c.completed).push(code)),
            timeout,
        ),
        s,
    )
}
fn deferred() -> (
    Disposal,
    tokio::sync::oneshot::Sender<Result<(), BoxError>>,
    Arc<AtomicUsize>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let pending = Mutex::new(Some(rx));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let dispose: Disposal = Arc::new(move || {
        count.fetch_add(1, Ordering::AcqRel);
        let rx = lock(&pending).take().unwrap();
        Box::pin(async move { rx.await.unwrap() })
    });
    (dispose, tx, calls)
}
async fn turns() {
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
}
#[tokio::test(start_paused = true)]
async fn completes_naturally_after_disposal_resolves_and_forces_exit_when_it_rejects() {
    let (c, s) = controller(Arc::new(|| Box::pin(async { Ok(()) })), None);
    c.shutdown(0).await;
    assert_eq!(*lock(&s.completed), [0]);
    assert!(lock(&s.forced).is_empty());
    let (c, s) = controller(
        Arc::new(|| Box::pin(async { Err("dispose failed".into()) })),
        None,
    );
    c.shutdown(1).await;
    assert_eq!(*lock(&s.forced), [1]);
    assert!(lock(&s.completed).is_empty());
}
#[tokio::test(start_paused = true)]
async fn uses_process_exit_code_for_default_normal_completion() {
    let (c, s) = controller(Arc::new(|| Box::pin(async { Ok(()) })), None);
    c.shutdown(7).await;
    assert_eq!(*lock(&s.completed), [7]);
    assert!(lock(&s.forced).is_empty());
}
#[tokio::test(start_paused = true)]
async fn forces_exit_when_graceful_disposal_reaches_its_bound() {
    let (dispose, release, _) = deferred();
    let (c, s) = controller(dispose, None);
    let pending = c.shutdown(0);
    turns().await;
    tokio::time::advance(Duration::from_millis(PROCESS_SHUTDOWN_TIMEOUT_MS - 1)).await;
    turns().await;
    assert!(lock(&s.forced).is_empty());
    tokio::time::advance(Duration::from_millis(1)).await;
    turns().await;
    assert_eq!(*lock(&s.forced), [0]);
    release.send(Ok(())).unwrap();
    pending.await;
    assert_eq!(*lock(&s.forced), [0]);
    assert!(lock(&s.completed).is_empty());
}
#[tokio::test(start_paused = true)]
async fn honors_a_caller_supplied_grace_period() {
    let (dispose, release, _) = deferred();
    let (c, s) = controller(dispose, Some(Duration::from_millis(25)));
    let pending = c.shutdown(0);
    turns().await;
    tokio::time::advance(Duration::from_millis(24)).await;
    turns().await;
    assert!(lock(&s.forced).is_empty());
    tokio::time::advance(Duration::from_millis(1)).await;
    turns().await;
    assert_eq!(*lock(&s.forced), [0]);
    release.send(Ok(())).unwrap();
    pending.await;
}
#[tokio::test(start_paused = true)]
async fn lets_ctrl_c_force_a_normal_shutdown_already_stuck_in_disposal() {
    let (dispose, release, _) = deferred();
    let (c, s) = controller(dispose, None);
    let pending = c.shutdown(0);
    c.interrupt(130);
    assert_eq!(*lock(&s.forced), [130]);
    release.send(Ok(())).unwrap();
    pending.await;
    assert_eq!(*lock(&s.forced), [130]);
    assert!(lock(&s.completed).is_empty());
}
#[tokio::test(start_paused = true)]
async fn forces_exit_after_disposal_started_by_a_signal() {
    let (dispose, release, _) = deferred();
    let (c, s) = controller(dispose, None);
    c.interrupt(143);
    release.send(Ok(())).unwrap();
    c.shutdown(0).await;
    assert_eq!(*lock(&s.forced), [143]);
    assert!(lock(&s.completed).is_empty());
}
#[tokio::test(start_paused = true)]
async fn drains_on_the_first_signal_and_forces_on_the_second_signal() {
    let (dispose, release, calls) = deferred();
    let (c, s) = controller(dispose, None);
    c.interrupt(143);
    turns().await;
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert!(lock(&s.forced).is_empty());
    c.interrupt(130);
    assert_eq!(*lock(&s.forced), [130]);
    release.send(Ok(())).unwrap();
    c.shutdown(0).await;
    assert_eq!(*lock(&s.forced), [130]);
}
#[tokio::test(start_paused = true)]
async fn coalesces_normal_shutdown_calls_without_treating_them_as_escalation() {
    let (dispose, release, calls) = deferred();
    let (c, s) = controller(dispose, None);
    let first = c.shutdown(0);
    let second = c.shutdown(1);
    assert!(first.ptr_eq(&second));
    assert!(lock(&s.forced).is_empty());
    release.send(Ok(())).unwrap();
    first.await;
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(*lock(&s.completed), [0]);
    assert!(lock(&s.forced).is_empty());
}
#[tokio::test(start_paused = true)]
async fn lets_a_signal_force_exit_while_natural_completion_drains_remaining_handles() {
    let (c, s) = controller(Arc::new(|| Box::pin(async { Ok(()) })), None);
    c.shutdown(0).await;
    c.interrupt(130);
    assert_eq!(*lock(&s.completed), [0]);
    assert_eq!(*lock(&s.forced), [130]);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_signal_starts_are_one_disposal_and_one_forced_exit() {
    let (dispose, release, calls) = deferred();
    let (c, s) = controller(dispose, None);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut jobs = vec![];
    for code in [130, 143] {
        let b = barrier.clone();
        let c = c.clone();
        jobs.push(tokio::spawn(async move {
            b.wait().await;
            c.interrupt(code);
        }));
    }
    barrier.wait().await;
    for j in jobs {
        j.await.unwrap();
    }
    release.send(Ok(())).unwrap();
    tokio::time::timeout(Duration::from_secs(2), c.shutdown(0))
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(lock(&s.forced).len(), 1);
    assert!(lock(&s.completed).is_empty());
}
