use crate::*;
use hoh_error::BoxError;
use hoh_kernel::{Contained, Event, ListenerFailure};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
#[derive(Default)]
pub struct DispatchValidation(Mutex<Option<BoxError>>);
impl DispatchValidation {
    pub fn reject(&self, error: BoxError) {
        let mut slot = lock(&self.0);
        if slot.is_none() {
            *slot = Some(error);
        }
    }
    pub(crate) fn take(&self) -> Result<()> {
        lock(&self.0).take().map_or(Ok(()), Err)
    }
}
pub struct SessionEventEmitted {
    pub session: Session,
    pub event: Arc<SessionEvent>,
    pub validation: DispatchValidation,
}
impl SessionEventEmitted {
    pub(crate) fn new(session: Session, event: Arc<SessionEvent>) -> Self {
        Self {
            session,
            event,
            validation: Default::default(),
        }
    }
}
impl Event for SessionEventEmitted {
    const NAME: &'static str = "session/event";
    type Mode = Contained;
}
pub struct SessionCreated {
    pub session: Session,
    pub validation: DispatchValidation,
    pub(crate) veto: Mutex<Option<BoxError>>,
}
impl Event for SessionCreated {
    const NAME: &'static str = "session/created";
    type Mode = Contained;
}
pub struct SessionDisposed {
    pub session: Session,
    pub validation: DispatchValidation,
}
impl Event for SessionDisposed {
    const NAME: &'static str = "session/disposed";
    type Mode = Contained;
}
pub(crate) type FlushFuture = Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>;
pub struct SessionFlush {
    pub session: Session,
    pub validation: DispatchValidation,
    pub(crate) pending: Mutex<Vec<FlushFuture>>,
}
impl SessionFlush {
    pub fn wait_for(&self, future: impl Future<Output = Result<()>> + Send + 'static) {
        let mut future: FlushFuture = Box::pin(future);
        // JavaScript invokes an async listener's prefix before the next listener.
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let queued: FlushFuture = match capture(|| future.as_mut().poll(&mut context)) {
            Ok(std::task::Poll::Ready(result)) => Box::pin(std::future::ready(result)),
            Ok(std::task::Poll::Pending) => future,
            Err(error) => Box::pin(std::future::ready(Err(error))),
        };
        lock(&self.pending).push(queued);
    }
}
impl Event for SessionFlush {
    const NAME: &'static str = "session/flush";
    type Mode = Contained;
}
pub(crate) fn report_observer(
    log: &hoh_kernel::Logger,
    id: &SessionId,
    name: &str,
    failure: ListenerFailure,
) {
    let (kind, error) = match failure {
        ListenerFailure::Threw(e) => ("threw", e),
        ListenerFailure::Rejected(e) => ("rejected", e),
    };
    let error_name = if let Some(error) = hoh_error::HarnessError::of(error.as_ref()) {
        error.name()
    } else if error.downcast_ref::<SessionForkError>().is_some() {
        "SessionForkError"
    } else if let Some(error) = error.downcast_ref::<SessionError>() {
        error.name()
    } else {
        "Error"
    };
    log.warn(format_args!(
        "session \"{id}\": {name} listener {kind}: {error_name}: {error}"
    ));
}
