//! Event routing for the local registry: subscriptions file by filter, and every commit
//! dispatches once to each matching listener with containment.
//!
//! Port of `src/events.ts`.

use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_jobs::{JobEvent, JobEventFilter, JobEventListener, JobEventOwners};
use hoh_kernel::{Context, Disposer, KernelError, Undo};
use hoh_scope::{AnonymousEntries, ScopeLayer, ScopedLayers, scope_of, store::EffectOptions};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

/// One registered listener and the filter it declared.
#[derive(Clone)]
pub struct Subscription {
    filter: JobEventFilter,
    listener: JobEventListener,
}

/// One scope's contributions: the job controllers attached from it and the
/// `{ owners: 'scope' }` subscriptions registered there. Both tables are anonymous because a
/// contribution is identified by its own disposer, never by a name a second registrant could
/// shadow.
#[derive(Default)]
pub struct JobLayer {
    /// The job controllers attached from this scope.
    pub controllers: AnonymousEntries<()>,
    /// The `{ owners: 'scope' }` subscriptions registered from this scope.
    pub scoped: AnonymousEntries<Subscription>,
}

impl ScopeLayer for JobLayer {
    fn is_empty(&self) -> bool {
        self.controllers.is_empty() && self.scoped.is_empty()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn kernel_error(error: BoxError) -> KernelError {
    match error.downcast::<KernelError>() {
        Ok(error) => *error,
        Err(error) => KernelError::Failed(Arc::new(error)),
    }
}

fn event_type(event: &JobEvent) -> &'static str {
    match event {
        JobEvent::Registered { .. } => "registered",
        JobEvent::Progress { .. } => "progress",
        JobEvent::Stopping { .. } => "stopping",
        JobEvent::Removed { .. } => "removed",
        JobEvent::Settled { .. } => "settled",
        JobEvent::Output { .. } => "output",
    }
}

/// Routes events to subscriptions. `{ owner }` and `{ owners: 'all' }` subscriptions are
/// evaluated against every event regardless of where they were registered; `{ owners: 'scope' }`
/// subscriptions file into the registering context's scope layer and receive the owners composed
/// under it, and the global layer, reached from an unscoped context, receives every owner.
pub struct JobEventHub {
    layers: ScopedLayers<JobLayer>,
    unscoped: Arc<Mutex<Vec<(u64, Subscription)>>>,
    next: AtomicU64,
    ctx: Context,
}

impl JobEventHub {
    pub fn new(layers: ScopedLayers<JobLayer>, ctx: Context) -> Self {
        Self {
            layers,
            unscoped: Arc::default(),
            next: AtomicU64::new(1),
            ctx,
        }
    }

    pub fn layers(&self) -> &ScopedLayers<JobLayer> {
        &self.layers
    }

    /// Register one listener as an effect of `ctx`.
    pub fn subscribe(
        &self,
        ctx: &Context,
        filter: JobEventFilter,
        listener: JobEventListener,
    ) -> Result<Disposer, KernelError> {
        let scoped = matches!(
            filter,
            JobEventFilter::Owners {
                owners: JobEventOwners::Scope
            }
        );
        let subscription = Subscription { filter, listener };
        if scoped {
            return self
                .layers
                .effect(
                    ctx,
                    |layer| Ok(layer.scoped.append(subscription)),
                    EffectOptions::new("jobs.events.subscribe()"),
                )
                .map_err(kernel_error);
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let table = self.unscoped.clone();
        ctx.effect("jobs.events.subscribe()", move || {
            lock(&table).push((id, subscription));
            Undo::sync(move || lock(&table).retain(|(entry, _)| *entry != id))
        })
    }

    /// Deliver one event to every matching subscription, containing each listener so an observer
    /// cannot break the commit already made.
    pub fn emit(&self, event: &JobEvent, owner: Option<&Arc<dyn Agent>>) {
        let owner_id = owner.map(|agent| agent.id().clone());
        let unscoped: Vec<Subscription> = lock(&self.unscoped)
            .iter()
            .map(|(_, subscription)| subscription.clone())
            .collect();
        for subscription in unscoped {
            if let JobEventFilter::Owner { owner: wanted } = &subscription.filter
                && owner_id.as_ref().is_some_and(|id| id != wanted)
            {
                continue;
            }
            self.deliver(&subscription, event);
        }
        for subscription in self.layers.global.scoped.values() {
            self.deliver(&subscription, event);
        }
        let scope = owner.and_then(|agent| scope_of(agent.ctx()));
        for layer in self.layers.chain_layers(scope.as_ref()) {
            for subscription in layer.scoped.values() {
                self.deliver(&subscription, event);
            }
        }
    }

    fn deliver(&self, subscription: &Subscription, event: &JobEvent) {
        let outcome = catch_unwind(AssertUnwindSafe(|| (subscription.listener)(event)));
        let failure = match outcome {
            Ok(Ok(())) => return,
            Ok(Err(error)) => format!("Error: {error}"),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".to_owned());
                format!("Error: {message}")
            }
        };
        self.ctx.logger().warn(format_args!(
            "jobs: event listener threw on {}: {failure}",
            event_type(event)
        ));
    }
}
