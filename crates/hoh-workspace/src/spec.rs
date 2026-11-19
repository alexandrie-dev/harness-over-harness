use crate::WorkspaceId;
use hoh_error::BoxError;
use hoh_session::SessionId;
use hoh_storage_domain::{
    DomainGlobalSpec, DomainSpec, DomainSpecInput, ValueSchema, define_domain, domain_table,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecord {
    pub path: String,
    pub title: String,
    pub session_ids: Vec<SessionId>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMutationOperation {
    Create,
    Delete,
}
impl std::fmt::Display for WorkspaceMutationOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Create => "create",
            Self::Delete => "delete",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePendingMutation {
    pub operation: WorkspaceMutationOperation,
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDomainState {
    pub initialized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_id: Option<WorkspaceId>,
    pub workspace_ids: Vec<WorkspaceId>,
    #[serde(default)]
    pub archived_session_ids: Vec<SessionId>,
    #[serde(default)]
    pub pinned_session_ids: Vec<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_mutation: Option<WorkspacePendingMutation>,
}
fn object(value: &Value) -> Result<&Map<String, Value>, BoxError> {
    value
        .as_object()
        .ok_or_else(|| "Invalid input: expected object".into())
}
fn string(object: &Map<String, Value>, key: &str) -> Result<String, BoxError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("Invalid input: expected string at {key}").into())
}
fn strings<T: From<String>>(object: &Map<String, Value>, key: &str) -> Result<Vec<T>, BoxError> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| -> BoxError { format!("Invalid input: expected array at {key}").into() })?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(|s| T::from(s.to_owned()))
                .ok_or_else(|| format!("Invalid input: expected string in {key}").into())
        })
        .collect()
}

pub fn workspace_record_schema() -> ValueSchema<WorkspaceRecord> {
    ValueSchema::new(|value| {
        let o = object(value)?;
        Ok(WorkspaceRecord {
            path: string(o, "path")?,
            title: string(o, "title")?,
            session_ids: strings(o, "sessionIds")?,
            created_at: string(o, "createdAt")?,
            updated_at: string(o, "updatedAt")?,
        })
    })
}

pub fn workspace_domain_state_schema() -> ValueSchema<WorkspaceDomainState> {
    ValueSchema::new(|value| {
        let o = object(value)?;
        let initialized =
            o.get("initialized")
                .and_then(Value::as_bool)
                .ok_or_else(|| -> BoxError {
                    "Invalid input: expected boolean at initialized".into()
                })?;
        let default_workspace_id = o
            .get("defaultWorkspaceId")
            .map(|value| {
                value
                    .as_str()
                    .map(|s| WorkspaceId::from(s.to_owned()))
                    .ok_or_else(|| -> BoxError {
                        "Invalid input: expected string at defaultWorkspaceId".into()
                    })
            })
            .transpose()?;
        let pending_mutation = o
            .get("pendingMutation")
            .map(|value| {
                let p = object(value)?;
                let operation = match string(p, "operation")?.as_str() {
                    "create" => WorkspaceMutationOperation::Create,
                    "delete" => WorkspaceMutationOperation::Delete,
                    _ => return Err("Invalid input: invalid pendingMutation operation".into()),
                };
                Ok::<_, BoxError>(WorkspacePendingMutation {
                    operation,
                    workspace_id: string(p, "workspaceId")?.into(),
                })
            })
            .transpose()?;
        Ok(WorkspaceDomainState {
            initialized,
            default_workspace_id,
            workspace_ids: strings(o, "workspaceIds")?,
            archived_session_ids: if o.contains_key("archivedSessionIds") {
                strings(o, "archivedSessionIds")?
            } else {
                vec![]
            },
            pinned_session_ids: if o.contains_key("pinnedSessionIds") {
                strings(o, "pinnedSessionIds")?
            } else {
                vec![]
            },
            pending_mutation,
        })
    })
}

pub fn workspace_domain_spec() -> Arc<DomainSpec> {
    let mut input = DomainSpecInput::new(
        "workspace",
        2,
        IndexMap::from([(
            "workspaces".into(),
            domain_table::<WorkspaceId, WorkspaceRecord>(workspace_record_schema()).into(),
        )]),
    );
    input.global = Some(
        DomainGlobalSpec {
            schema: workspace_domain_state_schema(),
            initial: WorkspaceDomainState::default(),
        }
        .into(),
    );
    Arc::new(define_domain(input).expect("the static workspace domain spec is valid"))
}
