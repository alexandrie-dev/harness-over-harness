use crate::corpus::SessionCorpus;
use crate::error::{Result, SessionQueryError, SessionQueryErrorCode as Code};
use crate::helpers::{
    build_session_event_records, build_session_event_search_documents,
    filter_session_event_documents, filter_session_results,
    materialize_session_event_result_filters, materialize_session_result_filters, trace_event,
    trace_session,
};
use crate::observation::{SessionObservation, SessionObservationOptions, SessionObservationReader};
use crate::types::*;
use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, Service, ServiceKey, Undo};
use hoh_session::{
    Session, SessionId, SessionSeedEventState, SessionSeq, SessionStore, Sessions, fold_surface,
    snapshot_session_event,
};
use hoh_timeout::AbortSignal;
use std::sync::Arc;

pub trait SessionSearchProvider: Send + Sync + 'static {
    fn search_sessions<'a>(
        &'a self,
        request: SessionSearchRequest,
        exec: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionSearchPage<SessionSearchHit>>>;

    fn search_events<'a>(
        &'a self,
        request: SessionEventSearchRequest,
        exec: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionEventSearchPage>>;
}

pub struct SessionQueryEngine {
    provider: Arc<dyn SessionSearchProvider>,
    read_window_max: u64,
    corpus: SessionCorpus,
    observations: SessionObservationReader,
}

impl SessionQueryEngine {
    pub fn new(
        ctx: Context,
        sessions: SessionStore,
        provider: Arc<dyn SessionSearchProvider>,
        config: Config,
    ) -> Result<Self> {
        if config.persisted_read_concurrency == 0 {
            return Err(SessionQueryError::boxed(
                "session-query: persistedReadConcurrency must be a positive safe integer",
                Code::InvalidConfig,
            ));
        }
        if config.persisted_read_concurrency > hoh_session::MAX_SAFE_INTEGER as usize {
            return Err(SessionQueryError::boxed(
                "session-query: persistedReadConcurrency must be a positive safe integer",
                Code::InvalidConfig,
            ));
        }
        if config.prepared_session_cache_size == 0 {
            return Err(SessionQueryError::boxed(
                "session-query: preparedSessionCacheSize must be a positive safe integer",
                Code::InvalidConfig,
            ));
        }
        if config.prepared_session_cache_size > hoh_session::MAX_SAFE_INTEGER as usize {
            return Err(SessionQueryError::boxed(
                "session-query: preparedSessionCacheSize must be a positive safe integer",
                Code::InvalidConfig,
            ));
        }
        let corpus = SessionCorpus::new(&ctx, sessions.clone(), config.persisted_read_concurrency)?;
        let observations =
            SessionObservationReader::new(ctx, sessions, config.prepared_session_cache_size);
        Ok(Self {
            provider,
            read_window_max: config.read_window_max,
            corpus,
            observations,
        })
    }

    pub async fn search_sessions(
        &self,
        request: SessionSearchRequest,
        exec: SessionSearchExecContext,
    ) -> Result<SessionSearchPage<SessionSearchHit>> {
        self.provider.search_sessions(request, exec).await
    }

    pub async fn search_events(
        &self,
        request: SessionEventSearchRequest,
        exec: SessionSearchExecContext,
    ) -> Result<SessionEventSearchPage> {
        self.provider.search_events(request, exec).await
    }

    pub async fn observe_session(
        &self,
        session_id: &SessionId,
        options: SessionObservationOptions,
    ) -> Result<SessionObservation> {
        self.observations.read(session_id, options).await
    }

    pub async fn inspect_session<T>(
        &self,
        session_id: &SessionId,
        read: impl FnOnce(&Session) -> T,
        signal: Option<AbortSignal>,
    ) -> Result<T> {
        self.observations.inspect(session_id, read, signal).await
    }

    pub async fn list_sessions(&self, signal: Option<AbortSignal>) -> Result<Vec<SessionRecord>> {
        self.corpus.list_sessions(signal).await
    }

