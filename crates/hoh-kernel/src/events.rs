use crate::*;
use futures_util::{
    FutureExt,
    stream::{FuturesOrdered, StreamExt},
};
use std::{any::TypeId, collections::HashMap, marker::PhantomData, panic::AssertUnwindSafe};
pub trait Event: Send + Sync + 'static {
    const NAME: &'static str;
    type Mode: Mode;
}
mod sealed {
    pub trait Sealed {}
}
pub trait Mode: sealed::Sealed + 'static {
    const DISPATCH: Dispatch;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch {
    Emit,
    Parallel,
    Serial,
    Bail,
    Waterfall,
}
pub struct Emit;
pub struct Parallel;
pub struct Serial<R>(PhantomData<fn() -> R>);
pub struct Bail<R>(PhantomData<fn() -> R>);
pub struct Waterfall<R>(PhantomData<fn() -> R>);
macro_rules! mode {
    ($t:ty, $v:ident) => {
        impl sealed::Sealed for $t {}
        impl Mode for $t {
            const DISPATCH: Dispatch = Dispatch::$v;
        }
    };
}
mode!(Emit, Emit);
mode!(Parallel, Parallel);
impl<R: 'static> sealed::Sealed for Serial<R> {}
impl<R: 'static> Mode for Serial<R> {
    const DISPATCH: Dispatch = Dispatch::Serial;
}
impl<R: 'static> sealed::Sealed for Bail<R> {}
impl<R: 'static> Mode for Bail<R> {
    const DISPATCH: Dispatch = Dispatch::Bail;
}
impl<R: 'static> sealed::Sealed for Waterfall<R> {}
impl<R: 'static> Mode for Waterfall<R> {
    const DISPATCH: Dispatch = Dispatch::Waterfall;
}
#[macro_export]
macro_rules! event {
    ($event:ty = $name:literal, $mode:ty) => {
        impl $crate::Event for $event {
            const NAME: &'static str = $name;
            type Mode = $mode;
        }
    };
}
pub struct Next<R> {
    pub(crate) call: Box<dyn FnOnce() -> Task<Result<R, Error>> + Send>,
}
impl<R: Send + 'static> Next<R> {
    pub fn run(self) -> impl Future<Output = Result<R, Error>> + Send + 'static {
        (self.call)()
    }
}
type Out = Box<dyn Any + Send>;
type SyncHandler = dyn Fn(&(dyn Any + Send + Sync)) -> Option<Out> + Send + Sync;
type AsyncHandler =
    dyn Fn(Value, Next<Option<Out>>) -> Task<Result<Option<Out>, Error>> + Send + Sync;
