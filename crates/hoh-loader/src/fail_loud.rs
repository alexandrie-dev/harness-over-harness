//! Host-owned fatal failure handlers; no process-global hooks or mutable registry.
use crate::cmdline::Subscription;
use futures_util::{FutureExt, future::BoxFuture};
use hoh_error::{BoxError, SharedError};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
pub const FAIL_LOUD_RELEASE_TIMEOUT_MS: u64 = 2_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailLoudEvent {
    UnhandledRejection,
    UncaughtException,
}
#[derive(Clone)]
pub struct FailLoudReason {
    pub error: SharedError,
    /// Native host's detailed inspection, including stack/properties where available.
    pub inspection: String,
}
impl FailLoudReason {
    pub fn new(error: SharedError) -> Self {
        let inspection = format!(
            "{}\n{:?}",
            crate::startup_diagnostics::format_activation_error(&error),
            error
        );
        Self { error, inspection }
    }
}
pub type FailureListener = Arc<dyn Fn(FailLoudReason) + Send + Sync>;
pub type Release = Arc<dyn Fn() -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>;
pub trait FailLoudProcess: Send + Sync {
    fn on(&self, event: FailLoudEvent, listener: FailureListener) -> Subscription;
    fn write_stderr(&self, text: &str);
    fn exit(&self, code: i32);
}
pub struct FailLoud {
    _handlers: Vec<Subscription>,
    assembled: Arc<Mutex<Vec<SharedError>>>,
}
impl FailLoud {
    /// Scope an already-assembled rejection until the host's next rejection checkpoint.
    pub fn retain_assembled(&self, reason: SharedError) -> Subscription {
        crate::runtime::lock(&self.assembled).push(reason.clone());
        let assembled = self.assembled.clone();
        Subscription::new(move || {
            let mut assembled = crate::runtime::lock(&assembled);
            if let Some(index) = assembled.iter().position(|item| item.ptr_eq(&reason)) {
                assembled.remove(index);
            }
        })
    }
}
pub fn install_fail_loud(
    bin: &str,
    process: Arc<dyn FailLoudProcess>,
    release: Option<Release>,
) -> FailLoud {
    let exiting = Arc::new(AtomicBool::new(false));
    let assembled = Arc::new(Mutex::new(Vec::<SharedError>::new()));
    let mut handlers = Vec::new();
    for (event, label) in [
        (FailLoudEvent::UnhandledRejection, "fatal load failure"),
        (FailLoudEvent::UncaughtException, "fatal uncaught exception"),
    ] {
        let bin = bin.to_owned();
        let proc = process.clone();
        let exiting = exiting.clone();
        let assembled = assembled.clone();
        let release = release.clone();
        handlers.push(process.on(
            event,
            Arc::new(move |reason| {
                if event == FailLoudEvent::UnhandledRejection
                    && crate::runtime::lock(&assembled)
                        .iter()
                        .any(|item| item.ptr_eq(&reason.error))
                {
                    return;
                }
                if exiting.swap(true, Ordering::AcqRel) {
                    return;
                }
                proc.write_stderr(&format!("{bin}: {label}: {}\n", reason.inspection));
                let Some(release) = release.clone() else {
                    proc.exit(1);
                    return;
                };
                let proc = proc.clone();
                let deadline = tokio::time::Instant::now()
                    + std::time::Duration::from_millis(FAIL_LOUD_RELEASE_TIMEOUT_MS);
                // This is the process host, outside any plugin; its exit grace must survive plugin disposal.
                tokio::spawn(async move {
                    let release =
                        std::panic::AssertUnwindSafe(async move { release().await }).catch_unwind();
                    let _ = tokio::time::timeout_at(deadline, release).await;
                    proc.exit(1);
                });
            }),
        ));
    }
    FailLoud {
        _handlers: handlers,
        assembled,
    }
}
