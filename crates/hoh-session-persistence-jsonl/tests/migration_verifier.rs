mod common;
use common::*;
use hoh_session_persistence_jsonl::{
    generation::*, migration_verifier::*, worker::VerificationRequest, *,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Semaphore, mpsc};
#[derive(Default)]
struct Fake {
    workers: Mutex<
        Vec<(
            VerificationRequest,
            mpsc::UnboundedSender<VerificationEvent>,
        )>,
    >,
    terminated: AtomicUsize,
    joined: AtomicUsize,
    termination: Mutex<Option<Arc<Semaphore>>>,
    termination_failure: Mutex<Option<String>>,
}
impl Fake {
    fn scheduler(self: &Arc<Self>) -> VerificationScheduler {
        let fake = self.clone();
        VerificationScheduler::with_worker_factory(Arc::new(move |request| {
            let (sender, events) = mpsc::unbounded_channel();
            fake.workers.lock().unwrap().push((request, sender));
            let a = fake.clone();
            let b = fake.clone();
            Ok(VerificationWorker {
                events,
                terminate: Arc::new(move || {
                    let a = a.clone();
                    Box::pin(async move {
                        a.terminated.fetch_add(1, Ordering::SeqCst);
                        let gate = a.termination.lock().unwrap().clone();
                        if let Some(gate) = gate {
                            gate.acquire().await.unwrap().forget();
                        }
                        if let Some(message) = a.termination_failure.lock().unwrap().clone() {
                            return Err(
                                Box::new(std::io::Error::other(message)) as hoh_error::BoxError
                            );
                        }
                        Ok(())
                    })
                }),
                join: Arc::new(move || {
                    b.joined.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                }),
            })
        }))
    }
    fn emit(&self, n: usize, event: VerificationEvent) {
        self.workers.lock().unwrap()[n].1.send(event).unwrap();
    }
    async fn ready(&self, n: usize) {
        for _ in 0..10000 {
            if self.workers.lock().unwrap().len() == n {
                return;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(self.workers.lock().unwrap().len(), n);
    }
}
fn request(path: &str) -> VerificationRequest {
    VerificationRequest {
        path: path.into(),
        compression: JsonlCompression::None,
        expected_id: "session".into(),
        expected_event_count: 0,
        expected_prefix: None,
    }
}
fn result() -> JsonlVerifiedGeneration {
    JsonlVerifiedGeneration {
        identity: JsonlPhysicalIdentity {
            dev: 1,
            ino: 2,
            size: 3,
            mtime_ns: 4,
            ctime_ns: 5,
        },
        bytes: 3,
        digest: "digest".into(),
    }
}
#[tokio::test]
async fn observes_active_cancellation_before_a_queued_worker_result() {
    for _ in 0..16 {
        let fake = Arc::new(Fake::default());
        let controller = hoh_timeout::AbortController::new();
        let reason =
            hoh_error::SharedError::new(std::io::Error::other("active verification cancelled"));
        let pending = fake
            .scheduler()
            .verify(request("/stage"), Some(&controller.signal()));
        fake.ready(1).await;
        controller.abort_with(reason.clone());
        fake.emit(0, VerificationEvent::Success(result()));
        fake.emit(0, VerificationEvent::Exit(0));
        let error = fail(pending.await, "active verification cancelled");
        assert!(
            hoh_error::SharedError::of(error.as_ref())
                .unwrap()
                .ptr_eq(&reason)
        );
        assert_eq!(fake.terminated.load(Ordering::SeqCst), 1);
        assert_eq!(fake.joined.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn retains_a_valid_response_until_natural_exit_without_terminating_the_worker() {
    let fake = Arc::new(Fake::default());
    let scheduler = fake.scheduler();
    let mut r = request("/stage");
    r.expected_event_count = 2;
    r.expected_prefix = Some(JsonlExpectedPrefix {
        bytes: 3,
        digest: "a".repeat(64),
    });
    let pending = scheduler.verify(r, None);
    fake.ready(1).await;
    assert_eq!(
        fake.workers.lock().unwrap()[0]
            .0
            .expected_prefix
            .as_ref()
            .unwrap()
            .digest,
        "a".repeat(64)
    );
    fake.emit(0, VerificationEvent::Success(result()));
    let mut pending = tokio::spawn(pending);
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    assert_eq!(fake.terminated.load(Ordering::SeqCst), 0);
    fake.emit(0, VerificationEvent::Exit(0));
    assert_eq!((&mut pending).await.unwrap().unwrap(), result());
    assert_eq!(fake.joined.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn rejects_a_nonzero_exit_after_a_valid_response() {
    let fake = Arc::new(Fake::default());
    let pending = fake.scheduler().verify(request("/stage"), None);
    fake.ready(1).await;
    fake.emit(0, VerificationEvent::Success(result()));
    fake.emit(0, VerificationEvent::Exit(7));
    fail(pending.await, "exited after reporting a result (code 7)");
    assert_eq!(fake.terminated.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn retains_a_worker_error_after_a_valid_response() {
    let fake = Arc::new(Fake::default());
    let pending = fake.scheduler().verify(request("/stage"), None);
    fake.ready(1).await;
    fake.emit(0, VerificationEvent::Success(result()));
    fake.emit(
        0,
        VerificationEvent::Failure(Box::new(std::io::Error::other("worker cleanup failed"))),
    );
    fail(pending.await, "worker cleanup failed");
    assert_eq!(fake.terminated.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn honors_cancellation_after_a_valid_response_and_joins_termination() {
    let fake = Arc::new(Fake::default());
    let gate = Arc::new(Semaphore::new(0));
    *fake.termination.lock().unwrap() = Some(gate.clone());
    let controller = hoh_timeout::AbortController::new();
    let pending = fake
        .scheduler()
        .verify(request("/stage"), Some(&controller.signal()));
    fake.ready(1).await;
    fake.emit(0, VerificationEvent::Success(result()));
    controller.abort();
    let task = tokio::spawn(pending);
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(fake.terminated.load(Ordering::SeqCst), 1);
    assert!(!task.is_finished());
    gate.add_permits(1);
    assert!(task.await.unwrap().is_err());
}
#[tokio::test]
async fn reconstructs_a_worker_reported_error() {
    let fake = Arc::new(Fake::default());
    let p = fake.scheduler().verify(request("/stage"), None);
    fake.ready(1).await;
    fake.emit(
        0,
        VerificationEvent::Failure(Box::new(std::io::Error::other("invalid stage"))),
    );
    fail(p.await, "invalid stage");
}
#[tokio::test]
async fn rejects_an_invalid_response() {
    let fake = Arc::new(Fake::default());
    let p = fake.scheduler().verify(request("/stage"), None);
    fake.ready(1).await;
    fake.emit(0, VerificationEvent::InvalidResponse);
    fail(p.await, "invalid response");
    assert_eq!(fake.terminated.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn rejects_an_early_exit() {
    let fake = Arc::new(Fake::default());
    let p = fake.scheduler().verify(request("/stage"), None);
    fake.ready(1).await;
    fake.emit(0, VerificationEvent::Exit(7));
    fail(p.await, "exited before reporting a result (code 7)");
}
#[tokio::test]
async fn aggregates_termination_failure_after_a_worker_failure() {
    let fake = Arc::new(Fake::default());
    *fake.termination_failure.lock().unwrap() = Some("terminate failed".into());
    let p = fake.scheduler().verify(request("/stage"), None);
    fake.ready(1).await;
    fake.emit(
        0,
        VerificationEvent::Failure(Box::new(std::io::Error::other("worker failed"))),
    );
    let e = fail(p.await, "termination failed");
    let JsonlError::Aggregate { errors, .. } = e.downcast_ref::<JsonlError>().unwrap() else {
        panic!("aggregate")
    };
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].to_string(), "worker failed");
    assert_eq!(errors[1].to_string(), "terminate failed");
}
#[tokio::test]
async fn ignores_terminal_signals_after_a_successful_worker_exit() {
    let fake = Arc::new(Fake::default());
    let p = fake.scheduler().verify(request("/stage"), None);
    fake.ready(1).await;
    fake.emit(0, VerificationEvent::Success(result()));
    fake.emit(0, VerificationEvent::Exit(0));
    fake.emit(
        0,
        VerificationEvent::Failure(Box::new(std::io::Error::other("late error"))),
    );
    fake.emit(0, VerificationEvent::Exit(1));
    fake.emit(0, VerificationEvent::InvalidResponse);
    assert_eq!(p.await.unwrap(), result());
    assert_eq!(fake.terminated.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn retains_the_concurrency_permit_until_the_successful_worker_exits() {
    let fake = Arc::new(Fake::default());
    let s = fake.scheduler();
    let a = s.verify(request("/first"), None);
    let b = s.verify(request("/second"), None);
    let c = s.verify(request("/third"), None);
    fake.ready(2).await;
    fake.emit(0, VerificationEvent::Success(result()));
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(fake.workers.lock().unwrap().len(), 2);
    fake.emit(0, VerificationEvent::Exit(0));
    a.await.unwrap();
    fake.ready(3).await;
    for n in [1, 2] {
        fake.emit(n, VerificationEvent::Success(result()));
        fake.emit(n, VerificationEvent::Exit(0));
    }
    b.await.unwrap();
    c.await.unwrap();
}
#[tokio::test]
async fn hands_a_released_permit_directly_to_the_oldest_waiter() {
    let fake = Arc::new(Fake::default());
    let s = fake.scheduler();
    let a = s.verify(request("/first"), None);
    let b = s.verify(request("/second"), None);
    fake.ready(2).await;
    let c = s.verify(request("/third"), None);
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    fake.emit(0, VerificationEvent::Success(result()));
    fake.emit(0, VerificationEvent::Exit(0));
    let d = s.verify(request("/fourth"), None);
    a.await.unwrap();
    fake.ready(3).await;
    assert_eq!(fake.workers.lock().unwrap()[2].0.path, "/third");
    fake.emit(1, VerificationEvent::Success(result()));
    fake.emit(1, VerificationEvent::Exit(0));
    b.await.unwrap();
    fake.ready(4).await;
    assert_eq!(fake.workers.lock().unwrap()[3].0.path, "/fourth");
    for n in [2, 3] {
        fake.emit(n, VerificationEvent::Success(result()));
        fake.emit(n, VerificationEvent::Exit(0));
    }
    c.await.unwrap();
    d.await.unwrap();
}
#[tokio::test]
async fn removes_an_aborted_waiter_without_starting_another_worker() {
    let fake = Arc::new(Fake::default());
    let s = fake.scheduler();
    let a = s.verify(request("/first"), None);
    let b = s.verify(request("/second"), None);
    fake.ready(2).await;
    let controller = hoh_timeout::AbortController::new();
    let c = s.verify(request("/queued"), Some(&controller.signal()));
    controller.abort();
    assert!(c.await.is_err());
    assert_eq!(fake.workers.lock().unwrap().len(), 2);
    for n in [0, 1] {
        fake.emit(n, VerificationEvent::Success(result()));
        fake.emit(n, VerificationEvent::Exit(0));
    }
    a.await.unwrap();
    b.await.unwrap();
}
#[tokio::test]
async fn dropping_the_caller_waiter_does_not_release_a_running_workers_permit() {
    let fake = Arc::new(Fake::default());
    let s = fake.scheduler();
    let a = s.verify(request("/first"), None);
    let b = s.verify(request("/second"), None);
    fake.ready(2).await;
    drop(a);
    let c = s.verify(request("/third"), None);
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(fake.workers.lock().unwrap().len(), 2);
    fake.emit(0, VerificationEvent::Success(result()));
    fake.emit(0, VerificationEvent::Exit(0));
    fake.ready(3).await;
    for n in [1, 2] {
        fake.emit(n, VerificationEvent::Success(result()));
        fake.emit(n, VerificationEvent::Exit(0));
    }
    b.await.unwrap();
    c.await.unwrap();
    assert_eq!(fake.joined.load(Ordering::SeqCst), 3);
}
