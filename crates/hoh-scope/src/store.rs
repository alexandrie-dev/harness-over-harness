//! Insertion-ordered, live registration tables and scope-owned aggregate layers.

use crate::{ScopeKey, lock, scope_chain_of, scope_of};
use hoh_kernel::{Context, Disposer, Error, Undo};
use indexmap::IndexMap;
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::ThreadId,
};

type UndoAction = Box<dyn FnOnce() + Send>;

/// An exact, synchronous, idempotent undo. Dropping it does not undo registration.
#[derive(Clone)]
pub struct EntryUndo(Arc<Mutex<Option<UndoAction>>>);
impl EntryUndo {
    pub fn new(undo: impl FnOnce() + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(undo)))))
    }
    pub fn undo(&self) {
        let action = lock(&self.0).take();
        if let Some(action) = action {
            action();
        }
    }
}
impl From<EntryUndo> for Undo {
    fn from(undo: EntryUndo) -> Self {
        Undo::sync(move || undo.undo())
    }
}

type Slot<V> = Option<(Option<String>, V)>;
struct Generation<V> {
    slots: Vec<Slot<V>>,
    names: IndexMap<String, usize>,
    count: usize,
}
impl<V> Default for Generation<V> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            names: IndexMap::new(),
            count: 0,
        }
    }
}
type Current<V> = Arc<Mutex<Arc<Mutex<Generation<V>>>>>;
struct Table<V> {
    current: Current<V>,
}
impl<V> Clone for Table<V> {
    fn clone(&self) -> Self {
        Self {
            current: self.current.clone(),
        }
    }
}
impl<V> Default for Table<V> {
    fn default() -> Self {
        Self {
            current: Arc::new(Mutex::new(Arc::new(Mutex::new(Generation::default())))),
        }
    }
}
impl<V: Send + 'static> Table<V> {
    fn insert(&self, name: Option<String>, value: V) -> Result<EntryUndo, ()> {
        let current = lock(&self.current);
        let generation = current.clone();
        let mut data = lock(&generation);
        if name
            .as_ref()
            .is_some_and(|name| data.names.contains_key(name))
        {
            return Err(());
        }
        let index = data.slots.len();
        if let Some(name) = &name {
            data.names.insert(name.clone(), index);
        }
        data.slots.push(Some((name, value)));
        data.count += 1;
        drop(data);
        drop(current);
        let table = self.clone();
        Ok(EntryUndo::new(move || {
            let mut current = lock(&table.current);
            let mut data = lock(&generation);
            let removed = data.slots[index].take();
            if let Some((name, _)) = &removed {
                if let Some(name) = name {
                    data.names.shift_remove(name);
                }
                data.count -= 1;
                if data.count == 0 && Arc::ptr_eq(&current, &generation) {
                    *current = Arc::new(Mutex::new(Generation::default()));
                }
            }
            // A value's destructor may call back into its registry.
            drop(data);
            drop(current);
            drop(removed);
        }))
    }
    fn cursor(&self) -> Cursor<V> {
        Cursor {
            generation: lock(&self.current).clone(),
            index: 0,
            done: false,
        }
    }
    fn is_empty(&self) -> bool {
        lock(&lock(&self.current)).count == 0
    }
}

struct Cursor<V> {
    generation: Arc<Mutex<Generation<V>>>,
    index: usize,
    done: bool,
}
impl<V> Cursor<V> {
    fn next<R>(&mut self, pick: impl FnOnce(&Option<String>, &V) -> R) -> Option<R> {
        if self.done {
            return None;
        }
        let data = lock(&self.generation);
        while self.index < data.slots.len() {
            let index = self.index;
            self.index += 1;
            if let Some((name, value)) = &data.slots[index] {
                return Some(pick(name, value));
            }
        }
        self.done = true;
        None
    }
}

/// Live values from one nonempty table generation. Exhaustion is permanent.
pub struct Values<V>(Cursor<V>);
impl<V: Clone> Iterator for Values<V> {
    type Item = V;
    fn next(&mut self) -> Option<V> {
        self.0.next(|_, value| value.clone())
    }
}
/// Live named keys in insertion order.
pub struct Keys<V>(Cursor<V>);
impl<V> Iterator for Keys<V> {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        self.0.next(|name, _| name.clone()).flatten()
    }
}
/// Live named entries in insertion order.
pub struct Entries<V>(Cursor<V>);
impl<V: Clone> Iterator for Entries<V> {
    type Item = (String, V);
    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .next(|name, value| name.clone().map(|name| (name, value.clone())))
            .flatten()
    }
}

