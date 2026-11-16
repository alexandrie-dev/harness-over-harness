use crate::*;
use std::{any::TypeId, collections::HashMap, sync::atomic::Ordering, time::Duration};
use tokio_util::sync::CancellationToken;
type Filter = Arc<dyn Fn(&Context) -> bool + Send + Sync>;
#[derive(Clone)]
pub struct Context {
    pub(crate) inner: Arc<fiber::Shared>,
    pub(crate) data: Arc<fiber::FiberData>,
    pub(crate) activation: Arc<effects::Activation>,
    pub(crate) realms: Arc<HashMap<String, Realm>>,
    intercepts: Arc<HashMap<String, serde_json::Value>>,
    tags: Arc<HashMap<TypeId, Value>>,
    pub(crate) filter: Option<Filter>,
}
impl Context {
    pub(crate) fn base(
        inner: Arc<fiber::Shared>,
        data: Arc<fiber::FiberData>,
        activation: Arc<effects::Activation>,
    ) -> Self {
        Self {
            inner,
            data,
            activation,
            realms: Arc::default(),
            intercepts: Arc::default(),
            tags: Arc::default(),
            filter: None,
        }
    }
    pub(crate) fn activation_context(
        &self,
        data: Arc<fiber::FiberData>,
        activation: Arc<effects::Activation>,
    ) -> Self {
        let mut ctx = self.clone();
        ctx.data = data;
        ctx.activation = activation;
        ctx
    }
    pub fn fiber(&self) -> Fiber {
        Fiber {
            inner: self.inner.clone(),
            data: self.data.clone(),
        }
    }
    pub fn root(&self) -> Self {
        Self::base(
            self.inner.clone(),
            self.inner.root.clone(),
            self.inner.root_activation.clone(),
        )
    }
    pub fn ensure_active(&self) -> Result<(), KernelError> {
        if self.inner.closed.load(Ordering::Acquire) {
            Err(KernelError::Closed)
        } else if lock(&self.activation.effects).closed {
            Err(KernelError::Inactive)
        } else {
            Ok(())
        }
    }
    pub fn token(&self) -> CancellationToken {
        self.activation.token.clone()
    }
    pub fn logger(&self) -> Logger {
        let options = self.intercepts.get("logger");
        let name = options
            .and_then(|v| v["name"].as_str())
            .map(Arc::<str>::from)
            .unwrap_or_else(|| {
                let mut result = String::new();
                let chars = self.data.name.chars().collect::<Vec<_>>();
                let mut state = 0;
                for (i, &c) in chars.iter().enumerate() {
                    if c.is_ascii_uppercase() {
                        if state == 2
                            || (state == 1
                                && chars.get(i + 1).is_some_and(char::is_ascii_lowercase))
                        {
                            result.push('-');
                        }
                        result.push(c.to_ascii_lowercase());
                        state = 1;
                    } else if c.is_ascii_lowercase() {
                        result.push(c);
                        state = 2;
                    } else if c == '-' || c == '_' {
                        if state != 0 {
                            result.push('-');
                        }
                        state = 0;
                    } else {
                        result.push(c);
                    }
                }
                result.into()
            });
        let level = options.and_then(|v| v["level"].as_u64()).map(|v| match v {
            0 => Level::Error,
            1 => Level::Warn,
            2 => Level::Info,
            _ => Level::Debug,
        });
        Logger {
            logs: self.inner.logs.clone(),
            inner: Arc::downgrade(&self.inner),
            activation: Arc::downgrade(&self.activation),
            data: Arc::downgrade(&self.data),
            name,
            owner_name: self.data.name.clone(),
            uid: self.data.uid,
            level,
        }
    }
    pub fn plugin<P: Plugin>(&self, raw: serde_json::Value) -> Result<Fiber, KernelError> {
        self.plugin_dyn(&PluginDef::of::<P>(), raw, PluginOptions::default())
    }
    pub fn plugin_dyn(
        &self,
        def: &PluginDef,
        raw: serde_json::Value,
        options: PluginOptions,
    ) -> Result<Fiber, KernelError> {
        self.create(def.clone(), raw, options)
    }
    pub fn inject<D, Fut>(
        &self,
        body: impl Fn(Context, D) -> Fut + Send + Sync + 'static,
    ) -> Result<Fiber, KernelError>
    where
        D: Inject,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let def = PluginDef::injected::<D, Fut>(body);
        self.create(def, serde_json::Value::Null, PluginOptions::default())
    }
    pub fn realm_of(&self, service: &str) -> Realm {
        self.realms.get(service).copied().unwrap_or(Realm::ROOT)
    }
    pub fn isolate(&self, service: &str, realm: Realm) -> Self {
        let mut ctx = self.clone();
        Arc::make_mut(&mut ctx.realms).insert(service.into(), realm);
        ctx
    }
    pub fn intercept(&self, service: &str, options: serde_json::Value) -> Self {
        let mut ctx = self.clone();
        let map = Arc::make_mut(&mut ctx.intercepts);
        let target = map
            .entry(service.into())
            .or_insert_with(|| serde_json::json!({}));
        if let (Some(target), Some(options)) = (target.as_object_mut(), options.as_object()) {
            target.extend(options.clone());
        }
        ctx
    }
    pub fn intercepted<T: serde::de::DeserializeOwned + Default>(&self, service: &str) -> T {
        match serde_json::from_value(
            self.intercepts
                .get(service)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        ) {
            Ok(v) => v,
            Err(e) => {
                self.logger()
                    .warn(format_args!("invalid intercept for {service}: {e}"));
                T::default()
            }
        }
    }
    pub fn with_tag<T: Send + Sync + 'static>(&self, tag: T) -> Self {
        let mut ctx = self.clone();
        Arc::make_mut(&mut ctx.tags).insert(TypeId::of::<T>(), Arc::new(tag));
        ctx
    }
    pub fn tag<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.tags
            .get(&TypeId::of::<T>())
            .cloned()?
            .downcast::<T>()
            .ok()
    }
    pub fn with_filter(&self, filter: impl Fn(&Context) -> bool + Send + Sync + 'static) -> Self {
        let mut ctx = self.clone();
        ctx.filter = Some(Arc::new(filter));
        ctx
    }
    pub fn get<K: ServiceKey>(&self) -> Option<Service<K>> {
        let p = self.binding(K::NAME)?;
        if p.key != TypeId::of::<K>() {
            return None;
        }
        Some(Service {
            value: p.value.downcast_ref::<Arc<K::Value>>()?.clone(),
        })
    }
    fn binding(&self, name: &str) -> Option<Arc<fiber::Provision>> {
        if self.ensure_active().is_err() {
            return None;
        }
        let key = (Arc::<str>::from(name), self.realm_of(name));
        if let Some(p) = read(&self.inner.own_published)
            .get(&key)
            .filter(|p| p.owner() == self.data.uid && !p.withdrawn())
        {
            return Some(p.clone());
        }
        if let Some(p) = self.activation.view.values.get(&key) {
            return Some(p.clone());
        }
        read(&self.inner.published).get(&key).cloned()
    }
    pub fn has(&self, service: &str) -> bool {
        self.binding(service).is_some()
    }
    pub fn view(&self, service: &str) -> Option<serde_json::Value> {
        let p = self.binding(service)?;
        (p.view)(&p.value)
    }
    pub fn provide<K: ServiceKey>(
        &self,
        value: Arc<K::Value>,
    ) -> impl Future<Output = Result<Disposer, KernelError>> + Send + 'static {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let references = Arc::downgrade(&value);
        let command = match self.reserve_effect("ctx.provide()") {
            Err(e) => {
                let _ = tx.send(Err(e));
                None
            }
            Ok(disposer) => Some(fiber::Command::Provide {
                ctx: self.clone(),
                key: TypeId::of::<K>(),
                name: K::NAME.into(),
                value: Arc::new(value),
                view: |v| v.downcast_ref::<Arc<K::Value>>().and_then(|v| K::view(v)),
                count: Arc::new(move || references.strong_count()),
                disposer,
                reply: tx,
            }),
        };
        if let Some(command) = command {
            let _ = self.inner.tx.send(command);
        }
        async move { rx.await.unwrap_or(Err(KernelError::Closed)) }
    }
    pub fn on_candidate<C: Config>(
        &self,
        check: impl Fn(&C) -> Result<(), Error> + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError> {
        self.config_type::<C>()?;
        let disposer = self.reserve_effect("ctx.on_candidate()")?;
        let hook = Arc::new(fiber::CandidateHook {
            effect: Arc::downgrade(&disposer.effect),
            check: Box::new(move |c| {
                c.downcast_ref::<C>()
                    .ok_or_else(|| Box::new(KernelError::ConfigType) as Error)
                    .and_then(&check)
            }),
        });
        write(&self.data.hooks).candidates.push(hook.clone());
        let data = Arc::downgrade(&self.data);
        disposer.effect.add(Undo::sync(move || {
            if let Some(data) = data.upgrade() {
                write(&data.hooks)
                    .candidates
                    .retain(|h| !Arc::ptr_eq(h, &hook));
            }
        }));
        disposer.effect.finish_setup();
        Ok(disposer)
    }
    pub fn on_update<C, Fut>(
        &self,
        handler: impl Fn(Arc<C>, Next<()>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where
        C: Config,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        self.config_type::<C>()?;
        let disposer = self.reserve_effect("ctx.on_update()")?;
        let hook = Arc::new(fiber::UpdateHook {
            effect: Arc::downgrade(&disposer.effect),
            handler: Box::new(move |c, next| {
                let c = c.downcast::<C>();
                let future = c.ok().map(|c| handler(c, next));
                Box::pin(async move {
                    match future {
                        Some(f) => f.await,
                        None => Err(Box::new(KernelError::ConfigType) as Error),
                    }
                })
            }),
        });
        write(&self.data.hooks).updates.push(hook.clone());
        let data = Arc::downgrade(&self.data);
        disposer.effect.add(Undo::sync(move || {
            if let Some(data) = data.upgrade() {
                write(&data.hooks)
                    .updates
                    .retain(|h| !Arc::ptr_eq(h, &hook));
            }
        }));
        disposer.effect.finish_setup();
        Ok(disposer)
    }
    fn config_type<C: Config>(&self) -> Result<(), KernelError> {
        if self.data.def.config_type == TypeId::of::<C>() {
            Ok(())
        } else {
            Err(KernelError::ConfigType)
        }
    }
    pub fn sleep(
        &self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), KernelError>> + Send + 'static {
        let active = self.ensure_active();
        let token = self.token();
        let sleep = tokio::time::sleep(duration);
        async move {
            active?;
            tokio::select! { biased; _=token.cancelled()=>Err(KernelError::Inactive),_=sleep=>Ok(()) }
        }
    }
    pub fn interval(
        &self,
        period: Duration,
    ) -> impl futures_util::Stream<Item = ()> + Send + 'static {
        let token = self.token();
        let active = self.ensure_active().is_ok();
        let interval = if active && !period.is_zero() {
            Some(tokio::time::interval_at(
                tokio::time::Instant::now() + period,
                period,
            ))
        } else {
            None
        };
        futures_util::StreamExt::fuse(futures_util::stream::unfold(
            (interval, token),
            |(interval, token)| async move {
                let mut interval = interval?;
                tokio::select! { biased; _=token.cancelled()=>None,_=interval.tick()=>Some(((),(Some(interval),token))) }
            },
        ))
    }
}
