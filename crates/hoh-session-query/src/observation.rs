use crate::corpus::read_cold_session_log;
use crate::error::{Result, SessionQueryError, SessionQueryErrorCode as Code};
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_session::{
    PrepareSessionOptions, RestoredSessionOptions, Session, SessionEvent, SessionHeader, SessionId,
    SessionLogOffset, SessionSeqCursor, SessionStore,
};
use hoh_session_persistence::{
    SessionPersistence, SessionPersistenceCorruptionError, SessionPersistenceIdentity,
    SessionPersistenceNotFoundError, SessionPersistenceRevision, SessionPersistenceService,
    SessionPersistenceStatOptions,
};
use hoh_session_projection::{ProjectionCheckpoint, ProjectionSnapshot, SessionProjections};
use hoh_session_projection_cache::SessionProjectionCache;
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use std::future::Future;
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectionMode {
    #[default]
    All,
    None,
}

#[derive(Clone, Debug, Default)]
pub struct SessionObservationOptions {
    pub signal: Option<AbortSignal>,
    pub projection_mode: ProjectionMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionObservationSource {
    Live,
    Prepared,
}

struct PreparedEntry {
    persistence_identity: SessionPersistenceIdentity,
    revision: SessionPersistenceRevision,
    session: Session,
    header: SessionHeader,
    events: Arc<[Arc<SessionEvent>]>,
    refs: AtomicUsize,
}

struct ReaderInner {
    ctx: Context,
    sessions: SessionStore,
    capacity: usize,
    cache: Mutex<IndexMap<SessionId, Arc<PreparedEntry>>>,
}

#[derive(Clone)]
pub struct SessionObservationReader {
    inner: Arc<ReaderInner>,
}

pub struct SessionObservation {
    source: SessionObservationSource,
    header: SessionHeader,
    inherited_event_count: SessionLogOffset,
    session: Session,
    cut: SessionLogOffset,
    events: Arc<OnceLock<Arc<[Arc<SessionEvent>]>>>,
    revision: Option<SessionPersistenceRevision>,
    projections: Option<ProjectionSnapshot>,
    entry: Option<Arc<PreparedEntry>>,
    reader: Weak<ReaderInner>,
    disposed: bool,
}

impl SessionObservation {
    pub fn source(&self) -> SessionObservationSource {
        self.source
    }

    pub fn header(&self) -> &SessionHeader {
        &self.header
    }

    pub fn inherited_event_count(&self) -> SessionLogOffset {
        self.inherited_event_count
    }

    pub fn events(&self) -> &[Arc<SessionEvent>] {
        self.events
            .get_or_init(|| {
                self.entry.as_ref().map_or_else(
                    || {
                        self.session
                            .snapshot_events(Some(SessionLogOffset::default()), Some(self.cut))
                    },
                    |entry| entry.events.clone(),
                )
            })
            .as_ref()
    }

    pub fn cursor(&self) -> SessionSeqCursor {
        let count = self.cut.get();
        if count == 0 {
            SessionSeqCursor::BeforeFirst
        } else {
            SessionSeqCursor::Seq(
                hoh_session::SessionSeq::new(count - 1).expect("session cut is safe"),
            )
        }
    }

    pub fn revision(&self) -> Option<&SessionPersistenceRevision> {
        self.revision.as_ref()
    }

    pub fn projections(&self) -> Option<&ProjectionSnapshot> {
        self.projections.as_ref()
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn retain(&self) -> Result<Self> {
        if self.disposed {
            return Err(Box::new(std::io::Error::other(format!(
                "session observation \"{}\" is disposed",
                self.header.id
            ))));
        }
        if let Some(entry) = &self.entry {
            entry.refs.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Self {
            source: self.source,
            header: self.header.clone(),
            inherited_event_count: self.inherited_event_count,
            session: self.session.clone(),
            cut: self.cut,
            events: self.events.clone(),
            revision: self.revision.clone(),
            projections: self.projections.clone(),
            entry: self.entry.clone(),
            reader: self.reader.clone(),
            disposed: false,
        })
    }

    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        if let Some(entry) = &self.entry
            && entry.refs.fetch_sub(1, Ordering::AcqRel) == 1
            && let Some(reader) = self.reader.upgrade()
        {
            evict(&reader, None);
        }
    }
}

impl Drop for SessionObservation {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl SessionObservationReader {
    pub fn new(ctx: Context, sessions: SessionStore, capacity: usize) -> Self {
        Self {
            inner: Arc::new(ReaderInner {
                ctx,
                sessions,
                capacity,
                cache: Mutex::new(IndexMap::new()),
            }),
        }
    }

