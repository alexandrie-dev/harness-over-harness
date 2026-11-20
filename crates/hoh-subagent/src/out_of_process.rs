//! Shared contract helpers for out-of-process subagent providers.

use futures_util::future::BoxFuture;
use hoh_error::{BoxError, SharedError};
use hoh_llm::ContentBlock;
use hoh_session::SessionId;
use hoh_timeout::AbortSignal;
use hoh_util_workspace_path::node_path::is_absolute;
use std::sync::{Arc, Mutex};

use crate::{
    error::{SubagentError, SubagentErrorCode},
    types::SubagentRun,
    types::{SubagentResult, SubagentStopReason},
};

pub const MAX_SUBAGENT_DIAGNOSTIC_BYTES: usize = 4_096;
const DIAGNOSTIC_TRUNCATION_SUFFIX: &str = "\n[diagnostic truncated]";

pub type SubagentErrorReporter =
    Box<dyn Fn(&(dyn std::error::Error + 'static), SubagentStopReason) + Send + Sync>;
pub type SubagentTeardown =
    Box<dyn FnOnce() -> BoxFuture<'static, Result<(), BoxError>> + Send + 'static>;

pub fn limit_subagent_diagnostic(diagnostic: &str) -> String {
    if diagnostic.len() <= MAX_SUBAGENT_DIAGNOSTIC_BYTES {
        return diagnostic.into();
    }
    let limit = MAX_SUBAGENT_DIAGNOSTIC_BYTES - DIAGNOSTIC_TRUNCATION_SUFFIX.len();
    let mut end = limit;
    while !diagnostic.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &diagnostic[..end], DIAGNOSTIC_TRUNCATION_SUFFIX)
}

pub fn normalize_subagent_diagnostic(mut result: SubagentResult) -> SubagentResult {
    if let Some(diagnostic) = result.diagnostic.take() {
        result.diagnostic = Some(limit_subagent_diagnostic(&diagnostic));
    }
    result
}

pub fn assert_positive_finite(prefix: &str, name: &str, value: f64) -> Result<(), BoxError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{prefix}: {name} must be a positive finite number").into());
    }
    Ok(())
}

pub fn assert_usable_cwd(prefix: &str, label: &str, cwd: &str) -> Result<String, BoxError> {
    if !is_absolute(cwd) {
        return Err(Box::new(SubagentError::error(
            format!("{prefix}: {label} must be an absolute path: {cwd}"),
            SubagentErrorCode::InvalidCwd,
        )));
    }
    let path = std::path::Path::new(cwd);
    if !path.is_dir() {
        return Err(Box::new(SubagentError::error(
            format!("{prefix}: {label} is not an accessible directory: {cwd}"),
            SubagentErrorCode::InvalidCwd,
        )));
    }
    #[cfg(unix)]
    if rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_err() {
        return Err(Box::new(SubagentError::error(
            format!("{prefix}: {label} is not an accessible directory: {cwd}"),
            SubagentErrorCode::InvalidCwd,
        )));
    }
    Ok(cwd.into())
}

pub struct RunResultSettlement {
    pub attempt: BoxFuture<'static, Result<SubagentResult, BoxError>>,
    pub collect_output: Box<dyn Fn() -> Vec<ContentBlock> + Send + Sync>,
    pub collect_diagnostic: Option<Box<dyn Fn() -> Option<String> + Send + Sync>>,
    /// Local cancellation may settle independently of the request signal.
    pub cancelled: Box<dyn Fn() -> bool + Send + Sync>,
    pub on_error: Option<SubagentErrorReporter>,
    pub signal: AbortSignal,
}

pub async fn settle_run_result(parts: RunResultSettlement) -> SubagentResult {
    let RunResultSettlement {
        attempt,
        collect_output,
        collect_diagnostic,
        cancelled,
        on_error,
        signal,
    } = parts;
    tokio::select! {
        biased;
        _ = signal.cancelled() => SubagentResult {
            output: collect_output(),
            structured: None,
            diagnostic: None,
            stop_reason: SubagentStopReason::Aborted,
        },
        result = attempt => match result {
            Ok(_result) if cancelled() || signal.aborted() => SubagentResult {
                output: collect_output(),
                structured: None,
                diagnostic: None,
                stop_reason: SubagentStopReason::Aborted,
            },
            Ok(result) => normalize_subagent_diagnostic(result),
            Err(_error) if cancelled() || signal.aborted() => SubagentResult {
                output: collect_output(),
                structured: None,
                diagnostic: None,
                stop_reason: SubagentStopReason::Aborted,
            },
            Err(error) => {
                if let Some(report) = on_error {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        report(error.as_ref(), SubagentStopReason::Error);
                    }));
                }
                SubagentResult {
                    output: collect_output(),
                    structured: None,
                    diagnostic: collect_diagnostic.and_then(|collect| collect()).map(|value| limit_subagent_diagnostic(&value)),
                    stop_reason: SubagentStopReason::Error,
                }
            }
        }
    }
}

