use crate::error::{Result, SessionQueryError, SessionQueryErrorCode as Code};
use crate::helpers::{assert_session_headers_compatible, sort_corpus};
use crate::types::{SessionRecord, SessionTitleObservation, SessionTitleObservationResult};
use futures_util::{StreamExt, stream::FuturesUnordered};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Fiber, Service, Undo};
use hoh_session::{
    Session, SessionEvent, SessionHeader, SessionId, SessionLogOffset, SessionSeedEventState,
    SessionStore, interrupted_turn_closers,
};
use hoh_session_persistence::{
    SessionAccess, SessionFormatStatus, SessionHandleReadOptions, SessionPersistence,
    SessionPersistenceCorruptionError, SessionPersistenceListOptions,
    SessionPersistenceOpenOptions, SessionPersistenceService,
};
use hoh_session_title::fold_session_title;
use hoh_timeout::AbortSignal;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug)]
pub struct ColdSessionLog {
    pub event_state: SessionSeedEventState,
    pub header: SessionHeader,
    pub inherited_event_count: SessionLogOffset,
    pub events: Vec<Arc<SessionEvent>>,
}

#[derive(Clone, Debug)]
pub struct LogicalSession {
    pub header: SessionHeader,
    pub inherited_event_count: SessionLogOffset,
    pub events: Vec<Arc<SessionEvent>>,
}

pub async fn read_cold_session_log(
    persistence: &dyn SessionPersistence,
    session_id: &SessionId,
    signal: Option<AbortSignal>,
) -> Result<ColdSessionLog> {
    let open_options = SessionPersistenceOpenOptions {
        signal: signal.clone(),
    };
    let read_options = SessionHandleReadOptions { signal };
    let handle = persistence
        .open(session_id, SessionAccess::Read, Some(&open_options))
        .await?;
    let read = match handle.read(Some(0), None, Some(&read_options)).await {
        Ok(read) => read,
        Err(cause) => {
            let _ = handle.close().await;
            return Err(cause);
        }
    };
    handle.close().await?;
    let mut events = read.events;
    events.extend(interrupted_turn_closers(&events)?);
    Ok(ColdSessionLog {
        event_state: read.event_state,
        header: handle.header().clone(),
        inherited_event_count: handle.inherited_event_count(),
        events,
    })
}

#[derive(hoh_kernel::Inject)]
struct PersistenceDeps {
    persistence: Service<SessionPersistenceService>,
}

#[derive(Clone)]
pub struct SessionCorpus {
    sessions: SessionStore,
    persistence: Arc<RwLock<Option<Service<SessionPersistenceService>>>>,
    _optional_persistence_fiber: Fiber,
    persisted_read_concurrency: usize,
}

impl SessionCorpus {
    pub fn new(
        ctx: &Context,
        sessions: SessionStore,
        persisted_read_concurrency: usize,
    ) -> Result<Self> {
        let persistence = Arc::new(RwLock::new(None));
        let slot = persistence.clone();
        let fiber = ctx.inject::<PersistenceDeps, _>(move |child, deps| {
            let slot = slot.clone();
            async move {
                let identity = deps.persistence.identity();
                let installed = deps.persistence.clone();
                child.try_effect(
                    "sessionQuery.persistenceBinding",
                    move |undos| -> std::result::Result<(), BoxError> {
                        *slot
                            .write()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(installed);
                        undos.push(Undo::sync(move || {
                            let mut current = slot
                                .write()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if current
                                .as_ref()
                                .is_some_and(|service| service.identity() == identity)
                            {
                                *current = None;
                            }
                        }))?;
                        Ok(())
                    },
                )?;
                Ok(())
            }
        })?;
        Ok(Self {
            sessions,
            persistence,
            _optional_persistence_fiber: fiber,
            persisted_read_concurrency,
        })
    }