    pub async fn read(
        &self,
        session_id: &SessionId,
        options: SessionObservationOptions,
    ) -> Result<SessionObservation> {
        loop {
            observation_abort(&options.signal)?;
            if let Some(session) = self.inner.sessions.get(session_id) {
                return self.live(session, options.projection_mode);
            }
            let Some(persistence) = self.inner.ctx.get::<SessionPersistenceService>() else {
                return Err(not_found(session_id, None));
            };
            let snapshot = self
                .stat_source(&*persistence, session_id, options.signal.clone())
                .await?;
            if let Some(session) = self.inner.sessions.get(session_id) {
                return self.live(session, options.projection_mode);
            }
            let identity = persistence.identity();
            let mut entry = self.cached(session_id, &identity, &snapshot.revision);
            if entry.is_none() {
                let loaded = self
                    .load_source(&*persistence, session_id, options.signal.clone())
                    .await?;
                observation_abort(&options.signal)?;
                if let Some(session) = self.inner.sessions.get(session_id) {
                    return self.live(session, options.projection_mode);
                }
                let events: Arc<[Arc<SessionEvent>]> = loaded.events.into();
                let session = match self.inner.sessions.prepare(
                    Some(session_id.clone()),
                    PrepareSessionOptions::Restored(RestoredSessionOptions {
                        seed: events.to_vec(),
                        meta: serde_json::to_value(&loaded.header)?,
                        inherited_event_count: loaded.inherited_event_count,
                        event_state: loaded.event_state,
                    }),
                ) {
                    Ok(session) => session,
                    Err(cause) => {
                        handle_prepare_failure(&self.inner.sessions, session_id, cause)?;
                        continue;
                    }
                };
                let created = Arc::new(PreparedEntry {
                    persistence_identity: identity,
                    revision: snapshot.revision,
                    session,
                    header: loaded.header,
                    events,
                    refs: AtomicUsize::new(0),
                });
                self.store(session_id.clone(), created.clone());
                entry = Some(created);
            }
            let entry = entry.expect("prepared entry exists");
            let projections = if options.projection_mode == ProjectionMode::None {
                None
            } else {
                self.prepared_projections(&entry).map_err(|cause| {
                    Box::new(SessionQueryError::with_cause(
                        format!("failed to project session \"{session_id}\": {cause}"),
                        Code::CorruptSession,
                        cause,
                    )) as BoxError
                })?
            };
            entry.refs.fetch_add(1, Ordering::AcqRel);
            return Ok(SessionObservation {
                source: SessionObservationSource::Prepared,
                header: entry.header.clone(),
                inherited_event_count: entry.session.inherited_event_count(),
                session: entry.session.clone(),
                cut: entry.session.seq(),
                events: Arc::new(OnceLock::new()),
                revision: Some(entry.revision.clone()),
                projections,
                entry: Some(entry),
                reader: Arc::downgrade(&self.inner),
                disposed: false,
            });
        }
    }

    pub async fn inspect<T>(
        &self,
        session_id: &SessionId,
        read: impl FnOnce(&Session) -> T,
        signal: Option<AbortSignal>,
    ) -> Result<T> {
        inspect_acquired(
            self.read(
                session_id,
                SessionObservationOptions {
                    signal: signal.clone(),
                    projection_mode: ProjectionMode::None,
                },
            ),
            read,
            signal,
        )
        .await
    }

    fn live(&self, session: Session, mode: ProjectionMode) -> Result<SessionObservation> {
        let header: SessionHeader = serde_json::from_value((*session.header()).clone())?;
        let projections = if mode == ProjectionMode::All {
            self.inner
                .ctx
                .get::<SessionProjections>()
                .map(|registry| registry.snapshot(&session, None))
                .transpose()
                .map_err(|cause| {
                    Box::new(SessionQueryError::with_cause(
                        format!("failed to project session \"{}\": {cause}", session.id()),
                        Code::CorruptSession,
                        cause,
                    )) as BoxError
                })?
        } else {
            None
        };
        let cut = session.seq();
        Ok(SessionObservation {
            source: SessionObservationSource::Live,
            header,
            inherited_event_count: session.inherited_event_count(),
            session,
            cut,
            events: Arc::new(OnceLock::new()),
            revision: None,
            projections,
            entry: None,
            reader: Arc::downgrade(&self.inner),
            disposed: false,
        })
    }