/// Named entries with a caller-owned duplicate diagnostic.
/// Store `Arc<T>` when borrowed object identity must be preserved.
pub struct NamedEntries<V, E = Error> {
    table: Table<V>,
    duplicate_error: Arc<dyn Fn(&str) -> E + Send + Sync>,
}
impl<V, E> Clone for NamedEntries<V, E> {
    fn clone(&self) -> Self {
        Self {
            table: self.table.clone(),
            duplicate_error: self.duplicate_error.clone(),
        }
    }
}
impl<V: Send + 'static, E> NamedEntries<V, E> {
    pub fn new(duplicate_error: impl Fn(&str) -> E + Send + Sync + 'static) -> Self {
        Self {
            table: Table::default(),
            duplicate_error: Arc::new(duplicate_error),
        }
    }
    pub fn insert(&self, name: impl Into<String>, value: V) -> Result<EntryUndo, E> {
        let name = name.into();
        self.table
            .insert(Some(name.clone()), value)
            .map_err(|()| (self.duplicate_error)(&name))
    }
    pub fn get(&self, name: &str) -> Option<V>
    where
        V: Clone,
    {
        let current = lock(&self.table.current);
        let data = lock(&current);
        let index = data.names.get(name)?;
        data.slots[*index].as_ref().map(|(_, value)| value.clone())
    }
    pub fn has(&self, name: &str) -> bool {
        lock(&lock(&self.table.current)).names.contains_key(name)
    }
    pub fn keys(&self) -> Keys<V> {
        Keys(self.table.cursor())
    }
    pub fn entries(&self) -> Entries<V>
    where
        V: Clone,
    {
        Entries(self.table.cursor())
    }
    pub fn values(&self) -> Values<V>
    where
        V: Clone,
    {
        Values(self.table.cursor())
    }
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

/// Equal values remain separate registrations, each with its own undo.
pub struct AnonymousEntries<V> {
    table: Table<V>,
}
impl<V> Clone for AnonymousEntries<V> {
    fn clone(&self) -> Self {
        Self {
            table: self.table.clone(),
        }
    }
}
impl<V> Default for AnonymousEntries<V> {
    fn default() -> Self {
        Self {
            table: Table::default(),
        }
    }
}
impl<V: Send + 'static> AnonymousEntries<V> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn append(&self, value: V) -> EntryUndo {
        // Anonymous insertions have no named index and therefore cannot collide.
        self.table
            .insert(None, value)
            .expect("anonymous insertion has no duplicate key")
    }
    pub fn values(&self) -> Values<V>
    where
        V: Clone,
    {
        Values(self.table.cursor())
    }
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

pub trait ScopeLayer: Send + Sync + 'static {
    fn is_empty(&self) -> bool;
}

/// Effect label and change notification policy. Notification defaults to true.
pub struct EffectOptions {
    pub label: &'static str,
    pub notify: bool,
}
impl EffectOptions {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            notify: true,
        }
    }
}

type Factory<L> = dyn Fn(Option<&ScopeKey>) -> Result<L, Error> + Send + Sync;
type OnChange = dyn Fn() -> Result<(), Error> + Send + Sync;

// JavaScript synchronous callbacks may reenter the registry. Serialize different
// threads without holding a mutex while calling factories, actions or observers.
#[derive(Default)]
struct MutationGate {
    state: Mutex<(Option<ThreadId>, usize)>,
    ready: Condvar,
}
struct Mutation<'a>(&'a MutationGate);
impl MutationGate {
    fn enter(&self) -> Mutation<'_> {
        let owner = std::thread::current().id();
        let mut state = lock(&self.state);
        while state.0.is_some_and(|current| current != owner) {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        state.0 = Some(owner);
        state.1 += 1;
        Mutation(self)
    }
}
impl Drop for Mutation<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.1 -= 1;
        if state.1 == 0 {
            state.0 = None;
            self.0.ready.notify_one();
        }
    }
}
struct LayersInner<L> {
    mutation: MutationGate,
    scoped: Mutex<IndexMap<ScopeKey, Arc<L>>>,
    create_layer: Box<Factory<L>>,
    on_change: Box<OnChange>,
}

