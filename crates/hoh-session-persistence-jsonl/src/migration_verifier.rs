//! Bounded verifier lifecycles. Success is accepted only after natural completion.
use crate::{
    JsonlError, Result,
    error::error,
    format::JsonlCompression,
    generation::{JsonlExpectedPrefix, JsonlVerifiedGeneration},
    io::lock,
    worker::VerificationRequest,
};
use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_timeout::{AbortController, AbortSignal};
use std::sync::{Arc, Mutex};
use tokio::sync::{Semaphore, mpsc};
pub const MAX_CONCURRENT_VERIFIERS: usize = 2;
pub enum VerificationEvent {
    Success(JsonlVerifiedGeneration),
    Failure(BoxError),
    InvalidResponse,
    Exit(i32),
}
pub struct VerificationWorker {
    pub events: mpsc::UnboundedReceiver<VerificationEvent>,
    pub terminate: Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>,
    pub join: Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>,
}
pub type VerificationWorkerFactory =
    Arc<dyn Fn(VerificationRequest) -> Result<VerificationWorker> + Send + Sync>;
#[derive(Clone)]
pub struct VerificationScheduler {
    slots: Arc<Semaphore>,
    factory: VerificationWorkerFactory,
}
impl Default for VerificationScheduler {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_VERIFIERS)),
            factory: Arc::new(native_worker),
        }
    }
}
impl VerificationScheduler {
    pub fn with_worker_factory(factory: VerificationWorkerFactory) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_VERIFIERS)),
            factory,
        }
    }
    pub fn verify(
        &self,
        request: VerificationRequest,
        signal: Option<&AbortSignal>,
    ) -> BoxFuture<'static, Result<JsonlVerifiedGeneration>> {
        let scheduler = self.clone();
        let signal = signal.cloned();
        // The scheduler owns the worker until join, even if a caller drops its waiter.
        let (send, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = send.send(scheduler.run(request, signal.as_ref()).await);
        });
        Box::pin(async move { receive.await.map_err(|e| Box::new(e) as BoxError)? })
    }
    async fn run(
        &self,
        request: VerificationRequest,
        signal: Option<&AbortSignal>,
    ) -> Result<JsonlVerifiedGeneration> {
        if let Some(signal) = signal {
            signal.throw_if_aborted()?;
        }
        let permit = if let Some(signal) = signal {
            tokio::select! {permit=self.slots.acquire()=>permit.map_err(|e|error(e.to_string()))?,_=signal.cancelled()=>{signal.throw_if_aborted()?;return Err(error("migration verifier aborted"));}}
        } else {
            self.slots
                .acquire()
                .await
                .map_err(|e| error(e.to_string()))?
        };
        if let Some(signal) = signal {
            signal.throw_if_aborted()?;
        }
        let mut worker = (self.factory)(request)?;
        let mut success = None;
        let result = loop {
            let event = if let Some(signal) = signal {
                tokio::select! {biased;_=signal.cancelled()=>{
                    let failure=signal.throw_if_aborted().err().map(|e|Box::new(e) as BoxError).unwrap_or_else(||error("migration verifier aborted"));
                    break Err(terminate(&worker,failure).await);
                },event=worker.events.recv()=>event}
            } else {
                worker.events.recv().await
            };
            match event {
                Some(VerificationEvent::Success(result)) => success = Some(result),
                Some(VerificationEvent::Failure(failure)) => {
                    break Err(terminate(&worker, failure).await);
                }
                Some(VerificationEvent::InvalidResponse) => {
                    break Err(terminate(
                        &worker,
                        error("migration verifier returned an invalid response"),
                    )
                    .await);
                }
                Some(VerificationEvent::Exit(code)) => {
                    (worker.join)().await?;
                    break match success {
                        None => Err(error(format!(
                            "migration verifier exited before reporting a result (code {code})"
                        ))),
                        Some(_) if code != 0 => Err(error(format!(
                            "migration verifier exited after reporting a result (code {code})"
                        ))),
                        Some(result) => Ok(result),
                    };
                }
                None => {
                    (worker.join)().await?;
                    break Err(error(
                        "migration verifier exited before reporting a result (code 0)",
                    ));
                }
            }
        };
        drop(permit);
        result
    }
}
async fn terminate(worker: &VerificationWorker, failure: BoxError) -> BoxError {
    let termination = (worker.terminate)().await;
    let joined = (worker.join)().await;
    let mut errors = vec![failure];
    if let Err(cleanup) = termination {
        errors.push(cleanup);
    }
    if let Err(cleanup) = joined {
        errors.push(cleanup);
    }
    if errors.len() == 1 {
        errors.remove(0)
    } else {
        Box::new(JsonlError::Aggregate {
            message: "migration verifier termination failed".into(),
            errors,
        })
    }
}
fn native_worker(request: VerificationRequest) -> Result<VerificationWorker> {
    let (send, events) = mpsc::unbounded_channel();
    let cancellation = Arc::new(AbortController::new());
    let signal = cancellation.signal();
    // This is the worker's independent native thread, not a plugin executor.
    let task = tokio::task::spawn_blocking(move || {
        let result = (|| -> Result<_> {
            signal.throw_if_aborted()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            // Mirror validation of Node workerData at the native worker boundary.
            let mut data = serde_json::json!({"path":request.path,"compression":request.compression,"expectedId":request.expected_id,"expectedEventCount":request.expected_event_count});
            if let Some(prefix) = &request.expected_prefix {
                data["expectedPrefix"] =
                    serde_json::json!({"bytes":prefix.bytes,"digest":prefix.digest});
            }
            let request = crate::worker::parse_request(&data)?;
            let result = runtime.block_on(crate::worker::verify(&request));
            signal.throw_if_aborted()?;
            result
        })();
        match result {
            Ok(result) => {
                let _ = send.send(VerificationEvent::Success(result));
            }
            Err(failure) => {
                // A Node Worker response reconstructs a plain Error in its parent.
                let _ = send.send(VerificationEvent::Failure(error(failure.to_string())));
            }
        }
        let _ = send.send(VerificationEvent::Exit(0));
    });
    let task = Arc::new(Mutex::new(Some(task)));
    let join_task = task.clone();
    let join: Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync> = Arc::new(move || {
        let task = lock(&join_task).take();
        Box::pin(async move {
            if let Some(task) = task {
                task.await?;
            }
            Ok(())
        })
    });
    let termination_join = join.clone();
    let terminate = Arc::new(move || {
        cancellation.abort();
        termination_join()
    });
    Ok(VerificationWorker {
        events,
        terminate,
        join,
    })
}
pub async fn verify_current_generation_in_worker(
    scheduler: &VerificationScheduler,
    path: &str,
    compression: JsonlCompression,
    id: &str,
    count: usize,
    prefix: Option<&JsonlExpectedPrefix>,
    signal: Option<&AbortSignal>,
) -> Result<JsonlVerifiedGeneration> {
    scheduler
        .verify(
            VerificationRequest {
                path: path.into(),
                compression,
                expected_id: id.into(),
                expected_event_count: count,
                expected_prefix: prefix.cloned(),
            },
            signal,
        )
        .await
}
