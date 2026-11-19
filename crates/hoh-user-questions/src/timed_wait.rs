//! Foreground question lifetime and answer-UI claims.

use hoh_error::{BoxError, HarnessError};
use hoh_timeout::{AbortController, AbortSignal};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{task::JoinHandle, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitFrame {
    pub remaining_ms: u64,
}

struct WaitState {
    claims: HashSet<u64>,
    next_claim: u64,
    timer: Option<JoinHandle<()>>,
    closed: bool,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One live foreground wait. The host timer runs only while no Client claim
/// exists, but its deadline remains absolute.
pub struct TimedQuestionWait {
    deadline: Instant,
    controller: AbortController,
    timeout: HarnessError,
    state: Mutex<WaitState>,
    parent_task: Mutex<Option<JoinHandle<()>>>,
}

impl TimedQuestionWait {
    pub fn new(deadline: Instant, parent: Option<AbortSignal>, timeout: HarnessError) -> Arc<Self> {
        let wait = Arc::new(Self {
            deadline,
            controller: AbortController::new(),
            timeout,
            state: Mutex::new(WaitState {
                claims: HashSet::new(),
                next_claim: 0,
                timer: None,
                closed: false,
            }),
            parent_task: Mutex::new(None),
        });
        if let Some(parent) = parent {
            if let Some(reason) = parent.reason() {
                wait.close(reason);
            } else {
                let weak = Arc::downgrade(&wait);
                *lock(&wait.parent_task) = Some(tokio::spawn(async move {
                    let reason = parent.cancelled().await;
                    if let Some(wait) = weak.upgrade() {
                        wait.close(reason);
                    }
                }));
            }
        }
        wait.schedule();
        wait
    }

    pub fn signal(&self) -> AbortSignal {
        self.controller.signal()
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub async fn done(&self) {
        let _ = self.signal().cancelled().await;
    }

    fn remaining_ms(&self) -> u64 {
        self.deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    fn schedule(self: &Arc<Self>) {
        let mut state = lock(&self.state);
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
        if state.closed || !state.claims.is_empty() {
            return;
        }
        let deadline = self.deadline;
        let weak = Arc::downgrade(self);
        state.timer = Some(tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            if let Some(wait) = weak.upgrade() {
                wait.close(wait.timeout.clone());
            }
        }));
    }

    /// Claim the wait and obtain the single Host-computed duration frame.
    /// Dropping the claim corresponds to closing the remote stream.
    pub fn attach(self: &Arc<Self>, signal: AbortSignal) -> Option<TimedQuestionClaim> {
        if signal.aborted() || self.signal().aborted() {
            return None;
        }
        let mut state = lock(&self.state);
        if state.closed {
            return None;
        }
        if state.claims.is_empty() && Instant::now() >= self.deadline {
            drop(state);
            self.close(self.timeout.clone());
            return None;
        }
        state.next_claim = state.next_claim.wrapping_add(1);
        let id = state.next_claim;
        state.claims.insert(id);
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
        drop(state);
        let weak = Arc::downgrade(self);
        let watched_signal = signal.clone();
        let abort_task = tokio::spawn(async move {
            let _ = watched_signal.cancelled().await;
            if let Some(wait) = weak.upgrade() {
                wait.release(id);
            }
        });
        Some(TimedQuestionClaim {
            wait: Arc::downgrade(self),
            id,
            signal,
            released: AtomicBool::new(false),
            abort_task: Mutex::new(Some(abort_task)),
            frame: WaitFrame {
                remaining_ms: self.remaining_ms(),
            },
        })
    }

    fn release(self: &Arc<Self>, id: u64) {
        if lock(&self.state).claims.remove(&id) {
            self.schedule();
        }
    }

    pub fn close(&self, reason: impl Into<BoxError>) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        state.closed = true;
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
        drop(state);
        if let Some(task) = lock(&self.parent_task).take() {
            task.abort();
        }
        self.controller.abort_with(reason);
    }

    #[cfg(test)]
    fn claim_count(&self) -> usize {
        lock(&self.state).claims.len()
    }
}

impl Drop for TimedQuestionWait {
    fn drop(&mut self) {
        if let Some(timer) = lock(&self.state).timer.take() {
            timer.abort();
        }
        if let Some(task) = lock(&self.parent_task).take() {
            task.abort();
        }
    }
}

pub struct TimedQuestionClaim {
    wait: Weak<TimedQuestionWait>,
    id: u64,
    signal: AbortSignal,
    released: AtomicBool,
    abort_task: Mutex<Option<JoinHandle<()>>>,
    frame: WaitFrame,
}

impl TimedQuestionClaim {
    pub fn frame(&self) -> WaitFrame {
        self.frame
    }

    /// Wait until either the Client stream or the business wait closes.
    pub async fn ended(&self) {
        let Some(wait) = self.wait.upgrade() else {
            return;
        };
        let wait_signal = wait.signal();
        tokio::select! {
            _ = self.signal.cancelled() => {}
            _ = wait_signal.cancelled() => {}
        }
    }

    pub fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(task) = lock(&self.abort_task).take() {
            task.abort();
        }
        if let Some(wait) = self.wait.upgrade() {
            wait.release(self.id);
        }
    }
}

