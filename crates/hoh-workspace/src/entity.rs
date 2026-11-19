use crate::{
    WorkspaceId, WorkspaceRecord, paths::realpath_normalize, registry::RegistryInner,
    serial::OperationQueue,
};
use hoh_error::{BoxError, SharedError};
use hoh_session::SessionId;
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceStatus {
    Ok,
    MissingDir,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{0}")]
pub struct WorkspaceMoveInvalidError(pub String);

#[derive(Debug)]
pub struct WorkspaceAttachValidationError {
    message: String,
    cause: BoxError,
}
impl std::fmt::Display for WorkspaceAttachValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for WorkspaceAttachValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[derive(Clone)]
pub struct Workspace {
    pub(crate) inner: Arc<WorkspaceInner>,
}
impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("id", self.id())
            .field("record", &self.record())
            .finish()
    }
}
pub(crate) struct WorkspaceInner {
    pub host: Weak<RegistryInner>,
    pub id: WorkspaceId,
    pub record: Mutex<WorkspaceRecord>,
    publication: OperationQueue,
}

impl Workspace {
    pub(crate) fn new(host: &Arc<RegistryInner>, id: WorkspaceId, record: WorkspaceRecord) -> Self {
        Self {
            inner: Arc::new(WorkspaceInner {
                host: Arc::downgrade(host),
                id,
                record: Mutex::new(record),
                publication: OperationQueue::default(),
            }),
        }
    }
    fn host(&self) -> Result<Arc<RegistryInner>, BoxError> {
        self.inner
            .host
            .upgrade()
            .ok_or_else(|| "workspace registry is disposed".into())
    }
    fn record(&self) -> WorkspaceRecord {
        self.inner
            .record
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub fn id(&self) -> &WorkspaceId {
        &self.inner.id
    }
    pub fn path(&self) -> String {
        self.record().path
    }
    pub fn title(&self) -> String {
        self.record().title
    }
    pub fn created_at(&self) -> String {
        self.record().created_at
    }
    pub fn updated_at(&self) -> String {
        self.record().updated_at
    }
    pub fn session_ids(&self) -> Vec<SessionId> {
        let record = self.record();
        self.inner.host.upgrade().map_or_else(Vec::new, |host| {
            record
                .session_ids
                .into_iter()
                .filter(|id| host.candidate_matches(id, &record.path))
                .collect()
        })
    }
    pub async fn set_title(&self, title: impl Into<String>) -> Result<(), BoxError> {
        let title = title.into();
        self.mutate(move |mut record| {
            record.title = title;
            Ok((record, true))
        })
        .await
    }
    pub async fn attach_session(&self, session_id: SessionId) -> Result<(), BoxError> {
        let snapshot = self.record();
        let host = self.host()?;
        if !snapshot.session_ids.contains(&session_id) {
            let header = host.read_session_header(&session_id).await?;
            let cwd = header.cwd.ok_or_else(|| format!("cannot attach session '{session_id}' to workspace '{}': its stored header carries no cwd to validate against", snapshot.path))?;
            let canonical = realpath_normalize(&cwd).await.map_err(|error| -> BoxError {
                Box::new(WorkspaceAttachValidationError {
                    message: format!("cannot attach session '{session_id}' to workspace '{}': its cwd '{cwd}' does not resolve, so it cannot be validated", snapshot.path),
                    cause: Box::new(error),
                })
            })?;
            if !tokio::fs::metadata(&canonical).await?.is_dir() {
                return Err(format!("cannot attach session '{session_id}' to workspace '{}': its cwd '{cwd}' is not a directory", snapshot.path).into());
            }
            if canonical != snapshot.path {
                return Err(format!("cannot attach session '{session_id}' to workspace '{}': its cwd resolves to '{canonical}'", snapshot.path).into());
            }
            host.remember_session_path(session_id.clone(), canonical);
        }
        self.mutate(move |mut record| {
            let changed = !record.session_ids.contains(&session_id);
            if changed {
                record.session_ids.insert(0, session_id);
            }
            Ok((record, changed))
        })
        .await
    }
    pub async fn insert_session_before(
        &self,
        session_id: SessionId,
        before: Option<SessionId>,
    ) -> Result<(), BoxError> {
        self.mutate(move |mut record| {
            if !record.session_ids.contains(&session_id) {
                return Err(Box::new(WorkspaceMoveInvalidError(format!("cannot move session '{session_id}' in workspace '{}': the session is not accounted", record.path))) as BoxError);
            }
            if let Some(anchor) = &before && !record.session_ids.contains(anchor) {
                return Err(Box::new(WorkspaceMoveInvalidError(format!("cannot move session '{session_id}' before '{anchor}' in workspace '{}': the anchor session is not accounted", record.path))) as BoxError);
            }
            if before.as_ref() == Some(&session_id) { return Ok((record, false)) }
            let previous = record.session_ids.clone();
            record.session_ids.retain(|id| id != &session_id);
            let at = before.as_ref().and_then(|id| record.session_ids.iter().position(|v| v == id)).unwrap_or(record.session_ids.len());
            record.session_ids.insert(at, session_id);
            let changed = record.session_ids != previous;
            Ok((record, changed))
        }).await
    }
    pub async fn detach_session(&self, session_id: SessionId) -> Result<(), BoxError> {
        self.mutate(move |mut record| {
            let previous = record.session_ids.len();
            record.session_ids.retain(|id| id != &session_id);
            let changed = record.session_ids.len() != previous;
            Ok((record, changed))
        })
        .await
    }
    pub async fn status(&self) -> WorkspaceStatus {
        if tokio::fs::metadata(self.path())
            .await
            .is_ok_and(|m| m.is_dir())
        {
            WorkspaceStatus::Ok
        } else {
            WorkspaceStatus::MissingDir
        }
    }
    async fn mutate(
        &self,
        transform: impl FnOnce(WorkspaceRecord) -> Result<(WorkspaceRecord, bool), BoxError>
        + Send
        + 'static,
    ) -> Result<(), BoxError> {
        let workspace = self.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        let ticket = workspace.inner.publication.reserve();
        tokio::spawn(async move {
            let result = async {
                let _turn = workspace.inner.publication.enter(ticket).await;
                workspace.mutate_inner(transform).await
            }
            .await;
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| -> BoxError { "workspace mutation task stopped".into() })?
    }

    async fn mutate_inner(
        &self,
        transform: impl FnOnce(WorkspaceRecord) -> Result<(WorkspaceRecord, bool), BoxError>
        + Send
        + 'static,
    ) -> Result<(), BoxError> {
        let host = self.host()?;
        let id = self.inner.id.clone();
        let host_for_update = host.clone();
        let unchanged = SharedError::new(Unchanged);
        let sentinel = unchanged.clone();
        let result = host
            .table()?
            .update(id, move |current| {
                let (mut next, changed) = transform(current.clone())?;
                next.session_ids
                    .retain(|session_id| host_for_update.candidate_matches(session_id, &next.path));
                if !changed && next.session_ids.len() == current.session_ids.len() {
                    return Err(Box::new(sentinel.clone()));
                }
                next.updated_at = crate::registry::now_iso();
                Ok(next)
            })
            .await;
        match result {
            Ok(next) => {
                *self.inner.record.lock().unwrap_or_else(|p| p.into_inner()) = (*next).clone();
                Ok(())
            }
            Err(error) if error.ptr_eq(&unchanged) => Ok(()),
            Err(error) if error.downcast_ref::<WorkspaceMoveInvalidError>().is_some() => {
                Err(Box::new(
                    error
                        .downcast_ref::<WorkspaceMoveInvalidError>()
                        .expect("checked")
                        .clone(),
                ))
            }
            Err(error) => Err(Box::new(error)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("workspace record unchanged (internal sentinel)")]
struct Unchanged;