    async fn stat_source(
        &self,
        persistence: &dyn SessionPersistence,
        session_id: &SessionId,
        signal: Option<AbortSignal>,
    ) -> Result<hoh_session_persistence::SessionPersistenceSnapshot> {
        let options = SessionPersistenceStatOptions {
            signal: signal.clone(),
        };
        let snapshot = match persistence.stat(session_id, Some(&options)).await {
            Ok(snapshot) => snapshot,
            Err(cause) => {
                observation_abort(&signal)?;
                return Err(map_persistence_failure(session_id, cause));
            }
        };
        observation_abort(&signal)?;
        let snapshot = snapshot.ok_or_else(|| not_found(session_id, None))?;
        if snapshot.header.id != *session_id {
            return Err(Box::new(SessionQueryError::new(
                format!(
                    "session persistence returned \"{}\" for \"{session_id}\"",
                    snapshot.header.id
                ),
                Code::SourceConflict,
            )));
        }
        Ok(snapshot)
    }

    async fn load_source(
        &self,
        persistence: &dyn SessionPersistence,
        session_id: &SessionId,
        signal: Option<AbortSignal>,
    ) -> Result<crate::ColdSessionLog> {
        match read_cold_session_log(persistence, session_id, signal.clone()).await {
            Ok(log) => Ok(log),
            Err(cause) => {
                observation_abort(&signal)?;
                Err(map_persistence_failure(session_id, cause))
            }
        }
    }

    fn cached(
        &self,
        id: &SessionId,
        identity: &SessionPersistenceIdentity,
        revision: &SessionPersistenceRevision,
    ) -> Option<Arc<PreparedEntry>> {
        let mut cache = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = cache.get(id)?.clone();
        if &entry.persistence_identity != identity || &entry.revision != revision {
            return None;
        }
        cache.shift_remove(id);
        cache.insert(id.clone(), entry.clone());
        Some(entry)
    }

    fn store(&self, id: SessionId, entry: Arc<PreparedEntry>) {
        {
            let mut cache = self
                .inner
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.shift_remove(&id);
            cache.insert(id, entry.clone());
        }
        evict(&self.inner, Some(&entry));
    }

