use super::{MAX_TIMER_DELAY_MS, lock, timer_instant};
use hoh_error::{BoxError, SharedError};
use std::{
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll, Wake, Waker},
};
use tokio::{
    sync::{Notify, futures::OwnedNotified, watch},
    time::{Instant, Sleep},
};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct AbortError {
    message: String,
    #[source]
    cause: Option<SharedError>,
}
impl AbortError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
        }
    }
    /// Node's abortable waits add the signal's reason as the AbortError cause.
    pub fn with_cause(mut self, cause: impl Into<BoxError>) -> Self {
        self.cause = Some(SharedError::new(cause));
        self
    }
}
impl Default for AbortError {
    fn default() -> Self {
        Self::new("This operation was aborted")
    }
}

pub fn is_abort_error(error: &(dyn Error + 'static)) -> bool {
    error.is::<AbortError>()
        || SharedError::of(error)
            .is_some_and(|reason| reason.downcast_ref::<AbortError>().is_some())
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("The operation was aborted due to timeout")]
pub struct DomTimeoutError;

enum Origin {
    Controller,
    Timer {
        at: Instant,
        reason: SharedError,
    },
    Token {
        token: CancellationToken,
        reason: SharedError,
    },
}
struct SignalState {
    reason: Option<SharedError>,
    aborted_at: Option<Instant>,
    settling: bool,
    followers: Vec<Weak<SignalInner>>,
    changed: watch::Sender<()>,
}
struct SignalInner {
    state: Mutex<SignalState>,
    notify: Arc<Notify>,
    origin: Origin,
    sources: Vec<AbortSignal>,
    token_registration: Mutex<Option<Pin<Box<WaitForCancellationFutureOwned>>>>,
}
struct TokenWake {
    signal: Weak<SignalInner>,
}
impl Wake for TokenWake {
    fn wake(self: Arc<Self>) {
        if let Some(inner) = self.signal.upgrade() {
            AbortSignal { inner }.refresh();
        }
    }
}
impl Drop for SignalInner {
    fn drop(&mut self) {
        for source in &self.sources {
            lock(&source.inner.state)
                .followers
                .retain(|follower| follower.as_ptr() != std::ptr::from_ref(self));
        }
    }
}

#[derive(Clone)]
pub struct AbortSignal {
    inner: Arc<SignalInner>,
}
impl fmt::Debug for AbortSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AbortSignal")
            .field("reason", &self.reason())
            .finish()
    }
}
impl AbortSignal {
    fn pending(origin: Origin, sources: Vec<Self>) -> Self {
        Self {
            inner: Arc::new(SignalInner {
                state: Mutex::new(SignalState {
                    reason: None,
                    aborted_at: None,
                    settling: false,
                    followers: vec![],
                    changed: watch::channel(()).0,
                }),
                notify: Arc::new(Notify::new()),
                origin,
                sources,
                token_registration: Mutex::new(None),
            }),
        }
    }
    pub fn aborted(&self) -> bool {
        self.reason().is_some()
    }
    pub fn reason(&self) -> Option<SharedError> {
        self.refresh();
        lock(&self.inner.state).reason.clone()
    }
    pub fn throw_if_aborted(&self) -> Result<(), SharedError> {
        match self.reason() {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }
    fn refresh(&self) {
        self.refresh_before(None);
    }
    fn event(&self) -> Option<(SharedError, Instant)> {
        let state = lock(&self.inner.state);
        state.reason.clone().zip(state.aborted_at)
    }
    fn refresh_before(&self, cutoff: Option<Instant>) {
        {
            let state = lock(&self.inner.state);
            if state.reason.is_some() || state.settling {
                return;
            }
        }
        let now = Instant::now();
        let event = match &self.inner.origin {
            Origin::Controller => None,
            Origin::Timer { at, reason } => (now >= *at
                && cutoff.is_none_or(|cutoff| *at < cutoff))
            .then(|| (reason.clone(), *at)),
            Origin::Token { token, reason } => (cutoff.is_none_or(|cutoff| now < cutoff)
                && token.is_cancelled())
            .then(|| (reason.clone(), now)),
        };
        if let Some((reason, at)) = event {
            self.abort_shared_at(reason, at);
        } else {
            for source in &self.inner.sources {
                source.refresh_before(cutoff);
                if let Some((reason, at)) = source.event()
                    && cutoff.is_none_or(|cutoff| at < cutoff)
                {
                    self.abort_shared_at(reason, at);
                    break;
                }
            }
        }
    }
    fn abort_shared(&self, reason: SharedError) {
        self.abort_shared_at(reason, Instant::now());
    }
    fn abort_shared_at(&self, reason: SharedError, at: Instant) {
        let followers = {
            let mut state = lock(&self.inner.state);
            if state.reason.is_some() || state.settling {
                return;
            }
            state.settling = true;
            state
                .followers
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        // Only an earlier event may displace this one, including unobserved elapsed timers.
        for follower in &followers {
            Self {
                inner: follower.clone(),
            }
            .refresh_before(Some(at));
        }
        let changed = {
            let mut state = lock(&self.inner.state);
            state.reason = Some(reason.clone());
            state.aborted_at = Some(at);
            state.settling = false;
            state.followers.clear();
            state.changed.clone()
        };
        changed.send_replace(());
        self.inner.notify.notify_waiters();
        for inner in followers {
            Self { inner }.abort_shared_at(reason.clone(), at);
        }
    }
    pub async fn cancelled(&self) -> SharedError {
        SignalWait::new(self.clone()).await
    }
    pub fn any(signals: impl IntoIterator<Item = AbortSignal>) -> AbortSignal {
        let signal = Self::pending(Origin::Controller, signals.into_iter().collect());
        for source in &signal.inner.sources {
            source.refresh();
            let reason = {
                let mut state = lock(&source.inner.state);
                if state.reason.is_none() {
                    state.followers.push(Arc::downgrade(&signal.inner));
                }
                state.reason.clone()
            };
            if let Some(reason) = reason {
                signal.abort_shared(reason);
                break;
            }
        }
        signal
    }
    pub fn timeout(timeout_ms: f64) -> AbortSignal {
        let delay = if !timeout_ms.is_finite() || timeout_ms > MAX_TIMER_DELAY_MS as f64 {
            1.0
        } else {
            timeout_ms
        };
        Self::pending(
            Origin::Timer {
                at: timer_instant(delay),
                reason: SharedError::new(DomTimeoutError),
            },
            vec![],
        )
    }
    pub fn from_token(token: CancellationToken) -> AbortSignal {
        let mut registration = Box::pin(token.clone().cancelled_owned());
        let signal = Self::pending(
            Origin::Token {
                token,
                reason: SharedError::new(AbortError::default()),
            },
            vec![],
        );
        // An owned registration with a weak wake target observes cancel synchronously, without a task.
        let waker = Waker::from(Arc::new(TokenWake {
            signal: Arc::downgrade(&signal.inner),
        }));
        let ready = registration
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_ready();
        *lock(&signal.inner.token_registration) = Some(registration);
        if ready {
            signal.refresh();
        }
        signal
    }
    pub(crate) fn subscribe(&self) -> watch::Receiver<()> {
        lock(&self.inner.state).changed.subscribe()
    }
}

struct SignalWait {
    signal: AbortSignal,
    notified: Pin<Box<OwnedNotified>>,
    timer: Option<Pin<Box<Sleep>>>,
    sources: Vec<SignalWait>,
}
impl SignalWait {
    fn new(signal: AbortSignal) -> Self {
        let notified = Box::pin(signal.inner.notify.clone().notified_owned());
        let timer = match signal.inner.origin {
            Origin::Timer { at, .. } => Some(Box::pin(tokio::time::sleep_until(at))),
            _ => None,
        };
        let sources = signal
            .inner
            .sources
            .iter()
            .cloned()
            .map(Self::new)
            .collect();
        Self {
            signal,
            notified,
            timer,
            sources,
        }
    }
}
impl Future for SignalWait {
    type Output = SharedError;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(reason) = this.signal.reason() {
            return Poll::Ready(reason);
        }
        if this.notified.as_mut().poll(cx).is_ready() {
            this.notified = Box::pin(this.signal.inner.notify.clone().notified_owned());
        }
        if let Some(timer) = &mut this.timer {
            let _ = timer.as_mut().poll(cx);
        }
        for source in &mut this.sources {
            if let Poll::Ready(reason) = Pin::new(source).poll(cx) {
                this.signal.abort_shared(reason);
                break;
            }
        }
        match this.signal.reason() {
            Some(reason) => Poll::Ready(reason),
            None => Poll::Pending,
        }
    }
}

#[derive(Debug)]
pub struct AbortController {
    signal: AbortSignal,
}
impl Default for AbortController {
    fn default() -> Self {
        Self::new()
    }
}
impl AbortController {
    pub fn new() -> Self {
        Self {
            signal: AbortSignal::pending(Origin::Controller, vec![]),
        }
    }
    pub fn signal(&self) -> AbortSignal {
        self.signal.clone()
    }
    pub fn abort(&self) {
        self.abort_with(AbortError::default());
    }
    pub fn abort_with(&self, reason: impl Into<BoxError>) {
        self.signal.abort_shared(SharedError::new(reason));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn dropped_signals_and_waits_leave_no_sources_tasks_or_timers() {
        let token = CancellationToken::new();
        for _ in 0..1000 {
            let source = AbortSignal::from_token(token.clone());
            let result = AbortSignal::any([source.clone(), AbortSignal::timeout(100.0)]);
            let weak = Arc::downgrade(&result.inner);
            {
                let wait = result.cancelled();
                tokio::pin!(wait);
                assert!(
                    std::future::poll_fn(|cx| Poll::Ready(wait.as_mut().poll(cx).is_pending()))
                        .await
                );
            }
            drop(result);
            assert!(weak.upgrade().is_none());
            assert!(lock(&source.inner.state).followers.is_empty());
            let weak = Arc::downgrade(&source.inner);
            drop(source);
            assert!(weak.upgrade().is_none());
        }
        token.cancel();
    }
}