    pub async fn read_session(&self, session_id: &SessionId) -> Result<SessionLogSnapshot> {
        let loaded = self.corpus.load(session_id, None).await?;
        Session::from_restore(
            session_id.clone(),
            &loaded.events,
            serde_json::to_value(&loaded.header)?,
            loaded.inherited_event_count,
            SessionSeedEventState::Detached,
            Some(
                hoh_session_format_catalog::message_projections::current_session_message_projections(),
            ),
        )?;
        Ok(SessionLogSnapshot {
            session: loaded.header,
            inherited_event_count: loaded.inherited_event_count,
            events: snapshot_events(&loaded.events)?,
        })
    }

    pub async fn filter_sessions(
        &self,
        filters: &[SessionResultFilter],
        signal: Option<AbortSignal>,
    ) -> Result<Vec<SessionRecord>> {
        let filters = materialize_session_result_filters(filters)?;
        filter_session_results(&self.corpus.list_sessions(signal).await?, &filters)
    }

    pub async fn read_title(
        &self,
        session_id: &SessionId,
        signal: Option<AbortSignal>,
    ) -> Result<Option<hoh_session_title::SessionTitleSnapshot>> {
        Ok(self.read_title_snapshot(session_id, signal).await?.title)
    }

    pub async fn read_title_snapshot(
        &self,
        session_id: &SessionId,
        signal: Option<AbortSignal>,
    ) -> Result<SessionTitleObservation> {
        let mut results = self
            .read_title_snapshots(std::slice::from_ref(session_id), signal)
            .await?;
        match results.pop().expect("one requested id yields one result") {
            SessionTitleObservationResult::Fulfilled { value, .. } => Ok(*value),
            SessionTitleObservationResult::Rejected { reason, .. } => Err(reason),
        }
    }

    pub async fn read_title_snapshots(
        &self,
        session_ids: &[SessionId],
        signal: Option<AbortSignal>,
    ) -> Result<Vec<SessionTitleObservationResult>> {
        self.corpus.read_title_snapshots(session_ids, signal).await
    }

    pub async fn list_events(&self, session_id: &SessionId) -> Result<Vec<SessionEventRecord>> {
        let loaded = self.corpus.load(session_id, None).await?;
        build_session_event_records(session_id, &loaded.events)
    }

    pub async fn filter_events(
        &self,
        session_id: &SessionId,
        filters: &[SessionEventResultFilter],
    ) -> Result<Vec<SessionEventSearchDocument>> {
        let filters = materialize_session_event_result_filters(filters)?;
        let loaded = self.corpus.load(session_id, None).await?;
        let documents = build_session_event_search_documents(session_id, &loaded.events)?;
        filter_session_event_documents(&documents, &filters)
    }

    pub async fn read_surface(&self, session_id: &SessionId) -> Result<SessionSurfaceSnapshot> {
        let loaded = self.corpus.load(session_id, None).await?;
        let projections =
            hoh_session_format_catalog::message_projections::current_session_message_projections();
        let projections = projections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let surface = fold_surface(&loaded.events, &projections).map_err(|cause| {
            Box::new(SessionQueryError::with_cause(
                format!("invalid session surface: {cause}"),
                Code::InvalidSurface,
                cause,
            )) as BoxError
        })?;
        let events = surface
            .nodes
            .iter()
            .filter_map(|seq| loaded.events.get(seq.get() as usize))
            .map(|event| snapshot_session_event(event).map(Arc::new))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(SessionSurfaceSnapshot {
            session: loaded.header,
            inherited_event_count: loaded.inherited_event_count,
            captured_through_seq: loaded.events.last().map(|event| event.seq()),
            events,
        })
    }

    pub async fn trace_session(
        &self,
        session_id: &SessionId,
        signal: Option<AbortSignal>,
    ) -> Result<SessionLineageTrace> {
        let records = self.corpus.list_sessions(signal.clone()).await?;
        if let Some(signal) = signal {
            signal
                .throw_if_aborted()
                .map_err(|reason| Box::new(reason) as BoxError)?;
        }
        trace_session(&records, session_id)
    }

