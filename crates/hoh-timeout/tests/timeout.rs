use hoh_error::{BoxError, SharedError};
use hoh_timeout::{
    AbortController, AbortError, AbortSignal, DomTimeoutError, IdleWatchdogError,
    MAX_TIMER_DELAY_MS, TimeoutReason, clamp_timeout, deadline, idle_watchdog, is_abort_error,
    timeout_of,
};
use std::{
    future::{self, Future},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::oneshot;

fn classify(signal: &AbortSignal, code: Option<&str>) -> Option<TimeoutReason> {
    let reason = signal.reason();
    timeout_of(reason.as_ref(), code).cloned()
}
async fn elapse(ms: u64) {
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(ms)).await;
    tokio::task::yield_now().await;
}
fn assert_same_timeout(error: IdleWatchdogError, signal: &AbortSignal) {
    let IdleWatchdogError::Timeout(reason) = error else {
        panic!("Expected timeout")
    };
    assert!(reason.ptr_eq(&signal.reason().unwrap()));
}

#[test]
fn timeout_reason_is_error_carrying_code_and_elapsed_ms() {
    let reason = TimeoutReason::new("BASH_TIMEOUT", 100.0);
    let _: &dyn std::error::Error = &reason;
    assert!(std::any::type_name_of_val(&reason).ends_with("TimeoutReason"));
    assert_eq!(reason.code, "BASH_TIMEOUT");
    assert_eq!(reason.timeout_ms, 100.0);
    assert_eq!(reason.to_string(), "BASH_TIMEOUT after 100ms");
}

