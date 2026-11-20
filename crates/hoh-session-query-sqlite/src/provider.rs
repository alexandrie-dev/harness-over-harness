use crate::query::{
    FTS_HIGHLIGHT_END, FTS_HIGHLIGHT_START, NormalizedEventRequest, NormalizedSessionRequest,
    assert_fts5_outer_predicate_count, assert_portable_binding_count, build_event_where,
    build_session_where, make_snippet, normalize_event_request, normalize_session_request,
    quote_fts_data, request_fingerprint_event, request_fingerprint_session, sanitize_fts_text,
    surface_name,
};
use crate::schema::{open_search_database_blocking, read_generation};
use crate::{Config, OpenAt};
use base64::prelude::{BASE64_URL_SAFE_NO_PAD, Engine as _};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::Context;
use hoh_session::{
    SESSION_FORMAT_VERSION, SessionHeader, SessionId, SessionLogOffset, SessionStore,
    snapshot_session_event,
};
use hoh_session_persistence::{
    SessionPersistenceIdentity, SessionPersistenceListOptions, SessionPersistenceRevision,
    SessionPersistenceService, SessionPersistenceSnapshot,
};
use hoh_session_query::{
    SessionEventSearchDocument, SessionEventSearchHit, SessionEventSearchPage,
    SessionEventSearchRequest, SessionEventSurface, SessionQueryError,
    SessionQueryErrorCode as Code, SessionRecord, SessionSearchCursor, SessionSearchExecContext,
    SessionSearchHit, SessionSearchPage, SessionSearchProvider, SessionSearchRequest,
    assert_session_headers_compatible, build_session_event_search_documents, read_cold_session_log,
};
use hoh_timeout::AbortSignal;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter, types::Value};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const STABLE_OBSERVATION_ATTEMPTS: usize = 2;
static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct SqliteSearchProvider {
    ctx: Context,
    sessions: SessionStore,
    config: Config,
    instance: String,
    serial: tokio::sync::Mutex<()>,
    state: Arc<Mutex<State>>,
    closed: Arc<AtomicBool>,
}

struct State {
    db: Option<Connection>,
    persistence_seen: bool,
    persistence_identity: Option<SessionPersistenceIdentity>,
    persistence_epoch: u64,
    global_generation: u64,
    local_generation: u64,
    ready_error: Option<SharedError>,
}

#[derive(Clone)]
struct ObservedSession {
    header: SessionHeader,
    inherited_event_count: SessionLogOffset,
    documents: Vec<SessionEventSearchDocument>,
    fingerprint: String,
}

struct Observation {
    persistence_visible: bool,
    identity: Option<SessionPersistenceIdentity>,
    persisted: HashMap<
        SessionId,
        (
            SessionPersistenceRevision,
            SessionHeader,
            Option<ObservedSession>,
        ),
    >,
    live: HashMap<SessionId, ObservedSession>,
}

