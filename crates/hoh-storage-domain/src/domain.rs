use crate::{
    error::{DomainError, DomainErrorCode, panic_error, plain},
    events::{DomainChanged, DomainChangedDeleted, DomainChangedPut},
    spec::{Data, DomainSpec},
};
use futures_util::FutureExt;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::Context;
use hoh_storage::{KvUnit, StorageError};
use indexmap::{IndexMap, IndexSet};
use serde::Serialize;
use serde_json::Value;
use std::{
    future::Future,
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot};

pub type DomainFuture<T> = Pin<Box<dyn Future<Output = Result<T, SharedError>> + Send + 'static>>;
type JobFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type Job = Box<dyn FnOnce(Arc<DomainInner>) -> JobFuture + Send>;
enum Command {
    Write(Arc<DomainInner>, Job),
    Close(Arc<DomainInner>),
}
pub(crate) struct State {
    pub tables: IndexMap<String, IndexMap<String, Data>>,
    pub global: Option<Data>,
    disposing: bool,
    closed: bool,
    failure: Option<SharedError>,
    disposal: Option<Result<(), SharedError>>,
    waiters: Vec<oneshot::Sender<Result<(), SharedError>>>,
}
pub(crate) struct DomainInner {
    pub name: String,
    ctx: Context,
    unit: Arc<dyn KvUnit>,
    pub state: Mutex<State>,
    sender: mpsc::UnboundedSender<Command>,
    on_closed: Box<dyn Fn() + Send + Sync>,
}
impl DomainInner {
    fn readable(&self) -> Result<std::sync::MutexGuard<'_, State>, SharedError> {
        let state = crate::lock(&self.state);
        if state.closed {
            return Err(self.closed_error());
        }
        if let Some(failure) = &state.failure {
            return Err(failure.clone());
        }
        Ok(state)
    }
    fn closed_error(&self) -> SharedError {
        SharedError::new(DomainError::new(
            DomainErrorCode::Closed,
            format!("domain '{}' is closed", self.name),
        ))
    }
    fn enqueue<T, F, Fut>(self: &Arc<Self>, job: F) -> DomainFuture<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, BoxError>> + Send + 'static,
    {
        let mut state = crate::lock(&self.state);
        if state.disposing {
            return ready(Err(self.closed_error()));
        }
        let (tx, rx) = oneshot::channel();
        let run: Job = Box::new(move |inner| {
            Box::pin(async move {
                let failure = crate::lock(&inner.state).failure.clone();
                let result = if let Some(failure) = failure {
                    Err(failure)
                } else {
                    match AssertUnwindSafe(async { job(inner.clone()).await })
                        .catch_unwind()
                        .await
                    {
                        Ok(Ok(value)) => Ok(value),
                        Ok(Err(error)) => Err(SharedError::new(error)),
                        Err(payload) => Err(SharedError::new(panic_error(payload))),
                    }
                };
                if let Err(error) = &result
                    && error
                        .downcast_ref::<StorageError>()
                        .is_some_and(|e| e.code() == "unknown-state")
                {
                    crate::lock(&inner.state).failure = Some(error.clone());
                }
                let _ = tx.send(result);
            })
        });
        if self.sender.send(Command::Write(self.clone(), run)).is_err() {
            state.disposing = true;
            return ready(Err(plain("domain write chain stopped")));
        }
        drop(state);
        Box::pin(async move {
            rx.await
                .unwrap_or_else(|_| Err(plain("domain write chain stopped")))
        })
    }
    fn close(self: &Arc<Self>) -> DomainFuture<()> {
        let mut state = crate::lock(&self.state);
        if let Some(result) = &state.disposal {
            return ready(result.clone());
        }
        let (tx, rx) = oneshot::channel();
        state.waiters.push(tx);
        if !state.disposing {
            state.disposing = true;
            let _ = self.sender.send(Command::Close(self.clone()));
        }
        drop(state);
        Box::pin(async move {
            rx.await
                .unwrap_or_else(|_| Err(plain("domain close task stopped")))
        })
    }
    fn emit(&self, table: String, key: String, value: Option<Value>) {
        let event = match value {
            Some(value) => DomainChanged::Put(DomainChangedPut {
                domain: self.name.clone(),
                table,
                key,
                value,
            }),
            None => DomainChanged::Deleted(DomainChangedDeleted {
                domain: self.name.clone(),
                table,
                key,
            }),
        };
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.ctx.emit(&event))) {
            self.ctx.logger().warn(format!(
                "domain '{}': domain/changed listener failed: {}",
                self.name,
                crate::error::js_error_string(panic_error(payload).as_ref())
            ));
        }
    }
}
pub(crate) fn ready<T: Send + 'static>(result: Result<T, SharedError>) -> DomainFuture<T> {
    Box::pin(async move { result })
}

