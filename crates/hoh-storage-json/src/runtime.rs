//! Eager promise entry and completion tracking; callers own write ordering.
use hoh_error::BoxError;
use hoh_storage::{BackendFuture, StorageError, StorageErrorCode};
use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::{oneshot, watch};

pub(crate) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct PlainError(pub String);

pub(crate) fn plain(message: impl Into<String>) -> BoxError {
    Box::new(PlainError(message.into()))
}

pub(crate) fn ready<T: Send + 'static>(result: Result<T, BoxError>) -> BackendFuture<'static, T> {
    Box::pin(std::future::ready(result))
}

pub(crate) fn spawn<T, F>(operation: F) -> BackendFuture<'static, T>
where
    T: Send + 'static,
    F: Future<Output = Result<T, BoxError>> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    // Like an already-created JavaScript promise, dropping its waiter does not
    // cancel the operation. Unit/backend close explicitly drains its completion.
    tokio::spawn(async move {
        let _ = tx.send(operation.await);
    });
    Box::pin(async move {
        rx.await
            .map_err(|_| plain("JSON storage operation task stopped"))?
    })
}

pub(crate) async fn drain(jobs: Vec<watch::Receiver<bool>>) {
    for mut job in jobs {
        while !*job.borrow_and_update() {
            if job.changed().await.is_err() {
                break;
            }
        }
    }
}

#[derive(Default)]
struct LifeState {
    closed: bool,
    unusable: Option<StorageError>,
    jobs: Vec<watch::Receiver<bool>>,
}

pub(crate) struct Lifecycle {
    name: String,
    state: Mutex<LifeState>,
    on_close: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>,
}

impl Lifecycle {
    pub(crate) fn new(
        name: String,
        on_close: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>,
    ) -> Self {
        Self {
            name,
            state: Mutex::default(),
            on_close,
        }
    }
    fn check_state(&self, state: &LifeState) -> Result<(), BoxError> {
        if state.closed {
            return Err(Box::new(StorageError::new(
                StorageErrorCode::Closed,
                format!("unit '{}' is closed", self.name),
            )));
        }
        if let Some(error) = &state.unusable {
            return Err(Box::new(error.clone()));
        }
        Ok(())
    }
    pub(crate) fn checked<T>(
        &self,
        operation: impl FnOnce() -> Result<T, BoxError>,
    ) -> Result<T, BoxError> {
        let state = lock(&self.state);
        self.check_state(&state)?;
        operation()
    }
    pub(crate) fn quarantine(&self, error: StorageError) {
        lock(&self.state).unusable = Some(error);
    }
    pub(crate) fn start<T, F>(
        &self,
        prepare: impl FnOnce() -> Result<F, BoxError>,
    ) -> BackendFuture<'static, T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, BoxError>> + Send + 'static,
    {
        let mut state = lock(&self.state);
        if let Err(error) = self.check_state(&state) {
            return ready(Err(error));
        }
        let operation = match prepare() {
            Ok(operation) => operation,
            Err(error) => return ready(Err(error)),
        };
        let (done, pending) = watch::channel(false);
        state.jobs.retain(|job| !*job.borrow());
        state.jobs.push(pending);
        spawn(async move {
            let result = operation.await;
            let _ = done.send(true);
            result
        })
    }
    pub(crate) fn close(&self) -> BackendFuture<'static, ()> {
        let (first, jobs) = {
            let mut state = lock(&self.state);
            let first = !state.closed;
            state.closed = true;
            (first, state.jobs.clone())
        };
        let on_close = self.on_close.clone();
        spawn(async move {
            drain(jobs).await;
            if first {
                on_close()?;
            }
            Ok(())
        })
    }
}