#[derive(Debug)]
struct SearchRow {
    header: SessionHeader,
    live: bool,
    persisted: bool,
    seq: hoh_session::SessionSeq,
    event_type: String,
    time: i64,
    surface: SessionEventSurface,
    marked_text: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorPayload {
    version: u8,
    instance: String,
    scope: String,
    fingerprint: String,
    generation: String,
    offset: u64,
}

impl SqliteSearchProvider {
    pub fn new(ctx: Context, sessions: SessionStore, config: Config) -> Self {
        let instance = format!(
            "hoh-{}-{}",
            std::process::id(),
            INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            ctx,
            sessions,
            config,
            instance,
            serial: tokio::sync::Mutex::new(()),
            state: Arc::new(Mutex::new(State {
                db: None,
                persistence_seen: false,
                persistence_identity: None,
                persistence_epoch: 0,
                global_generation: 0,
                local_generation: 0,
                ready_error: None,
            })),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn open_startup(&self) -> Result<(), BoxError> {
        self.ensure_ready(&None).await
    }

    pub async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let _serial = self.serial.lock().await;
        let state = self.state.clone();
        let _ = tokio::task::spawn_blocking(move || lock(&state).db = None).await;
    }

    async fn queue(
        &self,
        signal: &Option<AbortSignal>,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, BoxError> {
        if let Some(signal) = signal {
            tokio::select! {
                guard = self.serial.lock() => Ok(guard),
                _ = signal.cancelled() => Err(boxed("session-search aborted", Code::Aborted)),
            }
        } else {
            Ok(self.serial.lock().await)
        }
    }

    async fn ensure_ready(&self, signal: &Option<AbortSignal>) -> Result<(), BoxError> {
        let state = self.state.clone();
        let config = self.config.clone();
        let closed = self.closed.clone();
        let task =
            tokio::task::spawn_blocking(move || ensure_ready_blocking(&state, &config, &closed));
        wait_for_sqlite_task(task, signal).await
    }

    async fn reconcile(&self, signal: Option<AbortSignal>) -> Result<(), BoxError> {
        let observation = self.observe_stable(signal.clone()).await?;
        throw_if_aborted(&signal)?;
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || reconcile_observation(&state, observation))
            .await
            .map_err(sqlite_task_failure)?
    }

    async fn indexed_persisted(
        &self,
    ) -> Result<HashMap<SessionId, SessionPersistenceRevision>, BoxError> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let state = lock(&state);
            indexed_persisted(state.db.as_ref().ok_or_else(index_closed)?)
        })
        .await
        .map_err(sqlite_task_failure)?
    }

    fn assert_enabled(&self) -> Result<(), BoxError> {
        if self.config.open_at == OpenAt::Never {
            return Err(boxed(
                "session search is disabled: this deployment configures the session-query index with openAt \"never\"",
                Code::SearchDisabled,
            ));
        }
        Ok(())
    }

    async fn observe_stable(&self, signal: Option<AbortSignal>) -> Result<Observation, BoxError> {
        'attempt: for _ in 0..STABLE_OBSERVATION_ATTEMPTS {
            throw_if_aborted(&signal)?;
            let persistence = self.ctx.get::<SessionPersistenceService>();
            let identity = persistence.as_ref().map(|p| p.identity());
            let initially_live = self
                .sessions
                .list()
                .into_iter()
                .map(|s| s.id().clone())
                .collect::<HashSet<_>>();
            let indexed = self.indexed_persisted().await?;
            let can_reuse = {
                let state = lock(&self.state);
                !state.persistence_seen || state.persistence_identity == identity
            };
            let mut persisted = HashMap::new();
            if let Some(service) = &persistence {
                let options = SessionPersistenceListOptions {
                    signal: signal.clone(),
                };
                let before = match service.list(Some(&options)).await {
                    Ok(value) => value,
                    Err(cause) => {
                        if is_abort(&cause) || signal.as_ref().is_some_and(AbortSignal::aborted) {
                            return Err(aborted_with_cause(cause));
                        }
                        if self
                            .ctx
                            .get::<SessionPersistenceService>()
                            .as_ref()
                            .map(|p| p.identity())
                            != identity
                        {
                            continue 'attempt;
                        }
                        return Err(persistence_failure(
                            "session-search persistence observation failed",
                            cause,
                        ));
                    }
                };
                throw_if_aborted(&signal)?;
                for snapshot in before {
                    if persisted.contains_key(&snapshot.header.id) {
                        return Err(duplicate_snapshot(&snapshot.header.id));
                    }
                    let id = snapshot.header.id.clone();
                    let mut loaded = None;
                    if !(can_reuse && indexed.get(&id) == Some(&snapshot.revision))
                        && !initially_live.contains(&id)
                        && self.sessions.get(&id).is_none()
                    {
                        let cold =
                            match read_cold_session_log(&**service, &id, signal.clone()).await {
                                Ok(value) => value,
                                Err(cause) => {
                                    if is_abort(&cause)
                                        || signal.as_ref().is_some_and(AbortSignal::aborted)
                                    {
                                        return Err(aborted_with_cause(cause));
                                    }
                                    if self
                                        .ctx
                                        .get::<SessionPersistenceService>()
                                        .as_ref()
                                        .map(|p| p.identity())
                                        != identity
                                    {
                                        continue 'attempt;
                                    }
                                    return Err(persistence_failure(
                                        "session-search persistence observation failed",
                                        cause,
                                    ));
                                }
                            };
                        throw_if_aborted(&signal)?;
                        assert_session_headers_compatible(&snapshot.header, &cold.header)?;
                        loaded = Some(observe_session(
                            cold.header,
                            cold.inherited_event_count,
                            &cold.events,
                        )?);
                    }
                    persisted.insert(id, (snapshot.revision, snapshot.header, loaded));
                }
                let after = match service.list(Some(&options)).await {
                    Ok(value) => value,
                    Err(cause) => {
                        if is_abort(&cause) || signal.as_ref().is_some_and(AbortSignal::aborted) {
                            return Err(aborted_with_cause(cause));
                        }
                        if self
                            .ctx
                            .get::<SessionPersistenceService>()
                            .as_ref()
                            .map(|p| p.identity())
                            != identity
                        {
                            continue 'attempt;
                        }
                        return Err(persistence_failure(
                            "session-search persistence observation failed",
                            cause,
                        ));
                    }
                };
                throw_if_aborted(&signal)?;
                let after = materialize_snapshot_headers(after)?;
                if !same_snapshots(&persisted, &after)
                    || self
                        .ctx
                        .get::<SessionPersistenceService>()
                        .as_ref()
                        .map(|p| p.identity())
                        != identity
                {
                    continue;
                }
            }
            let mut live = HashMap::new();
            for session in self.sessions.list() {
                let observed = observe_live(&session)?;
                if let Some((_, durable, _)) = persisted.get(session.id()) {
                    assert_session_headers_compatible(&observed.header, durable)?;
                }
                live.insert(session.id().clone(), observed);
            }
            if initially_live != live.keys().cloned().collect() {
                continue;
            }
            return Ok(Observation {
                persistence_visible: persistence.is_some(),
                identity,
                persisted,
                live,
            });
        }
        Err(boxed(
            "session-search persistence observation did not stabilize after one retry",
            Code::PersistenceFailed,
        ))
    }

    async fn query_sessions(
        &self,
        request: &NormalizedSessionRequest,
        offset: u64,
        persistence_visible: bool,
    ) -> Result<Vec<SearchRow>, BoxError> {
        let session_where = build_session_where(&request.session_filters)?;
        let event_where = build_event_where(&request.event_filters)?;
        assert_fts5_outer_predicate_count(
            session_where.predicate_count + event_where.predicate_count,
        )?;
        let where_sql = [session_where.sql.as_str(), event_where.sql.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut bindings = selected_params(&request.query, persistence_visible);
        bindings.extend(session_where.params);
        bindings.extend(event_where.params);
        bindings.push(Value::Integer((request.limit + 1) as i64));
        bindings.push(Value::Integer(offset as i64));
        assert_portable_binding_count(bindings.len())?;
        let sql = format!(
            "{}, filtered AS (SELECT * FROM matched {}), ranked AS (
               SELECT *, ROW_NUMBER() OVER (
                 PARTITION BY session_id ORDER BY match_count DESC, document_length ASC, time DESC, seq DESC
               ) AS event_rank FROM filtered
             ) SELECT * FROM ranked WHERE event_rank = 1
             ORDER BY match_count DESC, document_length ASC, time DESC, session_id ASC, seq DESC
             LIMIT ? OFFSET ?",
            selected_sql(),
            if where_sql.is_empty() { String::new() } else { format!("WHERE {where_sql}") }
        );
        self.run_query(sql, bindings).await
    }

    async fn query_events(
        &self,
        request: &NormalizedEventRequest,
        offset: u64,
        persistence_visible: bool,
    ) -> Result<Vec<SearchRow>, BoxError> {
        let event_where = build_event_where(&request.filters)?;
        assert_fts5_outer_predicate_count(1 + event_where.predicate_count)?;
        let where_sql = if event_where.sql.is_empty() {
            "session_id = ?".to_owned()
        } else {
            format!("session_id = ? AND {}", event_where.sql)
        };
        let mut bindings = selected_params(&request.query, persistence_visible);
        bindings.push(Value::Text(request.session_id.to_string()));
        bindings.extend(event_where.params);
        bindings.push(Value::Integer((request.limit + 1) as i64));
        bindings.push(Value::Integer(offset as i64));
        assert_portable_binding_count(bindings.len())?;
        let sql = format!(
            "{} SELECT * FROM matched WHERE {where_sql}
             ORDER BY match_count DESC, document_length ASC, time DESC, seq DESC LIMIT ? OFFSET ?",
            selected_sql()
        );
        self.run_query(sql, bindings).await
    }

    async fn run_query(
        &self,
        sql: String,
        bindings: Vec<Value>,
    ) -> Result<Vec<SearchRow>, BoxError> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || run_query_blocking(&state, &sql, bindings))
            .await
            .map_err(sqlite_task_failure)?
    }

    async fn target(
        &self,
        id: &SessionId,
        persistence_visible: bool,
    ) -> Result<(SessionHeader, String), BoxError> {
        let state = self.state.clone();
        let id = id.clone();
        tokio::task::spawn_blocking(move || target_blocking(&state, &id, persistence_visible))
            .await
            .map_err(sqlite_task_failure)?
    }

    fn event_hit(&self, row: SearchRow) -> SessionEventSearchHit {
        SessionEventSearchHit {
            record: hoh_session_query::SessionEventRecord {
                session_id: row.header.id,
                seq: row.seq,
                r#type: row.event_type,
                time: row.time,
                surface: row.surface,
            },
            snippet: make_snippet(&row.marked_text, self.config.snippet_chars as usize),
        }
    }
}

