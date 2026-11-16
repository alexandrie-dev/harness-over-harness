//! Shared timeout arithmetic, signal fusion and classification; notification only.
use hoh_error::SharedError;
use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{task::JoinHandle, time::Instant};
mod abort;
pub use abort::{AbortController, AbortError, AbortSignal, DomTimeoutError, is_abort_error};

/// Internal reason carrying a capability-owned code and elapsed deadline.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{code} after {}ms", js_number(*.timeout_ms))]
pub struct TimeoutReason {
    pub code: String,
    pub timeout_ms: f64,
}

impl TimeoutReason {
    pub fn new(code: impl Into<String>, timeout_ms: f64) -> Self {
        Self {
            code: code.into(),
            timeout_ms,
        }
    }
}

pub const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

fn js_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == f64::INFINITY {
        return "Infinity".into();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if value.abs() >= 1e21 || value.abs() < 1e-6 {
        let scientific = format!("{value:e}");
        if let Some((mantissa, exponent)) = scientific.split_once('e') {
            return format!(
                "{mantissa}e{}{exponent}",
                if exponent.starts_with('-') { "" } else { "+" }
            );
        }
    }
    value.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeoutError {
    #[error("{0} must be a positive finite number")]
    InvalidHint(String),
    #[error("{0} must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}")]
    InvalidDelay(String),
    #[error("idleWatchdog is disposed")]
    Disposed,
    #[error("idleWatchdog next is already outstanding")]
    Outstanding,
}

fn assert_timer_delay(timeout_ms: f64, name: &str) -> Result<(), TimeoutError> {
    if !timeout_ms.is_finite() || timeout_ms <= 0.0 || timeout_ms > MAX_TIMER_DELAY_MS as f64 {
        return Err(TimeoutError::InvalidDelay(name.into()));
    }
    Ok(())
}

/// Validate an optional hint, fill the default, then cap it (default name: timeoutMs).
pub fn clamp_timeout(
    requested: Option<f64>,
    def: f64,
    max: f64,
    name: Option<&str>,
) -> Result<f64, TimeoutError> {
    if requested.is_some_and(|value| !value.is_finite() || value <= 0.0) {
        return Err(TimeoutError::InvalidHint(
            name.unwrap_or("timeoutMs").into(),
        ));
    }
    let value = requested.unwrap_or(def);
    // Math.min propagates NaN, whereas f64::min ignores one NaN operand.
    Ok(if value.is_nan() || max.is_nan() {
        f64::NAN
    } else if value == 0.0 && max == 0.0 && (value.is_sign_negative() || max.is_sign_negative()) {
        -0.0
    } else {
        value.min(max)
    })
}

/// Classify a signal's or caught error's shared reason, optionally scoped to a code.
/// For a signal: `let reason = signal.reason(); timeout_of(reason.as_ref(), code)`.
/// For a caught `BoxError`: use `SharedError::of(error.as_ref())` as the first argument.
pub fn timeout_of<'a>(
    reason: Option<&'a SharedError>,
    code: Option<&str>,
) -> Option<&'a TimeoutReason> {
    let reason = reason?.downcast_ref::<TimeoutReason>()?;
    if code.is_none_or(|code| code == reason.code) {
        Some(reason)
    } else {
        None
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn timer_instant(timeout_ms: f64) -> Instant {
    // Node's setTimeout truncates fractional delays and clamps values below 1ms.
    Instant::now() + Duration::from_millis(timeout_ms.max(1.0) as u64)
}

pub struct Deadline {
    pub signal: AbortSignal,
    timer: Option<JoinHandle<()>>,
}

impl Deadline {
    pub fn dispose(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
    }
}
impl Drop for Deadline {
    fn drop(&mut self) {
        self.dispose();
    }
}

pub fn deadline(
    upstream: Option<&AbortSignal>,
    timeout_ms: f64,
    code: &str,
) -> Result<Deadline, TimeoutError> {
    if timeout_ms <= 0.0 {
        return Ok(Deadline {
            signal: upstream
                .cloned()
                .unwrap_or_else(|| AbortController::new().signal()),
            timer: None,
        });
    }
    assert_timer_delay(timeout_ms, "deadline timeoutMs")?;
    let timer_signal = AbortController::new();
    let signal = AbortSignal::any(
        upstream
            .into_iter()
            .cloned()
            .chain(std::iter::once(timer_signal.signal())),
    );
    let reason = TimeoutReason::new(code, timeout_ms);
    let at = timer_instant(timeout_ms);
    let timer = tokio::spawn(async move {
        tokio::time::sleep_until(at).await;
        timer_signal.abort_with(reason);
    });
    Ok(Deadline {
        signal,
        timer: Some(timer),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum IdleWatchdogError {
    #[error(transparent)]
    State(#[from] TimeoutError),
    #[error(transparent)]
    Timeout(SharedError),
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
}

#[derive(Default)]
struct WatchdogState {
    timer: Option<JoinHandle<()>>,
    generation: u64,
    outstanding: bool,
    disposed: bool,
}
impl WatchdogState {
    fn clear_timer(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
    }
}

pub struct IdleWatchdog {
    pub signal: AbortSignal,
    timeout: Arc<AbortController>,
    reason: SharedError,
    timeout_ms: f64,
    state: Arc<Mutex<WatchdogState>>,
}

struct DemandGuard {
    state: Arc<Mutex<WatchdogState>>,
}

struct DemandTask<T> {
    task: JoinHandle<T>,
}

impl<T> Drop for DemandTask<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Drop for DemandGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        state.clear_timer();
        state.outstanding = false;
    }
}

impl IdleWatchdog {
    fn arm(&self, state: &mut WatchdogState) {
        state.clear_timer();
        let generation = state.generation;
        let at = timer_instant(self.timeout_ms);
        let shared_state = Arc::downgrade(&self.state);
        let timeout = self.timeout.clone();
        let reason = self.reason.clone();
        state.timer = Some(tokio::spawn(async move {
            tokio::time::sleep_until(at).await;
            if let Some(shared) = shared_state.upgrade() {
                let should_abort = {
                    let state = lock(&shared);
                    !state.disposed && state.outstanding && state.generation == generation
                };
                if should_abort {
                    timeout.abort_with(reason);
                }
            }
        }));
    }

    /// Start one iterator demand immediately, like an eager JavaScript promise.
    /// Timeout or dropping the returned future drops the demand rather than detaching it.
    pub fn next<F>(
        &self,
        demand: F,
    ) -> impl Future<Output = Result<F::Output, IdleWatchdogError>> + use<F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let mut expired = self.timeout.signal().subscribe();
        let guard = {
            let mut state = lock(&self.state);
            if state.disposed {
                Err(TimeoutError::Disposed)
            } else if state.outstanding {
                Err(TimeoutError::Outstanding)
            } else {
                state.outstanding = true;
                self.arm(&mut state);
                Ok(DemandGuard {
                    state: self.state.clone(),
                })
            }
        };
        let reason = self.reason.clone();
        let task = guard.map(|guard| {
            DemandTask {
                task: tokio::spawn(async move {
                    let mut demand = Box::pin(demand);
                    let result = tokio::select! {
                        biased;
                        expired = expired.changed() => match expired {
                            Ok(()) => Err(IdleWatchdogError::Timeout(reason)),
                            // Disposal may drop the sender; it must not settle demand.
                            Err(_) => Ok(demand.await),
                        },
                        value = &mut demand => Ok(value),
                    };
                    drop(guard);
                    result
                }),
            }
        });
        async move {
            let mut task = task?;
            (&mut task.task).await?
        }
    }

    pub fn pulse(&self) {
        let mut state = lock(&self.state);
        if state.disposed || !state.outstanding {
            return;
        }
        self.arm(&mut state);
    }

    pub fn dispose(&self) {
        let mut state = lock(&self.state);
        if state.disposed {
            return;
        }
        state.disposed = true;
        state.clear_timer();
    }
}
impl Drop for IdleWatchdog {
    fn drop(&mut self) {
        self.dispose();
    }
}

pub fn idle_watchdog(
    upstream: Option<&AbortSignal>,
    timeout_ms: f64,
    code: &str,
) -> Result<IdleWatchdog, TimeoutError> {
    assert_timer_delay(timeout_ms, "idleWatchdog timeoutMs")?;
    let timeout = Arc::new(AbortController::new());
    Ok(IdleWatchdog {
        signal: AbortSignal::any(
            upstream
                .into_iter()
                .cloned()
                .chain(std::iter::once(timeout.signal())),
        ),
        timeout,
        reason: SharedError::new(TimeoutReason::new(code, timeout_ms)),
        timeout_ms,
        state: Arc::new(Mutex::new(WatchdogState::default())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        task::{Context, Poll, Wake, Waker},
    };

    struct LockCheckingWake {
        state: Arc<Mutex<WatchdogState>>,
        calls: AtomicUsize,
        unlocked: AtomicBool,
    }
    impl Wake for LockCheckingWake {
        fn wake(self: Arc<Self>) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.state.try_lock().is_err() {
                self.unlocked.store(false, Ordering::SeqCst);
            }
        }
    }
    #[tokio::test(start_paused = true)]
    async fn watchdog_wakes_observers_after_releasing_its_state_lock() {
        let watchdog = idle_watchdog(None, 10.0, "IDLE").unwrap();
        let demand = watchdog.next(std::future::pending::<()>());
        let wake = Arc::new(LockCheckingWake {
            state: watchdog.state.clone(),
            calls: AtomicUsize::new(0),
            unlocked: AtomicBool::new(true),
        });
        let waker = Waker::from(wake.clone());
        let mut cancelled = Box::pin(watchdog.signal.cancelled());
        assert!(matches!(
            cancelled.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert!(wake.calls.load(Ordering::SeqCst) > 0);
        assert!(wake.unlocked.load(Ordering::SeqCst));
        assert!(matches!(demand.await, Err(IdleWatchdogError::Timeout(_))));
    }
}