    fn prepared_projections(&self, entry: &PreparedEntry) -> Result<Option<ProjectionSnapshot>> {
        let Some(registry) = self.inner.ctx.get::<SessionProjections>() else {
            return Ok(None);
        };
        if let Some(cache) = self.inner.ctx.get::<SessionProjectionCache>() {
            cache
                .hydrate_prepared(&entry.session, &entry.events)
                .map(Some)
        } else {
            registry
                .hydrate(
                    &entry.session,
                    &ProjectionCheckpoint::new(),
                    &entry.events,
                    SessionLogOffset::default(),
                )
                .map(Some)
        }
    }
}

async fn inspect_acquired<T>(
    acquired: impl Future<Output = Result<SessionObservation>>,
    read: impl FnOnce(&Session) -> T,
    signal: Option<AbortSignal>,
) -> Result<T> {
    let mut observation = acquired.await?;
    throw_exact(&signal)?;
    let result = read(observation.session());
    observation.dispose();
    Ok(result)
}

fn evict(reader: &ReaderInner, keep: Option<&Arc<PreparedEntry>>) {
    let mut cache = reader
        .cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while cache.len() > reader.capacity {
        let candidate = cache.iter().find_map(|(id, candidate)| {
            let is_keep = keep.is_some_and(|keep| Arc::ptr_eq(keep, candidate));
            (!is_keep && candidate.refs.load(Ordering::Acquire) == 0).then(|| id.clone())
        });
        let Some(id) = candidate else {
            break;
        };
        cache.shift_remove(&id);
    }
}

fn observation_abort(signal: &Option<AbortSignal>) -> Result<()> {
    let Some(reason) = signal.as_ref().and_then(AbortSignal::reason) else {
        return Ok(());
    };
    Err(Box::new(SessionQueryError::with_cause(
        "session observation was aborted",
        Code::Aborted,
        Box::new(reason) as BoxError,
    )))
}

fn throw_exact(signal: &Option<AbortSignal>) -> Result<()> {
    if let Some(signal) = signal {
        signal
            .throw_if_aborted()
            .map_err(|reason| Box::new(reason) as BoxError)?;
    }
    Ok(())
}

fn map_persistence_failure(session_id: &SessionId, cause: BoxError) -> BoxError {
    if cause
        .downcast_ref::<SessionPersistenceNotFoundError>()
        .is_some()
    {
        return not_found(session_id, Some(cause));
    }
    let corrupt = cause
        .downcast_ref::<SessionPersistenceCorruptionError>()
        .is_some();
    let (message, code) = if corrupt {
        (
            format!("stored session \"{session_id}\" is corrupt: {cause}"),
            Code::CorruptSession,
        )
    } else {
        (
            format!("failed to observe session \"{session_id}\": {cause}"),
            Code::PersistenceFailed,
        )
    };
    Box::new(SessionQueryError::with_cause(message, code, cause))
}

fn handle_prepare_failure(
    sessions: &SessionStore,
    session_id: &SessionId,
    cause: BoxError,
) -> Result<()> {
    if sessions.get(session_id).is_some() {
        return Ok(());
    }
    Err(Box::new(SessionQueryError::with_cause(
        format!("stored session \"{session_id}\" is corrupt: {cause}"),
        Code::CorruptSession,
        cause,
    )))
}

fn not_found(session_id: &SessionId, cause: Option<BoxError>) -> BoxError {
    let message = format!("session \"{session_id}\" not found");
    match cause {
        Some(cause) => Box::new(SessionQueryError::with_cause(
            message,
            Code::SessionNotFound,
            cause,
        )),
        None => SessionQueryError::boxed(message, Code::SessionNotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_kernel::Kernel;
    use hoh_session::{CreateSessionOptions, SessionSeqCursor};
    use hoh_timeout::AbortController;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn prepare_failure_retries_when_a_live_owner_claimed_the_id() {
        let kernel = Kernel::start();
        let store = SessionStore::new();
        let id = SessionId::from("prepare-live-collision");
        store
            .create(
                &kernel.root(),
                Some(id.clone()),
                CreateSessionOptions::default(),
            )
            .unwrap();
        assert!(
            handle_prepare_failure(
                &store,
                &id,
                Box::new(std::io::Error::other("prepare collision")),
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn inspect_after_acquire_returns_exact_abort_reason_and_releases_the_lease() {
        let kernel = Kernel::start();
        let store = SessionStore::new();
        let id = SessionId::from("inspect-abort-after-acquire");
        let session = store
            .create(
                &kernel.root(),
                Some(id.clone()),
                CreateSessionOptions::default(),
            )
            .unwrap();
        let header: SessionHeader = serde_json::from_value((*session.header()).clone()).unwrap();
        let entry = Arc::new(PreparedEntry {
            persistence_identity: SessionPersistenceIdentity::new(),
            revision: SessionPersistenceRevision::from("r1"),
            session: session.clone(),
            header: header.clone(),
            events: Arc::from(Vec::<Arc<SessionEvent>>::new()),
            refs: AtomicUsize::new(1),
        });
        let mut cache = IndexMap::new();
        cache.insert(id, entry.clone());
        let reader = Arc::new(ReaderInner {
            ctx: kernel.root(),
            sessions: store,
            capacity: 0,
            cache: Mutex::new(cache),
        });
        let observation = SessionObservation {
            source: SessionObservationSource::Prepared,
            header,
            inherited_event_count: SessionLogOffset::default(),
            session,
            cut: SessionLogOffset::default(),
            events: Arc::new(OnceLock::new()),
            revision: Some(SessionPersistenceRevision::from("r1")),
            projections: None,
            entry: Some(entry.clone()),
            reader: Arc::downgrade(&reader),
            disposed: false,
        };
        assert_eq!(observation.cursor(), SessionSeqCursor::BeforeFirst);

        let controller = AbortController::new();
        let signal = controller.signal();
        let called = Arc::new(AtomicBool::new(false));
        let getter_called = called.clone();
        let error = inspect_acquired(
            async {
                controller.abort_with(std::io::Error::other("cancelled after acquire"));
                Ok(observation)
            },
            move |_| {
                getter_called.store(true, Ordering::SeqCst);
            },
            Some(signal.clone()),
        )
        .await
        .unwrap_err();
        let expected = signal.reason().unwrap();
        assert!(
            hoh_error::SharedError::of(error.as_ref())
                .unwrap()
                .ptr_eq(&expected)
        );
        assert!(!called.load(Ordering::SeqCst));
        assert_eq!(entry.refs.load(Ordering::SeqCst), 0);
        assert!(reader.cache.lock().unwrap().is_empty());
        kernel.shutdown().await;
    }
}
