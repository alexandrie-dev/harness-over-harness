use crate::serial::OperationQueue;
use crate::{
    ArchiveSessionOptions, Workspace, WorkspaceActiveSessionError, WorkspaceAggregateError,
    WorkspaceArchivedSessionPinError, WorkspaceDomainState, WorkspaceId,
    WorkspaceMutationOperation, WorkspaceOrderInvalidError, WorkspacePendingMutation,
    WorkspaceRecord, WorkspaceSessionActivity, WorkspaceSessionArchived, WorkspaceSessionStop,
    WorkspaceSessionUnarchiving, WorkspaceUnknownSessionError, default_workspace_title,
    fully_qualified_workspace_path, live_header, realpath_normalize, workspace_domain_spec,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, KernelError, Service};
use hoh_session::{SessionHeader, SessionId, Sessions};
use hoh_session_persistence::{SessionPersistenceService, SessionPersistenceSnapshot};
use hoh_storage_domain::{Domain, DomainGlobal, KvTable, StorageDomain};
use hoh_util_time::{InstantResult, format_four_digit_year_instant, four_digit_year_epoch};
use indexmap::{IndexMap, IndexSet};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, Mutex},
};

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis() as i64)
        .unwrap_or(0)
}
pub(crate) fn now_iso() -> String {
    format_four_digit_year_instant(now_ms()).expect("the current date has a four-digit year")
}
fn epoch(value: &str) -> Option<i64> {
    match four_digit_year_epoch(value) {
        InstantResult::Accepted { epoch } => Some(epoch),
        _ => match hoh_util_time::parse_offset_instant(value) {
            InstantResult::Accepted { epoch } => Some(epoch),
            _ if value.len() == 10 => {
                match hoh_util_time::parse_offset_instant(&format!("{value}T00:00:00Z")) {
                    InstantResult::Accepted { epoch } => Some(epoch),
                    _ => None,
                }
            }
            _ => None,
        },
    }
}

fn js_iso(epoch: i64) -> Result<String, BoxError> {
    const DAY: i64 = 86_400_000;
    if !(-8_640_000_000_000_000..=8_640_000_000_000_000).contains(&epoch) {
        return Err("session createdAt is outside the JavaScript Date range".into());
    }
    let days = epoch.div_euclid(DAY);
    let rest = epoch.rem_euclid(DAY);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let year = if (0..=9_999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:06}", year.unsigned_abs())
    } else {
        format!("+{year:06}")
    };
    Ok(format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    ))
}
fn boxed(error: hoh_error::SharedError) -> BoxError {
    Box::new(error)
}

fn render_error(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(error) = hoh_error::HarnessError::of(error) {
        return if error.message().is_empty() {
            error.name().to_owned()
        } else {
            format!("{}: {}", error.name(), error.message())
        };
    }
    if let Some(error) = hoh_error::SharedError::of(error) {
        return render_error(error.get_ref());
    }
    format!("Error: {error}")
}