fn ensure_ready_blocking(
    state: &Mutex<State>,
    config: &Config,
    closed: &AtomicBool,
) -> Result<(), BoxError> {
    if closed.load(Ordering::Acquire) {
        return Err(index_closed());
    }
    let mut state = lock(state);
    if closed.load(Ordering::Acquire) {
        return Err(index_closed());
    }
    if let Some(error) = &state.ready_error {
        return Err(Box::new(error.clone()));
    }
    if state.db.is_none() {
        let opened = (|| -> Result<_, BoxError> {
            let db = open_search_database_blocking(&config.path, config.journal_mode)?;
            let generation = read_generation(&db)?;
            Ok((db, generation))
        })();
        let (db, generation) = match opened {
            Ok(opened) => opened,
            Err(cause) => {
                let error = SharedError::new(boxed_with_cause(
                    format!("session-search SQLite index failed to open: {cause}"),
                    Code::IndexFailed,
                    cause,
                ));
                state.ready_error = Some(error.clone());
                return Err(Box::new(error));
            }
        };
        state.global_generation = generation.max(0) as u64;
        state.local_generation = state.global_generation;
        state.db = Some(db);
    }
    Ok(())
}

async fn wait_for_sqlite_task<T: Send + 'static>(
    task: tokio::task::JoinHandle<Result<T, BoxError>>,
    signal: &Option<AbortSignal>,
) -> Result<T, BoxError> {
    if let Some(signal) = signal {
        tokio::select! {
            result = task => result.map_err(sqlite_task_failure)?,
            _ = signal.cancelled() => Err(boxed("session-search aborted", Code::Aborted)),
        }
    } else {
        task.await.map_err(sqlite_task_failure)?
    }
}

