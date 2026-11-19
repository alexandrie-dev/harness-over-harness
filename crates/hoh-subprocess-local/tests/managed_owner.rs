mod common;
use common::*;
use futures_util::FutureExt;
use hoh_error::{BoxError, SharedError};
use hoh_subprocess::*;
use hoh_subprocess_local::{
    LocalError, SubprocessAbortReason,
    managed_owner::{BoundProcessOwner, RangeExit, wait_with_abort},
    output::{SpillStorage, prepare_managed_process_binding},
    spawn::{LocalSubprocessHandle, ManagedProcessLaunch, bind_managed_process},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::{Notify, oneshot};
#[derive(Default)]
struct Owner {
    signals: Mutex<Vec<rustix::process::Signal>>,
    failures: Mutex<Vec<SharedError>>,
    calls: AtomicUsize,
    cleanups: AtomicUsize,
    cleanup_fails: AtomicBool,
    stopped: Arc<AtomicBool>,
    wake: Arc<Notify>,
}
impl Owner {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_waiters();
    }
}
impl BoundProcessOwner for Owner {
    fn signal(&self, signal: rustix::process::Signal) {
        self.signals.lock().unwrap().push(signal);
    }
    fn wait_for_exit(&self) -> RangeExit {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut failures = self.failures.lock().unwrap();
        if !failures.is_empty() {
            return futures_util::future::ready(Err(failures.remove(0)))
                .boxed()
                .shared();
        }
        drop(failures);
        let wake = self.wake.clone();
        let stopped = self.stopped.clone();
        async move {
            loop {
                let notified = wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if stopped.load(Ordering::Acquire) {
                    return Ok(());
                }
                notified.await;
            }
        }
        .boxed()
        .shared()
    }
    fn cleanup(&self) -> Result<(), BoxError> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        if self.cleanup_fails.load(Ordering::Acquire) {
            Err(Box::new(LocalError::Error(
                "protocol cleanup failed".into(),
            )))
        } else {
            Ok(())
        }
    }
}
type ResultSender = oneshot::Sender<Result<SubprocessOutcome, SharedError>>;
fn bind(ctx: &hoh_kernel::Context, owner: Arc<Owner>) -> (LocalSubprocessHandle, ResultSender) {
    let mut request = spec("unused");
    request.grace_ms = 10.0;
    request.stdio.stdout = SubprocessOutputMode::Inherit;
    request.stdio.stderr = SubprocessOutputMode::Inherit;
    let (send, receive) = oneshot::channel();
    let direct = subprocess_done(async move { receive.await.map_err(SharedError::new)? });
    let storage = Arc::new(SpillStorage::default());
    let binding = prepare_managed_process_binding(&storage, None, Arc::new(|_, _| Ok(()))).unwrap();
    let handle = bind_managed_process(
        ctx,
        request,
        ManagedProcessLaunch {
            stdin: None,
            stdout: None,
            stderr: None,
            control: None,
            direct,
            owner,
        },
        binding,
    )
    .unwrap();
    (handle, send)
}
async fn cleanup_observed(owner: &Owner) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while owner.cleanups.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn preserves_a_non_error_managed_direct_rejection() {
    let (kernel, ctx, _) = setup();
    let owner = Arc::new(Owner::default());
    owner.stop();
    let (handle, send) = bind(&ctx, owner);
    let rejection = SharedError::new(SubprocessAbortReason::Value(serde_json::Value::Null));
    send.send(Err(rejection.clone())).unwrap();
    let result = handle.done.clone().await.unwrap_err();
    assert!(result.ptr_eq(&rejection));
    assert!(matches!(
        result.downcast_ref::<SubprocessAbortReason>(),
        Some(SubprocessAbortReason::Value(serde_json::Value::Null))
    ));
    shutdown(kernel).await;
}
#[tokio::test]
async fn contains_a_managed_owner_cleanup_failure_after_direct_and_range_settlement() {
    let (kernel, ctx, _) = setup();
    let owner = Arc::new(Owner::default());
    owner.cleanup_fails.store(true, Ordering::Release);
    let (handle, send) = bind(&ctx, owner.clone());
    owner.stop();
    assert!(handle.wait_for_exit(None).await.unwrap());
    assert_eq!(owner.cleanups.load(Ordering::SeqCst), 0);
    send.send(Ok(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    }))
    .unwrap();
    assert_eq!(handle.done.clone().await.unwrap().exit_code, Some(0));
    cleanup_observed(&owner).await;
    assert_eq!(owner.cleanups.load(Ordering::SeqCst), 1);
    shutdown(kernel).await;
}
#[tokio::test]
async fn retries_an_early_range_read_but_cleans_and_retains_a_terminal_range_failure() {
    let (kernel, ctx, _) = setup();
    let owner = Arc::new(Owner::default());
    let early = SharedError::new(LocalError::Error("temporary range read failure".into()));
    let terminal = SharedError::new(LocalError::Error(
        "scope ended before launch request consumption".into(),
    ));
    *owner.failures.lock().unwrap() = vec![early.clone(), terminal.clone()];
    let (handle, send) = bind(&ctx, owner.clone());
    let failure = handle.wait_for_exit(None).await.unwrap_err();
    assert!(SharedError::of(failure.as_ref()).unwrap().ptr_eq(&early));
    assert_eq!(owner.cleanups.load(Ordering::SeqCst), 0);
    send.send(Ok(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    }))
    .unwrap();
    handle.done.clone().await.unwrap();
    let failure = handle.wait_for_exit(None).await.unwrap_err();
    assert!(SharedError::of(failure.as_ref()).unwrap().ptr_eq(&terminal));
    cleanup_observed(&owner).await;
    let failure = handle.wait_for_exit(None).await.unwrap_err();
    assert!(SharedError::of(failure.as_ref()).unwrap().ptr_eq(&terminal));
    assert_eq!(owner.calls.load(Ordering::SeqCst), 2);
    shutdown(kernel).await;
}
#[tokio::test]
async fn does_not_deliver_a_stale_escalation_after_range_exit_wins_the_timer_race() {
    let (kernel, ctx, _) = setup();
    let owner = Arc::new(Owner::default());
    let (handle, send) = bind(&ctx, owner.clone());
    handle.terminate();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while owner.signals.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    owner.stop();
    assert!(handle.wait_for_exit(None).await.unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(
        *owner.signals.lock().unwrap(),
        [rustix::process::Signal::TERM]
    );
    send.send(Ok(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    }))
    .unwrap();
    handle.done.clone().await.unwrap();
    shutdown(kernel).await;
}
#[tokio::test]
async fn does_not_restart_managed_termination_after_the_escalation_timer_fires() {
    let (kernel, ctx, _) = setup();
    let owner = Arc::new(Owner::default());
    let (handle, send) = bind(&ctx, owner.clone());
    handle.terminate();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while owner.signals.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.terminate();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(
        *owner.signals.lock().unwrap(),
        [rustix::process::Signal::TERM, rustix::process::Signal::KILL]
    );
    owner.stop();
    assert!(handle.wait_for_exit(None).await.unwrap());
    send.send(Ok(SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    }))
    .unwrap();
    handle.done.clone().await.unwrap();
    shutdown(kernel).await;
}
#[tokio::test]
async fn contains_a_late_wait_rejection_after_an_already_aborted_observation() {
    let controller = hoh_timeout::AbortController::new();
    controller.abort();
    let failure = SharedError::new(LocalError::Error("late observation failure".into()));
    assert!(
        !wait_with_abort(
            futures_util::future::ready(Err(failure)).boxed().shared(),
            Some(controller.signal())
        )
        .await
        .unwrap()
    );
}