/// Eager global state and lazy exact-scope overlays, owned by registration contexts.
pub struct ScopedLayers<L> {
    pub global: Arc<L>,
    inner: Arc<LayersInner<L>>,
}
impl<L> Clone for ScopedLayers<L> {
    fn clone(&self) -> Self {
        Self {
            global: self.global.clone(),
            inner: self.inner.clone(),
        }
    }
}
impl<L: ScopeLayer> ScopedLayers<L> {
    pub fn new(
        create_layer: impl Fn(Option<&ScopeKey>) -> L + Send + Sync + 'static,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let global = Arc::new(create_layer(None));
        Self {
            global,
            inner: Arc::new(LayersInner {
                mutation: MutationGate::default(),
                scoped: Mutex::new(IndexMap::new()),
                create_layer: Box::new(move |scope| Ok(create_layer(scope))),
                on_change: Box::new(move || {
                    on_change();
                    Ok(())
                }),
            }),
        }
    }
    /// Fallible source factories and notifications use this constructor.
    pub fn try_new(
        create_layer: impl Fn(Option<&ScopeKey>) -> Result<L, Error> + Send + Sync + 'static,
        on_change: impl Fn() -> Result<(), Error> + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let global = Arc::new(create_layer(None)?);
        Ok(Self {
            global,
            inner: Arc::new(LayersInner {
                mutation: MutationGate::default(),
                scoped: Mutex::new(IndexMap::new()),
                create_layer: Box::new(create_layer),
                on_change: Box::new(on_change),
            }),
        })
    }
    pub fn peek(&self, scope: Option<&ScopeKey>) -> Option<Arc<L>> {
        let _mutation = self.inner.mutation.enter();
        scope.and_then(|key| lock(&self.inner.scoped).get(key).cloned())
    }
    /// Existing overlays, farthest ancestor first and nearest scope last.
    pub fn chain_layers(&self, scope: Option<&ScopeKey>) -> Vec<Arc<L>> {
        let _mutation = self.inner.mutation.enter();
        let chain = scope_chain_of(scope);
        let scoped = lock(&self.inner.scoped);
        chain
            .iter()
            .rev()
            .filter_map(|key| scoped.get(key).cloned())
            .collect()
    }
    pub fn merge<V: Clone + Send + 'static, E>(
        &self,
        scope: Option<&ScopeKey>,
        pick: impl Fn(&L) -> &NamedEntries<V, E>,
    ) -> IndexMap<String, V> {
        let _mutation = self.inner.mutation.enter();
        let mut merged: IndexMap<_, _> = pick(&self.global).entries().collect();
        for layer in self.chain_layers(scope) {
            for (name, value) in pick(&layer).entries() {
                merged.insert(name, value);
            }
        }
        merged
    }
    /// Factory, mutation and notification form one synchronous transaction.
    /// Callbacks may reenter the registry on the same thread.
    /// The returned disposer is the exact one returned by `Context::try_effect`.
    pub fn effect(
        &self,
        ctx: &Context,
        action: impl FnOnce(&L) -> Result<EntryUndo, Error>,
        options: EffectOptions,
    ) -> Result<Disposer, Error> {
        let scope = scope_of(ctx);
        let _mutation = self.inner.mutation.enter();
        ctx.try_effect(options.label, |undos| {
            let (layer, undo) = {
                let (layer, created) = match &scope {
                    None => (self.global.clone(), false),
                    Some(key) => match self.peek(Some(key)) {
                        Some(layer) => (layer, false),
                        None => {
                            let layer = Arc::new((self.inner.create_layer)(Some(key))?);
                            lock(&self.inner.scoped).insert(key.clone(), layer.clone());
                            (layer, true)
                        }
                    },
                };
                match action(&layer) {
                    Ok(undo) => (layer, undo),
                    Err(error) => {
                        if created
                            && layer.is_empty()
                            && let Some(key) = &scope
                        {
                            lock(&self.inner.scoped).shift_remove(key);
                        }
                        return Err(error);
                    }
                }
            };
            let inner = self.inner.clone();
            let log = ctx.logger();
            undos.push(Undo::sync(move || {
                let _mutation = inner.mutation.enter();
                undo.undo();
                if let Some(key) = scope {
                    let mut scoped = lock(&inner.scoped);
                    if layer.is_empty() {
                        scoped.shift_remove(&key);
                    }
                }
                if options.notify
                    && let Err(error) = (inner.on_change)()
                {
                    log.error(error);
                }
            }))?;
            if options.notify {
                (self.inner.on_change)()?;
            }
            Ok(())
        })
    }
}
