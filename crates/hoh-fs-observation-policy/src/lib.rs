//! Event-only read-before-mutation policy for filesystem operations.

pub mod types;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use hoh_error::BoxError;
use hoh_fs::{
    FsActor, FsDeleteIntent, FsEditIntent, FsError, FsErrorCode, FsObservation, FsObserved,
    FsTarget, FsVersionGuard, FsWriteIntent, FsWriteIntentEvent,
};
use hoh_kernel::{Context, Undo, plugin};
use hoh_session::Session;
use hoh_tools::{Agent, ToolExecution};

pub use types::{FsObservationActor, FsObservationAgent};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct OwnerState {
    owner: StoredOwner,
    targets: HashMap<String, FsObservation>,
}

enum Owner {
    Tool(Arc<dyn Agent>),
    Direct(Session),
}

impl Owner {
    fn session(&self) -> Session {
        match self {
            Self::Tool(agent) => agent.session().clone(),
            Self::Direct(session) => session.clone(),
        }
    }
}

enum StoredOwner {
    Tool(Weak<dyn Agent>),
    Direct(Session),
}

impl StoredOwner {
    fn new(owner: &Owner) -> Self {
        match owner {
            Owner::Tool(agent) => Self::Tool(Arc::downgrade(agent)),
            Owner::Direct(session) => Self::Direct(session.clone()),
        }
    }

    fn is_alive(&self) -> bool {
        match self {
            Self::Tool(agent) => agent.strong_count() != 0,
            Self::Direct(_) => true,
        }
    }

    fn matches(&self, owner: &Owner) -> bool {
        let session = owner.session();
        match self {
            Self::Tool(agent) => agent
                .upgrade()
                .is_some_and(|agent| agent.session() == &session),
            Self::Direct(stored) => stored == &session,
        }
    }
}

#[derive(Default)]
struct ObservedStateGate {
    observed: Mutex<Vec<OwnerState>>,
}

impl ObservedStateGate {
    fn owner(actor: &Option<FsActor>) -> Option<Owner> {
        let actor = actor.as_ref()?;
        if let Some(execution) = actor.downcast_ref::<ToolExecution>() {
            return execution.agent.clone().map(Owner::Tool);
        }
        actor
            .downcast_ref::<FsObservationActor>()
            .and_then(FsObservationActor::session)
            .map(Owner::Direct)
    }

    fn get(&self, owner: &Owner, target: &FsTarget) -> Option<FsObservation> {
        let mut observed = lock(&self.observed);
        observed.retain(|state| state.owner.is_alive());
        observed
            .iter()
            .find(|state| state.owner.matches(owner))
            .and_then(|state| state.targets.get(target.target_key.as_str()))
            .cloned()
    }

    fn set(&self, owner: Owner, target: &FsTarget, observation: FsObservation) {
        let mut observed = lock(&self.observed);
        observed.retain(|state| state.owner.is_alive());
        let state = if let Some(index) = observed
            .iter()
            .position(|state| state.owner.matches(&owner))
        {
            &mut observed[index]
        } else {
            observed.push(OwnerState {
                owner: StoredOwner::new(&owner),
                targets: HashMap::new(),
            });
            // The vector was just extended, so its last element exists.
            observed.last_mut().expect("owner state was inserted")
        };
        state
            .targets
            .insert(target.target_key.to_string(), observation);
    }

    fn clear(&self) {
        lock(&self.observed).clear();
    }

    fn write_intent(&self, target: &FsTarget, actor: &Option<FsActor>) -> FsWriteIntent {
        match Self::owner(actor).and_then(|owner| self.get(&owner, target)) {
            Some(FsObservation::Present { version }) => FsWriteIntent::ReplaceIfVersion { version },
            Some(FsObservation::Absent) | None => FsWriteIntent::CreateIfAbsent,
        }
    }

    fn require_present(
        &self,
        target: &FsTarget,
        actor: &Option<FsActor>,
        verb: &str,
    ) -> Result<FsVersionGuard, BoxError> {
        let prior = Self::owner(actor).and_then(|owner| self.get(&owner, target));
        match prior {
            Some(FsObservation::Present { version }) => Ok(FsVersionGuard { version }),
            Some(FsObservation::Absent) => Err(FsError::new(
                format!("cannot {verb} \"{}\": not found", target.display_path),
                FsErrorCode::NotFound,
            )
            .into()),
            None => Err(FsError::new(
                format!("{verb} requires reading \"{}\" first", target.display_path),
                FsErrorCode::NotObserved,
            )
            .into()),
        }
    }

    fn observe(&self, event: &FsObserved) {
        if let Some(owner) = Self::owner(&event.actor) {
            self.set(owner, &event.target, event.observation.clone());
        }
    }
}

/// Register the four `fs/*` event listeners. This plugin provides no service.
#[plugin(FsObservationPolicy, name = "fs-observation-policy")]
pub async fn apply(ctx: Context) -> Result<(), BoxError> {
    let gate = Arc::new(ObservedStateGate::default());

    let clear = gate.clone();
    ctx.effect("fs-observation-policy observed-state teardown", || {
        Undo::sync(move || clear.clear())
    })?;

    let writes = gate.clone();
    ctx.on_waterfall::<FsWriteIntentEvent, Option<FsWriteIntent>, _>(move |event, _next| {
        let intent = writes.write_intent(&event.target, &event.actor);
        async move { Ok(Some(intent)) }
    })?;

    let edits = gate.clone();
    ctx.on_waterfall::<FsEditIntent, Option<FsVersionGuard>, _>(move |event, _next| {
        let intent = edits.require_present(&event.target, &event.actor, "edit");
        async move { intent.map(Some) }
    })?;

    let deletes = gate.clone();
    ctx.on_waterfall::<FsDeleteIntent, Option<FsVersionGuard>, _>(move |event, _next| {
        let intent = deletes.require_present(&event.target, &event.actor, "delete");
        async move { intent.map(Some) }
    })?;

    ctx.on::<FsObserved>(move |event| gate.observe(event))?;
    Ok(())
}
