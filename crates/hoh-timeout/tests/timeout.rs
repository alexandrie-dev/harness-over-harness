use hoh_timeout::{
    AbortSignal, IdleWatchdogError, MAX_TIMER_DELAY_MS, TimeoutReason, clamp_timeout, deadline,
    idle_watchdog, timeout_of,
};
use std::{
    any::Any,
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
    timeout_of(reason.as_deref(), code).cloned()
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
    let erased: Arc<dyn Any + Send + Sync> = reason;
    assert!(Arc::ptr_eq(&erased, &signal.reason().unwrap()));
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
    let upstream = AbortSignal::new();
    let d = deadline(Some(&upstream), 60_000.0, "BASH_TIMEOUT").unwrap();
    upstream.abort("user cancelled");
    assert!(d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[tokio::test(start_paused = true)]
async fn cancel_wins_before_timeout() {
    let upstream = AbortSignal::new();
    let d = deadline(Some(&upstream), 100.0, "BASH_TIMEOUT").unwrap();
    upstream.abort("user cancelled");
    elapse(200).await;
    assert!(d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[tokio::test(start_paused = true)]
async fn timeout_wins_before_cancellation() {
    let upstream = AbortSignal::new();
    let d = deadline(Some(&upstream), 100.0, "WEB_FETCH_TIMEOUT").unwrap();
    elapse(150).await;
    assert!(d.signal.aborted());
    assert_eq!(classify(&d.signal, None).unwrap().code, "WEB_FETCH_TIMEOUT");
    upstream.abort("too late");
    assert_eq!(classify(&d.signal, None).unwrap().code, "WEB_FETCH_TIMEOUT");
}
#[tokio::test(start_paused = true)]
async fn forwards_preaborted_upstream_immediately() {
    let upstream = AbortSignal::new();
    upstream.abort("already gone");
    let d = deadline(Some(&upstream), 60_000.0, "BASH_TIMEOUT").unwrap();
    assert!(d.signal.aborted());
    assert!(classify(&d.signal, None).is_none());
}
#[tokio::test(start_paused = true)]
async fn sentinel_arms_no_timer_and_forwards_upstream() {
    let upstream = AbortSignal::new();
    let d = deadline(Some(&upstream), 0.0, "BASH_TIMEOUT").unwrap();
    elapse(1_000_000).await;
    assert!(!d.signal.aborted());
    upstream.abort("kill");
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
    let reason = TimeoutReason::new("WEB_FETCH_TIMEOUT", 50.0);
    assert!(std::ptr::eq(
        timeout_of(Some(&reason), None).unwrap(),
        &reason
    ));
}
#[test]
fn returns_none_for_nontimeout_reason() {
    assert!(timeout_of(Some(&std::io::Error::other("other")), None).is_none());
    assert!(timeout_of(Some(&"user cancelled"), None).is_none());
    assert!(timeout_of(None, None).is_none());
}
#[test]
fn matches_only_requested_code() {
    let reason = TimeoutReason::new("BASH_TIMEOUT", 100.0);
    assert!(std::ptr::eq(
        timeout_of(Some(&reason), Some("BASH_TIMEOUT")).unwrap(),
        &reason
    ));
    assert!(timeout_of(Some(&reason), Some("WEB_FETCH_TIMEOUT")).is_none());
}
#[tokio::test(start_paused = true)]
async fn does_not_misclassify_outer_timeout_as_inner_code() {
    let outer = AbortSignal::new();
    outer.abort(TimeoutReason::new("OUTER_TIMEOUT", 30.0));
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
    let upstream = AbortSignal::new();
    let watchdog = idle_watchdog(Some(&upstream), 100.0, "LLM_STREAM_IDLE_TIMEOUT").unwrap();
    upstream.abort("caller cancelled");
    assert!(watchdog.signal.aborted());
    assert!(classify(&watchdog.signal, Some("LLM_STREAM_IDLE_TIMEOUT")).is_none());
    elapse(1_000).await;
    assert_eq!(
        watchdog.signal.reason().unwrap().downcast_ref::<&str>(),
        Some(&"caller cancelled")
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
    let upstream = AbortSignal::new();
    let watchdog = idle_watchdog(Some(&upstream), 100.0, "IDLE").unwrap();
    let pending = future::pending::<()>();
    let next = watchdog.next(pending);
    upstream.abort("caller cancelled");
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
    assert_eq!(reason.code, "IDLE");
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