fn reconcile_observation(state: &Mutex<State>, observation: Observation) -> Result<(), BoxError> {
    let mut state = lock(state);
    let pointer_changed =
        state.persistence_seen && state.persistence_identity != observation.identity;
    let mut had_writes = false;
    let mut next_local = state.local_generation;
    let db = state.db.as_mut().ok_or_else(index_closed)?;
    let mut next_main = read_generation(db)
        .map_err(index_failure("read generation"))?
        .max(0) as u64;
    let previous_main = next_main;
    let existing_persisted = indexed_persisted(db)?;
    let existing_live = indexed_live(db)?;

    let persistent_delete = if observation.persistence_visible {
        existing_persisted
            .keys()
            .filter(|id| !observation.persisted.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let persistent_change = observation
        .persisted
        .iter()
        .filter(|(id, (revision, _, loaded))| {
            loaded.is_some() && (pointer_changed || existing_persisted.get(*id) != Some(revision))
        })
        .map(|(id, (revision, _, loaded))| (id, revision, loaded.as_ref()))
        .collect::<Vec<_>>();
    if !persistent_delete.is_empty() || !persistent_change.is_empty() {
        next_main += 1;
        had_writes = true;
    }
    let live_delete = existing_live
        .keys()
        .filter(|id| !observation.live.contains_key(*id))
        .cloned()
        .collect::<Vec<_>>();
    let mut live_change = Vec::new();
    for (id, observed) in &observation.live {
        let persisted = observation.persisted.contains_key(id);
        if existing_live.get(id) != Some(&(observed.fingerprint.clone(), persisted)) {
            next_local = next_local.max(next_main) + 1;
            live_change.push((observed, next_local, persisted));
        }
    }
    had_writes |= !live_delete.is_empty() || !live_change.is_empty();

    if had_writes {
        let tx = db
            .transaction()
            .map_err(index_failure("begin reconciliation"))?;
        for id in persistent_delete {
            delete_session(&tx, "persisted", &id)?;
        }
        for (id, revision, loaded) in persistent_change {
            let loaded = loaded.ok_or_else(|| {
                boxed(
                    format!("missing loaded revision for session \"{id}\""),
                    Code::IndexFailed,
                )
            })?;
            replace_persisted(&tx, loaded, revision, next_main)?;
        }
        if next_main != previous_main {
            tx.execute(
                "UPDATE search_state SET global_generation = ? WHERE singleton = 1",
                params![next_main as i64],
            )
            .map_err(index_failure("write generation"))?;
        }
        for id in live_delete {
            delete_session(&tx, "live", &id)?;
        }
        for (entry, generation, persisted) in live_change {
            replace_live(&tx, entry, generation, persisted)?;
        }
        tx.commit()
            .map_err(index_failure("commit reconciliation"))?;
    }
    if had_writes || pointer_changed {
        state.global_generation += 1;
    }
    if pointer_changed {
        state.persistence_epoch += 1;
    }
    state.local_generation = next_local;
    state.persistence_seen = true;
    state.persistence_identity = observation.identity;
    Ok(())
}

fn run_query_blocking(
    state: &Mutex<State>,
    sql: &str,
    bindings: Vec<Value>,
) -> Result<Vec<SearchRow>, BoxError> {
    let state = lock(state);
    let db = state.db.as_ref().ok_or_else(index_closed)?;
    let mut statement = db.prepare(sql).map_err(index_failure("prepare search"))?;
    let rows = statement
        .query_map(params_from_iter(bindings), row_to_search)
        .map_err(index_failure("execute search"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(index_failure("read search rows"))
}

fn target_blocking(
    state: &Mutex<State>,
    id: &SessionId,
    persistence_visible: bool,
) -> Result<(SessionHeader, String), BoxError> {
    let state = lock(state);
    let db = state.db.as_ref().ok_or_else(index_closed)?;
    if let Some(value) = header_generation(db, "temp.live_sessions", id)? {
        return Ok((value.0, format!("live:{}", value.1)));
    }
    if persistence_visible && let Some(value) = header_generation(db, "persisted_sessions", id)? {
        return Ok((
            value.0,
            format!("persisted:{}:{}", state.persistence_epoch, value.1),
        ));
    }
    Err(boxed(
        format!("session \"{id}\" not found"),
        Code::SessionNotFound,
    ))
}

impl SessionSearchProvider for SqliteSearchProvider {
    fn search_sessions<'a>(
        &'a self,
        request: SessionSearchRequest,
        exec: SessionSearchExecContext,
    ) -> futures_util::future::BoxFuture<'a, Result<SessionSearchPage<SessionSearchHit>, BoxError>>
    {
        Box::pin(async move {
            self.assert_enabled()?;
            let normalized = normalize_session_request(request, self.config.limits())?;
            let _serial = self.queue(&exec.signal).await?;
            throw_if_aborted(&exec.signal)?;
            self.ensure_ready(&exec.signal).await?;
            self.reconcile(exec.signal.clone()).await?;
            throw_if_aborted(&exec.signal)?;
            let (generation, visible) = {
                let state = lock(&self.state);
                (
                    state.global_generation.to_string(),
                    state.persistence_identity.is_some(),
                )
            };
            let fingerprint = request_fingerprint_session(&normalized);
            let offset = match &normalized.cursor {
                Some(cursor) => decode_cursor(
                    cursor,
                    &self.instance,
                    "sessions",
                    &fingerprint,
                    &generation,
                )?,
                None => 0,
            };
            let rows = self.query_sessions(&normalized, offset, visible).await?;
            let has_more = rows.len() > normalized.limit as usize;
            let mut items = Vec::new();
            for row in rows.into_iter().take(normalized.limit as usize) {
                let session = SessionRecord {
                    header: row.header.clone(),
                    live: row.live,
                    persisted: row.persisted,
                    format_status: None,
                };
                items.push(SessionSearchHit {
                    session,
                    best_match: self.event_hit(row),
                });
            }
            Ok(SessionSearchPage {
                items,
                next_cursor: has_more.then(|| {
                    encode_cursor(&CursorPayload {
                        version: 1,
                        instance: self.instance.clone(),
                        scope: "sessions".to_owned(),
                        fingerprint,
                        generation,
                        offset: offset + normalized.limit,
                    })
                }),
            })
        })
    }

    fn search_events<'a>(
        &'a self,
        request: SessionEventSearchRequest,
        exec: SessionSearchExecContext,
    ) -> futures_util::future::BoxFuture<'a, Result<SessionEventSearchPage, BoxError>> {
        Box::pin(async move {
            self.assert_enabled()?;
            let normalized = normalize_event_request(request, self.config.limits())?;
            let _serial = self.queue(&exec.signal).await?;
            throw_if_aborted(&exec.signal)?;
            self.ensure_ready(&exec.signal).await?;
            self.reconcile(exec.signal.clone()).await?;
            throw_if_aborted(&exec.signal)?;
            let visible = lock(&self.state).persistence_identity.is_some();
            let (session, generation) = self.target(&normalized.session_id, visible).await?;
            let fingerprint = request_fingerprint_event(&normalized);
            let offset = match &normalized.cursor {
                Some(cursor) => {
                    decode_cursor(cursor, &self.instance, "events", &fingerprint, &generation)?
                }
                None => 0,
            };
            let rows = self.query_events(&normalized, offset, visible).await?;
            let has_more = rows.len() > normalized.limit as usize;
            let items = rows
                .into_iter()
                .take(normalized.limit as usize)
                .map(|row| self.event_hit(row))
                .collect();
            Ok(SessionEventSearchPage {
                session,
                items,
                next_cursor: has_more.then(|| {
                    encode_cursor(&CursorPayload {
                        version: 1,
                        instance: self.instance.clone(),
                        scope: "events".to_owned(),
                        fingerprint,
                        generation,
                        offset: offset + normalized.limit,
                    })
                }),
            })
        })
    }
}

fn observe_live(session: &hoh_session::Session) -> Result<ObservedSession, BoxError> {
    let header: SessionHeader =
        serde_json::from_value((*session.header()).clone()).map_err(|cause| {
            boxed_with_cause(
                format!("session \"{}\" has an invalid header", session.id()),
                Code::CorruptSession,
                cause,
            )
        })?;
    let events = session
        .snapshot_events(None, None)
        .iter()
        .map(|e| snapshot_session_event(e).map(Arc::new))
        .collect::<Result<Vec<_>, _>>()?;
    observe_session(header, session.inherited_event_count(), &events)
}

fn observe_session(
    header: SessionHeader,
    inherited_event_count: SessionLogOffset,
    events: &[Arc<hoh_session::SessionEvent>],
) -> Result<ObservedSession, BoxError> {
    let documents = build_session_event_search_documents(&header.id, events)?;
    let value = serde_json::json!({
        "header": header,
        "inheritedEventCount": inherited_event_count,
        "events": events,
    });
    let fingerprint = BASE64_URL_SAFE_NO_PAD.encode(Sha256::digest(
        hoh_util_values::js_json_stringify(&value).as_bytes(),
    ));
    let header = serde_json::from_value(value["header"].clone())?;
    Ok(ObservedSession {
        header,
        inherited_event_count,
        documents,
        fingerprint,
    })
}

fn same_snapshots(
    before: &HashMap<
        SessionId,
        (
            SessionPersistenceRevision,
            SessionHeader,
            Option<ObservedSession>,
        ),
    >,
    after: &HashMap<SessionId, (SessionPersistenceRevision, SessionHeader)>,
) -> bool {
    before.len() == after.len()
        && after.iter().all(|(id, (revision, header))| {
            before
                .get(id)
                .is_some_and(|(first_revision, first_header, _)| {
                    first_revision == revision && same_header(first_header, header)
                })
        })
}

fn materialize_snapshot_headers(
    snapshots: Vec<SessionPersistenceSnapshot>,
) -> Result<HashMap<SessionId, (SessionPersistenceRevision, SessionHeader)>, BoxError> {
    let mut result = HashMap::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let id = snapshot.header.id.clone();
        if result
            .insert(id.clone(), (snapshot.revision, snapshot.header))
            .is_some()
        {
            return Err(duplicate_snapshot(&id));
        }
    }
    Ok(result)
}

