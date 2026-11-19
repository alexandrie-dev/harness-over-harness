//! Optional startup observation for waterfall. The existing dispatch path is unchanged.
use super::{Callback, Error, Event, Listener, Next, Out, Value, Waterfall, unpack};
use crate::{Context, fiber, lock};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
};
use tokio::sync::oneshot;

/// Completion of the initial polling pass, independently of the answer.
///
/// Covers callback factories and their first future poll, including continuations
/// polled through `next.run()` during that pass. A pending user answer is not
/// awaited. Empty chains, early failures and unwinding also release the signal.
/// A continuation first polled after its parent's first poll is not part of this
/// initial pass. Dropping this observer does not cancel dispatch.
pub struct WaterfallStarted(oneshot::Receiver<()>);
impl Future for WaterfallStarted {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
        Pin::new(&mut self.get_mut().0).poll(cx).map(|_| ())
    }
}

struct Startup {
    pending: usize,
    ready: Option<oneshot::Sender<()>>,
}
#[derive(Clone)]
struct Parent {
    group: Arc<Mutex<Startup>>,
    polling: Arc<AtomicBool>,
}
impl Parent {
    fn child(&self) -> Option<Ticket> {
        if !self.polling.load(Ordering::Acquire) {
            return None;
        }
        let mut group = lock(&self.group);
        group.ready.as_ref()?;
        // Every increment owns one ticket, bounded by live dispatch tasks.
        group.pending += 1;
        Some(Ticket(Parent {
            group: self.group.clone(),
            polling: Arc::new(AtomicBool::new(false)),
        }))
    }
}
struct Ticket(Parent);
impl Ticket {
    fn root() -> (Self, WaterfallStarted) {
        let (send, receive) = oneshot::channel();
        (
            Self(Parent {
                group: Arc::new(Mutex::new(Startup {
                    pending: 1,
                    ready: Some(send),
                })),
                polling: Arc::new(AtomicBool::new(false)),
            }),
            WaterfallStarted(receive),
        )
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.0.polling.store(false, Ordering::Release);
        let ready = {
            let mut group = lock(&self.0.group);
            // A non-cloneable ticket decrements its one increment exactly once.
            group.pending -= 1;
            if group.pending == 0 {
                group.ready.take()
            } else {
                None
            }
        };
        // A woken observer may re-enter dispatch: never wake under the mutex.
        if let Some(ready) = ready {
            let _ = ready.send(());
        }
    }
}

struct FirstPoll<F> {
    future: Pin<Box<F>>,
    ticket: Option<Ticket>,
}
impl<F: Future> FirstPoll<F> {
    fn new(future: F, ticket: Option<Ticket>) -> Self {
        Self {
            future: Box::pin(future),
            ticket,
        }
    }
}
impl<F: Future> Future for FirstPoll<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        // Keep the ticket on the stack so a callback panic releases it as well.
        let ticket = this.ticket.take();
        if let Some(ticket) = &ticket {
            ticket.0.polling.store(true, Ordering::Release);
        }
        let result = this.future.as_mut().poll(cx);
        drop(ticket);
        result
    }
}

// This separate path deliberately preserves the original chain's task boundaries,
// liveness checks and error forwarding. Only the initial polls gain tickets.
fn chain(
    list: Arc<Vec<Arc<Listener>>>,
    index: usize,
    event: Value,
    tail: Next<Option<Out>>,
    parent: Option<Parent>,
) -> Next<Option<Out>> {
    Next {
        call: Box::new(move || {
            Box::pin(async move {
                let ticket = parent.as_ref().and_then(Parent::child);
                let descendant = ticket.as_ref().map(|ticket| ticket.0.clone());
                let task = fiber::spawn_inherited(FirstPoll::new(
                    async move {
                        let mut index = index;
                        while index < list.len() {
                            let listener = &list[index];
                            index += 1;
                            if !listener.alive() {
                                continue;
                            }
                            if let Callback::Async(f) = &listener.callback {
                                return f(
                                    event.clone(),
                                    chain(list.clone(), index, event, tail, descendant),
                                )
                                .await;
                            }
                        }
                        tail.run().await
                    },
                    ticket,
                ));
                task.await.map_err(|error| Box::new(error) as Error)?
            })
        }),
    }
}

impl Context {
    /// Start a waterfall with a separate signal for the initial polling pass.
    ///
    /// Returns `(started, answer)`. Awaiting `started` observes callback startup
    /// through initially delegated `next.run()` futures, without waiting for a
    /// pending answer. The result, filtering, order, liveness and panic/error
    /// forwarding match `waterfall`. Neither returned future cancels dispatch
    /// when dropped. Existing `waterfall` and its callers are unaffected.
    pub fn waterfall_with_start<E, R, Fut>(
        &self,
        event: impl Into<Arc<E>>,
        tail: impl FnOnce(Arc<E>) -> Fut + Send + 'static,
    ) -> (
        WaterfallStarted,
        impl Future<Output = Result<R, Error>> + Send + 'static,
    )
    where
        E: Event<Mode = Waterfall<R>>,
        R: Send + 'static,
        Fut: Future<Output = Result<R, Error>> + Send + 'static,
    {
        let event = event.into();
        let list = Arc::new(self.resolve(event.as_ref()));
        let tail_event = event.clone();
        let tail = Next {
            call: Box::new(move || {
                Box::pin(async move { Ok(Some(Box::new(tail(tail_event).await?) as Out)) })
            }),
        };
        let (ticket, started) = Ticket::root();
        let parent = ticket.0.clone();
        let task = fiber::spawn_inherited(FirstPoll::new(
            async move { unpack(chain(list, 0, event, tail, Some(parent)).run().await?) },
            Some(ticket),
        ));
        (started, async move {
            task.await.map_err(|error| Box::new(error) as Error)?
        })
    }
}
