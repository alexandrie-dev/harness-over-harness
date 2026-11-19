use crate::{owner::Owner, registration::*, *};
use futures_util::future::BoxFuture;
use hoh_error::SharedError;
use hoh_kernel::{Cfg, Context, Disposer, FiberState, NoConfig, Plugin, Undo};
use hoh_session::{
    MAX_SAFE_INTEGER, Session, SessionEvent, SessionEventEmitted, SessionHeader, SessionLogOffset,
    SessionSeq, SessionSeqCursor, SessionStore,
};
use indexmap::{IndexMap, IndexSet};
use serde::Serialize;
use std::{
    any::Any,
    collections::HashMap,
    marker::PhantomData,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

pub(crate) type ErasedState = Arc<dyn Any + Send + Sync>;
struct Cell {
    state: ErasedState,
    observed: i64,
    views: [Option<Arc<Value>>; 2],
}
struct CachedCell {
    life: Weak<Value>,
    cell: Arc<Mutex<Cell>>,
}
pub(crate) struct Registration {
    def: ErasedDefinition,
    refs: AtomicUsize,
    cells: Mutex<HashMap<usize, CachedCell>>,
}
struct Listeners<T: ?Sized> {
    slots: Vec<Option<(usize, Arc<T>)>>,
}
impl<T: ?Sized> Default for Listeners<T> {
    fn default() -> Self {
        Self { slots: vec![] }
    }
}
impl<T: ?Sized> Listeners<T> {
    fn insert(&mut self, listener: Arc<T>) -> usize {
        let key = Arc::as_ptr(&listener) as *const () as usize;
        if !self.slots.iter().flatten().any(|(id, _)| *id == key) {
            self.slots.push(Some((key, listener)));
        }
        key
    }
    fn remove(&mut self, key: usize) {
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|(id, _)| *id == key) {
                *slot = None;
                break;
            }
        }
    }
    fn any(&self) -> bool {
        self.slots.iter().any(Option::is_some)
    }
    fn at(&self, index: usize) -> Option<Arc<T>> {
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .map(|(_, listener)| listener.clone())
    }
}
type ChangeFn = dyn Fn(&Session, &str, Arc<Value>, SessionSeq) -> Result<()> + Send + Sync;
type EventFn = dyn Fn(&Session, &[ProjectionChange], SessionSeq) -> Result<()> + Send + Sync;
#[derive(Default)]
struct Table {
    registrations: IndexMap<String, Arc<Registration>>,
    summary: Option<Arc<IndexSet<String>>>,
    warned: IndexSet<String>,
    listeners: Listeners<ChangeFn>,
    event_listeners: Listeners<EventFn>,
}
pub(crate) struct Inner {
    owner: Owner,
    table: Mutex<Table>,
}
#[derive(Clone)]
pub struct SessionProjectionRegistry {
    inner: Arc<Inner>,
}
pub(crate) struct Release {
    registry: Weak<Inner>,
    target: Mutex<Option<Arc<Registration>>>,
    done: AtomicBool,
    effect: Mutex<Option<Disposer>>,
}
impl Release {
    fn run(&self) {
        if self.done.swap(true, Ordering::AcqRel) {
            return;
        }
        if let (Some(inner), Some(registration)) =
            (self.registry.upgrade(), lock(&self.target).clone())
        {
            let _operation = inner.owner.enter();
            if registration.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
                lock(&inner.table).summary = None;
            }
        }
    }
    pub(crate) fn dispose(&self) -> BoxFuture<'static, ()> {
        let effect = lock(&self.effect).take();
        if let Some(effect) = effect {
            Box::pin(effect.dispose())
        } else {
            self.run();
            Box::pin(std::future::ready(()))
        }
    }
    pub(crate) fn take_undo(&self) -> Undo {
        lock(&self.effect)
            .take()
            .map_or_else(Undo::none, Into::into)
    }
}
struct Cut {
    header: SessionHeader,
    inherited: SessionLogOffset,
    events: Arc<[Arc<SessionEvent>]>,
}
/// Session validation accepts integral floats (`1.0`) for these counters, as
/// JS does; the typed header holds integers, so normalize before decoding.
fn typed_header(raw: &Value) -> Result<SessionHeader> {
    let mut raw = raw.clone();
    for key in ["createdAt", "delegationDepth"] {
        if let Some(value) = raw.get_mut(key)
            && let Some(number) = value.as_f64().filter(|_| value.is_f64())
            && number.fract() == 0.
            && number.abs() <= MAX_SAFE_INTEGER as f64
        {
            *value = Value::from(number as i64);
        }
    }
    Ok(serde_json::from_value(raw)?)
}
impl Cut {
    fn capture(session: &Session) -> Result<Self> {
        Ok(Self {
            header: typed_header(&session.header())?,
            inherited: session.inherited_event_count(),
            events: session.snapshot_events(None, None),
        })
    }
    fn end(&self) -> i64 {
        self.events.len() as i64 - 1
    }
}
fn cursor(value: i64) -> Result<SessionSeqCursor> {
    if value < 0 {
        Ok(SessionSeqCursor::BeforeFirst)
    } else {
        Ok(SessionSeqCursor::Seq(SessionSeq::new(value as u64)?))
    }
}
fn signed(value: SessionSeqCursor) -> i64 {
    match value {
        SessionSeqCursor::BeforeFirst => -1,
        SessionSeqCursor::Seq(value) => value.get() as i64,
    }
}
/// Assign one own property to a plain JavaScript-object counterpart.
///
/// PORTING §1: `object["__proto__"] = value` invokes the inherited prototype
/// setter instead of creating an enumerable own property, so JSON omits it.
fn insert_plain_object_value<T>(values: &mut IndexMap<String, T>, key: &str, value: T) {
    if key != "__proto__" {
        values.insert(key.to_owned(), value);
    }
}
fn event_number(event: &SessionEvent) -> Option<u64> {
    let value = event.as_value().get("seq")?;
    value
        .as_u64()
        .filter(|value| *value <= MAX_SAFE_INTEGER)
        .or_else(|| {
            value
                .as_f64()
                .filter(|n| {
                    n.is_finite() && *n >= 0. && *n <= MAX_SAFE_INTEGER as f64 && n.fract() == 0.
                })
                .map(|n| n as u64)
        })
}
pub(crate) fn checked_version(key: &str, version: f64) -> Result<u64> {
    if !version.is_finite()
        || version < 0.
        || version > MAX_SAFE_INTEGER as f64
        || version.fract() != 0.
    {
        return Err(error(format!(
            "session projection {} stateVersion must be a non-negative integer, got {}",
            quote(key),
            hoh_util_values::js_number_string(version)
        )));
    }
    Ok(version as u64)
}

