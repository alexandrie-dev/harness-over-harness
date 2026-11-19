use crate::{
    CheckpointIdentity, CheckpointRecord, ProjectionCacheError, Result, SessionProjectionCache,
    error, lock, projection_cache_domain_spec, type_error,
};
use hoh_error::SharedError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_session::{
    Session, SessionCreated, SessionDisposed, SessionEvent, SessionEventEmitted, SessionHeader,
    SessionId, SessionLogOffset, SessionSeqCursor, Sessions,
};
use hoh_session_projection::{
    ProjectionCheckpoint, ProjectionKeySelection, ProjectionSnapshot, SessionProjections,
};
use hoh_storage_domain::{DomainError, KvTable, StorageDomain};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

const TITLE_KEY: &str = "title";

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(deserialize_with = "natural_u64")]
    pub write_every_events: u64,
    #[serde(deserialize_with = "natural_u64")]
    pub write_interval_ms: u64,
}

fn natural_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let value = Value::deserialize(deserializer)?;
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    if let Some(value) = value.as_f64()
        && value.is_finite()
        && value >= 0.0
        && value.fract() == 0.0
        && value < 18_446_744_073_709_551_616.0
    {
        return Ok(value as u64);
    }
    Err(D::Error::custom("expected a non-negative integer"))
}

#[derive(Clone)]
pub struct SessionProjectionCacheService {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: Context,
    projections: hoh_session_projection::SessionProjectionRegistry,
    sessions: hoh_session::SessionStore,
    table: Arc<KvTable<SessionId, CheckpointRecord>>,
    config: Config,
    synchronous: Mutex<()>,
    dirty: Mutex<DirtyTable>,
    write_order: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct DirtyTable {
    entries: Vec<DirtyState>,
    next_timer: u64,
}

struct DirtyState {
    session: Session,
    pending: u64,
    timer: Option<DirtyTimer>,
}

struct DirtyTimer {
    id: u64,
    cancel: tokio::sync::oneshot::Sender<()>,
}

struct CurrentIdentity {
    lifecycle: LifecycleIdentity,
    inherited_event_count: SessionLogOffset,
}

struct LifecycleIdentity {
    format_version: u64,
    created_at: u64,
    cwd: Option<String>,
    is_seeded: bool,
}

struct PreparedWrite {
    session: Session,
    rows: ProjectionCheckpoint,
    identity: CheckpointIdentity,
    flush_live_log: bool,
}

type WriteFuture = Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>;
type PermitFuture =
    Pin<Box<dyn Future<Output = tokio::sync::OwnedMutexGuard<()>> + Send + 'static>>;

enum StartedPermit {
    Ready(tokio::sync::OwnedMutexGuard<()>),
    Pending(PermitFuture),
}

impl StartedPermit {
    async fn acquire(self) -> tokio::sync::OwnedMutexGuard<()> {
        match self {
            Self::Ready(guard) => guard,
            Self::Pending(future) => future.await,
        }
    }
}

struct StartedWrite(WriteFuture);

impl StartedWrite {
    async fn settle(self) -> Result<()> {
        self.0.await
    }
}

impl SessionProjectionCacheService {
    fn record_for(
        &self,
        id: &SessionId,
        expected: &CurrentIdentity,
    ) -> Result<Option<Arc<CheckpointRecord>>> {
        Ok(self
            .inner
            .table
            .get(id)?
            .filter(|record| identity_matches(&record.identity, expected)))
    }

    pub fn cached_snapshot(
        &self,
        meta: &SessionHeader,
        keys: Option<ProjectionKeySelection<'_>>,
    ) -> Result<Option<ProjectionSnapshot>> {
        let expected = lifecycle_identity_of(meta)?;
        let Some(record) = self.inner.table.get(&meta.id)? else {
            return Ok(None);
        };
        if !current_lifecycle_matches(&record.identity, &expected) {
            return Ok(None);
        }
        self.view_record(&record, keys)
    }

    pub fn cached_predecessor_title(
        &self,
        meta: &SessionHeader,
    ) -> Result<Option<ProjectionSnapshot>> {
        let expected = lifecycle_identity_of(meta)?;
        let Some(record) = self.inner.table.get(&meta.id)? else {
            return Ok(None);
        };
        if !predecessor_identity_matches(&record.identity, &expected) {
            return Ok(None);
        }
        let keys = [TITLE_KEY.to_owned()];
        self.view_record(&record, Some(ProjectionKeySelection::List(&keys)))
    }