type DispatchHandler = dyn Fn(Dispatch, &'static str, &(dyn Any + Send + Sync)) + Send + Sync;
pub(crate) enum Callback {
    Sync(Box<SyncHandler>),
    Async(Box<AsyncHandler>),
    Dispatch(Box<DispatchHandler>),
}
pub(crate) struct Listener {
    ctx: Context,
    global: bool,
    effect: Arc<effects::Effect>,
    callback: Callback,
}
impl Listener {
    fn alive(&self) -> bool {
        !self.effect.is_running()
    }
}
#[derive(Default)]
pub(crate) struct Bus {
    listeners: RwLock<HashMap<TypeId, Vec<Arc<Listener>>>>,
    dispatch: RwLock<Vec<Arc<Listener>>>,
}
impl Bus {
    pub(crate) fn clear(&self) {
        let listeners = std::mem::take(&mut *write(&self.listeners));
        let dispatch = std::mem::take(&mut *write(&self.dispatch));
        drop(listeners);
        drop(dispatch);
    }
}
pub struct Listen<'a> {
    ctx: &'a Context,
    prepend: bool,
    global: bool,
}
impl<'a> Listen<'a> {
    pub fn on_update<C, Fut>(
        self,
        handler: impl Fn(Arc<C>, Next<()>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        C: Config,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let disposer = self.ctx.on_update::<C, Fut>(handler)?;
        if self.prepend {
            let mut hooks = write(&self.ctx.data.hooks);
            if let Some(index) = hooks.updates.iter().position(|h| {
                h.effect
                    .upgrade()
                    .is_some_and(|e| Arc::ptr_eq(&e, &disposer.effect))
            }) {
                let hook = hooks.updates.remove(index);
                hooks.updates.insert(0, hook);
            }
        }
        Ok(disposer)
    }
    pub fn prepend(mut self) -> Self {
        self.prepend = true;
        self
    }
    pub fn global(mut self) -> Self {
        self.global = true;
        self
    }
    fn register(self, key: Option<TypeId>, callback: Callback) -> Result<Disposer, KernelError> {
        let disposer = self.ctx.reserve_effect("ctx.on()")?;
        let listener = Arc::new(Listener {
            ctx: self.ctx.clone(),
            global: self.global,
            effect: disposer.effect.clone(),
            callback,
        });
        if let Some(key) = key {
            let mut bus = write(&self.ctx.inner.bus.listeners);
            let list = bus.entry(key).or_default();
            if self.prepend {
                list.insert(0, listener.clone());
            } else {
                list.push(listener.clone());
            }
        } else {
            write(&self.ctx.inner.bus.dispatch).push(listener.clone());
        }
        let inner = Arc::downgrade(&self.ctx.inner);
        disposer.effect.add(Undo::sync(move || {
            if let Some(inner) = inner.upgrade() {
                if let Some(key) = key {
                    let mut bus = write(&inner.bus.listeners);
                    if let Some(list) = bus.get_mut(&key) {
                        list.retain(|l| !Arc::ptr_eq(l, &listener));
                        if list.is_empty() {
                            bus.remove(&key);
                        }
                    }
                } else {
                    write(&inner.bus.dispatch).retain(|l| !Arc::ptr_eq(l, &listener));
                }
            }
        }));
        disposer.effect.finish_setup();
        Ok(disposer)
    }
    pub fn on<E>(self, f: impl Fn(&E) + Send + Sync + 'static) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Emit>,
    {
        self.register(
            Some(TypeId::of::<E>()),
            Callback::Sync(Box::new(move |e| {
                if let Some(e) = e.downcast_ref::<E>() {
                    f(e);
                }
                None
            })),
        )
    }
    pub fn on_bail<E, R>(
        self,
        f: impl Fn(&E) -> Option<R> + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Bail<R>>,
        R: Send + 'static,
    {
        self.register(
            Some(TypeId::of::<E>()),
            Callback::Sync(Box::new(move |e| {
                e.downcast_ref::<E>()
                    .and_then(&f)
                    .map(|r| Box::new(r) as Out)
            })),
        )
    }
    pub fn on_parallel<E, Fut>(
        self,
        f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Parallel>,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        self.register(
            Some(TypeId::of::<E>()),
            Callback::Async(Box::new(move |e, _| {
                let e = e.downcast::<E>();
                let future = e.ok().map(&f);
                Box::pin(async move {
                    if let Some(f) = future {
                        f.await?;
                    }
                    Ok(None)
                })
            })),
        )
    }
    pub fn on_serial<E, R, Fut>(
        self,
        f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Serial<R>>,
        R: Send + 'static,
        Fut: Future<Output = Result<Option<R>, Error>> + Send + 'static,
    {
        self.register(
            Some(TypeId::of::<E>()),
            Callback::Async(Box::new(move |e, _| {
                let future = e.downcast::<E>().ok().map(&f);
                Box::pin(async move {
                    match future {
                        Some(f) => Ok(f.await?.map(|r| Box::new(r) as Out)),
                        None => Ok(None),
                    }
                })
            })),
        )
    }
    pub fn on_waterfall<E, R, Fut>(
        self,
        f: impl Fn(Arc<E>, Next<R>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Waterfall<R>>,
        R: Send + 'static,
        Fut: Future<Output = Result<R, Error>> + Send + 'static,
    {
        self.register(
            Some(TypeId::of::<E>()),
            Callback::Async(Box::new(move |e, next| {
                let next = Next {
                    call: Box::new(move || Box::pin(async move { unpack(next.run().await?) })),
                };
                let future = e.downcast::<E>().ok().map(|e| f(e, next));
                Box::pin(async move {
                    match future {
                        Some(f) => Ok(Some(Box::new(f.await?) as Out)),
                        None => Err("event type mismatch".into()),
                    }
                })
            })),
        )
    }
}
fn unpack<R: Send + 'static>(out: Option<Out>) -> Result<R, Error> {
    out.and_then(|o| o.downcast::<R>().ok())
        .map(|v| *v)
        .ok_or_else(|| "event result type mismatch".into())
}
fn tail_none() -> Next<Option<Out>> {
    Next {
        call: Box::new(|| Box::pin(async { Ok(None) })),
    }
}
fn chain(
    list: Arc<Vec<Arc<Listener>>>,
    index: usize,
    event: Value,
    tail: Next<Option<Out>>,
) -> Next<Option<Out>> {
    Next {
        call: Box::new(move || {
            Box::pin(async move {
                let mut index = index;
                while index < list.len() {
                    let listener = &list[index];
                    index += 1;
                    if !listener.alive() {
                        continue;
                    }
                    if let Callback::Async(f) = &listener.callback {
                        return f(event.clone(), chain(list.clone(), index, event, tail)).await;
                    }
                }
                tail.run().await
            })
        }),
    }
}
impl Context {
    pub fn listen(&self) -> Listen<'_> {
        Listen {
            ctx: self,
            prepend: false,
            global: false,
        }
    }
    pub fn on<E>(&self, f: impl Fn(&E) + Send + Sync + 'static) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Emit>,
    {
        self.listen().on::<E>(f)
    }
    pub fn on_bail<E, R>(
        &self,
        f: impl Fn(&E) -> Option<R> + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Bail<R>>,
        R: Send + 'static,
    {
        self.listen().on_bail::<E, R>(f)
    }
    pub fn on_parallel<E, Fut>(
        &self,
        f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Parallel>,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        self.listen().on_parallel::<E, Fut>(f)
    }
    pub fn on_serial<E, R, Fut>(
        &self,
        f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Serial<R>>,
        R: Send + 'static,
        Fut: Future<Output = Result<Option<R>, Error>> + Send + 'static,
    {
        self.listen().on_serial::<E, R, Fut>(f)
    }
    pub fn on_waterfall<E, R, Fut>(
        &self,
        f: impl Fn(Arc<E>, Next<R>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        E: Event<Mode = Waterfall<R>>,
        R: Send + 'static,
        Fut: Future<Output = Result<R, Error>> + Send + 'static,
    {
        self.listen().on_waterfall::<E, R, Fut>(f)
    }
    pub fn on_dispatch(
        &self,
        f: impl Fn(Dispatch, &'static str, &(dyn Any + Send + Sync)) + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError> {
        self.listen()
            .register(None, Callback::Dispatch(Box::new(f)))
    }
    fn dispatch<E: Event>(&self, event: &E) -> Vec<Arc<Listener>> {
        if !E::NAME.starts_with("internal/") {
            let observers = read(&self.inner.bus.dispatch).clone();
            for l in observers {
                if l.alive()
                    && let Callback::Dispatch(f) = &l.callback
                    && std::panic::catch_unwind(AssertUnwindSafe(|| {
                        f(E::Mode::DISPATCH, E::NAME, event)
                    }))
                    .is_err()
                {
                    self.logger().error(format_args!(
                        "event \"{}\" dispatch observer panicked",
                        E::NAME
                    ));
                }
            }
        }
        let list = read(&self.inner.bus.listeners)
            .get(&TypeId::of::<E>())
            .cloned()
            .unwrap_or_default();
        list.into_iter()
            .filter(|l| {
                (l.global && TypeId::of::<E>() != TypeId::of::<VolatileUpdate>())
                    || self.filter.as_ref().is_none_or(|f| f(&l.ctx))
            })
            .collect()
    }
    pub fn emit<E: Event<Mode = Emit>>(&self, event: &E) {
        for l in self.dispatch(event) {
            if l.alive()
                && let Callback::Sync(f) = &l.callback
            {
                if E::NAME.starts_with("internal/")
                    || TypeId::of::<E>() == TypeId::of::<VolatileUpdate>()
                {
                    if std::panic::catch_unwind(AssertUnwindSafe(|| f(event))).is_err() {
                        self.logger()
                            .error(format_args!("event \"{}\" listener panicked", E::NAME));
                    }
                } else {
                    f(event);
                }
            }
        }
    }
    pub fn bail<E, R>(&self, event: &E) -> Option<R>
    where
        E: Event<Mode = Bail<R>>,
        R: Send + 'static,
    {
        for l in self.dispatch(event) {
            if l.alive()
                && let Callback::Sync(f) = &l.callback
                && let Some(r) = f(event).and_then(|r| r.downcast::<R>().ok())
            {
                return Some(*r);
            }
        }
        None
    }
    pub fn parallel<E: Event<Mode = Parallel>>(
        &self,
        event: impl Into<Arc<E>>,
    ) -> impl Future<Output = Result<(), KernelError>> + Send + 'static {
        let event = event.into();
        let list = self.dispatch(event.as_ref());
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut tasks = FuturesOrdered::new();
            for l in list {
                if l.alive()
                    && let Callback::Async(f) = &l.callback
                {
                    let future = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        f(event.clone(), tail_none())
                    }));
                    tasks.push_back(async move {
                        if !l.alive() {
                            return Ok(());
                        }
                        match future {
                            Ok(f) => match AssertUnwindSafe(f).catch_unwind().await {
                                Ok(r) => r.map(|_| ()),
                                Err(_) => Err("event listener panicked".into()),
                            },
                            Err(_) => Err("event listener panicked".into()),
                        }
                    });
                }
            }
            let results = tasks.collect::<Vec<_>>().await;
            let errors = results
                .into_iter()
                .filter_map(Result::err)
                .map(Arc::new)
                .collect::<Vec<_>>();
            let _ = tx.send(if errors.is_empty() {
                Ok(())
            } else {
                Err(KernelError::Listeners {
                    event: E::NAME,
                    errors,
                })
            });
        });
        async move { rx.await.unwrap_or(Err(KernelError::Closed)) }
    }
    pub fn serial<E, R>(
        &self,
        event: impl Into<Arc<E>>,
    ) -> impl Future<Output = Result<Option<R>, Error>> + Send + 'static
    where
        E: Event<Mode = Serial<R>>,
        R: Send + 'static,
    {
        let event = event.into();
        let list = self.dispatch(event.as_ref());
        let task = tokio::spawn(async move {
            for l in list {
                if l.alive()
                    && let Callback::Async(f) = &l.callback
                    && let Some(out) = f(event.clone(), tail_none()).await?
                {
                    return unpack(Some(out)).map(Some);
                }
            }
            Ok(None)
        });
        async move { task.await.map_err(|e| Box::new(e) as Error)? }
    }
    pub fn waterfall<E, R, Fut>(
        &self,
        event: impl Into<Arc<E>>,
        tail: impl FnOnce(Arc<E>) -> Fut + Send + 'static,
    ) -> impl Future<Output = Result<R, Error>> + Send + 'static
    where
        E: Event<Mode = Waterfall<R>>,
        R: Send + 'static,
        Fut: Future<Output = Result<R, Error>> + Send + 'static,
    {
        let event = event.into();
        let list = Arc::new(self.dispatch(event.as_ref()));
        let tail_event = event.clone();
        let tail = Next {
            call: Box::new(move || {
                Box::pin(async move { Ok(Some(Box::new(tail(tail_event).await?) as Out)) })
            }),
        };
        let task = tokio::spawn(async move { unpack(chain(list, 0, event, tail).run().await?) });
        async move { task.await.map_err(|e| Box::new(e) as Error)? }
    }
}
pub struct InternalPlugin {
    pub fiber: Fiber,
}
pub struct InternalStatus {
    pub fiber: Fiber,
    pub old: FiberState,
}
pub struct InternalService {
    pub name: Arc<str>,
    pub realm: Realm,
    pub available: bool,
}
pub struct VolatileUpdate {
    pub paths: Vec<String>,
}
event!(InternalPlugin = "internal/plugin", Emit);
event!(InternalStatus = "internal/status", Emit);
event!(InternalService = "internal/service", Emit);
event!(VolatileUpdate = "loader/volatile-update", Emit);