pub struct Domain {
    pub name: String,
    inner: Arc<DomainInner>,
    tables: IndexMap<String, Data>,
    global: Option<Data>,
}
pub type DomainImpl = Domain;
impl Domain {
    pub(crate) fn new(
        ctx: Context,
        spec: &DomainSpec,
        unit: Arc<dyn KvUnit>,
        tables: IndexMap<String, IndexMap<String, Data>>,
        global: Option<Data>,
        on_closed: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let inner = Arc::new(DomainInner {
            name: spec.name.clone(),
            ctx,
            unit,
            state: Mutex::new(State {
                tables,
                global,
                disposing: false,
                closed: false,
                failure: None,
                disposal: None,
                waiters: Vec::new(),
            }),
            sender,
            on_closed: Box::new(on_closed),
        });
        let handles = spec
            .tables
            .iter()
            .map(|(name, spec)| (name.clone(), (spec.handle)(inner.clone(), name.clone())))
            .collect();
        let global = spec
            .global
            .as_ref()
            .map(|spec| (spec.handle)(inner.clone(), String::new()));
        // A Promise-chain executor. Each queued command owns its state; an idle
        // worker owns only the receiver. Facility teardown drains via close().
        tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    Command::Write(inner, job) => job(inner).await,
                    Command::Close(inner) => {
                        let result = inner.unit.close().await.map_err(SharedError::new);
                        if result.is_ok() {
                            crate::lock(&inner.state).closed = true;
                        }
                        if result.is_ok() {
                            (inner.on_closed)();
                        }
                        let mut state = crate::lock(&inner.state);
                        state.disposal = Some(result.clone());
                        for waiter in state.waiters.drain(..) {
                            let _ = waiter.send(result.clone());
                        }
                        break;
                    }
                }
            }
        });
        Arc::new(Self {
            name: spec.name.clone(),
            inner,
            tables: handles,
            global,
        })
    }
    pub fn table<K, V>(&self, name: &str) -> Result<Arc<KvTable<K, V>>, SharedError>
    where
        K: AsRef<str> + From<String> + Send + Sync + 'static,
        V: Serialize + Send + Sync + 'static,
    {
        let handle = self
            .tables
            .get(name)
            .ok_or_else(|| plain(format!("domain '{}' declares no table '{name}'", self.name)))?;
        handle.clone().downcast::<KvTable<K, V>>().map_err(|_| {
            plain(format!(
                "domain '{}' table '{name}' has incompatible Rust types",
                self.name
            ))
        })
    }
    pub fn global<G: Serialize + Send + Sync + 'static>(
        &self,
    ) -> Result<Arc<DomainGlobal<G>>, SharedError> {
        self.global
            .as_ref()
            .ok_or_else(|| plain(format!("domain '{}' declares no global", self.name)))?
            .clone()
            .downcast::<DomainGlobal<G>>()
            .map_err(|_| plain("incompatible Rust global type"))
    }
    pub fn close(&self) -> DomainFuture<()> {
        self.inner.close()
    }
}