fn duplicate_snapshot(id: &SessionId) -> BoxError {
    let cause = std::io::Error::other(format!("persistence listed duplicate session \"{id}\""));
    persistence_failure(
        "session-search persistence observation failed",
        Box::new(cause),
    )
}

fn same_header(a: &SessionHeader, b: &SessionHeader) -> bool {
    a.id == b.id
        && a.created_at == b.created_at
        && a.cwd == b.cwd
        && a.parent_session == b.parent_session
        && a.is_seeded == b.is_seeded
        && a.delegation_depth.unwrap_or(0) == b.delegation_depth.unwrap_or(0)
        && a.agent_preset == b.agent_preset
}

fn indexed_persisted(
    db: &Connection,
) -> Result<HashMap<SessionId, SessionPersistenceRevision>, BoxError> {
    let mut statement = db
        .prepare("SELECT id, revision FROM persisted_sessions")
        .map_err(index_failure("list indexed persistence"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                SessionId::from(row.get::<_, String>(0)?),
                SessionPersistenceRevision::from(row.get::<_, String>(1)?),
            ))
        })
        .map_err(index_failure("list indexed persistence"))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(index_failure("read indexed persistence"))
}

fn indexed_live(db: &Connection) -> Result<HashMap<SessionId, (String, bool)>, BoxError> {
    let mut statement = db
        .prepare("SELECT id, fingerprint, persisted FROM temp.live_sessions")
        .map_err(index_failure("list indexed live sessions"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                SessionId::from(row.get::<_, String>(0)?),
                (row.get(1)?, row.get::<_, i64>(2)? == 1),
            ))
        })
        .map_err(index_failure("list indexed live sessions"))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(index_failure("read indexed live sessions"))
}

