//! Values of the caller that a teardown may need (`AsyncLocalStorage` of the source reaches the
//! teardown of a disposed fiber through its promise chain; `tokio::task_local!` does not cross
//! the kernel's own tasks). A plugin registers a capture function; `Fiber::dispose()`,
//! `Kernel::shutdown()` and `Disposer::dispose()` run the registered functions in the caller's
//! task, and the teardown tasks read the results through [`Context::dispose_captures`].
//!
//! The functions run synchronously in the caller: they must be instantaneous, must not block and
//! must only read. A panic in one is caught, its value is dropped and a warning is logged.
use crate::fiber::{Command, Shared};
use crate::*;
use std::{any::Any, panic::AssertUnwindSafe, sync::atomic::AtomicU64, sync::atomic::Ordering};
use tokio::sync::Notify;

pub(crate) type Captured = Arc<dyn Any + Send + Sync>;
type CaptureFn = dyn Fn() -> Option<Captured> + Send + Sync;

struct Entry {
    id: u64,
    /// Uids of the registering fiber and its ancestors: the function applies to the disposal of
    /// any of them (the registrant is inside the disposed subtree), not to unrelated fibers.
    owner: Vec<u32>,
    capture: Arc<CaptureFn>,
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Capture functions of one kernel, in registration order.
pub(crate) struct CaptureRegistry {
    /// Unique among the kernels of the process and never reused: scopes that outlive a kernel
    /// cannot be mistaken for the scopes of a later one.
    generation: u64,
    entries: Mutex<Vec<Entry>>,
    next: AtomicU64,
    /// Woken whenever a running teardown receives more values.
    pub(crate) changed: Arc<Notify>,
}

impl Default for CaptureRegistry {
    fn default() -> Self {
        Self {
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            entries: Mutex::default(),
            next: AtomicU64::new(0),
            changed: Arc::default(),
        }
    }
}

impl CaptureRegistry {
    fn add(&self, owner: Vec<u32>, capture: Arc<CaptureFn>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.entries).push(Entry { id, owner, capture });
        id
    }
    fn remove(&self, id: u64) {
        lock(&self.entries).retain(|entry| entry.id != id);
    }
    /// Run the functions that apply to disposing `target` (all of them without one) in the
    /// calling task. A function that panics contributes nothing.
    fn capture(&self, target: Option<u32>, log: &Logger) -> Vec<Captured> {
        let entries = lock(&self.entries)
            .iter()
            .filter(|entry| target.is_none_or(|uid| entry.owner.contains(&uid)))
            .map(|entry| entry.capture.clone())
            .collect::<Vec<_>>();
        let mut values = vec![];
        for capture in entries {
            match std::panic::catch_unwind(AssertUnwindSafe(|| capture())) {
                Ok(Some(value)) => values.push(value),
                Ok(None) => {}
                Err(_) => log.warn(format_args!(
                    "dispose capture function panicked; its value is dropped"
                )),
            }
        }
        values
    }
}

/// What the callers captured for one teardown. Shared and live: a call that arrives after the
/// teardown started is visible to it.
#[derive(Default)]
pub(crate) struct CaptureSet {
    values: Mutex<Vec<Captured>>,
}

impl CaptureSet {
    pub(crate) fn extend(&self, values: Vec<Captured>, changed: &Notify) {
        if values.is_empty() {
            return;
        }
        lock(&self.values).extend(values);
        changed.notify_waiters();
    }
}

/// The sets a teardown task carries, tagged with the kernel they belong to.
pub(crate) struct Carried {
    kernel: u64,
    sets: Vec<Arc<CaptureSet>>,
    changed: Arc<Notify>,
}

tokio::task_local! { static DISPOSE_CAPTURES: Arc<Carried>; }

pub(crate) fn kernel_id(shared: &Arc<Shared>) -> u64 {
    shared.captures.generation
}

pub(crate) fn carried(shared: &Arc<Shared>, sets: Vec<Arc<CaptureSet>>) -> Arc<Carried> {
    Arc::new(Carried {
        kernel: kernel_id(shared),
        sets,
        changed: shared.captures.changed.clone(),
    })
}

pub(crate) fn current() -> Option<Arc<Carried>> {
    DISPOSE_CAPTURES.try_with(Clone::clone).ok()
}

pub(crate) fn scope<F: Future>(
    carried: Arc<Carried>,
    future: F,
) -> impl Future<Output = F::Output> {
    DISPOSE_CAPTURES.scope(carried, future)
}

