//! Durable workspace registry and session grouping.

mod entity;
pub mod paths;
mod registry;
mod serial;
pub mod spec;

pub use entity::{
    Workspace, WorkspaceAttachValidationError, WorkspaceMoveInvalidError, WorkspaceStatus,
};
pub use paths::{
    PathPlatform, WorkspacePathError, default_workspace_title, default_workspace_title_for,
    fully_qualified_workspace_path, fully_qualified_workspace_path_for, realpath_normalize,
};
pub use registry::WorkspaceRegistry;
pub use spec::{
    WorkspaceDomainState, WorkspaceMutationOperation, WorkspacePendingMutation, WorkspaceRecord,
    workspace_domain_spec, workspace_domain_state_schema, workspace_record_schema,
};

use hoh_brand::branded_string;
use hoh_error::BoxError;
use hoh_kernel::{
    Cfg, Context, Event, Inject, NoConfig, Parallel, Plugin, Service, Undo, Waterfall,
};
use hoh_session::{SessionHeader, SessionId, Sessions};
use hoh_session_persistence::SessionPersistenceService;
use hoh_storage_domain::StorageDomain;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

branded_string!(pub WorkspaceId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActivityItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActivity {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<SessionActivityItem>>,
}

#[derive(Clone)]
pub struct WorkspaceSessionActivity {
    pub session_id: SessionId,
}
impl Event for WorkspaceSessionActivity {
    const NAME: &'static str = "workspace/session-activity";
    type Mode = Waterfall<Vec<SessionActivity>>;
}
#[derive(Clone)]
pub struct WorkspaceSessionStop {
    pub session_id: SessionId,
}
impl Event for WorkspaceSessionStop {
    const NAME: &'static str = "workspace/session-stop";
    type Mode = Parallel;
}
#[derive(Clone)]
pub struct WorkspaceSessionArchived {
    pub session_id: SessionId,
}
impl Event for WorkspaceSessionArchived {
    const NAME: &'static str = "workspace/session-archived";
    type Mode = Parallel;
}
#[derive(Clone)]
pub struct WorkspaceSessionUnarchiving {
    pub session_id: SessionId,
}
impl Event for WorkspaceSessionUnarchiving {
    const NAME: &'static str = "workspace/session-unarchiving";
    type Mode = Parallel;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArchiveSessionOptions {
    pub stop_activity: bool,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "cannot {verb} session '{session_id}': live sessions and session persistence hold no such session"
)]
pub struct WorkspaceUnknownSessionError {
    pub session_id: SessionId,
    pub verb: &'static str,
}
#[derive(Debug, thiserror::Error)]
#[error("cannot archive session '{session_id}': the session is active ({kinds})")]
pub struct WorkspaceActiveSessionError {
    pub session_id: SessionId,
    pub activity: Vec<SessionActivity>,
    kinds: String,
}
impl WorkspaceActiveSessionError {
    fn new(session_id: SessionId, activity: Vec<SessionActivity>) -> Self {
        let kinds = activity
            .iter()
            .map(|v| v.kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Self {
            session_id,
            activity,
            kinds,
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("cannot pin session '{session_id}': the session is archived")]
pub struct WorkspaceArchivedSessionPinError {
    pub session_id: SessionId,
}
#[derive(Debug, thiserror::Error)]
#[error("cannot reorder unknown workspace '{workspace_id}'")]
pub struct WorkspaceOrderInvalidError {
    pub workspace_id: WorkspaceId,
}

/// Rust representation of the source `AggregateError` used by two-write rollback failures.
#[derive(Debug)]
pub struct WorkspaceAggregateError {
    pub message: String,
    pub errors: Vec<BoxError>,
}
impl std::fmt::Display for WorkspaceAggregateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for WorkspaceAggregateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.errors.first().map(|error| error.as_ref() as _)
    }
}

hoh_kernel::service!(pub WorkspaceRegistryService: WorkspaceRegistry = "workspaceRegistry");

#[derive(Inject)]
pub struct Deps {
    storage_domain: Service<StorageDomain>,
    persistence: Service<SessionPersistenceService>,
}

pub struct WorkspaceRegistryPlugin;
impl Plugin for WorkspaceRegistryPlugin {
    const NAME: &'static str = "WorkspaceRegistry";
    type Config = NoConfig;
    type Deps = Deps;
    async fn apply(ctx: Context, deps: Deps, _: Cfg<NoConfig>) -> Result<(), BoxError> {
        let registry = WorkspaceRegistry::start(
            ctx.clone(),
            deps.storage_domain.clone(),
            deps.persistence.clone(),
        )
        .await?;
        let domain = registry.domain();
        ctx.effect("workspace.domainClose", move || {
            Undo::future(move || async move {
                domain
                    .close()
                    .await
                    .map_err(|error| Box::new(error) as BoxError)
            })
        })?;
        ctx.provide::<WorkspaceRegistryService>(Arc::new(registry))
            .await?;
        Ok(())
    }
}

fn live_header(ctx: &Context, id: &SessionId) -> Result<Option<SessionHeader>, BoxError> {
    let Some(session) = ctx.get::<Sessions>().and_then(|sessions| sessions.get(id)) else {
        return Ok(None);
    };
    Ok(Some(project_live_header(session.header().as_ref())?))
}

fn project_live_header(value: &serde_json::Value) -> Result<SessionHeader, BoxError> {
    let object = value
        .as_object()
        .ok_or("live session header is not an object")?;
    let id = object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("live session header has no string id")?;
    let number = object
        .get("createdAt")
        .ok_or("live session header has no createdAt")?;
    let created_at = number.as_i64().or_else(|| {
        let value = number.as_f64()?;
        (value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0)
            .then_some(value as i64)
    });
    let mut header = SessionHeader::new(
        id.to_owned().into(),
        created_at.ok_or("live session header createdAt is not a safe integer")?,
    );
    header.cwd = object
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok(header)
}