fn delete_session(db: &Connection, source: &str, id: &SessionId) -> Result<(), BoxError> {
    let (docs, sessions) = if source == "persisted" {
        ("persisted_docs", "persisted_sessions")
    } else {
        ("temp.live_docs", "temp.live_sessions")
    };
    db.execute(
        &format!("DELETE FROM {docs} WHERE session_id = ?"),
        params![id.to_string()],
    )
    .map_err(index_failure("delete indexed documents"))?;
    db.execute(
        &format!("DELETE FROM {sessions} WHERE id = ?"),
        params![id.to_string()],
    )
    .map_err(index_failure("delete indexed session"))?;
    Ok(())
}

fn replace_persisted(
    db: &Connection,
    entry: &ObservedSession,
    revision: &SessionPersistenceRevision,
    generation: u64,
) -> Result<(), BoxError> {
    delete_session(db, "persisted", &entry.header.id)?;
    insert_header(
        db,
        "persisted_sessions",
        entry,
        Some(revision.to_string()),
        generation,
        false,
    )?;
    insert_documents(db, "persisted_docs", entry)
}

fn replace_live(
    db: &Connection,
    entry: &ObservedSession,
    generation: u64,
    persisted: bool,
) -> Result<(), BoxError> {
    delete_session(db, "live", &entry.header.id)?;
    insert_header(
        db,
        "temp.live_sessions",
        entry,
        Some(entry.fingerprint.clone()),
        generation,
        persisted,
    )?;
    insert_documents(db, "temp.live_docs", entry)
}

fn insert_header(
    db: &Connection,
    table: &str,
    entry: &ObservedSession,
    token: Option<String>,
    generation: u64,
    persisted: bool,
) -> Result<(), BoxError> {
    let header = &entry.header;
    let seed = header
        .is_seeded
        .then_some(entry.inherited_event_count.get() as i64);
    if table == "persisted_sessions" {
        db.execute(
            "INSERT INTO persisted_sessions
             (id, version, created_at, cwd, parent_session, seed_length, delegation_depth, agent_preset, revision, generation)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![header.id.to_string(), header.version as i64, header.created_at, header.cwd,
                header.parent_session.as_ref().map(ToString::to_string), seed,
                header.delegation_depth.map(|v| v as i64), header.agent_preset, token, generation as i64],
        )
    } else {
        db.execute(
            "INSERT INTO temp.live_sessions
             (id, version, created_at, cwd, parent_session, seed_length, delegation_depth, agent_preset, fingerprint, persisted, generation)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![header.id.to_string(), header.version as i64, header.created_at, header.cwd,
                header.parent_session.as_ref().map(ToString::to_string), seed,
                header.delegation_depth.map(|v| v as i64), header.agent_preset, token,
                i64::from(persisted), generation as i64],
        )
    }
    .map_err(index_failure("insert indexed session"))?;
    Ok(())
}

fn insert_documents(db: &Connection, table: &str, entry: &ObservedSession) -> Result<(), BoxError> {
    let mut statement = db
        .prepare(&format!(
            "INSERT INTO {table} (text, session_id, seq, type, time, surface, codepoint_length)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        ))
        .map_err(index_failure("prepare indexed documents"))?;
    for document in &entry.documents {
        let text = sanitize_fts_text(&document.text);
        statement
            .execute(params![
                text,
                document.record.session_id.to_string(),
                document.record.seq.get() as i64,
                document.record.r#type,
                document.record.time,
                surface_name(&document.record.surface),
                text.chars().count() as i64,
            ])
            .map_err(index_failure("insert indexed document"))?;
    }
    Ok(())
}

fn selected_sql() -> &'static str {
    "WITH candidates AS (
      SELECT pd.session_id, ps.version, ps.created_at, ps.cwd, ps.parent_session, ps.seed_length,
        ps.delegation_depth, ps.agent_preset, 0 AS live, 1 AS persisted,
        CAST(pd.seq AS INTEGER) AS seq, pd.type, CAST(pd.time AS INTEGER) AS time, pd.surface,
        highlight(persisted_docs, 0, ?, ?) AS marked_text,
        CAST(pd.codepoint_length AS INTEGER) AS document_length
      FROM persisted_docs AS pd JOIN persisted_sessions AS ps ON ps.id = pd.session_id
      WHERE persisted_docs MATCH ? AND ? = 1
        AND NOT EXISTS (SELECT 1 FROM temp.live_sessions AS ls WHERE ls.id = pd.session_id)
      UNION ALL
      SELECT ld.session_id, ls.version, ls.created_at, ls.cwd, ls.parent_session, ls.seed_length,
        ls.delegation_depth, ls.agent_preset, 1 AS live,
        CASE WHEN ? = 1 THEN ls.persisted ELSE 0 END AS persisted,
        CAST(ld.seq AS INTEGER) AS seq, ld.type, CAST(ld.time AS INTEGER) AS time, ld.surface,
        highlight(live_docs, 0, ?, ?) AS marked_text,
        CAST(ld.codepoint_length AS INTEGER) AS document_length
      FROM temp.live_docs AS ld JOIN temp.live_sessions AS ls ON ls.id = ld.session_id
      WHERE live_docs MATCH ?
    ), matched AS (
      SELECT *, (length(CAST(marked_text AS BLOB)) - length(CAST(replace(marked_text, ?, '') AS BLOB))) / ? AS match_count
      FROM candidates
    )"
}

