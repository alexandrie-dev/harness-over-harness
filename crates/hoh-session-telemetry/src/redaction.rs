use crate::SessionTelemetryRecord;
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, Undo};
use std::sync::{Arc, Mutex, MutexGuard};

type Rule = dyn for<'a> Fn(
        &SessionTelemetryRecord,
        RedactionNext<'a>,
    ) -> Result<SessionTelemetryRecord, BoxError>
    + Send
    + Sync;

// SHIM(hoh-kernel): the source waterfall is synchronous on the session/event hot path;
// replace this lifecycle-owned registry when hoh-kernel provides synchronous Waterfall dispatch.

/// The continuation passed to a synchronous redaction rule.
pub struct RedactionNext<'a>(
    Option<Box<dyn FnOnce() -> Result<SessionTelemetryRecord, BoxError> + 'a>>,
);

impl RedactionNext<'_> {
    pub fn run(mut self) -> Result<SessionTelemetryRecord, BoxError> {
        self.0
            .take()
            .expect("redaction next continuation is single-use")()
    }
}

#[derive(Clone, Default)]
pub struct SessionTelemetryRedactions {
    inner: Arc<Mutex<Vec<Arc<Rule>>>>,
}

impl SessionTelemetryRedactions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mount one rule for the lifetime of `ctx`; earlier rules remain outermost.
    pub fn register<F>(&self, ctx: &Context, rule: F) -> Result<Disposer, BoxError>
    where
        F: for<'a> Fn(
                &SessionTelemetryRecord,
                RedactionNext<'a>,
            ) -> Result<SessionTelemetryRecord, BoxError>
            + Send
            + Sync
            + 'static,
    {
        let rule: Arc<Rule> = Arc::new(rule);
        let rules = self.inner.clone();
        ctx.effect("session-telemetry/record", move || {
            lock(&rules).push(rule.clone());
            Undo::sync(move || lock(&rules).retain(|entry| !Arc::ptr_eq(entry, &rule)))
        })
        .map_err(Into::into)
    }

    pub(crate) fn apply(
        &self,
        record: SessionTelemetryRecord,
    ) -> Result<SessionTelemetryRecord, BoxError> {
        let rules = lock(&self.inner).clone();
        apply_from(&rules, 0, record)
    }
}

fn apply_from(
    rules: &[Arc<Rule>],
    index: usize,
    record: SessionTelemetryRecord,
) -> Result<SessionTelemetryRecord, BoxError> {
    let Some(rule) = rules.get(index) else {
        return Ok(record);
    };
    let input = record.clone();
    rule(
        &input,
        RedactionNext(Some(Box::new(move || apply_from(rules, index + 1, record)))),
    )
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