impl SessionProjectionRegistry {
    pub fn new(ctx: &Context) -> Result<Self> {
        let registry = Self {
            inner: Arc::new(Inner {
                owner: Owner::default(),
                table: Mutex::new(Table::default()),
            }),
        };
        let created = registry.clone();
        let events = registry.clone();
        ctx.try_effect::<hoh_error::BoxError>("sessionProjections", |undos| {
            undos.push(SessionStore::on_created(ctx, move |session| {
                created.created(session)
            })?)?;
            undos.push(ctx.on_contained::<SessionEventEmitted>(move |event| {
                events.drive(&event.session, &event.event)
            })?)?;
            Ok(())
        })?;
        Ok(registry)
    }
    fn registration_at(&self, index: usize) -> Option<Arc<Registration>> {
        lock(&self.inner.table)
            .registrations
            .get_index(index)
            .map(|(_, value)| value.clone())
    }
    fn created(&self, session: &Session) -> Result<()> {
        let _operation = self.inner.owner.enter();
        if session.seq().get() != 0 {
            return Ok(());
        }
        let cut = Cut::capture(session)?;
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0
                || Self::cached(&registration, session).is_some()
            {
                continue;
            }
            let cell = Cell {
                state: (registration.def.init)(&cut.header, cut.inherited)?,
                observed: -1,
                views: [None, None],
            };
            Self::install(&registration, session, cell);
        }
        Ok(())
    }
    pub fn register<S: Serialize + Send + Sync + 'static>(
        &self,
        ctx: &Context,
        definition: ProjectionDefinition<S>,
    ) -> ProjectionRegistration<S> {
        let _operation = self.inner.owner.enter();
        let observed = Arc::new(AtomicBool::new(false));
        let release = Arc::new(Release {
            registry: Arc::downgrade(&self.inner),
            target: Mutex::new(None),
            done: AtomicBool::new(false),
            effect: Mutex::new(None),
        });
        let ready=(|| -> Result<ProjectionHandle<S>> {
            checked_version(&definition.key,definition.state_version as f64)?;
            let erased=erase(definition);
            let mut selected=None;
            let undo_release=release.clone();
            let effect=ctx.try_effect::<hoh_error::BoxError>("sessionProjections.register()",|undos| {
                let mut table=lock(&self.inner.table);
                let key=erased.key.clone();
                let registration=match table.registrations.get(&key) {
                    Some(existing) if existing.refs.load(Ordering::Acquire)>0 => {
                        if existing.def.state_version!=erased.state_version {
                            return Err(error(format!("session projection key {} is already registered at stateVersion {}; refusing to share it with stateVersion {}",quote(&key),existing.def.state_version,erased.state_version)));
                        }
                        let old_summary=existing.def.wire.as_ref().map(|wire|wire.summary);
                        let new_summary=erased.wire.as_ref().map(|wire|wire.summary);
                        if old_summary!=new_summary {
                            let text=|summary:Option<bool>| summary.map_or_else(||"undefined".into(),|value|value.to_string());
                            return Err(error(format!("session projection key {} is already registered with summary={}; refusing to share it with summary={}",quote(&key),text(old_summary),text(new_summary))));
                        }
                        existing.refs.fetch_add(1,Ordering::AcqRel);existing.clone()
                    }
                    _ => {
                        let registration=Arc::new(Registration { def:erased,refs:AtomicUsize::new(1),cells:Mutex::new(HashMap::new()) });
                        table.registrations.insert(key,registration.clone());table.summary=None;registration
                    }
                };
                drop(table);
                *lock(&release.target)=Some(registration.clone());selected=Some(registration);
                if let Err(cause)=undos.push(Undo::sync(move || undo_release.run())) { release.run();return Err(cause.into()); }
                Ok(())
            })?;
            *lock(&release.effect)=Some(effect);
            let registration=selected.ok_or_else(||error("session projection registration did not install a unit"))?;
            Ok(ProjectionHandle { registry:self.clone(),registration,release:release.clone(),_state:PhantomData })
        })().map_err(SharedError::new);
        // A successful handoff is settled by ProjectionRegistration::poll after
        // its first yield. Keeping that decision out of a spawned task preserves
        // the source order: register(); owner.dispose() rejects deterministically.
        // Immediate setup failures still need background logging when unobserved.
        let handoff = Arc::new(OnceLock::new());
        if let Some(cause) = ready.as_ref().err().cloned() {
            let observed = observed.clone();
            let owner = ctx.clone();
            let _ = ctx.spawn(
                "sessionProjections.register() handoff",
                move |cancel| async move {
                    tokio::task::yield_now().await;
                    if !cancel.is_cancelled()
                        && !observed.load(Ordering::Acquire)
                        && matches!(
                            owner.fiber().state(),
                            FiberState::Loading | FiberState::Active
                        )
                    {
                        owner.logger().error(&cause);
                    }
                    Ok(())
                },
            );
        }
        ProjectionRegistration {
            ready,
            owner: ctx.clone(),
            observed,
            handoff,
            release,
            yielded: false,
        }
    }
    fn cached(registration: &Registration, session: &Session) -> Option<Arc<Mutex<Cell>>> {
        let (key, _) = shim::session_identity(session);
        let mut cells = lock(&registration.cells);
        cells.retain(|_, cached| cached.life.strong_count() > 0);
        cells.get(&key).map(|cached| cached.cell.clone())
    }
    fn install(registration: &Registration, session: &Session, cell: Cell) -> Arc<Mutex<Cell>> {
        let (key, life) = shim::session_identity(session);
        let cell = Arc::new(Mutex::new(cell));
        lock(&registration.cells).insert(
            key,
            CachedCell {
                life,
                cell: cell.clone(),
            },
        );
        cell
    }
    fn build(
        definition: &ErasedDefinition,
        cut: &Cut,
        events: &[Arc<SessionEvent>],
    ) -> Result<Cell> {
        let mut state = (definition.init)(&cut.header, cut.inherited)?;
        for event in events {
            state = (definition.apply)(&state, event)?;
        }
        let observed = events
            .last()
            .and_then(|event| event_number(event))
            .map_or(-1, |seq| seq as i64);
        Ok(Cell {
            state,
            observed,
            views: [None, None],
        })
    }
    fn advance(
        definition: &ErasedDefinition,
        cell: &mut Cell,
        cut: &Cut,
        through: i64,
    ) -> Result<()> {
        if cell.observed >= through {
            return Ok(());
        }
        for seq in cell.observed + 1..=through {
            let event = cut
                .events
                .get(seq as usize)
                .filter(|event| event_number(event) == Some(seq as u64))
                .ok_or_else(|| {
                    error(format!(
                        "session projection {} cannot advance across missing seq {seq}",
                        quote(&definition.key)
                    ))
                })?;
            let next = (definition.apply)(&cell.state, event)?;
            if !(definition.same)(&next, &cell.state)? {
                cell.views[0] = cell.views[1].take();
            }
            cell.state = next;
            cell.observed = seq;
        }
        Ok(())
    }
    fn cell_for(
        registration: &Registration,
        session: &Session,
        cut: &Cut,
    ) -> Result<Arc<Mutex<Cell>>> {
        if let Some(cell) = Self::cached(registration, session) {
            Self::advance(&registration.def, &mut lock(&cell), cut, cut.end())?;
            Ok(cell)
        } else {
            Ok(Self::install(
                registration,
                session,
                Self::build(&registration.def, cut, &cut.events)?,
            ))
        }
    }
    pub(crate) fn read_registration(
        &self,
        registration: &Registration,
        session: &Session,
    ) -> Result<ErasedState> {
        let _operation = self.inner.owner.enter();
        let cut = Cut::capture(session)?;
        let cell = Self::cell_for(registration, session, &cut)?;
        let state = lock(&cell).state.clone();
        Ok(state)
    }
    pub fn state_of<S: Send + Sync + 'static>(
        &self,
        ctx: &Context,
        session: &Session,
        key: &str,
    ) -> Result<Option<Arc<S>>> {
        let _operation = self.inner.owner.enter();
        let (warn, selected) = {
            let mut table = lock(&self.inner.table);
            let warn = table.warned.insert(key.into());
            let registration = table
                .registrations
                .get(key)
                .filter(|entry| entry.refs.load(Ordering::Acquire) > 0)
                .cloned();
            (warn, registration)
        };
        if warn {
            ctx.logger().warn(format!("sessionProjections.stateOf(session, {}) is deprecated; read handle.stateOf(session) from an awaited registration, or use the owning domain service",quote(key)));
        }
        let Some(selected) = selected else {
            return Ok(None);
        };
        let cut = Cut::capture(session)?;
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0 {
                continue;
            }
            Self::cell_for(&registration, session, &cut)?;
        }
        let cell = Self::cell_for(&selected, session, &cut)?;
        let state = lock(&cell).state.clone();
        Ok(Some(typed(state)?))
    }
    pub fn on_changed(
        &self,
        ctx: &Context,
        listener: ProjectionChangeListener,
    ) -> Result<Disposer> {
        let _operation = self.inner.owner.enter();
        let inner = self.inner.clone();
        Ok(ctx.effect("sessionProjections.onChanged()", move || {
            let key = lock(&inner.table).listeners.insert(listener);
            Undo::sync(move || {
                let _operation = inner.owner.enter();
                lock(&inner.table).listeners.remove(key);
            })
        })?)
    }
    pub fn on_event_changed(
        &self,
        ctx: &Context,
        listener: ProjectionEventChangeListener,
    ) -> Result<Disposer> {
        let _operation = self.inner.owner.enter();
        let inner = self.inner.clone();
        Ok(ctx.effect("sessionProjections.onEventChanged()", move || {
            let key = lock(&inner.table).event_listeners.insert(listener);
            Undo::sync(move || {
                let _operation = inner.owner.enter();
                lock(&inner.table).event_listeners.remove(key);
            })
        })?)
    }
    fn selected(keys: Option<ProjectionKeySelection<'_>>, key: &str) -> bool {
        keys.is_none_or(|keys| keys.contains(key))
    }
    fn view_cell(registration: &Registration, cell: &Arc<Mutex<Cell>>) -> Result<Arc<Value>> {
        let wire = registration.def.wire.as_ref().ok_or_else(|| {
            error(format!(
                "session projection {} has no wire view",
                quote(&registration.def.key)
            ))
        })?;
        let state = lock(cell).state.clone();
        let raw = (wire.view)(&state)?;
        (wire.schema)(raw.as_deref())
    }
    pub fn snapshot(
        &self,
        session: &Session,
        keys: Option<ProjectionKeySelection<'_>>,
    ) -> Result<ProjectionSnapshot> {
        let _operation = self.inner.owner.enter();
        let cut = Cut::capture(session)?;
        let mut values = IndexMap::new();
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0
                || registration.def.wire.is_none()
                || !Self::selected(keys, &registration.def.key)
            {
                continue;
            }
            let cell = Self::cell_for(&registration, session, &cut)?;
            insert_plain_object_value(
                &mut values,
                &registration.def.key,
                Self::view_cell(&registration, &cell)?,
            );
        }
        Ok(ProjectionSnapshot {
            as_of_seq: cursor(cut.end())?,
            values,
        })
    }
    pub fn cached_snapshot(
        &self,
        session: &Session,
        keys: Option<ProjectionKeySelection<'_>>,
    ) -> Result<Option<ProjectionSnapshot>> {
        let _operation = self.inner.owner.enter();
        let mut values = IndexMap::new();
        let mut observed = None;
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0
                || registration.def.wire.is_none()
                || !Self::selected(keys, &registration.def.key)
            {
                continue;
            }
            let Some(cell) = Self::cached(&registration, session) else {
                continue;
            };
            insert_plain_object_value(
                &mut values,
                &registration.def.key,
                Self::view_cell(&registration, &cell)?,
            );
            let seq = lock(&cell).observed;
            observed = Some(observed.map_or(seq, |old: i64| old.min(seq)));
        }
        observed
            .map(|seq| {
                Ok(ProjectionSnapshot {
                    as_of_seq: cursor(seq)?,
                    values,
                })
            })
            .transpose()
    }
    pub fn summary_keys(&self) -> Arc<IndexSet<String>> {
        let _operation = self.inner.owner.enter();
        let mut table = lock(&self.inner.table);
        if let Some(keys) = &table.summary {
            return keys.clone();
        }
        let keys: Arc<IndexSet<String>> = Arc::new(
            table
                .registrations
                .values()
                .filter(|registration| {
                    registration.refs.load(Ordering::Acquire) > 0
                        && registration
                            .def
                            .wire
                            .as_ref()
                            .is_some_and(|wire| wire.summary)
                })
                .map(|registration| registration.def.key.clone())
                .collect(),
        );
        table.summary = Some(keys.clone());
        keys
    }
    pub fn checkpoint(&self, session: &Session) -> Result<ProjectionCheckpoint> {
        let _operation = self.inner.owner.enter();
        let cut = Cut::capture(session)?;
        let mut rows = IndexMap::new();
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            let cell = if registration.refs.load(Ordering::Acquire) == 0 {
                Self::cached(&registration, session)
            } else {
                Some(Self::cell_for(&registration, session, &cut)?)
            };
            let Some(cell) = cell else {
                continue;
            };
            let cell = lock(&cell);
            insert_plain_object_value(
                &mut rows,
                &registration.def.key,
                ProjectionCheckpointRow {
                    ver: registration.def.state_version,
                    seq: cursor(cell.observed)?,
                    val: (registration.def.encode)(&cell.state)?,
                },
            );
        }
        Ok(rows)
    }
    pub fn restore_floor(
        &self,
        checkpoint: &ProjectionCheckpoint,
    ) -> Result<Option<SessionLogOffset>> {
        let _operation = self.inner.owner.enter();
        let mut floor = None;
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0 {
                continue;
            }
            let need = checkpoint
                .get(&registration.def.key)
                .filter(|row| row.ver == registration.def.state_version)
                .map_or(0, |row| (signed(row.seq) + 1).max(0) as u64);
            floor = Some(floor.map_or(need, |old: u64| old.min(need)));
        }
        floor
            .map(|floor| SessionLogOffset::new(floor.saturating_sub(1)))
            .transpose()
    }
    pub fn view_checkpoint(
        &self,
        checkpoint: &ProjectionCheckpoint,
        keys: Option<ProjectionKeySelection<'_>>,
    ) -> Result<SessionProjectionMap> {
        let _operation = self.inner.owner.enter();
        let mut values = IndexMap::new();
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0
                || !Self::selected(keys, &registration.def.key)
            {
                continue;
            }
            let Some(wire) = &registration.def.wire else {
                continue;
            };
            let Some(row) = checkpoint
                .get(&registration.def.key)
                .filter(|row| row.ver == registration.def.state_version)
            else {
                continue;
            };
            let Ok(state) = (registration.def.parse)(row.val.as_ref()) else {
                continue;
            };
            let raw = (wire.view)(&state)?;
            let value = (wire.schema)(raw.as_deref())?;
            insert_plain_object_value(&mut values, &registration.def.key, value);
        }
        Ok(values)
    }
    fn restore_states(
        &self,
        checkpoint: &ProjectionCheckpoint,
        events: &[Arc<SessionEvent>],
        base_seq: SessionLogOffset,
        header: &SessionHeader,
        inherited: SessionLogOffset,
    ) -> Result<(ProjectionRestoreResult, IndexMap<String, ErasedState>)> {
        let base = base_seq.get() as i64;
        let end = match events.last() {
            Some(event) => event_number(event).ok_or_else(|| {
                error("session projection cannot restore an event with an invalid seq")
            })? as i64,
            None => base - 1,
        };
        let before = base - 1;
        let mut values = IndexMap::new();
        let mut refreshed = IndexMap::new();
        let mut states = IndexMap::new();
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0 {
                continue;
            }
            let definition = &registration.def;
            let row = checkpoint.get(&definition.key).filter(|row| {
                row.ver == definition.state_version
                    && signed(row.seq) >= before
                    && signed(row.seq) <= end
            });
            if row.is_none() && base > 0 {
                return Err(error(format!(
                    "session projection {} cannot restore from seq {base}: its checkpoint row is missing, version-mismatched, or beyond the supplied log end; re-read from seq 0",
                    quote(&definition.key)
                )));
            }
            let (mut state, from) = match row {
                Some(row) => ((definition.parse)(row.val.as_ref())?, signed(row.seq)),
                None => ((definition.init)(header, inherited)?, before),
            };
            let start = (from - base + 1) as usize;
            for (index, event) in events.iter().enumerate().skip(start) {
                let expected = base_seq.get() + index as u64;
                if event_number(event) != Some(expected) {
                    return Err(error(format!(
                        "session projection {} cannot restore across missing seq {expected}",
                        quote(&definition.key)
                    )));
                }
                state = (definition.apply)(&state, event)?;
            }
            if let Some(wire) = &definition.wire {
                let raw = (wire.view)(&state)?;
                insert_plain_object_value(
                    &mut values,
                    &definition.key,
                    (wire.schema)(raw.as_deref())?,
                );
            }
            insert_plain_object_value(
                &mut refreshed,
                &definition.key,
                ProjectionCheckpointRow {
                    ver: definition.state_version,
                    seq: cursor(end)?,
                    val: (definition.encode)(&state)?,
                },
            );
            states.insert(definition.key.clone(), state);
        }
        Ok((
            ProjectionRestoreResult {
                snapshot: ProjectionSnapshot {
                    as_of_seq: cursor(end)?,
                    values,
                },
                checkpoint: refreshed,
            },
            states,
        ))
    }
    pub fn restore(
        &self,
        checkpoint: &ProjectionCheckpoint,
        events: &[Arc<SessionEvent>],
        base_seq: SessionLogOffset,
        header: &SessionHeader,
        inherited: SessionLogOffset,
    ) -> Result<ProjectionRestoreResult> {
        let _operation = self.inner.owner.enter();
        Ok(self
            .restore_states(checkpoint, events, base_seq, header, inherited)?
            .0)
    }
    pub fn hydrate(
        &self,
        session: &Session,
        checkpoint: &ProjectionCheckpoint,
        events: &[Arc<SessionEvent>],
        base_seq: SessionLogOffset,
    ) -> Result<ProjectionSnapshot> {
        let _operation = self.inner.owner.enter();
        let cut = Cut::capture(session)?;
        let end = match events.last() {
            Some(event) => event_number(event).ok_or_else(|| {
                error("session projection cannot hydrate an event with an invalid seq")
            })? as i64,
            None => base_seq.get() as i64 - 1,
        };
        let mut complete = true;
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0 {
                continue;
            }
            if Self::cached(&registration, session).is_none_or(|cell| lock(&cell).observed != end) {
                complete = false;
                break;
            }
        }
        if complete {
            let mut values = IndexMap::new();
            let mut index = 0;
            while let Some(registration) = self.registration_at(index) {
                index += 1;
                if registration.refs.load(Ordering::Acquire) == 0 || registration.def.wire.is_none()
                {
                    continue;
                }
                if let Some(cell) = Self::cached(&registration, session) {
                    insert_plain_object_value(
                        &mut values,
                        &registration.def.key,
                        Self::view_cell(&registration, &cell)?,
                    );
                }
            }
            return Ok(ProjectionSnapshot {
                as_of_seq: cursor(end)?,
                values,
            });
        }
        let (restored, states) =
            self.restore_states(checkpoint, events, base_seq, &cut.header, cut.inherited)?;
        let mut index = 0;
        while let Some(registration) = self.registration_at(index) {
            index += 1;
            if registration.refs.load(Ordering::Acquire) == 0 {
                continue;
            }
            let Some(state) = states.get(&registration.def.key) else {
                continue;
            };
            if Self::cached(&registration, session).is_some_and(|cell| lock(&cell).observed > end) {
                continue;
            }
            Self::install(
                &registration,
                session,
                Cell {
                    state: state.clone(),
                    observed: end,
                    views: [None, None],
                },
            );
        }
        Ok(restored.snapshot)
    }
    fn drive(&self, session: &Session, event: &SessionEvent) -> Result<()> {
        let _operation = self.inner.owner.enter();
        let cut = Cut::capture(session)?;
        let seq = event_number(event)
            .ok_or_else(|| error("session projection cannot drive an event with an invalid seq"))?;
        let observed = {
            let table = lock(&self.inner.table);
            table.listeners.any() || table.event_listeners.any()
        };
        let mut changes = vec![];
        let driven: Result<()> = (|| {
            let mut index = 0;
            while let Some(registration) = self.registration_at(index) {
                index += 1;
                if registration.refs.load(Ordering::Acquire) == 0 {
                    continue;
                }
                let cell = match Self::cached(&registration, session) {
                    Some(cell) => {
                        if lock(&cell).observed >= seq as i64 {
                            continue;
                        }
                        Self::advance(&registration.def, &mut lock(&cell), &cut, seq as i64 - 1)?;
                        cell
                    }
                    None => {
                        let prefix = cut.events.get(..seq as usize).ok_or_else(|| {
                            error(format!(
                                "session projection {} cannot advance across missing seq {seq}",
                                quote(&registration.def.key)
                            ))
                        })?;
                        Self::install(
                            &registration,
                            session,
                            Self::build(&registration.def, &cut, prefix)?,
                        )
                    }
                };
                let (changed, state) = {
                    let mut cell = lock(&cell);
                    let next = (registration.def.apply)(&cell.state, event)?;
                    let changed = !(registration.def.same)(&next, &cell.state)?;
                    cell.state = next;
                    cell.observed = seq as i64;
                    (changed, cell.state.clone())
                };
                if changed && let Some(wire) = &registration.def.wire {
                    let previous = {
                        let mut cell = lock(&cell);
                        cell.views[0] = cell.views[1].clone();
                        cell.views[0].clone()
                    };
                    if observed {
                        let next = (wire.view)(&state)?;
                        lock(&cell).views[1] = next.clone();
                        if !same_view(previous.as_ref(), next.as_ref()) {
                            let value = (wire.schema)(next.as_deref())?;
                            changes.push(ProjectionChange {
                                key: registration.def.key.clone(),
                                val: value.clone(),
                            });
                            let mut listener_index = 0;
                            loop {
                                let (listener, end) = {
                                    let table = lock(&self.inner.table);
                                    (
                                        table.listeners.at(listener_index),
                                        table.listeners.slots.len(),
                                    )
                                };
                                if listener_index >= end {
                                    break;
                                }
                                listener_index += 1;
                                if let Some(listener) = listener {
                                    listener(
                                        session,
                                        &registration.def.key,
                                        value.clone(),
                                        SessionSeq::new(seq)?,
                                    )?;
                                }
                            }
                        }
                    } else {
                        lock(&cell).views[1] = None;
                    }
                }
            }
            Ok(())
        })();
        // Source finally: event listeners receive earlier changes even after failure.
        if !changes.is_empty() {
            let mut index = 0;
            loop {
                let (listener, end) = {
                    let table = lock(&self.inner.table);
                    (
                        table.event_listeners.at(index),
                        table.event_listeners.slots.len(),
                    )
                };
                if index >= end {
                    break;
                }
                index += 1;
                if let Some(listener) = listener {
                    listener(session, &changes, SessionSeq::new(seq)?)?;
                }
            }
        }
        driven
    }
}

impl Plugin for SessionProjectionRegistry {
    const NAME: &'static str = "SessionProjectionRegistry";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<()> {
        let registry = Self::new(&ctx)?;
        ctx.provide::<SessionProjections>(Arc::new(registry))
            .await?;
        Ok(())
    }
}