impl Drop for TimedQuestionClaim {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(code: &str) -> HarnessError {
        HarnessError::named("UserQuestionError", "closed", code)
    }

    #[tokio::test(start_paused = true)]
    async fn expires_an_unclaimed_wait_at_the_host_deadline() {
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_millis(50),
            None,
            error("ASK_TIMED_OUT"),
        );
        tokio::time::advance(Duration::from_millis(49)).await;
        assert!(!wait.signal().aborted());
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            hoh_error::HarnessError::of(wait.signal().reason().unwrap().get_ref())
                .unwrap()
                .code(),
            "ASK_TIMED_OUT"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn one_claim_pauses_the_host_timer_until_release() {
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_millis(50),
            None,
            error("ASK_TIMED_OUT"),
        );
        let client = AbortController::new();
        let claim = wait.attach(client.signal()).unwrap();
        assert_eq!(claim.frame().remaining_ms, 50);
        tokio::time::advance(Duration::from_millis(100)).await;
        assert!(!wait.signal().aborted());
        drop(claim);
        tokio::task::yield_now().await;
        assert!(wait.signal().aborted());
    }

    #[tokio::test(start_paused = true)]
    async fn aborting_the_client_signal_releases_a_live_claim() {
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_millis(50),
            None,
            error("ASK_TIMED_OUT"),
        );
        let client = AbortController::new();
        let claim = wait.attach(client.signal()).unwrap();
        tokio::time::advance(Duration::from_millis(20)).await;
        client.abort();
        claim.ended().await;
        tokio::time::advance(Duration::from_millis(29)).await;
        assert!(!wait.signal().aborted());
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(wait.signal().aborted());
        assert_eq!(wait.claim_count(), 0);
        drop(claim);
    }

    #[tokio::test(start_paused = true)]
    async fn multiple_claims_keep_the_timer_paused_until_the_last_releases() {
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_millis(10),
            None,
            error("ASK_TIMED_OUT"),
        );
        let a = wait.attach(AbortController::new().signal()).unwrap();
        let b = wait.attach(AbortController::new().signal()).unwrap();
        assert_eq!(wait.claim_count(), 2);
        tokio::time::advance(Duration::from_millis(20)).await;
        drop(a);
        tokio::task::yield_now().await;
        assert!(!wait.signal().aborted());
        drop(b);
        tokio::task::yield_now().await;
        assert!(wait.signal().aborted());
    }

    #[tokio::test(start_paused = true)]
    async fn parent_abort_closes_the_wait_with_the_parent_reason() {
        let parent = AbortController::new();
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_secs(1),
            Some(parent.signal()),
            error("ASK_TIMED_OUT"),
        );
        parent.abort_with(error("PARENT"));
        tokio::task::yield_now().await;
        let reason = wait.signal().reason().unwrap();
        assert_eq!(HarnessError::of(reason.get_ref()).unwrap().code(), "PARENT");
    }

    #[tokio::test(start_paused = true)]
    async fn parent_abort_ends_a_claim_without_aborting_the_parent_again() {
        let parent = AbortController::new();
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_secs(1),
            Some(parent.signal()),
            error("ASK_TIMED_OUT"),
        );
        let claim = wait.attach(AbortController::new().signal()).unwrap();
        parent.abort_with(error("PARENT"));
        claim.ended().await;
        assert_eq!(
            HarnessError::of(wait.signal().reason().unwrap().get_ref())
                .unwrap()
                .code(),
            "PARENT"
        );
        assert_eq!(
            HarnessError::of(parent.signal().reason().unwrap().get_ref())
                .unwrap()
                .code(),
            "PARENT"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_client_signal_never_claims_a_live_wait() {
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_secs(1),
            None,
            error("ASK_TIMED_OUT"),
        );
        let client = AbortController::new();
        client.abort();
        assert!(wait.attach(client.signal()).is_none());
        assert_eq!(wait.claim_count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn attach_after_the_deadline_closes_without_a_frame() {
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_millis(1),
            None,
            error("ASK_TIMED_OUT"),
        );
        // A claim holds the timer while the absolute deadline passes.
        let first = wait.attach(AbortController::new().signal()).unwrap();
        tokio::time::advance(Duration::from_millis(2)).await;
        drop(first);
        assert!(wait.attach(AbortController::new().signal()).is_none());
        assert!(wait.signal().aborted());
    }

    #[tokio::test(start_paused = true)]
    async fn close_is_idempotent_and_ends_claims() {
        let wait = TimedQuestionWait::new(
            Instant::now() + Duration::from_secs(1),
            None,
            error("ASK_TIMED_OUT"),
        );
        let claim = wait.attach(AbortController::new().signal()).unwrap();
        wait.close(error("FIRST"));
        wait.close(error("SECOND"));
        claim.ended().await;
        let reason = wait.signal().reason().unwrap();
        assert_eq!(HarnessError::of(reason.get_ref()).unwrap().code(), "FIRST");
    }
}