    fn view_record(
        &self,
        record: &CheckpointRecord,
        keys: Option<ProjectionKeySelection<'_>>,
    ) -> Result<Option<ProjectionSnapshot>> {
        let values = self.inner.projections.view_checkpoint(&record.rows, keys)?;
        let mut as_of_seq = None;
        for (key, row) in &record.rows {
            if !values.contains_key(key) {
                continue;
            }
            let seq = cursor_number(row.seq);
            as_of_seq = Some(as_of_seq.map_or(seq, |old: i64| old.min(seq)));
        }
        as_of_seq
            .map(|seq| {
                Ok(ProjectionSnapshot {
                    as_of_seq: cursor(seq)?,
                    values,
                })
            })
            .transpose()
    }

    pub fn hydrate_prepared(
        &self,
        session: &Session,
        events: &[Arc<SessionEvent>],
    ) -> Result<ProjectionSnapshot> {
        let identity = identity_of_session(session)?;
        let empty = ProjectionCheckpoint::new();
        let record = self.record_for(session.id(), &identity)?;
        let rows = record.as_ref().map_or(&empty, |record| &record.rows);
        let base = SessionLogOffset::new(0)?;
        match self.inner.projections.hydrate(session, rows, events, base) {
            Ok(snapshot) => Ok(snapshot),
            Err(_) if record.is_some() => self
                .inner
                .projections
                .hydrate(session, &empty, events, base),
            Err(cause) => Err(cause),
        }
    }

    pub async fn write(&self, session: &Session) -> Result<()> {
        let started = {
            let _synchronous = lock(&self.inner.synchronous);
            let prepared = self.prepare_write_locked(session)?;
            self.start_prepared_write(prepared)
        };
        started.settle().await
    }

    pub fn cold_snapshot(
        &self,
        meta: &SessionHeader,
        inherited_event_count: SessionLogOffset,
        events: &[Arc<SessionEvent>],
    ) -> Result<ProjectionSnapshot> {
        let synchronous = lock(&self.inner.synchronous);
        let identity = identity_of(meta, inherited_event_count)?;
        let empty = ProjectionCheckpoint::new();
        let record = self.record_for(&meta.id, &identity)?;
        let restored = self.inner.projections.restore(
            record.as_ref().map_or(&empty, |record| &record.rows),
            events,
            SessionLogOffset::new(0)?,
            meta,
            inherited_event_count,
        )?;
        let id = meta.id.clone();
        let rows = restored.checkpoint;
        let stored_identity = stored_identity(identity);
        let service = self.clone();
        let started = self.start_ordered_write(Box::pin(async move {
            service.put(id, stored_identity, rows).await
        }));
        drop(synchronous);
        let warning_id = meta.id.clone();
        let logger = self.inner.ctx.logger();
        let label = format!("sessionProjectionCache.coldSnapshot({warning_id})");
        if let Err(cause) = self.inner.ctx.spawn(label, move |_| async move {
            if let Err(cause) = started.settle().await {
                logger.warn(format!(
                    "session projection cache: cold-read write-back for \"{warning_id}\" failed (cache stays stale): {}",
                    js_error_string(cause.as_ref())
                ));
            }
            Ok(())
        }) {
            self.inner.ctx.logger().warn(format!(
                "session projection cache: cold-read write-back for \"{}\" failed (cache stays stale): {}",
                meta.id,
                js_error_string(&cause)
            ));
        }
        Ok(restored.snapshot)
    }

    fn prepare_write_locked(&self, session: &Session) -> Result<PreparedWrite> {
        let rows = self.inner.projections.checkpoint(session)?;
        self.mark_clean(session);
        let identity = stored_identity(identity_of_session(session)?);
        let flush_live_log = self
            .inner
            .sessions
            .get(session.id())
            .is_some_and(|live| live == *session);
        Ok(PreparedWrite {
            session: session.clone(),
            rows,
            identity,
            flush_live_log,
        })
    }

    async fn finish_write(&self, prepared: PreparedWrite) -> Result<()> {
        if prepared.flush_live_log {
            self.inner.sessions.flush(&prepared.session).await?;
        }
        self.put(
            prepared.session.id().clone(),
            prepared.identity,
            prepared.rows,
        )
        .await
    }