fn selected_params(query: &str, persistence_visible: bool) -> Vec<Value> {
    let expression = quote_fts_data(query);
    let visible = i64::from(persistence_visible);
    vec![
        Value::Text(FTS_HIGHLIGHT_START.to_string()),
        Value::Text(FTS_HIGHLIGHT_END.to_string()),
        Value::Text(expression.clone()),
        Value::Integer(visible),
        Value::Integer(visible),
        Value::Text(FTS_HIGHLIGHT_START.to_string()),
        Value::Text(FTS_HIGHLIGHT_END.to_string()),
        Value::Text(expression),
        Value::Text(FTS_HIGHLIGHT_START.to_string()),
        Value::Integer(FTS_HIGHLIGHT_START.len_utf8() as i64),
    ]
}

fn row_to_search(row: &Row<'_>) -> rusqlite::Result<SearchRow> {
    let surface = match row.get::<_, String>(13)?.as_str() {
        "current" => SessionEventSurface::Current,
        "shadowed" => SessionEventSurface::Shadowed,
        "log-only" => SessionEventSurface::LogOnly,
        value => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(format!("unknown surface {value}"))),
            ));
        }
    };
    let id = SessionId::from(row.get::<_, String>(0)?);
    Ok(SearchRow {
        header: SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id,
            created_at: row.get(2)?,
            cwd: row.get(3)?,
            parent_session: row.get::<_, Option<String>>(4)?.map(SessionId::from),
            is_seeded: row.get::<_, Option<i64>>(5)?.is_some(),
            origin: None,
            delegation_depth: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
            agent_preset: row.get(7)?,
            extra: Default::default(),
        },
        live: row.get::<_, i64>(8)? == 1,
        persisted: row.get::<_, i64>(9)? == 1,
        seq: hoh_session::SessionSeq::new(row.get::<_, i64>(10)? as u64).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::other(e.to_string())),
            )
        })?,
        event_type: row.get(11)?,
        time: row.get(12)?,
        surface,
        marked_text: row.get(14)?,
    })
}

fn header_generation(
    db: &Connection,
    table: &str,
    id: &SessionId,
) -> Result<Option<(SessionHeader, u64)>, BoxError> {
    let sql = format!(
        "SELECT id, version, created_at, cwd, parent_session, seed_length, delegation_depth, agent_preset, generation FROM {table} WHERE id = ?"
    );
    db.query_row(&sql, params![id.to_string()], |row| {
        Ok((
            SessionHeader {
                version: SESSION_FORMAT_VERSION,
                id: SessionId::from(row.get::<_, String>(0)?),
                created_at: row.get(2)?,
                cwd: row.get(3)?,
                parent_session: row.get::<_, Option<String>>(4)?.map(SessionId::from),
                is_seeded: row.get::<_, Option<i64>>(5)?.is_some(),
                origin: None,
                delegation_depth: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                agent_preset: row.get(7)?,
                extra: Default::default(),
            },
            row.get::<_, i64>(8)? as u64,
        ))
    })
    .optional()
    .map_err(index_failure("read indexed session"))
}

fn encode_cursor(payload: &CursorPayload) -> SessionSearchCursor {
    SessionSearchCursor(
        BASE64_URL_SAFE_NO_PAD.encode(
            hoh_util_values::js_json_stringify(
                &serde_json::to_value(payload).expect("cursor serializes"),
            )
            .as_bytes(),
        ),
    )
}

fn decode_cursor(
    cursor: &SessionSearchCursor,
    instance: &str,
    scope: &str,
    fingerprint: &str,
    generation: &str,
) -> Result<u64, BoxError> {
    let payload = BASE64_URL_SAFE_NO_PAD
        .decode(cursor.0.as_bytes())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorPayload>(&bytes).ok())
        .filter(|p| {
            p.version == 1
                && p.instance == instance
                && p.scope == scope
                && p.fingerprint == fingerprint
                && p.offset <= hoh_session::MAX_SAFE_INTEGER
        })
        .ok_or_else(|| boxed("session-search cursor is invalid", Code::InvalidCursor))?;
    if payload.generation != generation {
        return Err(boxed(
            "session-search cursor is stale because its relevant corpus changed",
            Code::StaleCursor,
        ));
    }
    Ok(payload.offset)
}

fn throw_if_aborted(signal: &Option<AbortSignal>) -> Result<(), BoxError> {
    if signal.as_ref().is_some_and(AbortSignal::aborted) {
        return Err(boxed("session-search aborted", Code::Aborted));
    }
    Ok(())
}

fn boxed(message: impl Into<String>, code: Code) -> BoxError {
    Box::new(SessionQueryError::new(message, code))
}

fn boxed_with_cause(
    message: impl Into<String>,
    code: Code,
    cause: impl Into<BoxError>,
) -> BoxError {
    Box::new(SessionQueryError::with_cause(message, code, cause))
}

fn index_closed() -> BoxError {
    boxed("session-search SQLite index is closed", Code::IndexFailed)
}

fn sqlite_task_failure(cause: tokio::task::JoinError) -> BoxError {
    boxed_with_cause(
        format!("session-search SQLite task failed: {cause}"),
        Code::IndexFailed,
        cause,
    )
}

fn index_failure(_operation: &'static str) -> impl FnOnce(rusqlite::Error) -> BoxError {
    move |cause| {
        boxed_with_cause(
            format!("session-search reconciliation failed: {cause}"),
            Code::IndexFailed,
            cause,
        )
    }
}

fn is_abort(error: &BoxError) -> bool {
    hoh_error::HarnessError::of(error.as_ref()).is_some_and(|error| {
        error.is_named(SessionQueryError::NAME) && error.code() == Code::Aborted.as_str()
    })
}

