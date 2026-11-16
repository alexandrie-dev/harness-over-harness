use crate::*;
use futures_util::FutureExt;
use indexmap::{IndexMap, IndexSet};
use serde_json::Value as Json;
use std::{
    any::TypeId,
    collections::HashMap,
    panic::AssertUnwindSafe,
    sync::{
        Weak,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot};
#[cfg(test)]
#[path = "fiber_review_tests.rs"]
mod review_tests;
tokio::task_local! { static APPLY_ANCESTRY: Vec<u32>; }
pub(crate) fn sync_scope<R>(ancestry: Vec<u32>, f: impl FnOnce() -> R) -> R {
    APPLY_ANCESTRY.sync_scope(ancestry, f)
}
pub(crate) fn spawn_for<F>(ancestry: Vec<u32>, future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(APPLY_ANCESTRY.scope(ancestry, future))
}
pub(crate) fn spawn_inherited<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    spawn_for(
        APPLY_ANCESTRY.try_with(Clone::clone).unwrap_or_default(),
        future,
    )
}
type Reply<T> = oneshot::Sender<T>;
type ServiceAddress = (Arc<str>, Realm);
type Apply = dyn Fn(Context, View, Arc<RwLock<Value>>) -> Task<Result<(), Error>> + Send + Sync;
type Parse = dyn Fn(&Json) -> Result<(Json, Value), KernelError> + Send + Sync;
type Candidate = dyn Fn(&Value) -> Result<(), Error> + Send + Sync;
type Update = dyn Fn(Value, Next<()>) -> Task<Result<(), Error>> + Send + Sync;
pub trait Plugin: Send + Sync + 'static {
    const NAME: &'static str;
    type Config: Config;
    type Deps: Inject;
    fn apply(
        ctx: Context,
        deps: Self::Deps,
        config: Cfg<Self::Config>,
    ) -> impl Future<Output = Result<(), Error>> + Send;
}
#[derive(Clone)]
pub struct PluginDef {
    name: &'static str,
    schema: Arc<Json>,
    deps: Arc<Vec<Dep>>,
    pub(crate) config_type: TypeId,
    parse: Arc<Parse>,
    apply: Arc<Apply>,
}
impl PluginDef {
    pub fn of<P: Plugin>() -> Self {
        let schema =
            serde_json::to_value(schemars::schema_for!(P::Config)).unwrap_or(Json::Bool(false));
        let parse_schema = schema.clone();
        Self {
            name: P::NAME,
            schema: Arc::new(schema),
            deps: Arc::new(P::Deps::deps()),
            config_type: TypeId::of::<P::Config>(),
            parse: Arc::new(move |raw| {
                let mut normalized =
                    if TypeId::of::<P::Config>() == TypeId::of::<NoConfig>() && raw.is_null() {
                        serde_json::json!({})
                    } else {
                        config::normalized(&parse_schema, raw)
                    };
                config::validate(&parse_schema, &normalized)?;
                config::coerce_integers(&parse_schema, &parse_schema, &mut normalized);
                let parsed: P::Config = serde_json::from_value(normalized.clone())
                    .map_err(|e| KernelError::Validation(config::issue(e.to_string())))?;
                let serialized = serde_json::to_value(&parsed)
                    .map_err(|e| KernelError::Validation(config::issue(e.to_string())))?;
                let effective = config::merge(normalized, serialized);
                Ok((effective, Arc::new(parsed)))
            }),
            apply: Arc::new(|ctx, view, value| {
                Box::pin(async move {
                    P::apply(
                        ctx,
                        P::Deps::resolve(&view),
                        Cfg {
                            value,
                            marker: std::marker::PhantomData,
                        },
                    )
                    .await
                })
            }),
        }
    }
    pub(crate) fn injected<D, Fut>(body: impl Fn(Context, D) -> Fut + Send + Sync + 'static) -> Self
    where
        D: Inject,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let mut def = Self::of::<Empty>();
        def.name = "";
        def.deps = Arc::new(D::deps());
        def.apply = Arc::new(move |ctx, view, _| Box::pin(body(ctx, D::resolve(&view))));
        def
    }
    pub fn name(&self) -> &'static str {
        self.name
    }
    pub fn schema(&self) -> &Json {
        &self.schema
    }
    pub fn deps(&self) -> &[Dep] {
        &self.deps
    }
}
struct Empty;
impl Plugin for Empty {
    const NAME: &'static str = "root";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        Ok(())
    }
}
#[derive(Default)]
pub struct PluginOptions {
    pub entry_id: Option<Arc<str>>,
    pub inject: Vec<Arc<str>>,
    pub resolver: Option<Arc<dyn ConfigResolver>>,
}
pub trait ConfigResolver: Send + Sync + 'static {
    fn resolve(&self, ctx: &Context, raw: &Json) -> Result<Json, Error>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState {
    Pending,
    Loading,
    Active,
    Failed,
    Disposed,
    Unloading,
}
pub struct Snapshot {
    pub fibers: Vec<FiberInfo>,
    pub provisions: Vec<ProvisionInfo>,
}
pub struct FiberInfo {
    pub uid: u32,
    pub parent: u32,
    pub name: Arc<str>,
    pub entry_id: Option<Arc<str>>,
    pub state: FiberState,
    pub missing: Vec<Arc<str>>,
    pub error: Option<String>,
}
pub struct ProvisionInfo {
    pub name: Arc<str>,
    pub realm: Realm,
    pub owner: u32,
    pub active: bool,
    pub dependents: Vec<u32>,
}
pub(crate) struct PublishedFiber {
    state: FiberState,
    disposed: bool,
    raw: Arc<Json>,
    effective: Option<Arc<Json>>,
    error: Option<KernelError>,
}
#[derive(Default)]
pub(crate) struct Hooks {
    pub(crate) candidates: Vec<Arc<CandidateHook>>,
    pub(crate) updates: Vec<Arc<UpdateHook>>,
}
pub(crate) struct CandidateHook {
    pub(crate) effect: Weak<effects::Effect>,
    pub(crate) check: Box<Candidate>,
}
pub(crate) struct UpdateHook {
    pub(crate) effect: Weak<effects::Effect>,
    pub(crate) handler: Box<Update>,
}
pub(crate) struct FiberData {
    pub(crate) uid: u32,
    parent: u32,
    pub(crate) name: Arc<str>,
    entry_id: Option<Arc<str>>,
    pub(crate) def: PluginDef,
    state: RwLock<PublishedFiber>,
    pub(crate) hooks: RwLock<Hooks>,
    resolver: Option<Arc<dyn ConfigResolver>>,
    inject: Vec<Arc<str>>,
    pub(crate) ancestry: Vec<u32>,
    parent_effect: Weak<effects::Effect>,
}
impl FiberData {
    pub(crate) fn declares(&self, name: &str) -> bool {
        self.def.deps.iter().any(|dep| dep.name == name)
            || self.inject.iter().any(|dep| dep.as_ref() == name)
    }
}
#[derive(Clone)]
pub struct Fiber {
    pub(crate) inner: Arc<Shared>,
    pub(crate) data: Arc<FiberData>,
}
impl Fiber {
    /// The exact parent-owned child effect, suitable for adoption by an ordered composite.
    pub fn disposer(&self) -> Option<Disposer> {
        self.data
            .parent_effect
            .upgrade()
            .map(|effect| Disposer { effect })
    }
    pub fn uid(&self) -> u32 {
        self.data.uid
    }
    pub fn name(&self) -> Arc<str> {
        self.data.name.clone()
    }
    pub fn entry_id(&self) -> Option<Arc<str>> {
        self.data.entry_id.clone()
    }
    pub fn state(&self) -> FiberState {
        read(&self.data.state).state
    }
    pub fn is_disposed(&self) -> bool {
        read(&self.data.state).disposed
    }
    pub fn raw_config(&self) -> Arc<Json> {
        read(&self.data.state).raw.clone()
    }
    pub fn effective_config(&self) -> Option<Arc<Json>> {
        read(&self.data.state).effective.clone()
    }
    pub fn resolve_config(&self, candidate: &Json) -> Result<Json, KernelError> {
        self.resolve(candidate).map(|v| v.0)
    }
    fn resolve(&self, raw: &Json) -> Result<(Json, Value), KernelError> {
        std::panic::catch_unwind(AssertUnwindSafe(|| self.resolve_inner(raw))).unwrap_or_else(
            |_| {
                Err(KernelError::Validation(config::issue(
                    "configuration resolution panicked",
                )))
            },
        )
    }
    fn resolve_inner(&self, raw: &Json) -> Result<(Json, Value), KernelError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(KernelError::Closed);
        }
        let ctx = read(&self.inner.contexts)
            .get(&self.uid())
            .cloned()
            .ok_or(KernelError::Inactive)?;
        let raw = if let Some(resolver) = &self.data.resolver {
            resolver
                .resolve(&ctx, raw)
                .map_err(|e| KernelError::Validation(config::issue(e.to_string())))?
        } else {
            raw.clone()
        };
        let (effective, typed) = (self.data.def.parse)(&raw)?;
        let hooks = read(&self.data.hooks).candidates.clone();
        for hook in hooks {
            if hook.effect.upgrade().is_some_and(|e| !e.is_running()) {
                (hook.check)(&typed)
                    .map_err(|e| KernelError::Validation(config::issue(e.to_string())))?;
            }
        }
        Ok((effective, typed))
    }
    fn wait_reply(
        &self,
        mut rx: oneshot::Receiver<Result<(), KernelError>>,
    ) -> impl Future<Output = Result<(), KernelError>> + Send + 'static {
        let uid = self.uid();
        let closed = self.inner.closed_token.clone();
        async move {
            match rx.try_recv() {
                Ok(result) => return result,
                Err(oneshot::error::TryRecvError::Closed) => return Err(KernelError::Closed),
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
            if APPLY_ANCESTRY
                .try_with(|a| a.contains(&uid))
                .unwrap_or(false)
            {
                return Err(KernelError::SelfWait);
            }
            tokio::select! {biased;result=rx=>result.unwrap_or(Err(KernelError::Closed)),_=closed.cancelled()=>Err(KernelError::Closed)}
        }
    }
    pub fn settled(&self) -> impl Future<Output = Result<(), KernelError>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::Wait(self.uid(), tx));
        let fiber = self.clone();
        let closed = self.inner.closed_token.clone();
        async move {
            let settlement = tokio::select! {biased;result=rx=>result.map_err(|_|KernelError::Closed)?,_=closed.cancelled()=>return Err(KernelError::Closed)};
            match settlement {
                Settlement::Ready(result) => result,
                Settlement::Waiting(rx) => fiber.wait_reply(rx).await,
            }
        }
    }
    pub fn update(
        &self,
        raw: Json,
    ) -> impl Future<Output = Result<(), KernelError>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::Update(self.uid(), raw, tx));
        self.wait_reply(rx)
    }
    /// Retain source input without notifying or replacing the running instance (Cordis rawConfig setter).
    pub fn retain_raw_config(
        &self,
        raw: Json,
    ) -> impl Future<Output = Result<(), KernelError>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::RetainRaw(self.uid(), raw, tx));
        async move { rx.await.unwrap_or(Err(KernelError::Closed)) }
    }
    pub fn restart(&self) -> impl Future<Output = Result<(), KernelError>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::Restart(self.uid(), tx));
        self.wait_reply(rx)
    }
    fn restart_from_update(
        &self,
    ) -> impl Future<Output = Result<(), KernelError>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::Restart(self.uid(), tx));
        let closed = self.inner.closed_token.clone();
        async move {
            tokio::select! {biased;result=rx=>result.unwrap_or(Err(KernelError::Closed)),_=closed.cancelled()=>Err(KernelError::Closed)}
        }
    }
    pub fn commit_volatile(
        &self,
        raw: Json,
    ) -> impl Future<Output = Result<bool, KernelError>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::Volatile(self.uid(), raw, tx));
        async move { rx.await.unwrap_or(Err(KernelError::Closed)) }
    }
    pub fn dispose(&self) -> impl Future<Output = ()> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::Dispose(self.uid(), tx));
        let uid = self.uid();
        let log = self.log();
        async move {
            if APPLY_ANCESTRY
                .try_with(|a| a.contains(&uid))
                .unwrap_or(false)
            {
                log.kernel_error(&KernelError::SelfWait);
                return;
            }
            let _ = rx.await;
        }
    }
    fn log(&self) -> Logger {
        read(&self.inner.contexts)
            .get(&self.uid())
            .cloned()
            .unwrap_or_else(|| {
                Context::base(
                    self.inner.clone(),
                    self.data.clone(),
                    self.inner.root_activation.clone(),
                )
            })
            .logger()
    }
}
pub(crate) struct Provision {
    id: u64,
    pub(crate) name: Arc<str>,
    realm: Realm,
    owner: u32,
    pub(crate) key: TypeId,
    pub(crate) value: Value,
    pub(crate) view: fn(&Value) -> Option<Json>,
    count: ReferenceCount,
    withdrawn: AtomicBool,
    dependents: Mutex<IndexSet<u32>>,
}
impl Provision {
    pub(crate) fn owner(&self) -> u32 {
        self.owner
    }
    pub(crate) fn withdrawn(&self) -> bool {
        self.withdrawn.load(Ordering::Acquire)
    }
}
struct Binding {
    provision: Arc<Provision>,
    waiters: Vec<Reply<()>>,
}
pub(crate) struct Shared {
    pub(crate) tx: mpsc::UnboundedSender<Command>,
    pub(crate) root: Arc<FiberData>,
    pub(crate) root_activation: Arc<effects::Activation>,
    pub(crate) closed: AtomicBool,
    closed_token: tokio_util::sync::CancellationToken,
    next_uid: AtomicU32,
    pub(crate) published: RwLock<IndexMap<ServiceAddress, Arc<Provision>>>,
    pub(crate) own_published: RwLock<IndexMap<ServiceAddress, Arc<Provision>>>,
    pub(crate) contexts: RwLock<HashMap<u32, Context>>,
    pub(crate) bus: events::Bus,
    pub(crate) logs: Arc<Mutex<logger::Logs>>,
}
#[derive(Clone)]
pub struct Kernel {
    inner: Arc<Shared>,
}
impl Kernel {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let def = PluginDef::of::<Empty>();
        let root = Arc::new(FiberData {
            uid: 0,
            parent: 0,
            name: "root".into(),
            entry_id: None,
            def,
            state: RwLock::new(PublishedFiber {
                state: FiberState::Active,
                disposed: false,
                raw: Arc::new(Json::Null),
                effective: None,
                error: None,
            }),
            hooks: RwLock::default(),
            resolver: None,
            inject: vec![],
            ancestry: vec![0],
            parent_effect: Weak::new(),
        });
        let inner = Arc::new(Shared {
            tx,
            root: root.clone(),
            root_activation: effects::Activation::new(View::empty()),
            closed: AtomicBool::new(false),
            closed_token: tokio_util::sync::CancellationToken::new(),
            next_uid: AtomicU32::new(1),
            published: RwLock::default(),
            own_published: RwLock::default(),
            contexts: RwLock::default(),
            bus: events::Bus::default(),
            logs: Arc::new(Mutex::new(logger::Logs::default())),
        });
        let ctx = Context::base(inner.clone(), root, inner.root_activation.clone());
        write(&inner.contexts).insert(0, ctx.clone());
        let mut actor = Actor {
            shared: inner.clone(),
            nodes: IndexMap::new(),
            bindings: IndexMap::new(),
            types: HashMap::new(),
            next_binding: 1,
            shutting_down: false,
            root_done: false,
            shutdown_waiters: vec![],
            notified: IndexMap::new(),
            #[cfg(test)]
            type_scans: 0,
        };
        actor.nodes.insert(0, Node::root(ctx));
        tokio::spawn(async move {
            let _ = AssertUnwindSafe(actor.run(rx)).catch_unwind().await;
        });
        Self { inner }
    }
    pub fn root(&self) -> Context {
        Context::base(
            self.inner.clone(),
            self.inner.root.clone(),
            self.inner.root_activation.clone(),
        )
    }
    pub fn snapshot(&self) -> impl Future<Output = Snapshot> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::Snapshot(tx));
        async move {
            rx.await.unwrap_or(Snapshot {
                fibers: vec![],
                provisions: vec![],
            })
        }
    }
    pub fn shutdown(&self) -> impl Future<Output = ()> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let _ = self.inner.tx.send(Command::Shutdown(tx));
        async move {
            let _ = rx.await;
        }
    }
}
impl Context {
    pub(crate) fn create(
        &self,
        def: PluginDef,
        raw: Json,
        options: PluginOptions,
    ) -> Result<Fiber, KernelError> {
        self.create_context(def, raw, options, false)
            .map(|ctx| ctx.fiber())
    }
    /// A synchronously usable, named child context backed by a no-op plugin.
    pub fn child(&self, name: &'static str) -> Result<Context, KernelError> {
        let mut def = PluginDef::of::<Empty>();
        def.name = name;
        self.create_context(def, Json::Null, PluginOptions::default(), true)
    }
    fn create_context(
        &self,
        def: PluginDef,
        raw: Json,
        options: PluginOptions,
        eager: bool,
    ) -> Result<Context, KernelError> {
        let disposer = self.reserve_effect("ctx.plugin()")?;
        let uid = self.inner.next_uid.fetch_add(1, Ordering::Relaxed);
        let mut ancestry = self.data.ancestry.clone();
        ancestry.push(uid);
        let name = if def.name.is_empty() {
            self.data.name.clone()
        } else {
            Arc::from(def.name)
        };
        let data = Arc::new(FiberData {
            uid,
            parent: self.data.uid,
            name,
            entry_id: options.entry_id,
            def,
            state: RwLock::new(PublishedFiber {
                state: FiberState::Pending,
                disposed: false,
                raw: Arc::new(raw),
                effective: None,
                error: None,
            }),
            hooks: RwLock::default(),
            resolver: options.resolver,
            inject: options.inject,
            ancestry,
            parent_effect: Arc::downgrade(&disposer.effect),
        });
        let fiber = Fiber {
            inner: self.inner.clone(),
            data,
        };
        let activation = effects::Activation::new(View::empty());
        if !eager {
            activation.close();
        }
        let ctx = self.child_context(fiber.data.clone(), activation, eager);
        let child = fiber.clone();
        disposer.effect.add(Undo::future(move || async move {
            child.dispose().await;
            Ok(())
        }));
        if eager {
            let registered = {
                let mut contexts = write(&self.inner.contexts);
                if self.inner.closed.load(Ordering::Acquire) {
                    false
                } else {
                    contexts.insert(uid, ctx.clone());
                    true
                }
            };
            if !registered {
                disposer.effect.finish_setup();
                disposer.effect.begin();
                ctx.activation.close();
                ctx.activation.abandon();
                return Err(KernelError::Closed);
            }
        }
        let sent = self
            .inner
            .tx
            .send(Command::Create(ctx.clone(), self.clone(), eager));
        disposer.effect.finish_setup();
        if sent.is_err() {
            if eager {
                write(&self.inner.contexts).remove(&uid);
                ctx.activation.close();
                ctx.activation.abandon();
            }
            disposer.effect.begin();
            return Err(KernelError::Closed);
        }
        Ok(ctx)
    }
}
pub(crate) enum Settlement {
    Ready(Result<(), KernelError>),
    Waiting(oneshot::Receiver<Result<(), KernelError>>),
}
pub(crate) enum Command {
    #[cfg(test)]
    Fault,
    #[cfg(test)]
    TypeScans(Reply<usize>),
    Create(Context, Context, bool),
    Start(u32, u64),
    Loaded(u32, u64, Result<(), Error>),
    Unloaded(u32),
    Wait(u32, Reply<Settlement>),
    Restart(u32, Reply<Result<(), KernelError>>),
    Update(u32, Json, Reply<Result<(), KernelError>>),
    RetainRaw(u32, Json, Reply<Result<(), KernelError>>),
    UpdateDone(
        u32,
        u64,
        Arc<Json>,
        Json,
        Value,
        bool,
        Result<(), Error>,
        Reply<Result<(), KernelError>>,
    ),
    Volatile(u32, Json, Reply<Result<bool, KernelError>>),
    Dispose(u32, Reply<()>),
    Snapshot(Reply<Snapshot>),
    Shutdown(Reply<()>),
    Provide {
        ctx: Context,
        key: TypeId,
        name: Arc<str>,
        value: Value,
        view: fn(&Value) -> Option<Json>,
        count: ReferenceCount,
        disposer: Disposer,
        reply: Reply<Result<Disposer, KernelError>>,
    },
    Withdraw(u64, Reply<()>),
}
#[derive(Clone, PartialEq, Eq)]
struct Epoch {
    revision: u64,
    bindings: Vec<u64>,
}
struct Node {
    ctx: Context,
    target: Option<Epoch>,
    loaded: Option<Epoch>,
    revision: u64,
    generation: u64,
    unload_started: bool,
    waiters: Vec<Reply<Result<(), KernelError>>>,
    dispose_waiters: Vec<Reply<()>>,
    retired: Vec<ReferenceProbe>,
    committed: Arc<RwLock<View>>,
    preactivated: bool,
}
impl Node {
    fn root(ctx: Context) -> Self {
        Self {
            ctx,
            target: Some(Epoch {
                revision: 0,
                bindings: vec![],
            }),
            loaded: Some(Epoch {
                revision: 0,
                bindings: vec![],
            }),
            revision: 0,
            generation: 0,
            unload_started: false,
            waiters: vec![],
            dispose_waiters: vec![],
            retired: vec![],
            committed: Arc::new(RwLock::new(View::empty())),
            preactivated: false,
        }
    }
    fn state(&self) -> FiberState {
        read(&self.ctx.data.state).state
    }
    fn disposed(&self) -> bool {
        read(&self.ctx.data.state).disposed
    }
}
type ReferenceCount = Arc<dyn Fn() -> usize + Send + Sync>;
struct ReferenceProbe {
    name: Arc<str>,
    count: ReferenceCount,
}
struct Actor {
    shared: Arc<Shared>,
    nodes: IndexMap<u32, Node>,
    bindings: IndexMap<u64, Binding>,
    types: HashMap<Arc<str>, TypeId>,
    next_binding: u64,
    shutting_down: bool,
    root_done: bool,
    shutdown_waiters: Vec<Reply<()>>,
    notified: IndexMap<ServiceAddress, (u64, bool)>,
    #[cfg(test)]
    type_scans: usize,
}
impl Actor {
    async fn run(&mut self, mut rx: mpsc::UnboundedReceiver<Command>) {
        while let Some(command) = rx.recv().await {
            self.command(command);
            self.drive();
            if self.shutting_down && self.root_done && self.nodes.len() == 1 {
                break;
            }
        }
        self.shared.closed.store(true, Ordering::Release);
        for waiter in self.shutdown_waiters.drain(..) {
            let _ = waiter.send(());
        }
    }
    fn status(&self, uid: u32, state: FiberState) {
        let Some(node) = self.nodes.get(&uid) else {
            return;
        };
        let old = {
            let mut data = write(&node.ctx.data.state);
            let old = data.state;
            data.state = state;
            old
        };
        if old != state {
            node.ctx.emit(&InternalStatus {
                fiber: node.ctx.fiber(),
                old,
            });
        }
    }
    fn command(&mut self, command: Command) {
        match command {
            #[cfg(test)]
            Command::TypeScans(reply) => {
                let _ = reply.send(self.type_scans);
            }
            #[cfg(test)]
            Command::Fault => {
                panic!("injected actor fault");
            }
            Command::Create(ctx, parent, eager) => {
                let fiber = ctx.fiber();
                let mut node = Node::root(ctx.clone());
                node.preactivated = eager;
                if eager {
                    *write(&ctx.activation.committed) = Arc::downgrade(&node.committed);
                }
                node.target = None;
                node.loaded = None;
                if parent.ensure_active().is_err() {
                    write(&fiber.data.state).disposed = true;
                    ctx.activation.close();
                    if !eager {
                        write(&fiber.data.state).state = FiberState::Disposed;
                        if let Some(effect) = fiber.data.parent_effect.upgrade() {
                            effect.retire_child();
                        }
                        return;
                    }
                }
                write(&self.shared.contexts).insert(fiber.uid(), ctx.clone());
                self.nodes.insert(fiber.uid(), node);
                let disposed = read(&ctx.data.state).disposed;
                if !disposed {
                    ctx.emit(&InternalPlugin { fiber });
                }
            }
            Command::Start(uid, generation) => self.start(uid, generation),
            Command::Loaded(uid, generation, result) => self.loaded(uid, generation, result),
            Command::Unloaded(uid) => {
                let Some(node) = self.nodes.get_mut(&uid) else {
                    return;
                };
                let bindings =
                    std::mem::replace(&mut node.committed, Arc::new(RwLock::new(View::empty())));
                for p in read(&bindings).values.values() {
                    lock(&p.dependents).shift_remove(&uid);
                }
                drop(bindings);
                node.loaded = None;
                node.unload_started = false;
                let disposed = node.disposed();
                let failed = read(&node.ctx.data.state).error.is_some();
                let activation = effects::Activation::new(View::empty());
                activation.close();
                node.ctx = node
                    .ctx
                    .activation_context(node.ctx.data.clone(), activation);
                write(&self.shared.contexts).insert(uid, node.ctx.clone());
                for probe in node.retired.drain(..) {
                    if (probe.count)() > 0 {
                        node.ctx.logger().warn(format_args!(
                            "service \"{}\" is still referenced after its provider unloaded",
                            probe.name
                        ));
                    }
                }
                if uid == 0 {
                    node.unload_started = true;
                    self.root_done = true;
                }
                self.status(
                    uid,
                    if uid == 0 {
                        FiberState::Active
                    } else if disposed {
                        FiberState::Disposed
                    } else if failed {
                        FiberState::Failed
                    } else {
                        FiberState::Pending
                    },
                );
            }
            Command::Wait(uid, reply) => {
                if let Some(node) = self.nodes.get_mut(&uid) {
                    if matches!(node.state(), FiberState::Loading | FiberState::Unloading) {
                        let (tx, rx) = oneshot::channel();
                        node.waiters.push(tx);
                        let _ = reply.send(Settlement::Waiting(rx));
                    } else {
                        let result = read(&node.ctx.data.state).error.clone().map_or(Ok(()), Err);
                        let _ = reply.send(Settlement::Ready(result));
                    }
                } else {
                    let _ = reply.send(Settlement::Ready(Ok(())));
                }
            }
            Command::Restart(uid, reply) => {
                if let Some(node) = self.nodes.get_mut(&uid) {
                    if node.disposed() {
                        let _ = reply.send(Err(KernelError::Inactive));
                    } else {
                        node.revision += 1;
                        node.waiters.push(reply);
                    }
                } else {
                    let _ = reply.send(Err(KernelError::Inactive));
                }
            }
            Command::Dispose(uid, reply) => {
                if uid == 0 {
                    self.command(Command::Shutdown(reply));
                    return;
                }
                if let Some(node) = self.nodes.get_mut(&uid) {
                    node.dispose_waiters.push(reply);
                    if !node.disposed() {
                        write(&node.ctx.data.state).disposed = true;
                        let ctx = node.ctx.clone();
                        ctx.emit(&InternalPlugin { fiber: ctx.fiber() });
                    }
                } else {
                    let _ = reply.send(());
                }
            }
            Command::Update(uid, raw, reply) => self.update(uid, raw, reply),
            Command::RetainRaw(uid, raw, reply) => {
                let result = if let Some(node) = self.nodes.get(&uid).filter(|n| !n.disposed()) {
                    write(&node.ctx.data.state).raw = Arc::new(raw);
                    Ok(())
                } else {
                    Err(KernelError::Inactive)
                };
                let _ = reply.send(result);
            }
            Command::UpdateDone(
                uid,
                generation,
                raw,
                effective,
                typed,
                restarted,
                result,
                reply,
            ) => {
                if let Some(node) = self.nodes.get_mut(&uid)
                    && result.is_ok()
                    && !restarted
                    && node.generation == generation
                    && !node.disposed()
                    && Arc::ptr_eq(&read(&node.ctx.data.state).raw, &raw)
                    && !lock(&node.ctx.activation.effects).closed
                {
                    write(&node.ctx.data.state).effective = Some(Arc::new(effective));
                    *write(&node.ctx.activation.cfg) = typed;
                }
                let _ = reply.send(result.map_err(|e| KernelError::Failed(Arc::new(e))));
            }
            Command::Volatile(uid, raw, reply) => {
                let result = self.volatile(uid, raw);
                let _ = reply.send(result);
            }
            Command::Provide {
                ctx,
                key,
                name,
                value,
                view,
                count,
                disposer,
                reply,
            } => {
                let address = (name.clone(), ctx.realm_of(&name));
                let error = if ctx.ensure_active().is_err() {
                    Some(KernelError::Inactive)
                } else if self.types.get(&name).is_some_and(|t| *t != key) {
                    Some(KernelError::ServiceType {
                        name: name.to_string(),
                    })
                } else {
                    self.bindings
                        .values()
                        .find(|b| {
                            b.provision.name == name
                                && b.provision.realm == address.1
                                && !b.provision.withdrawn.load(Ordering::Acquire)
                        })
                        .map(|b| KernelError::DuplicateService {
                            name: name.to_string(),
                            owner: self
                                .nodes
                                .get(&b.provision.owner)
                                .map(|n| n.ctx.data.name.to_string())
                                .unwrap_or_default(),
                        })
                };
                if let Some(error) = error {
                    disposer.effect.finish_setup();
                    disposer.effect.begin();
                    let _ = reply.send(Err(error));
                    return;
                }
                self.types.insert(name.clone(), key);
                let id = self.next_binding;
                self.next_binding += 1;
                let p = Arc::new(Provision {
                    id,
                    name,
                    realm: address.1,
                    owner: ctx.data.uid,
                    key,
                    value,
                    view,
                    count,
                    withdrawn: AtomicBool::new(false),
                    dependents: Mutex::default(),
                });
                self.bindings.insert(
                    id,
                    Binding {
                        provision: p,
                        waiters: vec![],
                    },
                );
                let tx = self.shared.tx.clone();
                let (withdraw, rx) = oneshot::channel();
                disposer.effect.on_begin(move || {
                    let _ = tx.send(Command::Withdraw(id, withdraw));
                });
                disposer.effect.add(Undo::future(move || async move {
                    let _ = rx.await;
                    Ok(())
                }));
                disposer.effect.finish_setup();
                self.publish();
                let _ = reply.send(Ok(disposer));
            }
            Command::Withdraw(id, reply) => {
                if let Some(binding) = self.bindings.get_mut(&id) {
                    binding.provision.withdrawn.store(true, Ordering::Release);
                    binding.waiters.push(reply);
                } else {
                    let _ = reply.send(());
                }
            }
            Command::Snapshot(reply) => {
                let _ = reply.send(self.snapshot());
            }
            Command::Shutdown(reply) => {
                self.shutdown_waiters.push(reply);
                self.shutting_down = true;
                if let Some(root) = self.nodes.get(&0) {
                    root.ctx.activation.close();
                }
                for (&uid, node) in &self.nodes {
                    if uid != 0 && !node.disposed() {
                        write(&node.ctx.data.state).disposed = true;
                        node.ctx.emit(&InternalPlugin {
                            fiber: node.ctx.fiber(),
                        });
                    }
                }
            }
        }
    }
    fn visible(&self, p: &Provision) -> bool {
        !p.withdrawn.load(Ordering::Acquire)
            && self.nodes.get(&p.owner).is_some_and(|n| {
                n.state() == FiberState::Active
                    && !n.disposed()
                    && !(p.owner == 0 && self.shutting_down)
            })
    }
    fn publish(&mut self) {
        let all = self
            .bindings
            .values()
            .filter(|b| !b.provision.withdrawn.load(Ordering::Acquire))
            .map(|b| {
                (
                    (b.provision.name.clone(), b.provision.realm),
                    b.provision.clone(),
                )
            })
            .collect::<IndexMap<_, _>>();
        let records = all
            .iter()
            .map(|(address, p)| (address.clone(), (p.id, self.visible(p))))
            .collect::<IndexMap<_, _>>();
        *write(&self.shared.own_published) = all;
        *write(&self.shared.published) = self
            .bindings
            .values()
            .filter(|b| self.visible(&b.provision))
            .map(|b| {
                (
                    (b.provision.name.clone(), b.provision.realm),
                    b.provision.clone(),
                )
            })
            .collect();
        let old = std::mem::replace(&mut self.notified, records);
        let root = Context::base(
            self.shared.clone(),
            self.shared.root.clone(),
            self.shared.root_activation.clone(),
        );
        for (address, (id, _)) in &old {
            if self.notified.get(address).is_none_or(|(new, _)| new != id) {
                root.emit(&InternalService {
                    name: address.0.clone(),
                    realm: address.1,
                    available: false,
                });
            }
        }
        for (address, record) in &self.notified {
            if record.1 && old.get(address) != Some(record) {
                root.emit(&InternalService {
                    name: address.0.clone(),
                    realm: address.1,
                    available: record.1,
                });
            }
        }
    }
    fn wanted(&self, node: &Node) -> (Option<Epoch>, View, Vec<Arc<str>>) {
        let mut view = View::empty();
        let mut ids = vec![];
        let mut missing = vec![];
        let table = read(&self.shared.published);
        for (name, key) in node
            .ctx
            .data
            .def
            .deps
            .iter()
            .map(|d| (Arc::<str>::from(d.name), Some(d.key)))
            .chain(node.ctx.data.inject.iter().cloned().map(|n| (n, None)))
        {
            let address = (name.clone(), node.ctx.realm_of(&name));
            if let Some(p) = table
                .get(&address)
                .filter(|p| key.is_none_or(|key| p.key == key))
            {
                ids.push(p.id);
                view.values.insert(address, p.clone());
            } else {
                missing.push(name);
            }
        }
        let allowed = !node.disposed() && read(&node.ctx.data.state).error.is_none();
        (
            if allowed && missing.is_empty() {
                Some(Epoch {
                    revision: node.revision,
                    bindings: ids,
                })
            } else {
                None
            },
            view,
            missing,
        )
    }
    fn drive(&mut self) {
        let mut release_types = false;
        loop {
            self.publish();
            let mut changed = false;
            let uids = self.nodes.keys().copied().collect::<Vec<_>>();
            for uid in uids.into_iter().filter(|u| *u != 0) {
                let (target, view, _) = self.wanted(&self.nodes[&uid]);
                let node = &mut self.nodes[&uid];
                node.target = target;
                if node.loaded != node.target
                    && matches!(node.state(), FiberState::Loading | FiberState::Active)
                {
                    node.ctx.activation.close();
                }
                match node.state() {
                    FiberState::Pending if node.preactivated && node.disposed() => {
                        node.preactivated = false;
                        node.ctx.activation.close();
                        self.status(uid, FiberState::Unloading);
                        changed = true;
                    }
                    FiberState::Pending if node.target.is_some() => {
                        node.generation += 1;
                        node.loaded = node.target.clone();
                        node.unload_started = false;
                        for p in view.values.values() {
                            if p.owner != uid {
                                lock(&p.dependents).insert(uid);
                            }
                        }
                        let activation = if node.preactivated {
                            node.preactivated = false;
                            *write(&node.ctx.activation.view) = view;
                            node.ctx.activation.clone()
                        } else {
                            effects::Activation::new(view)
                        };
                        node.committed = Arc::new(RwLock::new(read(&activation.view).clone()));
                        *write(&activation.committed) = Arc::downgrade(&node.committed);
                        node.ctx = node
                            .ctx
                            .activation_context(node.ctx.data.clone(), activation);
                        write(&self.shared.contexts).insert(uid, node.ctx.clone());
                        let generation = node.generation;
                        self.status(uid, FiberState::Loading);
                        let _ = self.shared.tx.send(Command::Start(uid, generation));
                        changed = true;
                    }
                    FiberState::Active if node.loaded != node.target => {
                        self.status(uid, FiberState::Unloading);
                        changed = true;
                    }
                    FiberState::Pending | FiberState::Failed if node.disposed() => {
                        self.status(uid, FiberState::Disposed);
                        changed = true;
                    }
                    FiberState::Failed if read(&node.ctx.data.state).error.is_none() => {
                        self.status(uid, FiberState::Pending);
                        changed = true;
                    }
                    _ => {}
                }
            }
            let ready = self
                .nodes
                .iter()
                .filter(|(uid, n)| {
                    (n.state() == FiberState::Unloading || (**uid == 0 && self.shutting_down))
                        && !n.unload_started
                        && !self.bindings.values().any(|b| {
                            b.provision.owner == **uid && !lock(&b.provision.dependents).is_empty()
                        })
                })
                .map(|(uid, _)| *uid)
                .collect::<Vec<_>>();
            for uid in ready {
                let node = &mut self.nodes[&uid];
                node.unload_started = true;
                let activation = node.ctx.activation.clone();
                let tx = self.shared.tx.clone();
                tokio::spawn(async move {
                    activation.unload().await;
                    let _ = tx.send(Command::Unloaded(uid));
                });
            }
            let withdrawn = self
                .bindings
                .iter()
                .filter(|(_, b)| {
                    b.provision.withdrawn.load(Ordering::Acquire)
                        && lock(&b.provision.dependents).is_empty()
                })
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            for id in withdrawn {
                if let Some(binding) = self.bindings.shift_remove(&id) {
                    release_types = true;
                    if let Some(owner) = self.nodes.get_mut(&binding.provision.owner) {
                        owner.retired.push(ReferenceProbe {
                            name: binding.provision.name.clone(),
                            count: binding.provision.count.clone(),
                        });
                    }
                    for waiter in binding.waiters {
                        let _ = waiter.send(());
                    }
                }
            }
            let disposed = self
                .nodes
                .iter()
                .filter(|(uid, n)| **uid != 0 && n.state() == FiberState::Disposed)
                .map(|(uid, _)| *uid)
                .collect::<Vec<_>>();
            for uid in disposed {
                if let Some(mut node) = self.nodes.shift_remove(&uid) {
                    release_types = true;
                    if let Some(effect) = node.ctx.data.parent_effect.upgrade() {
                        effect.retire_child();
                    }
                    let dispose_waiters = std::mem::take(&mut node.dispose_waiters);
                    let waiters = std::mem::take(&mut node.waiters);
                    write(&self.shared.contexts).remove(&uid);
                    drop(node);
                    for waiter in dispose_waiters {
                        let _ = waiter.send(());
                    }
                    for waiter in waiters {
                        let _ = waiter.send(Ok(()));
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for node in self.nodes.values_mut() {
            if !matches!(node.state(), FiberState::Loading | FiberState::Unloading) {
                let result = read(&node.ctx.data.state).error.clone().map_or(Ok(()), Err);
                for waiter in node.waiters.drain(..) {
                    let _ = waiter.send(result.clone());
                }
            }
        }
        if release_types {
            #[cfg(test)]
            {
                self.type_scans += 1;
            }
            self.types.retain(|name, _| {
                self.bindings.values().any(|b| b.provision.name == *name)
                    || self.nodes.values().any(|n| {
                        n.ctx.data.def.deps.iter().any(|d| d.name == name.as_ref())
                            || n.ctx.data.inject.contains(name)
                    })
            });
        }
    }
    fn start(&mut self, uid: u32, generation: u64) {
        let Some(node) = self.nodes.get(&uid) else {
            return;
        };
        if node.generation != generation {
            return;
        }
        if node.target != node.loaded || node.disposed() {
            self.loaded(uid, generation, Ok(()));
            return;
        }
        let resolved = node.ctx.fiber().resolve(&node.ctx.fiber().raw_config());
        match resolved {
            Err(error) => self.loaded(uid, generation, Err(Box::new(error))),
            Ok((effective, typed)) => {
                write(&node.ctx.data.state).effective = Some(Arc::new(effective));
                *write(&node.ctx.activation.cfg) = typed;
                let ctx = node.ctx.clone();
                let apply = ctx.data.def.apply.clone();
                let view = read(&node.committed).clone();
                let cfg = ctx.activation.cfg.clone();
                let tx = self.shared.tx.clone();
                let ancestry = ctx.data.ancestry.clone();
                tokio::spawn(async move {
                    let result = AssertUnwindSafe(
                        APPLY_ANCESTRY.scope(ancestry, async move { apply(ctx, view, cfg).await }),
                    )
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err("plugin apply panicked".into()));
                    let _ = tx.send(Command::Loaded(uid, generation, result));
                });
            }
        }
    }
    fn loaded(&mut self, uid: u32, generation: u64, result: Result<(), Error>) {
        let Some(node) = self.nodes.get_mut(&uid) else {
            return;
        };
        if node.generation != generation {
            return;
        }
        if let Err(error) = result {
            let stale = node.target != node.loaded || node.disposed();
            if stale
                && error
                    .downcast_ref::<KernelError>()
                    .is_some_and(|e| matches!(e, KernelError::Inactive))
            {
                node.ctx.logger().debug(&error);
            } else {
                if let Some(kernel) = error.downcast_ref::<KernelError>() {
                    node.ctx.logger().kernel_error(kernel);
                } else {
                    node.ctx.logger().error(&error);
                }
            }
            if !stale {
                write(&node.ctx.data.state).error = Some(KernelError::Failed(Arc::new(error)));
                node.target = None;
            }
        }
        if node.loaded == node.target && !node.disposed() {
            self.status(uid, FiberState::Active);
        } else {
            node.ctx.activation.close();
            self.status(uid, FiberState::Unloading);
        }
    }
    fn update(&mut self, uid: u32, raw: Json, reply: Reply<Result<(), KernelError>>) {
        let Some(node) = self.nodes.get_mut(&uid) else {
            let _ = reply.send(Err(KernelError::Inactive));
            return;
        };
        if node.disposed() {
            let _ = reply.send(Err(KernelError::Inactive));
            return;
        }
        let loaded = node.target.is_some();
        let candidate = if loaded {
            match node.ctx.fiber().resolve(&raw) {
                Ok(c) => Some(c),
                Err(e) => {
                    let _ = reply.send(Err(e));
                    return;
                }
            }
        } else {
            None
        };
        {
            let mut data = write(&node.ctx.data.state);
            data.raw = Arc::new(raw);
            data.error = None;
        }
        let hooks = read(&node.ctx.data.hooks).updates.clone();
        if let Some((effective, typed)) = candidate
            && !hooks.is_empty()
        {
            let generation = node.generation;
            let raw = read(&node.ctx.data.state).raw.clone();
            let fiber = node.ctx.fiber();
            let tx = self.shared.tx.clone();
            let restarted = Arc::new(AtomicBool::new(false));
            let flag = restarted.clone();
            let tail = Next {
                call: Box::new(move || {
                    flag.store(true, Ordering::Release);
                    Box::pin(async move {
                        fiber
                            .restart_from_update()
                            .await
                            .map_err(|e| Box::new(e) as Error)
                    })
                }),
            };
            let next = update_chain(Arc::new(hooks), 0, typed.clone(), tail);
            spawn_for(node.ctx.data.ancestry.clone(), async move {
                let result = AssertUnwindSafe(next.run())
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err("update handler panicked".into()));
                let _ = tx.send(Command::UpdateDone(
                    uid,
                    generation,
                    raw,
                    effective,
                    typed,
                    restarted.load(Ordering::Acquire),
                    result,
                    reply,
                ));
            });
        } else {
            node.revision += 1;
            node.waiters.push(reply);
        }
    }
    fn volatile(&mut self, uid: u32, raw: Json) -> Result<bool, KernelError> {
        let node = self.nodes.get(&uid).ok_or(KernelError::Inactive)?;
        if node.state() != FiberState::Active || node.disposed() {
            return Err(KernelError::Inactive);
        }
        let (effective, typed) = match node.ctx.fiber().resolve(&raw) {
            Ok(candidate) => candidate,
            Err(error) => {
                write(&node.ctx.data.state).raw = Arc::new(raw);
                return Err(error);
            }
        };
        let old = node
            .ctx
            .fiber()
            .effective_config()
            .ok_or(KernelError::Inactive)?;
        if !config::equal_except_volatile(node.ctx.data.def.schema(), &old, &effective) {
            return Ok(false);
        }
        let paths = config::volatile_paths(node.ctx.data.def.schema(), &old, &effective);
        {
            let mut state = write(&node.ctx.data.state);
            state.raw = Arc::new(raw);
            state.effective = Some(Arc::new(effective));
        }
        *write(&node.ctx.activation.cfg) = typed;
        let uid = node.ctx.data.uid;
        node.ctx
            .with_filter(move |ctx| ctx.data.uid == uid)
            .emit(&VolatileUpdate { paths });
        Ok(true)
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            fibers: self
                .nodes
                .values()
                .map(|n| {
                    let state = read(&n.ctx.data.state);
                    FiberInfo {
                        uid: n.ctx.data.uid,
                        parent: n.ctx.data.parent,
                        name: n.ctx.data.name.clone(),
                        entry_id: n.ctx.data.entry_id.clone(),
                        state: state.state,
                        missing: self.wanted(n).2,
                        error: state.error.as_ref().map(ToString::to_string),
                    }
                })
                .collect(),
            provisions: self
                .bindings
                .values()
                .map(|b| ProvisionInfo {
                    name: b.provision.name.clone(),
                    realm: b.provision.realm,
                    owner: b.provision.owner,
                    active: self.visible(&b.provision),
                    dependents: lock(&b.provision.dependents).iter().copied().collect(),
                })
                .collect(),
        }
    }
}

fn update_chain(
    hooks: Arc<Vec<Arc<UpdateHook>>>,
    index: usize,
    value: Value,
    tail: Next<()>,
) -> Next<()> {
    Next {
        call: Box::new(move || {
            Box::pin(async move {
                spawn_inherited(async move {
                    let mut index = index;
                    while index < hooks.len() {
                        let hook = &hooks[index];
                        index += 1;
                        if hook.effect.upgrade().is_some_and(|e| !e.is_running()) {
                            return (hook.handler)(
                                value.clone(),
                                update_chain(hooks.clone(), index, value, tail),
                            )
                            .await;
                        }
                    }
                    tail.run().await
                })
                .await
                .map_err(|error| Box::new(error) as Error)?
            })
        }),
    }
}
impl Drop for Actor {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.closed_token.cancel();
        self.shared.root_activation.close();
        for node in self.nodes.values_mut() {
            for waiter in node.waiters.drain(..) {
                let _ = waiter.send(Err(KernelError::Closed));
            }
            node.ctx.activation.close();
            node.ctx.activation.abandon();
        }
        self.shared.bus.clear();
        let contexts = std::mem::take(&mut *write(&self.shared.contexts));
        for ctx in contexts.values() {
            ctx.activation.close();
            ctx.activation.abandon();
        }
        let published = std::mem::take(&mut *write(&self.shared.published));
        let own = std::mem::take(&mut *write(&self.shared.own_published));
        drop(contexts);
        drop(published);
        drop(own);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn actor_failure_closes_all_transition_waiters() {
        let kernel = Kernel::start();
        service!(Key:u32="failure-test");
        kernel.root().provide::<Key>(Arc::new(1)).await.unwrap();
        let fiber = kernel.root().plugin::<Empty>(Json::Null).unwrap();
        let a = fiber.settled();
        let b = fiber.settled();
        kernel.inner.tx.send(Command::Fault).ok();
        assert!(matches!(a.await, Err(KernelError::Closed)));
        assert!(matches!(b.await, Err(KernelError::Closed)));
        assert!(matches!(fiber.restart().await, Err(KernelError::Closed)));
        assert!(read(&kernel.inner.published).is_empty());
        assert!(read(&kernel.inner.own_published).is_empty());
    }
}