    fn start_prepared_write(&self, prepared: PreparedWrite) -> StartedWrite {
        let service = self.clone();
        self.start_ordered_write(Box::pin(
            async move { service.finish_write(prepared).await },
        ))
    }

    fn start_ordered_write(&self, future: WriteFuture) -> StartedWrite {
        let mut permit: PermitFuture = Box::pin(self.inner.write_order.clone().lock_owned());
        let mut cx = TaskContext::from_waker(Waker::noop());
        let permit = match permit.as_mut().poll(&mut cx) {
            Poll::Ready(guard) => StartedPermit::Ready(guard),
            Poll::Pending => StartedPermit::Pending(permit),
        };
        StartedWrite(Box::pin(async move {
            let _guard = permit.acquire().await;
            future.await
        }))
    }

    async fn put(
        &self,
        id: SessionId,
        identity: CheckpointIdentity,
        rows: ProjectionCheckpoint,
    ) -> Result<()> {
        let failure = || {
            type_error(
                "projection checkpoint is not losslessly JSON-serializable (a unit state violates the plain-JSON contract)",
            )
        };
        if rows.values().any(|row| {
            row.val
                .as_ref()
                .is_none_or(|value| !hoh_util_values::is_json_value(value))
        }) {
            return Err(failure());
        }
        let value = serde_json::to_value(&rows).map_err(|_| failure())?;
        let detached = hoh_util_values::snapshot_json_value(&value).ok_or_else(failure)?;
        let rows = serde_json::from_value(detached).map_err(|_| failure())?;
        self.inner
            .table
            .put(id, CheckpointRecord { identity, rows })
            .await?;
        Ok(())
    }

    fn flush_soft(&self, session: Session, trigger: &'static str) {
        let started = {
            let _synchronous = lock(&self.inner.synchronous);
            self.prepare_write_locked(&session)
                .map(|prepared| self.start_prepared_write(prepared))
        };
        self.launch_soft(session, trigger, started);
    }

    fn launch_soft(&self, session: Session, trigger: &'static str, started: Result<StartedWrite>) {
        let started = match started {
            Ok(started) => started,
            Err(cause) => {
                self.warn_write(trigger, session.id(), cause.as_ref());
                return;
            }
        };
        let logger = self.inner.ctx.logger();
        let id = session.id().clone();
        let label = format!("sessionProjectionCache.{trigger}({id})");
        if let Err(cause) = self.inner.ctx.spawn(label, move |_| async move {
            if let Err(cause) = started.settle().await {
                logger.warn(format!(
                    "session projection cache: {trigger} write for \"{id}\" failed (cache stays stale): {}",
                    js_error_string(cause.as_ref())
                ));
            }
            Ok(())
        }) {
            self.warn_write(trigger, session.id(), &cause);
        }
    }