pub struct SubprocessRunHandleParts {
    pub id: SessionId,
    pub result: BoxFuture<'static, Result<SubagentResult, BoxError>>,
    pub request_cancel: Arc<dyn Fn() + Send + Sync>,
    pub teardown: SubagentTeardown,
}

struct SubprocessRunHandle {
    id: SessionId,
    inner: Arc<SubprocessRunInner>,
}

type SharedSettlement<T> = tokio::sync::watch::Receiver<Option<Result<T, SharedError>>>;

struct SubprocessRunInner {
    result: SharedSettlement<SubagentResult>,
    request_cancel: Arc<dyn Fn() + Send + Sync>,
    teardown: Mutex<Option<SubagentTeardown>>,
    disposal: Mutex<Option<SharedSettlement<()>>>,
}

async fn wait_for_settlement<T: Clone>(
    mut receiver: SharedSettlement<T>,
    unavailable: &'static str,
) -> Result<T, BoxError> {
    loop {
        if let Some(settled) = receiver.borrow().clone() {
            return settled.map_err(|error| Box::new(error) as BoxError);
        }
        if receiver.changed().await.is_err() {
            return Err(unavailable.into());
        }
    }
}

impl SubagentRun for SubprocessRunHandle {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn result(&self) -> BoxFuture<'static, Result<SubagentResult, BoxError>> {
        Box::pin(wait_for_settlement(
            self.inner.result.clone(),
            "subagent result is unavailable",
        ))
    }

    fn dispose(&self) -> BoxFuture<'static, Result<(), BoxError>> {
        let inner = self.inner.clone();
        let receiver = {
            let mut disposal = inner
                .disposal
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(receiver) = disposal.as_ref() {
                receiver.clone()
            } else {
                (inner.request_cancel)();
                let teardown = inner
                    .teardown
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let (sender, receiver) = tokio::sync::watch::channel(None);
                *disposal = Some(receiver.clone());
                tokio::spawn(async move {
                    let settled = match teardown {
                        Some(teardown) => teardown().await.map_err(SharedError::new),
                        None => Ok(()),
                    };
                    let _ = sender.send(Some(settled));
                });
                receiver
            }
        };
        Box::pin(wait_for_settlement(
            receiver,
            "subagent teardown is unavailable",
        ))
    }
}