    fn persistence(&self) -> Option<Service<SessionPersistenceService>> {
        self.persistence
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub async fn list_sessions(&self, signal: Option<AbortSignal>) -> Result<Vec<SessionRecord>> {
        throw_exact(&signal)?;
        let persisted = match self.persistence() {
            Some(persistence) => list_persisted(&*persistence, signal.clone()).await?,
            None => Vec::new(),
        };
        throw_exact(&signal)?;
        let mut records = persisted
            .into_iter()
            .map(|snapshot| {
                (
                    snapshot.header.id.clone(),
                    SessionRecord {
                        header: snapshot.header,
                        live: false,
                        persisted: true,
                        format_status: snapshot.format_status,
                    },
                )
            })
            .collect::<indexmap::IndexMap<_, _>>();
        for session in self.sessions.list() {
            let header = session_header(&session)?;
            let durable = records.get(&header.id);
            if let Some(durable) = durable {
                assert_session_headers_compatible(&header, &durable.header)?;
            }
            records.insert(
                header.id.clone(),
                SessionRecord {
                    header,
                    live: true,
                    persisted: durable.is_some(),
                    format_status: Some(SessionFormatStatus::Current),
                },
            );
        }
        let mut records = records.into_values().collect::<Vec<_>>();
        sort_corpus(&mut records);
        Ok(records)
    }

    pub async fn load(
        &self,
        session_id: &SessionId,
        signal: Option<AbortSignal>,
    ) -> Result<LogicalSession> {
        throw_exact(&signal)?;
        if let Some(session) = self.sessions.get(session_id) {
            return snapshot_live(&session);
        }
        let Some(persistence) = self.persistence() else {
            return Err(not_found(session_id));
        };
        let listed = list_persisted(&*persistence, signal.clone())
            .await?
            .into_iter()
            .find(|snapshot| &snapshot.header.id == session_id)
            .ok_or_else(|| not_found(session_id))?;
        throw_exact(&signal)?;
        let loaded = inspect_persisted(&*persistence, session_id, signal.clone()).await?;
        throw_exact(&signal)?;
        if let Some(session) = self.sessions.get(session_id) {
            return snapshot_live(&session);
        }
        assert_session_headers_compatible(&loaded.header, &listed.header)?;
        Ok(LogicalSession {
            header: loaded.header,
            inherited_event_count: loaded.inherited_event_count,
            events: clone_events(&loaded.events),
        })
    }

    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }

    pub fn persisted_read_concurrency(&self) -> usize {
        self.persisted_read_concurrency
    }

    pub(crate) fn optional_persistence_disposer(&self) -> Option<hoh_kernel::Disposer> {
        self._optional_persistence_fiber.disposer()
    }

    pub(crate) async fn dispose_optional_persistence(&self) {
        self._optional_persistence_fiber.dispose().await;
    }

    pub async fn read_title_snapshots(
        &self,
        session_ids: &[SessionId],
        signal: Option<AbortSignal>,
    ) -> Result<Vec<SessionTitleObservationResult>> {
        let mut ids = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for id in session_ids {
            if seen.insert(id.clone()) {
                ids.push(id.clone());
            }
        }
        throw_exact(&signal)?;
        let mut resolved = HashMap::new();
        let mut unresolved = Vec::new();
        for id in &ids {
            if let Some(session) = self.sessions.get(id) {
                resolved.insert(
                    id.clone(),
                    match snapshot_live(&session) {
                        Ok(source) => SessionTitleObservationResult::Fulfilled {
                            session_id: id.clone(),
                            value: Box::new(SessionTitleObservation {
                                session: source.header,
                                title: fold_session_title(&source.events),
                            }),
                        },
                        Err(reason) => SessionTitleObservationResult::Rejected {
                            session_id: id.clone(),
                            reason,
                        },
                    },
                );
            } else {
                unresolved.push(id.clone());
            }
        }
        if unresolved.is_empty() {
            return Ok(ordered_title_results(&ids, &mut resolved));
        }
        let Some(persistence) = self.persistence() else {
            for id in unresolved {
                resolved.insert(
                    id.clone(),
                    SessionTitleObservationResult::Rejected {
                        session_id: id.clone(),
                        reason: not_found(&id),
                    },
                );
            }
            return Ok(ordered_title_results(&ids, &mut resolved));
        };
        let persisted = match list_persisted(&*persistence, signal.clone()).await {
            Ok(persisted) => persisted,
            Err(error) => {
                throw_exact(&signal)?;
                let shared = SharedError::new(error);
                for id in unresolved {
                    resolved.insert(
                        id.clone(),
                        SessionTitleObservationResult::Rejected {
                            session_id: id,
                            reason: Box::new(shared.clone()),
                        },
                    );
                }
                return Ok(ordered_title_results(&ids, &mut resolved));
            }
        };
        throw_exact(&signal)?;
        let persisted = persisted
            .into_iter()
            .map(|snapshot| (snapshot.header.id.clone(), snapshot.header))
            .collect::<HashMap<_, _>>();
        let mut queue = unresolved.into_iter();
        let mut active = FuturesUnordered::new();
        let limit = self.persisted_read_concurrency;
        for _ in 0..limit.min(queue.len()) {
            if let Some(id) = queue.next() {
                active.push(self.title_worker(
                    persistence.clone(),
                    id,
                    persisted.clone(),
                    signal.clone(),
                ));
            }
        }
        let mut cancelled = false;
        while let Some((id, result)) = active.next().await {
            if signal.as_ref().is_some_and(AbortSignal::aborted) {
                cancelled = true;
            } else {
                resolved.insert(
                    id.clone(),
                    match result {
                        Ok(value) => SessionTitleObservationResult::Fulfilled {
                            session_id: id,
                            value: Box::new(value),
                        },
                        Err(reason) => SessionTitleObservationResult::Rejected {
                            session_id: id,
                            reason,
                        },
                    },
                );
                if let Some(next) = queue.next() {
                    active.push(self.title_worker(
                        persistence.clone(),
                        next,
                        persisted.clone(),
                        signal.clone(),
                    ));
                }
            }
        }
        if cancelled {
            throw_exact(&signal)?;
        }
        throw_exact(&signal)?;
        Ok(ordered_title_results(&ids, &mut resolved))
    }