#[test]
fn pure_arithmetic_preserves_javascript_number_edge_cases() {
    assert!(clamp_timeout(None, f64::NAN, 100.0, None).unwrap().is_nan());
    assert!(clamp_timeout(None, 100.0, f64::NAN, None).unwrap().is_nan());
    assert!(
        clamp_timeout(None, 0.0, -0.0, None)
            .unwrap()
            .is_sign_negative()
    );
    for (ms, expected) in [
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (-0.0, "0"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (1e-6, "0.000001"),
        (1.5, "1.5"),
    ] {
        assert_eq!(
            TimeoutReason::new("T", ms).to_string(),
            format!("T after {expected}ms")
        );
    }
}
#[test]
fn fills_default_when_hint_absent() {
    assert_eq!(
        clamp_timeout(None, 120_000.0, 600_000.0, None).unwrap(),
        120_000.0
    );
}
#[test]
fn caps_hint_at_max() {
    assert_eq!(
        clamp_timeout(Some(999_999.0), 120_000.0, 600_000.0, None).unwrap(),
        600_000.0
    );
}
#[test]
fn keeps_valid_hint_under_cap() {
    assert_eq!(
        clamp_timeout(Some(5_000.0), 120_000.0, 600_000.0, None).unwrap(),
        5_000.0
    );
}
#[test]
fn caps_default_itself_at_max() {
    assert_eq!(
        clamp_timeout(None, 900_000.0, 600_000.0, None).unwrap(),
        600_000.0
    );
}
#[test]
fn rejects_nonfinite_hint_with_caller_name() {
    assert_eq!(
        clamp_timeout(
            Some(f64::NAN),
            100.0,
            200.0,
            Some("bash-local: request.timeoutMs")
        )
        .unwrap_err()
        .to_string(),
        "bash-local: request.timeoutMs must be a positive finite number"
    );
    assert_eq!(
        clamp_timeout(Some(f64::INFINITY), 100.0, 200.0, None)
            .unwrap_err()
            .to_string(),
        "timeoutMs must be a positive finite number"
    );
}
#[test]
fn rejects_nonpositive_hint() {
    for value in [0.0, -1.0] {
        assert!(
            clamp_timeout(Some(value), 100.0, 200.0, None)
                .unwrap_err()
                .to_string()
                .contains("must be a positive finite number")
        );
    }
}

#[tokio::test(start_paused = true)]
async fn deadline_aborts_with_timeout_reason_after_elapsed_ms() {
    let d = deadline(None, 100.0, "BASH_TIMEOUT").unwrap();
    assert!(!d.signal.aborted());
    elapse(100).await;
    assert!(d.signal.aborted());
    let reason = classify(&d.signal, None).unwrap();
    assert_eq!(reason.code, "BASH_TIMEOUT");
    assert_eq!(reason.timeout_ms, 100.0);
}
#[tokio::test(start_paused = true)]
async fn deadline_disposal_clears_timer() {
    let mut d = deadline(None, 100.0, "BASH_TIMEOUT").unwrap();
    d.dispose();
    elapse(1_000).await;
    assert!(!d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[test]
fn rejects_delays_node_would_clamp_to_one_ms() {
    for value in [MAX_TIMER_DELAY_MS as f64 + 1.0, f64::INFINITY] {
        let error = deadline(None, value, "BASH_TIMEOUT").err().unwrap();
        assert!(
            error
                .to_string()
                .contains(&format!("no greater than {MAX_TIMER_DELAY_MS}"))
        );
    }
}
#[tokio::test(start_paused = true)]
async fn aborts_on_upstream_cancellation_not_timeout() {
    let controller = AbortController::new();
    let upstream = controller.signal();
    let d = deadline(Some(&upstream), 60_000.0, "BASH_TIMEOUT").unwrap();
    controller.abort_with(std::io::Error::other("user cancelled"));
    assert!(d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[tokio::test(start_paused = true)]
async fn cancel_wins_before_timeout() {
    let controller = AbortController::new();
    let upstream = controller.signal();
    let d = deadline(Some(&upstream), 100.0, "BASH_TIMEOUT").unwrap();
    controller.abort_with(std::io::Error::other("user cancelled"));
    elapse(200).await;
    assert!(d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[tokio::test(start_paused = true)]
async fn timeout_wins_before_cancellation() {
    let controller = AbortController::new();
    let upstream = controller.signal();
    let d = deadline(Some(&upstream), 100.0, "WEB_FETCH_TIMEOUT").unwrap();
    elapse(150).await;
    assert!(d.signal.aborted());
    assert_eq!(classify(&d.signal, None).unwrap().code, "WEB_FETCH_TIMEOUT");
    controller.abort_with(std::io::Error::other("too late"));
    assert_eq!(classify(&d.signal, None).unwrap().code, "WEB_FETCH_TIMEOUT");
}
#[tokio::test(start_paused = true)]
async fn forwards_preaborted_upstream_immediately() {
    let controller = AbortController::new();
    let upstream = controller.signal();
    controller.abort_with(std::io::Error::other("already gone"));
    let d = deadline(Some(&upstream), 60_000.0, "BASH_TIMEOUT").unwrap();
    assert!(d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[tokio::test(start_paused = true)]
async fn sentinel_arms_no_timer_and_forwards_upstream() {
    let controller = AbortController::new();
    let upstream = controller.signal();
    let d = deadline(Some(&upstream), 0.0, "BASH_TIMEOUT").unwrap();
    elapse(1_000_000).await;
    assert!(!d.signal.aborted());
    controller.abort_with(std::io::Error::other("kill"));
    assert!(d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[tokio::test(start_paused = true)]
async fn sentinel_without_upstream_never_aborts_and_disposal_is_noop() {
    let mut d = deadline(None, 0.0, "BASH_TIMEOUT").unwrap();
    d.dispose();
    elapse(1_000_000).await;
    assert!(!d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[test]
fn negative_timeout_same_as_zero() {
    let mut d = deadline(None, -5.0, "BASH_TIMEOUT").unwrap();
    assert!(!d.signal.aborted());
    d.dispose();
}
#[test]
fn classifies_bare_timeout_reason_carrier() {
    let reason = SharedError::new(TimeoutReason::new("WEB_FETCH_TIMEOUT", 50.0));
    assert!(std::ptr::eq(
        timeout_of(Some(&reason), None).unwrap(),
        reason.downcast_ref::<TimeoutReason>().unwrap()
    ));
}
#[test]
fn returns_none_for_nontimeout_reason() {
    assert!(
        timeout_of(
            Some(&SharedError::new(std::io::Error::other("other"))),
            None
        )
        .is_none()
    );
    assert!(
        timeout_of(
            Some(&SharedError::new(std::io::Error::other("user cancelled"))),
            None
        )
        .is_none()
    );
    assert!(timeout_of(None, None).is_none());
}
#[test]
fn matches_only_requested_code() {
    let reason = SharedError::new(TimeoutReason::new("BASH_TIMEOUT", 100.0));
    assert!(std::ptr::eq(
        timeout_of(Some(&reason), Some("BASH_TIMEOUT")).unwrap(),
        reason.downcast_ref::<TimeoutReason>().unwrap()
    ));
    assert!(timeout_of(Some(&reason), Some("WEB_FETCH_TIMEOUT")).is_none());
}
#[tokio::test(start_paused = true)]
async fn does_not_misclassify_outer_timeout_as_inner_code() {
    let controller = AbortController::new();
    let outer = controller.signal();
    controller.abort_with(TimeoutReason::new("OUTER_TIMEOUT", 30.0));
    let inner = deadline(Some(&outer), 60_000.0, "BASH_TIMEOUT").unwrap();
    assert!(inner.signal.aborted());
    assert!(classify(&inner.signal, Some("BASH_TIMEOUT")).is_none());
    assert_eq!(classify(&inner.signal, None).unwrap().code, "OUTER_TIMEOUT");
}

#[tokio::test(start_paused = true)]
async fn watchdog_arms_only_during_next_and_rearms_same_signal() {
    let (first_tx, first) = oneshot::channel();
    let (second_tx, second) = oneshot::channel::<u32>();
    let watchdog = idle_watchdog(None, 100.0, "LLM_STREAM_IDLE_TIMEOUT").unwrap();
    let stable_signal = &watchdog.signal;
    let first_next = watchdog.next(first);
    elapse(99).await;
    assert!(!stable_signal.aborted());
    first_tx.send((false, 1)).unwrap();
    assert_eq!(first_next.await.unwrap().unwrap(), (false, 1));
    elapse(10_000).await;
    assert!(!stable_signal.aborted());
    assert!(std::ptr::eq(stable_signal, &watchdog.signal));
    let second_next = watchdog.next(second);
    elapse(100).await;
    assert_eq!(
        classify(stable_signal, Some("LLM_STREAM_IDLE_TIMEOUT"))
            .unwrap()
            .timeout_ms,
        100.0
    );
    assert_same_timeout(second_next.await.unwrap_err(), stable_signal);
    assert!(second_tx.send(2).is_err());
}

#[tokio::test(start_paused = true)]
async fn watchdog_rearms_outstanding_demand_on_activity_pulse() {
    let (tx, pending) = oneshot::channel::<()>();
    let watchdog = idle_watchdog(None, 100.0, "LLM_STREAM_IDLE_TIMEOUT").unwrap();
    watchdog.pulse();
    elapse(1_000).await;
    assert!(!watchdog.signal.aborted());
    let next = watchdog.next(pending);
    elapse(99).await;
    watchdog.pulse();
    elapse(99).await;
    assert!(!watchdog.signal.aborted());
    elapse(1).await;
    assert_eq!(
        classify(&watchdog.signal, Some("LLM_STREAM_IDLE_TIMEOUT"))
            .unwrap()
            .timeout_ms,
        100.0
    );
    assert_same_timeout(next.await.unwrap_err(), &watchdog.signal);
    assert!(tx.send(()).is_err());
    watchdog.dispose();
    watchdog.pulse();
}
#[tokio::test(start_paused = true)]
async fn watchdog_keeps_upstream_abort_distinct() {
    let controller = AbortController::new();
    let upstream = controller.signal();
    let watchdog = idle_watchdog(Some(&upstream), 100.0, "LLM_STREAM_IDLE_TIMEOUT").unwrap();
    controller.abort_with(std::io::Error::other("caller cancelled"));
    assert!(watchdog.signal.aborted());
    assert!(classify(&watchdog.signal, Some("LLM_STREAM_IDLE_TIMEOUT")).is_none());
    elapse(1_000).await;
    assert_eq!(
        watchdog
            .signal
            .reason()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .to_string(),
        "caller cancelled"
    );
}
#[tokio::test(start_paused = true)]
async fn watchdog_clears_outstanding_arm_on_disposal() {
    let (tx, pending) = oneshot::channel();
    let watchdog = idle_watchdog(None, 100.0, "LLM_STREAM_IDLE_TIMEOUT").unwrap();
    let next = watchdog.next(pending);
    watchdog.dispose();
    elapse(1_000).await;
    assert!(!watchdog.signal.aborted());
    tx.send((true, ())).unwrap();
    assert_eq!(next.await.unwrap().unwrap(), (true, ()));
    let done = future::ready((true, ()));
    assert!(
        watchdog
            .next(done)
            .await
            .unwrap_err()
            .to_string()
            .contains("disposed")
    );
    watchdog.dispose();
}
#[tokio::test(start_paused = true)]
async fn watchdog_rejects_demand_that_never_settles() {
    let watchdog = idle_watchdog(None, 100.0, "LLM_STREAM_IDLE_TIMEOUT").unwrap();
    let pending = future::pending::<()>();
    let next = watchdog.next(pending);
    elapse(100).await;
    assert_same_timeout(next.await.unwrap_err(), &watchdog.signal);
    assert_eq!(
        classify(&watchdog.signal, Some("LLM_STREAM_IDLE_TIMEOUT"))
            .unwrap()
            .timeout_ms,
        100.0
    );
}
#[tokio::test(start_paused = true)]
async fn watchdog_rejects_invalid_bounds_and_concurrent_demand() {
    for value in [0.0, f64::NAN] {
        assert!(
            idle_watchdog(None, value, "IDLE")
                .err()
                .unwrap()
                .to_string()
                .contains("positive finite")
        );
    }
    assert!(
        idle_watchdog(None, MAX_TIMER_DELAY_MS as f64 + 1.0, "IDLE")
            .err()
            .unwrap()
            .to_string()
            .contains(&format!("no greater than {MAX_TIMER_DELAY_MS}"))
    );
    let (tx, pending) = oneshot::channel();
    let watchdog = idle_watchdog(None, 100.0, "IDLE").unwrap();
    let first = watchdog.next(pending);
    let another = future::pending::<()>();
    assert!(
        watchdog
            .next(another)
            .await
            .unwrap_err()
            .to_string()
            .contains("already outstanding")
    );
    tx.send((true, ())).unwrap();
    assert_eq!(first.await.unwrap().unwrap(), (true, ()));
}

#[tokio::test(start_paused = true)]
async fn upstream_abort_does_not_shorten_outstanding_demand() {
    let controller = AbortController::new();
    let upstream = controller.signal();
    let watchdog = idle_watchdog(Some(&upstream), 100.0, "IDLE").unwrap();
    let pending = future::pending::<()>();
    let next = watchdog.next(pending);
    controller.abort_with(std::io::Error::other("caller cancelled"));
    assert!(watchdog.signal.aborted());
    tokio::pin!(next);
    tokio::select! {
        _ = &mut next => panic!("Upstream must not settle the demand"),
        _ = elapse(99) => {}
    }
    elapse(1).await;
    let IdleWatchdogError::Timeout(reason) = next.await.unwrap_err() else {
        panic!("Expected timeout")
    };
    assert_eq!(reason.downcast_ref::<TimeoutReason>().unwrap().code, "IDLE");
    assert!(classify(&watchdog.signal, None).is_none());
}

#[tokio::test(start_paused = true)]
async fn settled_demand_disarms_even_before_caller_reads_result() {
    let (tx, demand) = oneshot::channel();
    let watchdog = idle_watchdog(None, 100.0, "IDLE").unwrap();
    let next = watchdog.next(demand);
    tx.send(1).unwrap();
    elapse(0).await;
    elapse(1_000).await;
    assert!(!watchdog.signal.aborted());
    assert_eq!(next.await.unwrap().unwrap(), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_watchdog_does_not_settle_outstanding_demand() {
    let (tx, rx) = oneshot::channel();
    let watchdog = idle_watchdog(None, 100.0, "IDLE").unwrap();
    let next = watchdog.next(rx);
    elapse(0).await;
    drop(watchdog);
    elapse(1_000).await;
    tx.send(42).unwrap();
    assert_eq!(next.await.unwrap().unwrap(), 42);
}

struct NeverResolvingDemand {
    dropped: Arc<AtomicBool>,
}

impl Future for NeverResolvingDemand {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
}

impl Drop for NeverResolvingDemand {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_drops_never_resolving_demand_within_bound() {
    let dropped = Arc::new(AtomicBool::new(false));
    let watchdog = idle_watchdog(None, 100.0, "IDLE").unwrap();
    let next = watchdog.next(NeverResolvingDemand {
        dropped: dropped.clone(),
    });
    elapse(99).await;
    assert!(!dropped.load(Ordering::SeqCst));
    elapse(1).await;
    let error = tokio::time::timeout(Duration::from_millis(1), next)
        .await
        .unwrap()
        .unwrap_err();
    assert_same_timeout(error, &watchdog.signal);
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn dropping_watchdog_and_next_does_not_detach_demand() {
    let dropped = Arc::new(AtomicBool::new(false));
    let watchdog = idle_watchdog(None, 100.0, "IDLE").unwrap();
    let next = watchdog.next(NeverResolvingDemand {
        dropped: dropped.clone(),
    });
    elapse(0).await;
    assert!(!dropped.load(Ordering::SeqCst));
    drop(next);
    drop(watchdog);
    elapse(0).await;
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn controller_starts_unaborted_and_is_send_sync() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<AbortController>();
    send_sync::<AbortSignal>();
    let controller = AbortController::new();
    let signal = controller.signal();
    assert!(!signal.aborted());
    assert!(signal.reason().is_none());
    assert!(signal.throw_if_aborted().is_ok());
    assert!(format!("{signal:?}").contains("AbortSignal"));
}
#[test]
fn first_abort_wins_and_default_reason_is_abort_error() {
    let controller = AbortController::default();
    controller.abort();
    let reason = controller.signal().reason().unwrap();
    assert!(is_abort_error(&reason));
    assert_eq!(reason.to_string(), "This operation was aborted");
    controller.abort_with(std::io::Error::other("late"));
    controller.abort();
    assert!(reason.ptr_eq(&controller.signal().reason().unwrap()));
    let named = AbortController::new();
    named.abort_with(std::io::Error::other("first"));
    let first = named.signal().reason().unwrap();
    named.abort();
    assert!(first.ptr_eq(&named.signal().reason().unwrap()));
}
#[tokio::test]
async fn reason_identity_survives_throw_wait_question_mark_and_boxing() {
    let controller = AbortController::new();
    controller.abort_with(std::io::Error::other("reason"));
    let signal = controller.signal();
    let reason = signal.reason().unwrap();
    assert!(reason.ptr_eq(&signal.reason().unwrap()));
    assert!(reason.ptr_eq(&signal.throw_if_aborted().unwrap_err()));
    assert!(reason.ptr_eq(&signal.cancelled().await));
    fn operation(signal: &AbortSignal) -> Result<(), BoxError> {
        signal.throw_if_aborted()?;
        Ok(())
    }
    assert!(reason.ptr_eq(&SharedError::new(operation(&signal).unwrap_err())));
}
#[test]
fn forwarding_keeps_the_same_reason() {
    let a = AbortController::new();
    let b = AbortController::new();
    a.abort_with(std::io::Error::other("reason"));
    b.abort_with(a.signal().reason().unwrap());
    assert!(
        a.signal()
            .reason()
            .unwrap()
            .ptr_eq(&b.signal().reason().unwrap())
    );
}
#[tokio::test]
async fn cancellation_wakes_every_waiter() {
    let controller = AbortController::new();
    let mut waiters = Vec::new();
    for _ in 0..8 {
        let signal = controller.signal();
        waiters.push(tokio::spawn(async move { signal.cancelled().await }));
    }
    tokio::task::yield_now().await;
    controller.abort();
    let reason = controller.signal().reason().unwrap();
    for waiter in waiters {
        assert!(reason.ptr_eq(&waiter.await.unwrap()));
    }
}
#[test]
fn any_adopts_preaborted_sources_in_input_order() {
    let a = AbortController::new();
    let b = AbortController::new();
    a.abort();
    b.abort();
    let result = AbortSignal::any([b.signal(), a.signal()]);
    assert!(result.aborted());
    assert!(
        result
            .reason()
            .unwrap()
            .ptr_eq(&b.signal().reason().unwrap())
    );
}
#[tokio::test]
async fn any_uses_the_first_abort_in_time_not_input_order() {
    let a = AbortController::new();
    let b = AbortController::new();
    let result = AbortSignal::any([b.signal(), a.signal()]);
    a.abort_with(std::io::Error::other("first"));
    b.abort_with(std::io::Error::other("second"));
    assert!(
        result
            .cancelled()
            .await
            .ptr_eq(&a.signal().reason().unwrap())
    );
}
#[tokio::test(start_paused = true)]
async fn empty_any_never_aborts() {
    let signal = AbortSignal::any([]);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), signal.cancelled())
            .await
            .is_err()
    );
    assert!(!signal.aborted());
}
#[tokio::test(start_paused = true)]
async fn timeout_truncates_delay_and_has_dom_timeout_reason() {
    let signal = AbortSignal::timeout(10.9);
    elapse(9).await;
    assert!(!signal.aborted());
    elapse(1).await;
    assert!(signal.aborted());
    let reason = signal.cancelled().await;
    assert!(reason.downcast_ref::<DomTimeoutError>().is_some());
    assert_eq!(
        reason.to_string(),
        "The operation was aborted due to timeout"
    );
    assert!(reason.ptr_eq(&signal.reason().unwrap()));
    for delay in [0.0, -1.0, 0.9] {
        let signal = AbortSignal::timeout(delay);
        assert!(!signal.aborted());
        elapse(1).await;
        assert!(signal.aborted());
    }
}
#[tokio::test(start_paused = true)]
async fn timeout_wakes_a_waiter_and_any_observes_it() {
    let early = AbortSignal::timeout(10.0);
    let late = AbortSignal::timeout(20.0);
    let result = AbortSignal::any([late, early.clone()]);
    let reason = result.cancelled().await;
    assert!(reason.ptr_eq(&early.reason().unwrap()));
}
#[tokio::test(start_paused = true)]
async fn any_preserves_the_earlier_elapsed_timer_before_observation() {
    let early = AbortSignal::timeout(10.0);
    let late = AbortSignal::timeout(20.0);
    let result = AbortSignal::any([late, early.clone()]);
    elapse(30).await;
    assert!(result.reason().unwrap().ptr_eq(&early.reason().unwrap()));
}
#[tokio::test(start_paused = true)]
async fn several_unobserved_timers_choose_the_earliest_in_any_input_order() {
    for early_last in [false, true] {
        let early = AbortSignal::timeout(10.0);
        let middle = AbortSignal::timeout(20.0);
        let late = AbortSignal::timeout(30.0);
        let sources = if early_last {
            [late, middle, early.clone()]
        } else {
            [late, early.clone(), middle]
        };
        let result = AbortSignal::any(sources);
        elapse(40).await;
        assert!(result.reason().unwrap().ptr_eq(&early.reason().unwrap()));
    }
}
#[tokio::test(start_paused = true)]
async fn an_elapsed_timeout_precedes_a_later_controller_abort() {
    let early = AbortSignal::timeout(10.0);
    let controller = AbortController::new();
    let result = AbortSignal::any([controller.signal(), early.clone()]);
    elapse(20).await;
    controller.abort();
    assert!(result.reason().unwrap().ptr_eq(&early.reason().unwrap()));
}
#[tokio::test]
async fn token_bridge_observes_cancellation_without_waiters_and_keeps_identity() {
    let token = tokio_util::sync::CancellationToken::new();
    let signal = AbortSignal::from_token(token.clone());
    assert!(!signal.aborted());
    token.cancel();
    assert!(signal.aborted());
    let reason = signal.reason().unwrap();
    assert!(is_abort_error(&reason));
    assert!(reason.ptr_eq(&signal.reason().unwrap()));
    assert!(reason.ptr_eq(&signal.cancelled().await));
}
#[tokio::test]
async fn token_bridge_wakes_waiters() {
    let token = tokio_util::sync::CancellationToken::new();
    let signal = AbortSignal::from_token(token.clone());
    let waiting = {
        let signal = signal.clone();
        tokio::spawn(async move { signal.cancelled().await })
    };
    tokio::task::yield_now().await;
    token.cancel();
    assert!(waiting.await.unwrap().ptr_eq(&signal.reason().unwrap()));
}
#[tokio::test]
async fn token_and_controller_can_each_win_any() {
    for token_first in [true, false] {
        let token = tokio_util::sync::CancellationToken::new();
        let bridged = AbortSignal::from_token(token.clone());
        let controller = AbortController::new();
        let result = AbortSignal::any([controller.signal(), bridged.clone()]);
        if token_first {
            token.cancel();
            controller.abort();
        } else {
            controller.abort();
            token.cancel();
        }
        let expected = if token_first {
            bridged.reason().unwrap()
        } else {
            controller.signal().reason().unwrap()
        };
        assert!(result.cancelled().await.ptr_eq(&expected));
    }
}
#[test]
fn any_keeps_token_cancellation_order_without_an_executor_or_observers() {
    let first = tokio_util::sync::CancellationToken::new();
    let second = tokio_util::sync::CancellationToken::new();
    let first_signal = AbortSignal::from_token(first.clone());
    let second_signal = AbortSignal::from_token(second.clone());
    let result = AbortSignal::any([second_signal, first_signal.clone()]);
    first.cancel();
    second.cancel();
    assert!(
        result
            .reason()
            .unwrap()
            .ptr_eq(&first_signal.reason().unwrap())
    );
}
#[test]
fn abort_error_recognition_is_transparent_but_never_walks_sources() {
    let error = AbortError::new("custom");
    assert_eq!(error.to_string(), "custom");
    assert!(is_abort_error(&error));
    assert!(is_abort_error(&SharedError::new(error)));
    #[derive(Debug, thiserror::Error)]
    #[error("outer")]
    struct Outer {
        #[source]
        cause: AbortError,
    }
    assert!(!is_abort_error(&Outer {
        cause: AbortError::default()
    }));
    assert!(!is_abort_error(&SharedError::new(Outer {
        cause: AbortError::default()
    })));
    assert!(!is_abort_error(&DomTimeoutError));
}
#[test]
fn timeout_of_classifies_both_signal_and_caught_reason() {
    let controller = AbortController::new();
    controller.abort_with(TimeoutReason::new("LIMIT", 12.0));
    let signal = controller.signal();
    let signal_reason = signal.reason();
    assert_eq!(
        timeout_of(signal_reason.as_ref(), Some("LIMIT"))
            .unwrap()
            .timeout_ms,
        12.0
    );
    let caught = signal.throw_if_aborted().unwrap_err();
    assert_eq!(timeout_of(Some(&caught), None).unwrap().code, "LIMIT");
    assert!(timeout_of(Some(&caught), Some("OTHER")).is_none());
}
#[tokio::test(start_paused = true)]
async fn dropping_deadline_disposes_timer_even_if_signal_survives() {
    let deadline = deadline(None, 10.0, "DEADLINE").unwrap();
    let signal = deadline.signal.clone();
    drop(deadline);
    elapse(100).await;
    assert!(!signal.aborted());
}