fn preserves_missing_candidate(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

fn ordered_entities(
    workspace_ids: &[WorkspaceId],
    entities: &IndexMap<WorkspaceId, Workspace>,
) -> Result<Vec<Workspace>, BoxError> {
    workspace_ids
        .iter()
        .map(|id| {
            entities.get(id).cloned().ok_or_else(|| {
                format!("workspace registry order references missing workspace '{id}'").into()
            })
        })
        .collect()
}

#[derive(Clone)]
pub struct WorkspaceRegistry {
    pub(crate) inner: Arc<RegistryInner>,
}

pub(crate) struct RegistryInner {
    pub ctx: Context,
    persistence: Service<SessionPersistenceService>,
    domain: Arc<Domain>,
    table: Arc<KvTable<WorkspaceId, WorkspaceRecord>>,
    global: Arc<DomainGlobal<WorkspaceDomainState>>,
    state: Mutex<WorkspaceDomainState>,
    entities: Mutex<IndexMap<WorkspaceId, Workspace>>,
    headers: Mutex<HashMap<SessionId, SessionHeader>>,
    session_paths: Mutex<HashMap<SessionId, String>>,
    missing_session_paths: Mutex<HashSet<SessionId>>,
    invalid_session_paths: Mutex<HashMap<SessionId, String>>,
    operation: OperationQueue,
}

impl WorkspaceRegistry {
    async fn enqueue_operation<T, F, Fut>(&self, operation: F) -> Result<T, BoxError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<RegistryInner>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, BoxError>> + Send + 'static,
    {
        let inner = self.inner.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        let ticket = inner.operation.reserve();
        tokio::spawn(async move {
            let result = async {
                let _turn = inner.operation.enter(ticket).await;
                inner.recover_pending_mutation().await?;
                operation(inner.clone()).await
            }
            .await;
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| -> BoxError { "workspace registry operation task stopped".into() })?
    }

    pub async fn start(
        ctx: Context,
        facility: Service<StorageDomain>,
        persistence: Service<SessionPersistenceService>,
    ) -> Result<Self, BoxError> {
        let domain = facility
            .open(workspace_domain_spec())
            .await
            .map_err(boxed)?;
        let table = domain
            .table::<WorkspaceId, WorkspaceRecord>("workspaces")
            .map_err(boxed)?;
        let global = domain.global::<WorkspaceDomainState>().map_err(boxed)?;
        let state = (*global.get().map_err(boxed)?).clone();
        let registry = Self {
            inner: Arc::new(RegistryInner {
                ctx,
                persistence,
                domain,
                table,
                global,
                state: Mutex::new(state),
                entities: Mutex::new(IndexMap::new()),
                headers: Mutex::new(HashMap::new()),
                session_paths: Mutex::new(HashMap::new()),
                missing_session_paths: Mutex::new(HashSet::new()),
                invalid_session_paths: Mutex::new(HashMap::new()),
                operation: OperationQueue::default(),
            }),
        };
        let initialized = async {
            registry.inner.recover_pending_mutation().await?;
            registry.inner.validate_stored_state()?;
            if !registry.inner.state().initialized {
                let headers = registry.inner.list_stored_headers().await?;
                registry.inner.replace_header_index(&headers).await;
                registry.inner.bootstrap(&headers).await?;
            } else if registry.inner.table.size().map_err(boxed)? > 0 {
                let headers = registry.inner.list_stored_headers().await?;
                registry.inner.replace_header_index(&headers).await;
            }
            registry.inner.index_live_sessions().await?;
            registry.inner.validate_stored_state()?;
            registry.inner.rebuild_entities()?;
            registry.inner.report_filtered_candidates()?;
            Ok::<(), BoxError>(())
        }
        .await;
        if let Err(error) = initialized {
            let _ = registry.inner.domain.close().await;
            return Err(error);
        }
        Ok(registry)
    }

    pub fn domain(&self) -> Arc<Domain> {
        self.inner.domain.clone()
    }

    pub async fn create(&self, path: &str, title: Option<String>) -> Result<Workspace, BoxError> {
        let canonical = realpath_normalize(path).await?;
        if !tokio::fs::metadata(&canonical).await?.is_dir() {
            return Err(format!(
                "cannot create a workspace at '{canonical}': path is not a directory"
            )
            .into());
        }
        self.enqueue_operation(move |inner| async move {
            inner.create_canonical(canonical, title, false).await
        })
        .await
    }

    pub async fn initialize_default<F, Fut>(
        &self,
        resolve_directory: F,
    ) -> Result<Option<Workspace>, BoxError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<String, BoxError>> + Send + 'static,
    {
        self.enqueue_operation(move |inner| async move {
            let state = inner.state();
            if let Some(id) = state.default_workspace_id {
                return Ok(inner.entities().get(&id).cloned());
            }
            let sessions = inner
                .ctx
                .get::<Sessions>()
                .ok_or("default Workspace initialization requires the Session store")?;
            if !state.workspace_ids.is_empty()
                || !state.archived_session_ids.is_empty()
                || !sessions.list().is_empty()
                || !inner.list_stored_headers().await?.is_empty()
            {
                return Ok(None);
            }
            let path = resolve_directory().await?;
            if !fully_qualified_workspace_path(&path) {
                return Err(format!("Workspace path is not fully qualified: '{path}'").into());
            }
            tokio::fs::create_dir_all(&path).await?;
            let canonical = realpath_normalize(&path).await?;
            if !inner.list_stored_headers().await?.is_empty() || !sessions.list().is_empty() {
                return Ok(None);
            }
            let title = default_workspace_title(&path);
            inner
                .create_canonical(canonical, Some(title), true)
                .await
                .map(Some)
        })
        .await
    }

    pub fn get(&self, id: &WorkspaceId) -> Option<Workspace> {
        self.inner.entities().get(id).cloned()
    }
    pub fn list(&self) -> Result<Vec<Workspace>, BoxError> {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entities = self.inner.entities();
        ordered_entities(&state.workspace_ids, &entities)
    }
    pub async fn delete(&self, id: &WorkspaceId) -> Result<bool, BoxError> {
        let id = id.clone();
        self.enqueue_operation(move |inner| async move { inner.delete_known(&id).await })
            .await
    }
    pub async fn insert_before(
        &self,
        id: &WorkspaceId,
        before: Option<&WorkspaceId>,
    ) -> Result<Vec<WorkspaceId>, BoxError> {
        let id = id.clone();
        let before = before.cloned();
        self.enqueue_operation(move |inner| async move {
            let state = inner.state();
            if !state.workspace_ids.contains(&id) {
                return Err(Box::new(WorkspaceOrderInvalidError {
                    workspace_id: id.clone(),
                }) as BoxError);
            }
            if let Some(before) = &before
                && !state.workspace_ids.contains(before)
            {
                return Err(Box::new(WorkspaceOrderInvalidError {
                    workspace_id: before.clone(),
                }) as BoxError);
            }
            if before.as_ref() == Some(&id) {
                return Ok(state.workspace_ids);
            }
            let mut ids = state.workspace_ids.clone();
            ids.retain(|v| v != &id);
            let at = before
                .as_ref()
                .and_then(|v| ids.iter().position(|candidate| candidate == v))
                .unwrap_or(ids.len());
            ids.insert(at, id);
            if ids != state.workspace_ids {
                let mut next = state;
                next.workspace_ids = ids.clone();
                inner.set_state(next).await?;
            }
            Ok(ids)
        })
        .await
    }
    pub fn archived_session_ids(&self) -> Vec<SessionId> {
        self.inner.state().archived_session_ids
    }
    pub fn pinned_session_ids(&self) -> Vec<SessionId> {
        self.inner.state().pinned_session_ids
    }

    pub async fn archive_session(
        &self,
        session_id: SessionId,
        options: ArchiveSessionOptions,
    ) -> Result<(), BoxError> {
        self.enqueue_operation(move |inner| async move {
            inner.archive_known(session_id, options).await
        })
        .await
    }
    pub async fn unarchive_session(&self, session_id: &SessionId) -> Result<(), BoxError> {
        let session_id = session_id.clone();
        self.enqueue_operation(move |inner| async move { inner.unarchive_known(&session_id).await })
            .await
    }
    pub async fn pin_session(&self, session_id: SessionId) -> Result<(), BoxError> {
        self.enqueue_operation(move |inner| async move { inner.pin_known(session_id).await })
            .await
    }
    pub async fn unpin_session(&self, session_id: &SessionId) -> Result<(), BoxError> {
        let session_id = session_id.clone();
        self.enqueue_operation(move |inner| async move { inner.unpin_known(&session_id).await })
            .await
    }
    pub async fn resolve_by_path(&self, path: &str) -> Result<Option<Workspace>, BoxError> {
        let canonical = realpath_normalize(path).await?;
        Ok(self
            .inner
            .entities()
            .values()
            .find(|entity| entity.path() == canonical)
            .cloned())
    }
}

impl RegistryInner {
    fn state(&self) -> WorkspaceDomainState {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    fn entities(&self) -> std::sync::MutexGuard<'_, IndexMap<WorkspaceId, Workspace>> {
        self.entities.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub(crate) fn table(&self) -> Result<Arc<KvTable<WorkspaceId, WorkspaceRecord>>, BoxError> {
        Ok(self.table.clone())
    }
    async fn set_state(&self, state: WorkspaceDomainState) -> Result<(), BoxError> {
        self.global.set(state.clone()).await.map_err(boxed)?;
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = state;
        Ok(())
    }
    pub(crate) fn candidate_matches(&self, id: &SessionId, path: &str) -> bool {
        self.session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .is_some_and(|v| v == path)
            || self
                .missing_session_paths
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(id)
    }
    pub(crate) fn remember_session_path(&self, id: SessionId, path: String) {
        self.session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), path);
        self.missing_session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        self.invalid_session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }
    pub(crate) async fn read_session_header(
        &self,
        id: &SessionId,
    ) -> Result<SessionHeader, BoxError> {
        if let Some(header) = live_header(&self.ctx, id)? {
            self.headers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id.clone(), header.clone());
            return Ok(header);
        }
        if let Some(header) = self
            .headers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
        {
            return Ok(header);
        }
        let headers = self.list_stored_headers().await?;
        self.index_headers(&headers).await;
        self.headers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| {
                format!("cannot validate session '{id}': session persistence holds no such session")
                    .into()
            })
    }
    async fn list_stored_headers(&self) -> Result<Vec<SessionHeader>, BoxError> {
        Ok(self
            .persistence
            .list(None)
            .await?
            .into_iter()
            .map(|snapshot: SessionPersistenceSnapshot| snapshot.header)
            .collect())
    }
    async fn session_known(&self, id: &SessionId) -> Result<bool, BoxError> {
        if self
            .ctx
            .get::<Sessions>()
            .and_then(|sessions| sessions.get(id))
            .is_some()
        {
            return Ok(true);
        }
        if self
            .headers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(id)
        {
            return Ok(true);
        }
        let headers = self.list_stored_headers().await?;
        self.index_headers(&headers).await;
        Ok(self
            .headers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(id))
    }
    async fn replace_header_index(&self, headers: &[SessionHeader]) {
        self.headers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.missing_session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.invalid_session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.index_headers(headers).await;
    }
    async fn index_headers(&self, headers: &[SessionHeader]) {
        for header in headers {
            self.index_header(header).await;
        }
    }
    async fn index_header(&self, header: &SessionHeader) {
        let id = header.id.clone();
        self.headers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), header.clone());
        self.session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        self.missing_session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        let Some(cwd) = &header.cwd else {
            self.invalid_session_paths
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id, "header has no cwd".into());
            return;
        };
        match realpath_normalize(cwd).await {
            Ok(path) => match tokio::fs::metadata(&path).await {
                Ok(metadata) if metadata.is_dir() => self.remember_session_path(id, path),
                Ok(_) => {
                    self.invalid_session_paths
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(id, format!("cwd '{cwd}' is not a directory"));
                }
                Err(error) => self.record_path_error(id, cwd, &error),
            },
            Err(crate::WorkspacePathError::Io(error)) if fully_qualified_workspace_path(cwd) => {
                self.record_path_error(id, cwd, &error);
            }
            Err(_) => {
                self.invalid_session_paths
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(id, format!("cwd '{cwd}' does not resolve"));
            }
        }
    }
    fn record_path_error(&self, id: SessionId, cwd: &str, error: &std::io::Error) {
        if preserves_missing_candidate(error) {
            self.missing_session_paths
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id.clone());
        }
        self.invalid_session_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, format!("cwd '{cwd}' does not resolve"));
    }
    async fn index_live_sessions(&self) -> Result<(), BoxError> {
        let Some(sessions) = self.ctx.get::<Sessions>() else {
            return Ok(());
        };
        let headers = sessions
            .list()
            .into_iter()
            .filter_map(|session| live_header(&self.ctx, session.id()).transpose())
            .collect::<Result<Vec<SessionHeader>, _>>()?;
        self.index_headers(&headers).await;
        Ok(())
    }

    async fn archive_known(
        &self,
        session_id: SessionId,
        options: ArchiveSessionOptions,
    ) -> Result<(), BoxError> {
        if self.state().archived_session_ids.contains(&session_id) {
            return Ok(());
        }
        if !self.session_known(&session_id).await? {
            return Err(Box::new(WorkspaceUnknownSessionError {
                session_id,
                verb: "archive",
            }));
        }
        if !options.stop_activity {
            let activity = self
                .ctx
                .waterfall(
                    WorkspaceSessionActivity {
                        session_id: session_id.clone(),
                    },
                    |_| async { Ok(vec![]) },
                )
                .await?;
            if !activity.is_empty() {
                return Err(Box::new(WorkspaceActiveSessionError::new(
                    session_id, activity,
                )));
            }
        }
        let mut state = self.state();
        state.archived_session_ids.push(session_id.clone());
        state.pinned_session_ids.retain(|id| id != &session_id);
        self.set_state(state).await?;
        if options.stop_activity
            && let Err(error) = self
                .ctx
                .parallel(WorkspaceSessionStop {
                    session_id: session_id.clone(),
                })
                .await
        {
            match error {
                KernelError::Listeners { errors, .. } => {
                    for failure in errors {
                        self.ctx.logger().warn(format!(
                            "workspace: stopping session '{session_id}' for archive failed: Error: {failure}"
                        ));
                    }
                }
                error => self.ctx.logger().warn(format!(
                    "workspace: stopping session '{session_id}' for archive failed: Error: {error}"
                )),
            }
        }
        if let Err(error) = self
            .ctx
            .parallel(WorkspaceSessionArchived { session_id })
            .await
        {
            self.ctx.logger().warn(format!(
                "Session archived, but archive follow-up admission failed: {error}"
            ));
        }
        Ok(())
    }

    async fn unarchive_known(&self, session_id: &SessionId) -> Result<(), BoxError> {
        let mut state = self.state();
        if !state.archived_session_ids.contains(session_id) {
            return Ok(());
        }
        self.ctx
            .parallel(WorkspaceSessionUnarchiving {
                session_id: session_id.clone(),
            })
            .await?;
        state.archived_session_ids.retain(|id| id != session_id);
        self.set_state(state).await
    }

    async fn pin_known(&self, session_id: SessionId) -> Result<(), BoxError> {
        let mut state = self.state();
        if state.pinned_session_ids.contains(&session_id) {
            return Ok(());
        }
        if state.archived_session_ids.contains(&session_id) {
            return Err(Box::new(WorkspaceArchivedSessionPinError { session_id }));
        }
        if !self.session_known(&session_id).await? {
            return Err(Box::new(WorkspaceUnknownSessionError {
                session_id,
                verb: "pin",
            }));
        }
        state.pinned_session_ids.insert(0, session_id);
        self.set_state(state).await
    }

    async fn unpin_known(&self, session_id: &SessionId) -> Result<(), BoxError> {
        let mut state = self.state();
        if !state.pinned_session_ids.contains(session_id) {
            return Ok(());
        }
        state.pinned_session_ids.retain(|id| id != session_id);
        self.set_state(state).await
    }

    async fn create_canonical(
        self: &Arc<Self>,
        canonical: String,
        title: Option<String>,
        first_use: bool,
    ) -> Result<Workspace, BoxError> {
        if let Some(entity) = self
            .entities()
            .values()
            .find(|v| v.path() == canonical)
            .cloned()
        {
            return Ok(entity);
        }
        let id: WorkspaceId = hoh_util_crypto::random_uuid().to_string().into();
        let now = now_iso();
        let record = WorkspaceRecord {
            path: canonical.clone(),
            title: title.unwrap_or_else(|| default_workspace_title(&canonical)),
            session_ids: vec![],
            created_at: now.clone(),
            updated_at: now,
        };
        let entity = Workspace::new(self, id.clone(), record.clone());
        self.entities().insert(id.clone(), entity.clone());
        let state = self.state();
        let mut pending = state.clone();
        pending.pending_mutation = Some(WorkspacePendingMutation {
            operation: WorkspaceMutationOperation::Create,
            workspace_id: id.clone(),
        });
        if let Err(error) = self.set_state(pending).await {
            self.entities().shift_remove(&id);
            return Err(error);
        }
        if let Err(error) = self.table.put(id.clone(), record).await.map_err(boxed) {
            self.entities().shift_remove(&id);
            if let Err(rollback) = self.set_state(state.clone()).await {
                return Err(Box::new(WorkspaceAggregateError {
                    message: format!(
                        "workspace '{id}' record write and pending-marker rollback both failed"
                    ),
                    errors: vec![error, rollback],
                }));
            }
            return Err(error);
        }
        let mut final_state = state.clone();
        final_state.initialized = true;
        final_state.pending_mutation = None;
        final_state.workspace_ids.insert(0, id.clone());
        if first_use {
            final_state.default_workspace_id = Some(id.clone());
        }
        if let Err(error) = self.set_state(final_state).await {
            self.entities().shift_remove(&id);
            if let Err(rollback) = self.table.delete(id.clone()).await.map_err(boxed) {
                return Err(Box::new(WorkspaceAggregateError {
                    message: format!(
                        "workspace '{id}' order write and record rollback both failed; the pending marker remains recoverable"
                    ),
                    errors: vec![error, rollback],
                }));
            }
            if let Err(rollback) = self.set_state(state).await {
                return Err(Box::new(WorkspaceAggregateError {
                    message: format!(
                        "workspace '{id}' order write and pending-marker rollback both failed"
                    ),
                    errors: vec![error, rollback],
                }));
            }
            return Err(error);
        }
        Ok(entity)
    }

    async fn delete_known(&self, id: &WorkspaceId) -> Result<bool, BoxError> {
        let Some(entity) = self.entities().get(id).cloned() else {
            return Ok(false);
        };
        let state = self.state();
        let mut next = state.clone();
        next.initialized = true;
        next.workspace_ids.retain(|v| v != id);
        next.pending_mutation = None;
        let mut pending = next.clone();
        pending.pending_mutation = Some(WorkspacePendingMutation {
            operation: WorkspaceMutationOperation::Delete,
            workspace_id: id.clone(),
        });
        self.set_state(pending).await?;
        self.entities().shift_remove(id);
        if let Err(error) = self.table.delete(id.clone()).await.map_err(boxed) {
            self.entities().insert(id.clone(), entity);
            if let Err(rollback) = self.set_state(state).await {
                self.entities().shift_remove(id);
                return Err(Box::new(WorkspaceAggregateError {
                    message: format!(
                        "workspace '{id}' record deletion and registry-order rollback both failed"
                    ),
                    errors: vec![error, rollback],
                }));
            }
            return Err(error);
        }
        if let Err(error) = self.set_state(next).await {
            self.ctx.logger().warn(format!(
                "workspace '{id}' was deleted but its pending marker could not be cleared: {}",
                render_error(error.as_ref())
            ));
        }
        Ok(true)
    }

    async fn recover_pending_mutation(&self) -> Result<(), BoxError> {
        let state = self.state();
        let Some(pending) = state.pending_mutation.clone() else {
            return Ok(());
        };
        if state.workspace_ids.contains(&pending.workspace_id) {
            return Err(format!("workspace domain is inconsistent: pending {} workspace '{}' is still present in registry order", pending.operation, pending.workspace_id).into());
        }
        self.table
            .delete(pending.workspace_id)
            .await
            .map_err(boxed)?;
        let mut next = state;
        next.pending_mutation = None;
        self.set_state(next).await
    }

    async fn bootstrap(&self, headers: &[SessionHeader]) -> Result<(), BoxError> {
        let state = self.state();
        let mut groups: IndexMap<String, Vec<SessionHeader>> = IndexMap::new();
        for header in headers {
            if let Some(path) = self
                .session_paths
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&header.id)
                .cloned()
            {
                groups.entry(path).or_default().push(header.clone());
            }
        }
        let mut id_collator =
            feruca::Collator::new(feruca::Tailoring::Cldr(feruca::Locale::Root), false, false);
        let mut groups = groups
            .into_iter()
            .map(|(path, mut headers)| {
                headers.sort_by(|a, b| {
                    b.created_at
                        .cmp(&a.created_at)
                        .then_with(|| id_collator.collate(a.id.as_str(), b.id.as_str()))
                });
                let newest = headers[0].created_at;
                (path, headers, newest)
            })
            .collect::<Vec<_>>();
        let mut path_collator =
            feruca::Collator::new(feruca::Tailoring::Cldr(feruca::Locale::Root), false, false);
        groups.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| path_collator.collate(&a.0, &b.0))
        });
        let entries = self.table.entries().map_err(boxed)?.collect::<Vec<_>>();
        let mut by_path: IndexMap<String, WorkspaceId> = entries
            .iter()
            .map(|(id, r)| (r.path.clone(), id.clone()))
            .collect();
        let mut accounted: IndexMap<SessionId, WorkspaceId> = entries
            .iter()
            .flat_map(|(id, r)| r.session_ids.iter().cloned().map(|s| (s, id.clone())))
            .collect();
        for (path, group, newest) in &groups {
            if let Some(id) = by_path.get(path).cloned() {
                let current = self.table.get(&id).map_err(boxed)?.expect("indexed record");
                let historical = group
                    .iter()
                    .map(|h| h.id.clone())
                    .filter(|s| accounted.get(s).is_none_or(|owner| owner == &id))
                    .collect::<Vec<_>>();
                let historical_set = historical.iter().cloned().collect::<HashSet<_>>();
                let mut ids = historical.clone();
                ids.extend(
                    current
                        .session_ids
                        .iter()
                        .filter(|s| !historical_set.contains(*s))
                        .cloned(),
                );
                if ids != current.session_ids {
                    self.table
                        .update(id.clone(), move |r| {
                            let mut r = r.clone();
                            r.session_ids = ids;
                            r.updated_at = now_iso();
                            Ok(r)
                        })
                        .await
                        .map_err(boxed)?;
                }
                for session in historical {
                    accounted.insert(session, id.clone());
                }
            } else {
                let ids = group
                    .iter()
                    .map(|h| h.id.clone())
                    .filter(|s| !accounted.contains_key(s))
                    .collect::<Vec<_>>();
                if ids.is_empty() {
                    continue;
                }
                let id: WorkspaceId = hoh_util_crypto::random_uuid().to_string().into();
                let created = js_iso(*newest)?;
                self.table
                    .put(
                        id.clone(),
                        WorkspaceRecord {
                            path: path.clone(),
                            title: default_workspace_title(path),
                            session_ids: ids.clone(),
                            created_at: created.clone(),
                            updated_at: created,
                        },
                    )
                    .await
                    .map_err(boxed)?;
                by_path.insert(path.clone(), id.clone());
                for session in ids {
                    accounted.insert(session, id.clone());
                }
            }
        }
        let rank: HashMap<String, i64> = groups.iter().map(|(p, _, n)| (p.clone(), *n)).collect();
        let prior: HashMap<WorkspaceId, usize> = state
            .workspace_ids
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, id)| (id, i))
            .collect();
        let mut entries = self.table.entries().map_err(boxed)?.collect::<Vec<_>>();
        let mut workspace_collator =
            feruca::Collator::new(feruca::Tailoring::Cldr(feruca::Locale::Root), false, false);
        entries.sort_by(|(ai, a), (bi, b)| {
            let left = rank.get(&a.path).copied().or_else(|| epoch(&a.created_at));
            let right = rank.get(&b.path).copied().or_else(|| epoch(&b.created_at));
            right
                .zip(left)
                .map_or(std::cmp::Ordering::Equal, |(right, left)| right.cmp(&left))
                .then_with(|| {
                    prior
                        .get(ai)
                        .unwrap_or(&usize::MAX)
                        .cmp(prior.get(bi).unwrap_or(&usize::MAX))
                })
                .then_with(|| workspace_collator.collate(ai.as_str(), bi.as_str()))
        });
        let ids = entries.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        if state.workspace_ids != ids {
            let mut interim = state.clone();
            interim.workspace_ids = ids.clone();
            interim.initialized = false;
            interim.default_workspace_id = None;
            interim.pending_mutation = None;
            self.set_state(interim).await?;
        }
        let mut done = state;
        done.workspace_ids = ids;
        done.initialized = true;
        done.default_workspace_id = None;
        done.pending_mutation = None;
        self.set_state(done).await
    }

    fn validate_stored_state(&self) -> Result<(), BoxError> {
        let state = self.state();
        let mut order = IndexSet::new();
        for id in &state.workspace_ids {
            if !order.insert(id.clone()) {
                return Err(format!(
                    "workspace domain is inconsistent: registry order repeats workspace '{id}'"
                )
                .into());
            }
            if self.table.get(id).map_err(boxed)?.is_none() {
                return Err(format!("workspace domain is inconsistent: registry order references missing workspace '{id}'").into());
            }
        }
        if state.initialized && order.len() as u64 != self.table.size().map_err(boxed)? {
            let orphan = self
                .table
                .keys()
                .map_err(boxed)?
                .find(|id| !order.contains(id))
                .expect("size differs");
            return Err(format!("workspace domain is inconsistent: workspace '{orphan}' is absent from registry order").into());
        }
        let mut paths = HashMap::new();
        let mut accounted = HashMap::new();
        for (id, record) in self.table.entries().map_err(boxed)? {
            if let Some(holder) = paths.insert(record.path.clone(), id.clone()) {
                return Err(format!("workspace domain is inconsistent: path '{}' is claimed by both workspace '{holder}' and workspace '{id}'",record.path).into());
            }
            for session in &record.session_ids {
                if let Some(holder) = accounted.insert(session.clone(), id.clone()) {
                    return Err(format!("workspace domain is inconsistent: session '{session}' is accounted by both workspace '{holder}' and workspace '{id}'").into());
                }
            }
        }
        Ok(())
    }
    fn rebuild_entities(self: &Arc<Self>) -> Result<(), BoxError> {
        self.entities().clear();
        for id in self.state().workspace_ids {
            let record = (*self.table.get(&id).map_err(boxed)?.ok_or_else(|| {
                format!("workspace registry order references missing workspace '{id}'")
            })?)
            .clone();
            self.entities()
                .insert(id.clone(), Workspace::new(self, id, record));
        }
        Ok(())
    }
    fn report_filtered_candidates(&self) -> Result<(), BoxError> {
        for entity in self.entities().values() {
            let record = self
                .table
                .get(entity.id())
                .map_err(boxed)?
                .expect("entity record");
            for session in &record.session_ids {
                if self.candidate_matches(session, &record.path) {
                    continue;
                }
                let path = self
                    .session_paths
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(session)
                    .cloned();
                let reason = self
                    .invalid_session_paths
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(session)
                    .cloned()
                    .unwrap_or_else(|| {
                        if self
                            .headers
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .contains_key(session)
                        {
                            format!(
                                "canonical cwd '{}' differs from workspace path '{}'",
                                path.unwrap_or_default(),
                                record.path
                            )
                        } else {
                            "session header is missing".into()
                        }
                    });
                self.ctx.logger().warn(format!(
                    "workspace '{}' filtered session '{session}' from membership: {reason}",
                    entity.id()
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_not_found_preserves_a_missing_path_candidate() {
        assert!(preserves_missing_candidate(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::NotADirectory,
            std::io::ErrorKind::Other,
        ] {
            assert!(!preserves_missing_candidate(&std::io::Error::from(kind)));
        }
    }

    #[test]
    fn ordered_cache_reports_the_exact_missing_workspace() {
        let error =
            ordered_entities(&[WorkspaceId::from("missing")], &IndexMap::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "workspace registry order references missing workspace 'missing'"
        );
    }

    #[test]
    fn error_renderer_matches_javascript_error_strings() {
        let ordinary: BoxError = "boom".into();
        assert_eq!(render_error(ordinary.as_ref()), "Error: boom");
        let named = hoh_error::HarnessError::named("WorkspaceError", "bad state", "BAD_STATE");
        assert_eq!(render_error(&named), "WorkspaceError: bad state");
    }
}
