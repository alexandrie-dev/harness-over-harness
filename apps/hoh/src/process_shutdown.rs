//! Coalesced disposal, bounded exit, and escalation on a repeated signal.
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use hoh_error::BoxError;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
pub const PROCESS_SHUTDOWN_TIMEOUT_MS: u64 = 5_000;
pub type Disposal = Arc<dyn Fn() -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>;
pub type Exit = Arc<dyn Fn(i32) + Send + Sync>;
type Pending = Shared<BoxFuture<'static, ()>>;
struct State {
    pending: Option<Pending>,
    timer: Option<tokio::task::JoinHandle<()>>,
    completed: bool,
    force_exited: bool,
}
struct Inner {
    dispose: Disposal,
    force_exit: Exit,
    complete: Exit,
    timeout: Duration,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct ProcessShutdown(Arc<Inner>);
fn lock<T>(state: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}
pub fn create_process_shutdown(
    dispose: Disposal,
    force_exit: Exit,
    complete: Exit,
    timeout: Option<Duration>,
) -> ProcessShutdown {
    ProcessShutdown(Arc::new(Inner {
        dispose,
        force_exit,
        complete,
        timeout: timeout.unwrap_or(Duration::from_millis(PROCESS_SHUTDOWN_TIMEOUT_MS)),
        state: Mutex::new(State {
            pending: None,
            timer: None,
            completed: false,
            force_exited: false,
        }),
    }))
}
impl ProcessShutdown {
    fn force(&self, code: i32) {
        let timer = {
            let mut state = lock(&self.0.state);
            if state.force_exited {
                return;
            }
            state.force_exited = true;
            state.timer.take()
        };
        if let Some(timer) = timer {
            timer.abort();
        }
        (self.0.force_exit)(code);
    }
    fn complete(&self, code: i32) {
        let timer = {
            let mut state = lock(&self.0.state);
            if state.completed || state.force_exited {
                return;
            }
            state.completed = true;
            state.timer.take()
        };
        if let Some(timer) = timer {
            timer.abort();
        }
        (self.0.complete)(code);
    }
    fn start(&self, code: i32, force_after_dispose: bool) -> Pending {
        self.start_locked(code, force_after_dispose, lock(&self.0.state))
    }
    fn start_locked(
        &self,
        code: i32,
        force_after_dispose: bool,
        mut state: std::sync::MutexGuard<'_, State>,
    ) -> Pending {
        if let Some(pending) = &state.pending {
            return pending.clone();
        }
        let (done, wait) = tokio::sync::oneshot::channel();
        let pending = async move {
            let _ = wait.await;
        }
        .boxed()
        .shared();
        state.pending = Some(pending.clone());
        let this = self.clone();
        let deadline = tokio::time::Instant::now() + self.0.timeout;
        state.timer = Some(tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            this.force(code);
        }));
        let this = self.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            let result = std::panic::AssertUnwindSafe(async { (this.0.dispose)().await })
                .catch_unwind()
                .await;
            if !matches!(result, Ok(Ok(()))) || force_after_dispose {
                this.force(code);
            } else {
                this.complete(code);
            }
            let _ = done.send(());
        });
        pending
    }
    /// Starts disposal immediately; cloned futures join the same completion.
    pub fn shutdown(&self, code: i32) -> Pending {
        self.start(code, false)
    }
    pub fn interrupt(&self, code: i32) {
        let state = lock(&self.0.state);
        if state.pending.is_some() {
            drop(state);
            self.force(code);
        } else {
            drop(self.start_locked(code, true, state));
        }
    }
}