    async fn title_worker(
        &self,
        persistence: Service<SessionPersistenceService>,
        id: SessionId,
        persisted: HashMap<SessionId, SessionHeader>,
        signal: Option<AbortSignal>,
    ) -> (SessionId, Result<SessionTitleObservation>) {
        let result = async {
            throw_exact(&signal)?;
            let Some(listed) = persisted.get(&id) else {
                if let Some(session) = self.sessions.get(&id) {
                    let source = snapshot_live(&session)?;
                    return Ok(SessionTitleObservation {
                        session: source.header,
                        title: fold_session_title(&source.events),
                    });
                }
                return Err(not_found(&id));
            };
            let loaded = inspect_persisted(&*persistence, &id, signal.clone()).await?;
            throw_exact(&signal)?;
            if let Some(session) = self.sessions.get(&id) {
                let source = snapshot_live(&session)?;
                return Ok(SessionTitleObservation {
                    session: source.header,
                    title: fold_session_title(&source.events),
                });
            }
            assert_session_headers_compatible(&loaded.header, listed)?;
            Ok(SessionTitleObservation {
                session: loaded.header,
                title: fold_session_title(&loaded.events),
            })
        }
        .await;
        (id, result)
    }
}

fn ordered_title_results(
    ids: &[SessionId],
    resolved: &mut HashMap<SessionId, SessionTitleObservationResult>,
) -> Vec<SessionTitleObservationResult> {
    ids.iter().filter_map(|id| resolved.remove(id)).collect()
}

fn throw_exact(signal: &Option<AbortSignal>) -> Result<()> {
    if let Some(signal) = signal {
        signal
            .throw_if_aborted()
            .map_err(|reason| Box::new(reason) as BoxError)?;
    }
    Ok(())
}

fn session_header(session: &Session) -> Result<SessionHeader> {
    serde_json::from_value((*session.header()).clone()).map_err(|cause| {
        Box::new(SessionQueryError::with_cause(
            format!("session \"{}\" has an invalid header", session.id()),
            Code::CorruptSession,
            cause,
        )) as BoxError
    })
}

fn clone_events(events: &[Arc<SessionEvent>]) -> Vec<Arc<SessionEvent>> {
    events
        .iter()
        .map(|event| Arc::new((**event).clone()))
        .collect()
}

fn snapshot_live(session: &Session) -> Result<LogicalSession> {
    Ok(LogicalSession {
        header: session_header(session)?,
        inherited_event_count: session.inherited_event_count(),
        events: clone_events(&session.snapshot_events(None, None)),
    })
}

async fn list_persisted(
    persistence: &dyn SessionPersistence,
    signal: Option<AbortSignal>,
) -> Result<Vec<hoh_session_persistence::SessionPersistenceSnapshot>> {
    let options = SessionPersistenceListOptions {
        signal: signal.clone(),
    };
    match persistence.list(Some(&options)).await {
        Ok(result) => Ok(result),
        Err(cause) => {
            if signal.as_ref().is_some_and(AbortSignal::aborted) {
                return throw_exact(&signal).map(|()| unreachable!());
            }
            Err(Box::new(SessionQueryError::with_cause(
                format!("session persistence listing failed: {cause}"),
                Code::PersistenceFailed,
                cause,
            )))
        }
    }
}

async fn inspect_persisted(
    persistence: &dyn SessionPersistence,
    session_id: &SessionId,
    signal: Option<AbortSignal>,
) -> Result<ColdSessionLog> {
    match read_cold_session_log(persistence, session_id, signal.clone()).await {
        Ok(log) => Ok(log),
        Err(cause) => {
            if signal.as_ref().is_some_and(AbortSignal::aborted) {
                return throw_exact(&signal).map(|()| unreachable!());
            }
            let corrupt = cause
                .downcast_ref::<SessionPersistenceCorruptionError>()
                .is_some();
            let code = if corrupt {
                Code::CorruptSession
            } else {
                Code::PersistenceFailed
            };
            let prefix = if corrupt {
                format!("stored session \"{session_id}\" is corrupt")
            } else {
                format!("failed to read stored session \"{session_id}\"")
            };
            Err(Box::new(SessionQueryError::with_cause(
                format!("{prefix}: {cause}"),
                code,
                cause,
            )))
        }
    }
}

fn not_found(session_id: &SessionId) -> BoxError {
    SessionQueryError::boxed(
        format!("session \"{session_id}\" not found"),
        Code::SessionNotFound,
    )
}
