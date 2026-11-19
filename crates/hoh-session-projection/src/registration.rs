use crate::{
    registry::{ErasedState, Registration, Release},
    *,
};
use futures_util::future::BoxFuture;
use hoh_error::SharedError;
use hoh_kernel::{Context, Undo};
use hoh_session::{Session, SessionEvent, SessionHeader, SessionLogOffset};
use serde::Serialize;
use std::{
    any::Any,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
};

pub struct ProjectionHandle<S> {
    pub(crate) registry: SessionProjectionRegistry,
    pub(crate) registration: Arc<Registration>,
    pub(crate) release: Arc<Release>,
    pub(crate) _state: PhantomData<fn() -> S>,
}
impl<S> Clone for ProjectionHandle<S> {
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            registration: self.registration.clone(),
            release: self.release.clone(),
            _state: PhantomData,
        }
    }
}
impl<S: Send + Sync + 'static> ProjectionHandle<S> {
    pub fn state_of(&self, session: &Session) -> Result<Arc<S>> {
        typed(
            self.registry
                .read_registration(&self.registration, session)?,
        )
    }
    pub fn reader_for(&self, session: Session) -> ProjectionReader<S> {
        let handle = self.clone();
        Arc::new(move || handle.state_of(&session))
    }
    pub fn dispose(&self) -> BoxFuture<'static, ()> {
        self.release.dispose()
    }
}
pub struct ProjectionRegistration<S> {
    pub(crate) ready: std::result::Result<ProjectionHandle<S>, SharedError>,
    pub(crate) owner: Context,
    pub(crate) observed: Arc<AtomicBool>,
    pub(crate) handoff: Arc<Handoff>,
    pub(crate) release: Arc<Release>,
    pub(crate) yielded: bool,
}
impl<S> ProjectionRegistration<S> {
    /// Native counterpart of calling the old callable disposer before await.
    pub fn dispose(&self) -> BoxFuture<'static, ()> {
        self.release.dispose()
    }
    /// Transfer the original kernel effect into a composing effect.
    pub fn into_undo(self) -> Undo {
        self.release.take_undo()
    }
}
impl<S> Future for ProjectionRegistration<S> {
    type Output = Result<ProjectionHandle<S>>;
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.observed.store(true, Ordering::Release);
        if !this.yielded {
            this.yielded = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if this.ready.is_ok()
            && let Err(error) = settle_handoff(&this.handoff, &this.owner)
        {
            this.ready = Err(error.clone());
        }
        Poll::Ready(match &this.ready {
            Ok(handle) => Ok(handle.clone()),
            Err(error) => Err(error.clone().into()),
        })
    }
}
/// Owner-activity verdict taken once, at the asynchronous handoff.
pub(crate) type Handoff = OnceLock<std::result::Result<(), SharedError>>;
pub(crate) fn settle_handoff(
    handoff: &Handoff,
    owner: &Context,
) -> std::result::Result<(), SharedError> {
    handoff
        .get_or_init(|| owner.ensure_active().map_err(SharedError::new))
        .clone()
}
pub(crate) fn typed<S: Send + Sync + 'static>(state: ErasedState) -> Result<Arc<S>> {
    state
        .downcast::<S>()
        .map_err(|_| error("session projection state has an incompatible Rust type"))
}
pub(crate) type Parse = Arc<dyn Fn(Option<&Value>) -> Result<ErasedState> + Send + Sync>;
pub(crate) type Init =
    Arc<dyn Fn(&SessionHeader, SessionLogOffset) -> Result<ErasedState> + Send + Sync>;
pub(crate) type Apply =
    Arc<dyn Fn(&ErasedState, &SessionEvent) -> Result<ErasedState> + Send + Sync>;
pub(crate) type Encode = Arc<dyn Fn(&ErasedState) -> Result<Option<Value>> + Send + Sync>;
pub(crate) type Same = Arc<dyn Fn(&ErasedState, &ErasedState) -> Result<bool> + Send + Sync>;
pub(crate) type View = Arc<dyn Fn(&ErasedState) -> Result<Option<Arc<Value>>> + Send + Sync>;
pub(crate) struct ErasedWire {
    pub schema: ProjectionViewSchema,
    pub view: View,
    pub summary: bool,
}
pub(crate) struct ErasedDefinition {
    pub key: String,
    pub state_version: u64,
    pub parse: Parse,
    pub init: Init,
    pub apply: Apply,
    pub encode: Encode,
    pub same: Same,
    pub wire: Option<ErasedWire>,
}
pub(crate) fn erase<S: Serialize + Send + Sync + 'static>(
    definition: ProjectionDefinition<S>,
) -> ErasedDefinition {
    let schema = definition.state_schema;
    let init = definition.init;
    let apply = definition.apply;
    let wire = definition.wire.map(|wire| ErasedWire {
        schema: wire.view_schema,
        summary: wire.summary,
        view: Arc::new(move |state| (wire.view)(&typed::<S>(state.clone())?)),
    });
    ErasedDefinition {
        key: definition.key,
        state_version: definition.state_version,
        wire,
        parse: Arc::new(move |value| Ok(schema(value)? as Arc<dyn Any + Send + Sync>)),
        init: Arc::new(move |header, cut| Ok(init(header, cut)? as Arc<dyn Any + Send + Sync>)),
        apply: Arc::new(move |state, event| {
            Ok(apply(&typed::<S>(state.clone())?, event)? as Arc<dyn Any + Send + Sync>)
        }),
        encode: Arc::new(move |state| Ok(state_json::encode(typed::<S>(state.clone())?.as_ref())?)),
        same: Arc::new(move |left, right| {
            if Arc::ptr_eq(left, right) {
                return Ok(true);
            }
            let left = state_json::encode(typed::<S>(left.clone())?.as_ref())?;
            let right = state_json::encode(typed::<S>(right.clone())?.as_ref())?;
            Ok(same_primitive(left.as_ref(), right.as_ref()))
        }),
    }
}
pub(crate) fn same_primitive(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(a)), Some(Value::Bool(b))) => a == b,
        (Some(Value::String(a)), Some(Value::String(b))) => a == b,
        (Some(Value::Number(a)), Some(Value::Number(b))) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) if a == 0. && b == 0. => {
                a.is_sign_negative() == b.is_sign_negative()
            }
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
        _ => false,
    }
}
pub(crate) fn same_view(left: Option<&Arc<Value>>, right: Option<&Arc<Value>>) -> bool {
    if let (Some(left), Some(right)) = (left, right)
        && Arc::ptr_eq(left, right)
    {
        return true;
    }
    same_primitive(left.map(AsRef::as_ref), right.map(AsRef::as_ref))
}