/// Publish a process-backed run handle. Result settlement is eager and disposal is idempotent.
pub fn subprocess_run_handle(parts: SubprocessRunHandleParts) -> Arc<dyn SubagentRun> {
    // Materialize the source Promise eagerly before the handle is returned. The task owns the
    // computation, so cancelling any individual `result()` waiter cannot consume it.
    let (result_sender, result_receiver) = tokio::sync::watch::channel(None);
    tokio::spawn(async move {
        let settled = parts.result.await.map_err(SharedError::new);
        let _ = result_sender.send(Some(settled));
    });
    Arc::new(SubprocessRunHandle {
        id: parts.id,
        inner: Arc::new(SubprocessRunInner {
            result: result_receiver,
            request_cancel: parts.request_cancel,
            teardown: Mutex::new(Some(parts.teardown)),
            disposal: Mutex::new(None),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NO_START_CAPABILITIES, SubagentCapabilities};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn diagnostic_truncation_is_utf8_safe_and_bounded() {
        let value = "é".repeat(3_000);
        let limited = limit_subagent_diagnostic(&value);
        assert!(limited.len() <= MAX_SUBAGENT_DIAGNOSTIC_BYTES);
        assert!(limited.ends_with(DIAGNOSTIC_TRUNCATION_SUFFIX));
    }

    #[test]
    fn positive_finite_rejects_invalid_bounds() {
        assert!(assert_positive_finite("p", "graceMs", 1.0).is_ok());
        for value in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert!(assert_positive_finite("subagent-acp", "timeout", value).is_err());
        }
    }

    #[test]
    fn no_start_capabilities_advertises_nothing() {
        assert_eq!(NO_START_CAPABILITIES, SubagentCapabilities::default());
    }

    #[tokio::test]
    async fn successful_settlement_passes_through() {
        let signal = hoh_timeout::AbortController::new().signal();
        let result = settle_run_result(RunResultSettlement {
            attempt: Box::pin(async {
                Ok(SubagentResult::completed(vec![ContentBlock::text("done")]))
            }),
            collect_output: Box::new(Vec::new),
            collect_diagnostic: None,
            cancelled: Box::new(|| false),
            on_error: None,
            signal,
        })
        .await;
        assert_eq!(result.stop_reason, SubagentStopReason::Completed);
        assert_eq!(result.output[0].text_value(), Some("done"));
    }

    #[tokio::test]
    async fn failed_settlement_is_flattened_and_contains_sink_failure() {
        let signal = hoh_timeout::AbortController::new().signal();
        let seen = Arc::new(AtomicUsize::new(0));
        let sink_seen = seen.clone();
        let result = settle_run_result(RunResultSettlement {
            attempt: Box::pin(async { Err("transport died".into()) }),
            collect_output: Box::new(|| vec![ContentBlock::text("partial")]),
            collect_diagnostic: Some(Box::new(|| Some("safe diagnostic".into()))),
            cancelled: Box::new(|| false),
            on_error: Some(Box::new(move |_, reason| {
                assert_eq!(reason, SubagentStopReason::Error);
                sink_seen.fetch_add(1, Ordering::SeqCst);
                panic!("contained sink failure");
            })),
            signal,
        })
        .await;
        assert_eq!(seen.load(Ordering::SeqCst), 1);
        assert_eq!(result.stop_reason, SubagentStopReason::Error);
        assert_eq!(result.diagnostic.as_deref(), Some("safe diagnostic"));
    }

    #[tokio::test]
    async fn subprocess_handle_disposal_is_idempotent() {
        let cancellations = Arc::new(AtomicUsize::new(0));
        let teardowns = Arc::new(AtomicUsize::new(0));
        let cancel_count = cancellations.clone();
        let teardown_count = teardowns.clone();
        let run = subprocess_run_handle(SubprocessRunHandleParts {
            id: "run-1".into(),
            result: Box::pin(async { Ok(SubagentResult::completed(Vec::new())) }),
            request_cancel: Arc::new(move || {
                cancel_count.fetch_add(1, Ordering::SeqCst);
            }),
            teardown: Box::new(move || {
                Box::pin(async move {
                    teardown_count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        });
        run.dispose().await.unwrap();
        run.dispose().await.unwrap();
        assert_eq!(cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(teardowns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn local_cancellation_flattens_a_queued_failure_without_reporting_error() {
        let signal = hoh_timeout::AbortController::new().signal();
        let reports = Arc::new(AtomicUsize::new(0));
        let report_count = reports.clone();
        let result = settle_run_result(RunResultSettlement {
            attempt: Box::pin(async { Err("transport failed after cancellation".into()) }),
            collect_output: Box::new(|| vec![ContentBlock::text("partial")]),
            collect_diagnostic: Some(Box::new(|| Some("must not escape".into()))),
            cancelled: Box::new(|| true),
            on_error: Some(Box::new(move |_, _| {
                report_count.fetch_add(1, Ordering::SeqCst);
            })),
            signal,
        })
        .await;
        assert_eq!(result.stop_reason, SubagentStopReason::Aborted);
        assert_eq!(result.output[0].text_value(), Some("partial"));
        assert_eq!(result.diagnostic, None);
        assert_eq!(reports.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_result_waiter_does_not_consume_shared_result() {
        let run = subprocess_run_handle(SubprocessRunHandleParts {
            id: "run-result-cancel".into(),
            result: Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                Ok(SubagentResult::completed(vec![ContentBlock::text("done")]))
            }),
            request_cancel: Arc::new(|| {}),
            teardown: Box::new(|| Box::pin(async { Ok(()) })),
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), run.result())
                .await
                .is_err()
        );
        let result = run.result().await.unwrap();
        assert_eq!(result.output[0].text_value(), Some("done"));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_dispose_waiter_does_not_abandon_shared_teardown() {
        let cancellations = Arc::new(AtomicUsize::new(0));
        let teardowns = Arc::new(AtomicUsize::new(0));
        let cancel_count = cancellations.clone();
        let teardown_count = teardowns.clone();
        let run = subprocess_run_handle(SubprocessRunHandleParts {
            id: "run-dispose-cancel".into(),
            result: Box::pin(async { Ok(SubagentResult::completed(Vec::new())) }),
            request_cancel: Arc::new(move || {
                cancel_count.fetch_add(1, Ordering::SeqCst);
            }),
            teardown: Box::new(move || {
                Box::pin(async move {
                    teardown_count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    Ok(())
                })
            }),
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), run.dispose())
                .await
                .is_err()
        );
        let second = run.dispose();
        tokio::pin!(second);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), &mut second)
                .await
                .is_err()
        );
        second.await.unwrap();
        assert_eq!(cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(teardowns.load(Ordering::SeqCst), 1);
    }
}