fn aborted_with_cause(cause: BoxError) -> BoxError {
    boxed_with_cause("session-search aborted", Code::Aborted, cause)
}

fn persistence_failure(message: &str, cause: BoxError) -> BoxError {
    if let Some(error) = hoh_error::HarnessError::of(&*cause)
        && error.is_named(SessionQueryError::NAME)
    {
        return cause;
    }
    boxed_with_cause(
        format!("{message}: {cause}"),
        Code::PersistenceFailed,
        cause,
    )
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use hoh_timeout::AbortController;

    use super::*;

    fn empty_state() -> Mutex<State> {
        Mutex::new(State {
            db: None,
            persistence_seen: false,
            persistence_identity: None,
            persistence_epoch: 0,
            global_generation: 0,
            local_generation: 0,
            ready_error: None,
        })
    }

    fn snapshot(id: &str, revision: &str) -> SessionPersistenceSnapshot {
        SessionPersistenceSnapshot {
            header: SessionHeader::new(SessionId::from(id), 1),
            format_status: None,
            revision: SessionPersistenceRevision::from(revision),
            event_count: None,
            size_bytes: None,
            location: None,
        }
    }

    fn observation(
        persistence_visible: bool,
        identity: Option<SessionPersistenceIdentity>,
        id: &str,
        revision: &str,
        loaded: bool,
    ) -> Observation {
        let header = SessionHeader::new(SessionId::from(id), 1);
        let observed = loaded
            .then(|| observe_session(header.clone(), SessionLogOffset::default(), &[]).unwrap());
        Observation {
            persistence_visible,
            identity,
            persisted: HashMap::from([(
                header.id.clone(),
                (SessionPersistenceRevision::from(revision), header, observed),
            )]),
            live: HashMap::new(),
        }
    }

    #[test]
    fn failed_readiness_is_cached_for_the_provider_lifetime() {
        let state = empty_state();
        let config = Config {
            path: "bad\0path".into(),
            ..Config::default()
        };
        let closed = AtomicBool::new(false);
        let first = ensure_ready_blocking(&state, &config, &closed).unwrap_err();
        let first = first.downcast_ref::<SharedError>().unwrap().clone();
        let second = ensure_ready_blocking(&state, &config, &closed).unwrap_err();
        let second = second.downcast_ref::<SharedError>().unwrap();
        assert!(first.ptr_eq(second));
    }

    #[tokio::test]
    async fn readiness_wait_aborts_without_waiting_for_blocking_open_cleanup() {
        let controller = AbortController::new();
        controller.abort();
        let task = tokio::task::spawn_blocking(|| {
            std::thread::sleep(Duration::from_millis(150));
            Ok::<_, BoxError>(())
        });
        let started = Instant::now();
        let error = wait_for_sqlite_task(task, &Some(controller.signal()))
            .await
            .unwrap_err();
        assert_eq!(
            hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
            Code::Aborted.as_str()
        );
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn rejects_duplicate_ids_in_the_second_snapshot_list() {
        let error =
            materialize_snapshot_headers(vec![snapshot("same", "one"), snapshot("same", "two")])
                .unwrap_err();
        let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), Code::PersistenceFailed.as_str());
        assert_eq!(
            error.message(),
            "session-search persistence observation failed: persistence listed duplicate session \"same\""
        );
    }

    #[test]
    fn durable_generation_does_not_absorb_topology_only_cursor_epochs() {
        let db = open_search_database_blocking(":memory:", crate::JournalMode::Wal).unwrap();
        let state = empty_state();
        lock(&state).db = Some(db);
        let first_identity = SessionPersistenceIdentity::new();
        reconcile_observation(
            &state,
            observation(true, Some(first_identity), "stored", "one", true),
        )
        .unwrap();
        assert_eq!(
            read_generation(lock(&state).db.as_ref().unwrap()).unwrap(),
            1
        );

        reconcile_observation(
            &state,
            Observation {
                persistence_visible: false,
                identity: None,
                persisted: HashMap::new(),
                live: HashMap::new(),
            },
        )
        .unwrap();
        assert_eq!(
            read_generation(lock(&state).db.as_ref().unwrap()).unwrap(),
            1
        );

        reconcile_observation(
            &state,
            observation(
                true,
                Some(SessionPersistenceIdentity::new()),
                "stored",
                "one",
                true,
            ),
        )
        .unwrap();
        let state = lock(&state);
        assert_eq!(read_generation(state.db.as_ref().unwrap()).unwrap(), 2);
        assert_eq!(state.global_generation, 3);
    }

    #[test]
    fn reconciliation_sqlite_failures_keep_the_exact_source_prefix() {
        let db = open_search_database_blocking(":memory:", crate::JournalMode::Wal).unwrap();
        db.execute_batch("PRAGMA query_only = ON;").unwrap();
        let state = empty_state();
        lock(&state).db = Some(db);
        let header = SessionHeader::new(SessionId::from("readonly"), 1);
        let observed = observe_session(header.clone(), SessionLogOffset::default(), &[]).unwrap();
        let error = reconcile_observation(
            &state,
            Observation {
                persistence_visible: false,
                identity: None,
                persisted: HashMap::new(),
                live: HashMap::from([(header.id.clone(), observed)]),
            },
        )
        .unwrap_err();
        let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), Code::IndexFailed.as_str());
        assert_eq!(
            error.message(),
            "session-search reconciliation failed: attempt to write a readonly database"
        );
    }
}