/// Everything a disposal in the calling task hands over: the registered functions' values, and
/// what the teardown task the caller itself runs in already carries (a disposal started from a
/// cleanup passes the original caller's values on).
pub(crate) fn collect(shared: &Arc<Shared>, target: Option<u32>, log: &Logger) -> Vec<Captured> {
    let mut values = shared.captures.capture(target, log);
    if let Some(carried) = current().filter(|carried| carried.kernel == kernel_id(shared)) {
        for set in &carried.sets {
            values.extend(lock(&set.values).iter().cloned());
        }
    }
    values
}

/// Values of one kernel in scope, each distinct value once, oldest first.
fn values_of<T: Send + Sync + 'static>(kernel: u64) -> Option<Vec<Arc<T>>> {
    DISPOSE_CAPTURES
        .try_with(|carried| {
            if carried.kernel != kernel {
                return None;
            }
            let mut seen: Vec<*const ()> = vec![];
            let mut out = vec![];
            for set in &carried.sets {
                for value in lock(&set.values).iter() {
                    let id = Arc::as_ptr(value) as *const ();
                    if seen.contains(&id) {
                        continue;
                    }
                    seen.push(id);
                    if let Ok(value) = value.clone().downcast::<T>() {
                        out.push(value);
                    }
                }
            }
            Some(out)
        })
        .ok()
        .flatten()
}

impl Context {
    /// Register a function that disposing this fiber or one of its ancestors
    /// (`Fiber::dispose()`, `Kernel::shutdown()`, `Disposer::dispose()`) runs in its caller's
    /// task: the value it returns is handed to the teardown through
    /// [`dispose_captures`](Self::dispose_captures).
    ///
    /// The function runs synchronously inside the disposing call, before the command is sent:
    /// it must be instantaneous, must only read (typically a `tokio::task_local!` of the
    /// caller) and must not block. A panic is caught, the value dropped and a warning logged.
    /// It is unregistered when the returned disposer runs or this fiber unloads.
    pub fn capture_on_dispose<T: Send + Sync + 'static>(
        &self,
        capture: impl Fn() -> Option<Arc<T>> + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError> {
        let inner = self.inner.clone();
        let owner = self.data.ancestry.clone();
        self.try_effect("kernel.captureOnDispose()", move |undos| {
            let id = inner.captures.add(
                owner,
                Arc::new(move || capture().map(|value| value as Captured)),
            );
            undos.push(Undo::sync(move || inner.captures.remove(id)))
        })
    }

    /// The values of type `T` that the callers of the disposal this task is tearing down
    /// captured: one per call, for the disposed fiber and its ancestors (a descendant is
    /// unloaded because an ancestor was disposed), each distinct value once. Empty outside a
    /// teardown, for the teardown of another kernel, and for a disposal of an unrelated fiber.
    /// Tasks the kernel starts for the teardown (effect cleanups) see the same values.
    pub fn dispose_captures<T: Send + Sync + 'static>(&self) -> Vec<Arc<T>> {
        values_of(kernel_id(&self.inner)).unwrap_or_default()
    }

    /// Wait until the running teardown has more than `seen` values of type `T`, and return all
    /// of them. A disposal that arrives while the teardown is already running adds its values
    /// here. Never resolves outside a teardown of this kernel.
    pub async fn next_dispose_captures<T: Send + Sync + 'static>(
        &self,
        seen: usize,
    ) -> Vec<Arc<T>> {
        let kernel = kernel_id(&self.inner);
        let Some(changed) = DISPOSE_CAPTURES
            .try_with(|carried| (carried.kernel == kernel).then(|| carried.changed.clone()))
            .ok()
            .flatten()
        else {
            return std::future::pending().await;
        };
        loop {
            let notified = changed.notified();
            tokio::pin!(notified);
            // Register interest before reading so a value added in between is not missed.
            notified.as_mut().enable();
            let values = values_of::<T>(kernel).unwrap_or_default();
            if values.len() > seen {
                return values;
            }
            notified.await;
        }
    }
}

/// Run the capture for a disposal and hand the values to the actor in order before `command`.
pub(crate) fn send_captures(shared: &Arc<Shared>, uid: u32, log: &Logger) {
    let values = collect(shared, Some(uid), log);
    if !values.is_empty() {
        let _ = shared.tx.send(Command::Capture(uid, values));
    }
}

#[cfg(test)]
#[path = "dispose_capture_tests.rs"]
mod tests;