    pub async fn trace_event(
        &self,
        request: &SessionEventTraceRequest,
        signal: Option<AbortSignal>,
    ) -> Result<SessionEventTraceObservation> {
        let loaded = self
            .corpus
            .load(&request.session_id, signal.clone())
            .await?;
        if let Some(signal) = signal {
            signal
                .throw_if_aborted()
                .map_err(|reason| Box::new(reason) as BoxError)?;
        }
        Ok(SessionEventTraceObservation {
            session: loaded.header,
            trace: trace_event(&request.session_id, &loaded.events, request.seq)?,
        })
    }

    pub async fn read_event(
        &self,
        request: &SessionEventReadRequest,
        signal: Option<AbortSignal>,
    ) -> Result<SessionEventWindow> {
        let before = self.read_window("before", request.before)?;
        let after = self.read_window("after", request.after)?;
        let loaded = self
            .corpus
            .load(&request.session_id, signal.clone())
            .await?;
        if let Some(signal) = signal {
            signal
                .throw_if_aborted()
                .map_err(|reason| Box::new(reason) as BoxError)?;
        }
        let index = request.seq.get() as usize;
        let Some(target) = loaded
            .events
            .get(index)
            .filter(|event| event.seq() == request.seq)
        else {
            return Err(SessionQueryError::boxed(
                format!(
                    "session \"{}\" has no event at seq {}",
                    request.session_id, request.seq
                ),
                Code::EventNotFound,
            ));
        };
        let start = index.saturating_sub(before as usize);
        let end = index
            .saturating_add(after as usize)
            .min(loaded.events.len() - 1);
        let target = Arc::new(snapshot_session_event(target)?);
        let mut events = Vec::with_capacity(end - start + 1);
        for (offset, event) in loaded.events[start..=end].iter().enumerate() {
            if start + offset == index {
                events.push(target.clone());
            } else {
                events.push(Arc::new(snapshot_session_event(event)?));
            }
        }
        Ok(SessionEventWindow {
            session: loaded.header,
            inherited_event_count: loaded.inherited_event_count,
            target,
            events,
            start_seq: SessionSeq::new(start as u64)?,
            end_seq: SessionSeq::new(end as u64)?,
        })
    }

    fn read_window(&self, name: &str, value: Option<u64>) -> Result<u64> {
        let value = value.unwrap_or(0);
        if value > self.read_window_max {
            return Err(SessionQueryError::boxed(
                format!(
                    "{name} must be an integer between 0 and {}",
                    self.read_window_max
                ),
                Code::InvalidWindow,
            ));
        }
        Ok(value)
    }
}

pub async fn mount_session_query(
    ctx: &Context,
    sessions: Service<Sessions>,
    provider: Arc<dyn SessionSearchProvider>,
    config: Config,
) -> Result<Disposer> {
    if ctx.get::<crate::SessionQuery>().is_some() {
        return Err(Box::new(std::io::Error::other(format!(
            "service \"{}\" is already registered",
            crate::SessionQuery::NAME
        ))));
    }
    let engine = Arc::new(SessionQueryEngine::new(
        ctx.clone(),
        (*sessions).clone(),
        provider,
        config,
    )?);
    let provision = match ctx.provide::<crate::SessionQuery>(engine.clone()).await {
        Ok(provision) => provision,
        Err(cause) => {
            engine.corpus.dispose_optional_persistence().await;
            return Err(Box::new(cause));
        }
    };
    let optional = engine.corpus.optional_persistence_disposer();
    ctx.try_effect(
        "sessionQuery.mount",
        move |undos| -> std::result::Result<(), BoxError> {
            if let Some(optional) = optional {
                undos.push(Undo::from(optional))?;
            }
            undos.push(Undo::from(provision))?;
            Ok(())
        },
    )
}

fn snapshot_events(
    events: &[Arc<hoh_session::SessionEvent>],
) -> Result<Vec<Arc<hoh_session::SessionEvent>>> {
    events
        .iter()
        .map(|event| snapshot_session_event(event).map(Arc::new))
        .collect()
}