pub struct KvTable<K = String, V = Value> {
    inner: Arc<DomainInner>,
    name: String,
    types: PhantomData<fn(K) -> V>,
}
impl<K, V> KvTable<K, V>
where
    K: AsRef<str> + From<String> + Send + Sync + 'static,
    V: Serialize + Send + Sync + 'static,
{
    pub(crate) fn new(inner: Arc<DomainInner>, name: String) -> Self {
        Self {
            inner,
            name,
            types: PhantomData,
        }
    }
    pub fn get(&self, key: &K) -> Result<Option<Arc<V>>, SharedError> {
        self.inner.readable()?.tables[&self.name]
            .get(key.as_ref())
            .cloned()
            .map(|v| {
                v.downcast::<V>()
                    .map_err(|_| plain("incompatible Rust record type"))
            })
            .transpose()
    }
    pub fn entries(&self) -> Result<std::vec::IntoIter<(K, Arc<V>)>, SharedError> {
        let records = self.inner.readable()?.tables[&self.name]
            .iter()
            .map(|(key, value)| {
                Ok((
                    K::from(key.clone()),
                    value
                        .clone()
                        .downcast::<V>()
                        .map_err(|_| plain("incompatible Rust record type"))?,
                ))
            })
            .collect::<Result<Vec<_>, SharedError>>()?;
        Ok(records.into_iter())
    }
    pub fn keys(&self) -> Result<std::vec::IntoIter<K>, SharedError> {
        Ok(self.inner.readable()?.tables[&self.name]
            .keys()
            .cloned()
            .map(K::from)
            .collect::<Vec<_>>()
            .into_iter())
    }
    pub fn size(&self) -> Result<u64, SharedError> {
        Ok(self.inner.readable()?.tables[&self.name].len() as u64)
    }
    pub fn put(&self, key: K, value: V) -> DomainFuture<()> {
        let table = self.name.clone();
        let key = key.as_ref().to_owned();
        let value = Arc::new(value);
        self.inner.enqueue(move |inner| async move {
            let json = serde_json::to_value(value.as_ref())?;
            inner.unit.put_record(&table, &key, json.clone()).await?;
            // Table handles are created only for the domain's immutable table set.
            crate::lock(&inner.state)
                .tables
                .get_mut(&table)
                .expect("declared table remains present")
                .insert(key.clone(), value);
            inner.emit(table, key, Some(json));
            Ok(())
        })
    }
    pub fn delete(&self, key: K) -> DomainFuture<bool> {
        let table = self.name.clone();
        let key = key.as_ref().to_owned();
        self.inner.enqueue(move |inner| async move {
            if !crate::lock(&inner.state).tables[&table].contains_key(&key) {
                return Ok(false);
            }
            inner.unit.delete_record(&table, &key).await?;
            // Table handles are created only for the domain's immutable table set.
            crate::lock(&inner.state)
                .tables
                .get_mut(&table)
                .expect("declared table remains present")
                .shift_remove(&key);
            inner.emit(table, key, None);
            Ok(true)
        })
    }
    pub fn delete_many(&self, keys: Vec<K>) -> DomainFuture<u64> {
        let table = self.name.clone();
        let keys = keys
            .into_iter()
            .map(|k| k.as_ref().to_owned())
            .collect::<IndexSet<_>>();
        self.inner.enqueue(move |inner| async move {
            let present = {
                let state = crate::lock(&inner.state);
                keys.into_iter()
                    .filter(|key| state.tables[&table].contains_key(key))
                    .collect::<Vec<_>>()
            };
            if present.is_empty() {
                return Ok(0);
            }
            inner.unit.delete_records(&table, &present).await?;
            {
                let mut state = crate::lock(&inner.state);
                // No operation can remove a declared table from the table set.
                let records = state
                    .tables
                    .get_mut(&table)
                    .expect("declared table remains present");
                for key in &present {
                    records.shift_remove(key);
                }
            }
            for key in &present {
                inner.emit(table.clone(), key.clone(), None);
            }
            Ok(present.len() as u64)
        })
    }
    pub fn update(
        &self,
        key: K,
        transform: impl FnOnce(&V) -> V + Send + 'static,
    ) -> DomainFuture<Arc<V>> {
        let table = self.name.clone();
        let key = key.as_ref().to_owned();
        self.inner.enqueue(move |inner| async move {
            let current = crate::lock(&inner.state).tables[&table]
                .get(&key)
                .cloned()
                .ok_or_else(|| {
                    DomainError::new(
                        DomainErrorCode::MissingKey,
                        format!(
                            "domain '{}' table '{table}' has no record '{key}' to update",
                            inner.name
                        ),
                    )
                })?;
            let current = current
                .downcast::<V>()
                .map_err(|_| crate::error::PlainError("incompatible Rust record type".into()))?;
            let value = Arc::new(transform(&current));
            let json = serde_json::to_value(value.as_ref())?;
            inner.unit.put_record(&table, &key, json.clone()).await?;
            // Table handles are created only for the domain's immutable table set.
            crate::lock(&inner.state)
                .tables
                .get_mut(&table)
                .expect("declared table remains present")
                .insert(key.clone(), value.clone());
            inner.emit(table, key, Some(json));
            Ok(value)
        })
    }
}
pub struct DomainGlobal<G = Value> {
    inner: Arc<DomainInner>,
    ty: PhantomData<fn() -> G>,
}
impl<G: Serialize + Send + Sync + 'static> DomainGlobal<G> {
    pub(crate) fn new(inner: Arc<DomainInner>) -> Self {
        Self {
            inner,
            ty: PhantomData,
        }
    }
    pub fn get(&self) -> Result<Arc<G>, SharedError> {
        self.inner
            .readable()?
            .global
            .clone()
            .ok_or_else(|| plain("missing declared global"))?
            .downcast::<G>()
            .map_err(|_| plain("incompatible Rust global type"))
    }
    pub fn set(&self, value: G) -> DomainFuture<()> {
        let value = Arc::new(value);
        self.inner.enqueue(move |inner| async move {
            let json = serde_json::to_value(value.as_ref())?;
            inner.unit.set_global(json.clone()).await?;
            crate::lock(&inner.state).global = Some(value);
            inner.emit(String::new(), String::new(), Some(json));
            Ok(())
        })
    }
}