    fn warn_write(&self, trigger: &str, id: &SessionId, cause: &(dyn std::error::Error + 'static)) {
        self.inner.ctx.logger().warn(format!(
            "session projection cache: {trigger} write for \"{id}\" failed (cache stays stale): {}",
            js_error_string(cause)
        ));
    }

    fn mark_clean(&self, session: &Session) {
        let mut dirty = lock(&self.inner.dirty);
        if let Some(state) = dirty
            .entries
            .iter_mut()
            .find(|state| state.session == *session)
        {
            state.pending = 0;
            if let Some(timer) = state.timer.take() {
                let _ = timer.cancel.send(());
            }
        }
    }

    fn retire(&self, session: &Session) {
        lock(&self.inner.dirty)
            .entries
            .retain(|state| state.session != *session);
    }

    fn on_event(&self, session: &Session, event: &SessionEvent) -> Result<()> {
        let synchronous = lock(&self.inner.synchronous);
        if event.event_type() == "turn/end" {
            let started = self
                .prepare_write_locked(session)
                .map(|prepared| self.start_prepared_write(prepared));
            drop(synchronous);
            self.launch_soft(session.clone(), "turn/end", started);
            return Ok(());
        }
        let mut timer = None;
        let threshold = {
            let mut dirty = lock(&self.inner.dirty);
            let index = dirty
                .entries
                .iter()
                .position(|state| state.session == *session)
                .unwrap_or_else(|| {
                    dirty.entries.push(DirtyState {
                        session: session.clone(),
                        pending: 0,
                        timer: None,
                    });
                    dirty.entries.len() - 1
                });
            dirty.entries[index].pending += 1;
            if dirty.entries[index].pending >= self.inner.config.write_every_events {
                true
            } else {
                if dirty.entries[index].timer.is_none() {
                    dirty.next_timer = dirty.next_timer.wrapping_add(1);
                    let id = dirty.next_timer;
                    let (cancel, cancelled) = tokio::sync::oneshot::channel();
                    dirty.entries[index].timer = Some(DirtyTimer { id, cancel });
                    timer = Some((id, cancelled));
                }
                false
            }
        };
        if threshold {
            let started = self
                .prepare_write_locked(session)
                .map(|prepared| self.start_prepared_write(prepared));
            drop(synchronous);
            self.launch_soft(session.clone(), "count threshold", started);
        } else if let Some((timer, cancelled)) = timer {
            drop(synchronous);
            self.arm_timer(session.clone(), timer, cancelled)?;
        }
        Ok(())
    }

    fn arm_timer(
        &self,
        session: Session,
        timer: u64,
        mut cancelled: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        let service = self.clone();
        // Node setTimeout warns and schedules 1ms when its signed 32-bit delay overflows.
        let delay_ms = if self.inner.config.write_interval_ms > hoh_timeout::MAX_TIMER_DELAY_MS {
            1
        } else {
            self.inner.config.write_interval_ms
        };
        let delay = Duration::from_millis(delay_ms);
        let label = format!("sessionProjectionCache.interval({})", session.id());
        let clock = self.inner.ctx.clone();
        self.inner.ctx.spawn(label, move |_| async move {
            tokio::select! {
                biased;
                _ = &mut cancelled => return Ok(()),
                result = clock.sleep(delay) => if result.is_err() { return Ok(()); },
            }
            let claimed = {
                let dirty = lock(&service.inner.dirty);
                dirty
                    .entries
                    .iter()
                    .find(|state| state.session == session)
                    .is_some_and(|state| {
                        state
                            .timer
                            .as_ref()
                            .is_some_and(|active| active.id == timer)
                    })
            };
            if claimed {
                service.flush_soft(session, "interval");
            }
            Ok(())
        })?;
        Ok(())
    }

    fn install_write_path(&self) -> Result<()> {
        let events = self.clone();
        self.inner
            .ctx
            .on_contained::<SessionEventEmitted>(move |event| {
                events.on_event(&event.session, &event.event)
            })?;
        let created = self.clone();
        self.inner
            .ctx
            .on_contained::<SessionCreated>(move |event| {
                created.flush_soft(event.session.clone(), "create");
                Ok(())
            })?;
        let disposed = self.clone();
        self.inner
            .ctx
            .on_contained::<SessionDisposed>(move |event| {
                disposed.flush_soft(event.session.clone(), "detach");
                disposed.mark_clean(&event.session);
                disposed.retire(&event.session);
                Ok(())
            })?;
        let dirty = self.inner.clone();
        self.inner
            .ctx
            .effect("sessionProjectionCache.timers", move || {
                Undo::sync(move || lock(&dirty.dirty).entries.clear())
            })?;
        Ok(())
    }
}

#[derive(Inject)]
pub struct Deps {
    storage_domain: Service<StorageDomain>,
    projections: Service<SessionProjections>,
    sessions: Service<Sessions>,
}

pub struct SessionProjectionCachePlugin;

impl Plugin for SessionProjectionCachePlugin {
    const NAME: &'static str = "SessionProjectionCache";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<()> {
        let config = (*config.get()).clone();
        if config.write_every_events == 0 {
            return Err(error("writeEveryEvents must be at least 1"));
        }
        if config.write_interval_ms == 0 {
            return Err(error("writeIntervalMs must be at least 1"));
        }
        let domain = deps
            .storage_domain
            .open(projection_cache_domain_spec()?)
            .await?;
        let close = domain.clone();
        ctx.effect("sessionProjectionCache.domainClose", move || {
            Undo::future(move || async move {
                close.close().await?;
                Ok(())
            })
        })?;
        let service = SessionProjectionCacheService {
            inner: Arc::new(Inner {
                ctx: ctx.clone(),
                projections: (*deps.projections).clone(),
                sessions: (*deps.sessions).clone(),
                table: domain.table("sessions")?,
                config,
                synchronous: Mutex::new(()),
                dirty: Mutex::new(DirtyTable::default()),
                write_order: Arc::new(tokio::sync::Mutex::new(())),
            }),
        };
        service.install_write_path()?;
        ctx.provide::<SessionProjectionCache>(Arc::new(service))
            .await?;
        Ok(())
    }
}

fn lifecycle_identity_of(header: &SessionHeader) -> Result<LifecycleIdentity> {
    let created_at = u64::try_from(header.created_at)
        .map_err(|_| error("checkpoint identity createdAt must be a non-negative integer"))?;
    Ok(LifecycleIdentity {
        format_version: header.version,
        created_at,
        cwd: header.cwd.clone(),
        is_seeded: header.is_seeded,
    })
}

fn identity_of(
    header: &SessionHeader,
    inherited_event_count: SessionLogOffset,
) -> Result<CurrentIdentity> {
    if !header.is_seeded && inherited_event_count.get() != 0 {
        return Err(error(
            "unseeded projection-cache identity inherited event count must be 0",
        ));
    }
    Ok(CurrentIdentity {
        lifecycle: lifecycle_identity_of(header)?,
        inherited_event_count,
    })
}

fn identity_of_session(session: &Session) -> Result<CurrentIdentity> {
    let mut raw = session.header().as_ref().clone();
    // Session validation follows Number.isSafeInteger and accepts integral JSON
    // floats; serde's i64 decoder does not, so normalize the typed boundary.
    for key in ["createdAt", "delegationDepth"] {
        if let Some(value) = raw.get_mut(key)
            && let Some(number) = value.as_f64().filter(|_| value.is_f64())
            && number.fract() == 0.0
            && number.abs() <= hoh_session::MAX_SAFE_INTEGER as f64
        {
            *value = Value::from(number as i64);
        }
    }
    let header: SessionHeader = serde_json::from_value(raw)?;
    identity_of(&header, session.inherited_event_count())
}

fn stored_identity(identity: CurrentIdentity) -> CheckpointIdentity {
    CheckpointIdentity {
        format_version: Some(identity.lifecycle.format_version),
        created_at: identity.lifecycle.created_at,
        cwd: identity.lifecycle.cwd,
        is_seeded: Some(identity.lifecycle.is_seeded),
        inherited_event_count: Some(identity.inherited_event_count),
    }
}

fn lifecycle_identity_matches(stored: &CheckpointIdentity, expected: &LifecycleIdentity) -> bool {
    stored.created_at == expected.created_at
        && stored.cwd == expected.cwd
        && stored.is_seeded.unwrap_or(false) == expected.is_seeded
}

fn current_lifecycle_matches(stored: &CheckpointIdentity, expected: &LifecycleIdentity) -> bool {
    stored.format_version == Some(expected.format_version)
        && lifecycle_identity_matches(stored, expected)
}

fn predecessor_identity_matches(stored: &CheckpointIdentity, expected: &LifecycleIdentity) -> bool {
    stored
        .format_version
        .is_none_or(|format| format < expected.format_version)
        && lifecycle_identity_matches(stored, expected)
}

fn identity_matches(stored: &CheckpointIdentity, expected: &CurrentIdentity) -> bool {
    current_lifecycle_matches(stored, &expected.lifecycle)
        && stored.inherited_event_count.unwrap_or_default() == expected.inherited_event_count
}

fn cursor_number(value: SessionSeqCursor) -> i64 {
    match value {
        SessionSeqCursor::BeforeFirst => -1,
        SessionSeqCursor::Seq(value) => value.get() as i64,
    }
}

fn cursor(value: i64) -> Result<SessionSeqCursor> {
    if value < 0 {
        Ok(SessionSeqCursor::BeforeFirst)
    } else {
        Ok(SessionSeqCursor::Seq(hoh_session::SessionSeq::new(
            value as u64,
        )?))
    }
}

fn js_error_string(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(shared) = error.downcast_ref::<SharedError>() {
        return js_error_string(shared.get_ref());
    }
    let name = if let Some(error) = error.downcast_ref::<ProjectionCacheError>() {
        error.name()
    } else if let Some(error) = error.downcast_ref::<DomainError>() {
        error.name()
    } else if let Some(error) = error.downcast_ref::<hoh_storage::StorageError>() {
        error.name()
    } else if let Some(error) = error.downcast_ref::<hoh_session::SessionError>() {
        error.name()
    } else if let Some(error) = hoh_error::HarnessError::of(error) {
        error.name()
    } else {
        "Error"
    };
    format!("{name}: {error}")
}
