//! Shared timeout arithmetic, signal fusion and classification; notification only.
use std::{
    any::Any,
    future::Future,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle, time::Instant};

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

/// Classify a bare reason, optionally scoped to a capability-owned code.
pub fn timeout_of<'a>(
    reason: Option<&'a (dyn Any + Send + Sync)>,
    code: Option<&str>,
) -> Option<&'a TimeoutReason> {
    let reason = reason?.downcast_ref::<TimeoutReason>()?;
    if code.is_none_or(|code| code == reason.code) {
        Some(reason)
    } else {
        None
    }
}

type Reason = Arc<dyn Any + Send + Sync>;
struct SignalState {
    reason: Option<Reason>,
    followers: Vec<Weak<Mutex<SignalState>>>,
    changed: watch::Sender<()>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Provisional abort signal, backed by Tokio notifications; first reason wins.
#[derive(Clone)]
pub struct AbortSignal {
    inner: Arc<Mutex<SignalState>>,
}

impl Default for AbortSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl AbortSignal {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SignalState {
                reason: None,
                followers: vec![],
                changed: watch::channel(()).0,
            })),
        }
    }
    pub fn aborted(&self) -> bool {
        lock(&self.inner).reason.is_some()
    }
    pub fn reason(&self) -> Option<Reason> {
        lock(&self.inner).reason.clone()
    }
    pub fn abort(&self, reason: impl Any + Send + Sync) {
        self.abort_shared(Arc::new(reason));
    }

    fn abort_shared(&self, reason: Reason) {
        let followers = {
            let mut state = lock(&self.inner);
            if state.reason.is_some() {
                return;
            }
            state.reason = Some(reason.clone());
            state.changed.send_replace(());
            std::mem::take(&mut state.followers)
        };
        for follower in followers {
            if let Some(inner) = follower.upgrade() {
                Self { inner }.abort_shared(reason.clone());
            }
        }
    }

    fn fuse(upstream: Option<&Self>, timer: &Self) -> Self {
        let signal = Self::new();
        for source in upstream.into_iter().chain(std::iter::once(timer)) {
            let mut state = lock(&source.inner);
            if let Some(reason) = &state.reason {
                signal.abort_shared(reason.clone());
            } else {
                state
                    .followers
                    .retain(|follower| follower.strong_count() > 0);
                state.followers.push(Arc::downgrade(&signal.inner));
            }
        }
        signal
    }

    /// Wait for cancellation, including an already-aborted signal.
    pub async fn cancelled(&self) -> Reason {
        let mut changed = lock(&self.inner).changed.subscribe();
        loop {
            if let Some(reason) = self.reason() {
                return reason;
            }
            // The sender lives in self for the entire wait.
            let _ = changed.changed().await;
        }
    }
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
            signal: upstream.cloned().unwrap_or_default(),
            timer: None,
        });
    }
    assert_timer_delay(timeout_ms, "deadline timeoutMs")?;
    let timer_signal = AbortSignal::new();
    let signal = AbortSignal::fuse(upstream, &timer_signal);
    let reason = TimeoutReason::new(code, timeout_ms);
    let at = timer_instant(timeout_ms);
    let timer = tokio::spawn(async move {
        tokio::time::sleep_until(at).await;
        timer_signal.abort(reason);
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
    Timeout(Arc<TimeoutReason>),
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
    timeout: AbortSignal,
    reason: Arc<TimeoutReason>,
    state: Arc<Mutex<WatchdogState>>,
}

struct DemandGuard {
    state: Arc<Mutex<WatchdogState>>,
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
        let at = timer_instant(self.reason.timeout_ms);
        let shared_state = Arc::downgrade(&self.state);
        let timeout = self.timeout.clone();
        let reason: Reason = self.reason.clone();
        state.timer = Some(tokio::spawn(async move {
            tokio::time::sleep_until(at).await;
            if let Some(shared) = shared_state.upgrade() {
                let state = lock(&shared);
                if !state.disposed && state.outstanding && state.generation == generation {
                    timeout.abort_shared(reason);
                }
            }
        }));
    }

    /// Start one iterator demand immediately, like an eager JavaScript promise.
    /// On timeout the demand keeps running and must release resources via the signal.
    pub fn next<F>(
        &self,
        demand: F,
    ) -> impl Future<Output = Result<F::Output, IdleWatchdogError>> + use<F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let mut expired = lock(&self.timeout.inner).changed.subscribe();
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
            tokio::spawn(async move {
                let mut demand = Box::pin(demand);
                let result = tokio::select! {
                    biased;
                    _ = expired.changed() => {
                        // Promise.race abandons rather than cancels the original demand.
                        // Its owner must stop transport work by observing signal.
                        tokio::spawn(demand);
                        Err(IdleWatchdogError::Timeout(reason))
                    },
                    value = &mut demand => Ok(value),
                };
                drop(guard);
                result
            })
        });
        async move { task?.await? }
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
    let timeout = AbortSignal::new();
    Ok(IdleWatchdog {
        signal: AbortSignal::fuse(upstream, &timeout),
        timeout,
        reason: Arc::new(TimeoutReason::new(code, timeout_ms)),
        state: Arc::new(Mutex::new(WatchdogState::default())),
    })
}
