//! Shared per-call sandbox policy resolver and current model context.

use hoh_agent::AssembleAgent;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Inject, Plugin, Service, Undo};
use hoh_sandbox::{SandboxExecutionPolicy, SandboxInvalidRootError, SandboxMode};
use hoh_session::{Session, SessionHeader};
use hoh_session_projection::{ProjectionDefinition, ProjectionHandle, SessionProjections};
use hoh_system_prompt::{
    PromptContext, PromptContextOrderName, PromptText, SystemPrompt, SystemPromptKey,
};
use hoh_util_values::js_json_stringify;
use hoh_util_workspace_path::node_path;
use indexmap::IndexSet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard};

/// Every supported sandbox mode, in source advertisement order.
pub const SANDBOX_MODES: [SandboxMode; 3] = [
    SandboxMode::ReadOnly,
    SandboxMode::WorkspaceWrite,
    SandboxMode::DangerFullAccess,
];

fn read_only() -> SandboxMode {
    SandboxMode::ReadOnly
}

/// Deployment defaults owned by the shared policy service.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// File-sandbox mode used without a session override.
    #[serde(default = "read_only")]
    pub mode: SandboxMode,
    /// Absolute fallback for agentless calls and sessions without a cwd.
    pub workspace_root: Option<String>,
    /// Extra absolute directories granted by workspace-write.
    pub additional_writable_roots: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: SandboxMode::ReadOnly,
            workspace_root: None,
            additional_writable_roots: Vec::new(),
        }
    }
}

/// Inputs selecting the complete policy for one capability call.
#[derive(Clone, Copy, Debug, Default)]
pub struct SandboxPolicyRequest<'a> {
    pub session: Option<&'a Session>,
    /// An already-approved one-call override.
    pub mode: Option<SandboxMode>,
}

/// Durable payload of one `sandbox/mode` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxModeEvent {
    pub mode: SandboxMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SandboxModeSource>,
}

impl SandboxModeEvent {
    pub const TYPE: &'static str = "sandbox/mode";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxModeSource {
    Delegation,
}

/// Append exactly one durable sandbox-mode switch.
pub fn set_sandbox_mode(session: &Session, mode: SandboxMode) -> Result<(), BoxError> {
    session.append(
        SandboxModeEvent::TYPE,
        serde_json::to_value(SandboxModeEvent { mode, source: None })?,
        None,
    )?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum PolicyError {
    #[error(
        "sandbox-policy: workspace root must be an absolute execution-world path without NUL bytes"
    )]
    WorkspaceRoot,
    #[error(
        "sandbox-policy: writable directories must be absolute execution-world paths without NUL bytes"
    )]
    WritableRoot,
    #[error("sandbox-policy: sandboxMode projection state must be null or a sandbox mode")]
    ProjectionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SandboxModeProjectionState(Option<SandboxMode>);

impl Serialize for SandboxModeProjectionState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self.0 {
            Some(mode) => mode.serialize(serializer),
            None => serializer.serialize_unit(),
        }
    }
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Node's host-platform `path.isAbsolute` contract, without touching the filesystem.
fn is_execution_world_path(path: &str) -> bool {
    node_path::is_absolute(path) && !path.contains('\0')
}

fn resolve_additional_writable_roots(
    roots: impl IntoIterator<Item = String>,
) -> Result<Option<Arc<[String]>>, BoxError> {
    let mut distinct = IndexSet::new();
    for root in roots {
        if !is_execution_world_path(&root) {
            return Err(PolicyError::WritableRoot.into());
        }
        distinct.insert(root);
    }
    if distinct.is_empty() {
        Ok(None)
    } else {
        Ok(Some(distinct.into_iter().collect::<Vec<_>>().into()))
    }
}

fn sandbox_mode_state(raw: Option<&Value>) -> Result<Arc<SandboxModeProjectionState>, BoxError> {
    match raw {
        Some(Value::Null) => Ok(Arc::new(SandboxModeProjectionState(None))),
        Some(value @ Value::String(_)) => Ok(Arc::new(SandboxModeProjectionState(Some(
            serde_json::from_value(value.clone())?,
        )))),
        _ => Err(PolicyError::ProjectionState.into()),
    }
}

fn sandbox_mode_definition() -> ProjectionDefinition<SandboxModeProjectionState> {
    ProjectionDefinition {
        key: "sandboxMode".into(),
        state_version: 1,
        state_schema: Arc::new(sandbox_mode_state),
        init: Arc::new(|_, _| Ok(Arc::new(SandboxModeProjectionState(None)))),
        apply: Arc::new(|state, event| {
            if event.event_type() != SandboxModeEvent::TYPE {
                return Ok(state.clone());
            }
            let mode = serde_json::from_value::<SandboxMode>(
                event.data().get("mode").cloned().unwrap_or(Value::Null),
            )?;
            if state.0 == Some(mode) {
                Ok(state.clone())
            } else {
                Ok(Arc::new(SandboxModeProjectionState(Some(mode))))
            }
        }),
        wire: None,
    }
}

#[derive(Clone)]
struct WritableRegistration {
    token: Arc<()>,
    directory: String,
}

struct SessionRegistrations {
    session: Session,
    roots: Vec<WritableRegistration>,
}

#[derive(Default)]
struct WritableRoots(Mutex<Vec<SessionRegistrations>>);

impl WritableRoots {
    fn insert(&self, session: Session, registration: WritableRegistration) {
        let mut entries = lock(&self.0);
        if let Some(entry) = entries.iter_mut().find(|entry| entry.session == session) {
            entry.roots.push(registration);
        } else {
            entries.push(SessionRegistrations {
                session,
                roots: vec![registration],
            });
        }
    }

    fn remove(&self, session: &Session, token: &Arc<()>) {
        let mut entries = lock(&self.0);
        if let Some(index) = entries.iter().position(|entry| &entry.session == session) {
            entries[index]
                .roots
                .retain(|entry| !Arc::ptr_eq(&entry.token, token));
            if entries[index].roots.is_empty() {
                entries.remove(index);
            }
        }
    }

    fn roots_for(&self, session: &Session) -> Vec<String> {
        lock(&self.0)
            .iter()
            .find(|entry| &entry.session == session)
            .map(|entry| {
                entry
                    .roots
                    .iter()
                    .map(|entry| entry.directory.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The canonical `sandboxPolicy` service.
#[derive(Clone)]
pub struct SandboxPolicyService {
    pub default_mode: SandboxMode,
    pub workspace_root: String,
    pub additional_writable_roots: Option<Arc<[String]>>,
    writable_root_registrations: Arc<WritableRoots>,
    sandbox_mode_projection: ProjectionHandle<SandboxModeProjectionState>,
}

hoh_kernel::service!(pub SandboxPolicy: SandboxPolicyService = "sandboxPolicy");

impl SandboxPolicyService {
    fn from_resolved_config(
        default_mode: SandboxMode,
        workspace_root: String,
        additional_writable_roots: Option<Arc<[String]>>,
        sandbox_mode_projection: ProjectionHandle<SandboxModeProjectionState>,
    ) -> Self {
        Self {
            default_mode,
            workspace_root,
            additional_writable_roots,
            writable_root_registrations: Arc::new(WritableRoots::default()),
            sandbox_mode_projection,
        }
    }

    /// Resolve one complete snapshot. Explicit mode outranks session override, then default.
    pub fn resolve(
        &self,
        request: SandboxPolicyRequest<'_>,
    ) -> Result<SandboxExecutionPolicy, BoxError> {
        let session_roots = request
            .session
            .map(|session| self.writable_root_registrations.roots_for(session))
            .unwrap_or_default();
        let additional_writable_roots = resolve_additional_writable_roots(
            self.additional_writable_roots
                .iter()
                .flat_map(|roots| roots.iter().cloned())
                .chain(session_roots),
        )?
        .map(|roots| roots.to_vec());
        let mode = request.mode.or(match request.session {
            Some(session) => self.override_of(session)?,
            None => None,
        });
        Ok(SandboxExecutionPolicy {
            mode: mode.unwrap_or(self.default_mode),
            workspace_root: self.session_workspace_root(request.session)?,
            additional_writable_roots,
            session_id: request.session.map(|session| session.id().clone()),
        })
    }

    fn session_workspace_root(&self, session: Option<&Session>) -> Result<String, BoxError> {
        let Some(session) = session else {
            return Ok(self.workspace_root.clone());
        };
        let header: SessionHeader = serde_json::from_value((*session.header()).clone())?;
        let Some(cwd) = header.cwd else {
            return Ok(self.workspace_root.clone());
        };
        if !is_execution_world_path(&cwd) {
            return Err(SandboxInvalidRootError::new(
                &cwd,
                "the Session working directory must be an absolute execution-world path without NUL bytes",
                Some("sandbox workspace root"),
            )
            .into());
        }
        Ok(cwd)
    }

    /// Add one caller-owned, process-local writable-root registration.
    pub fn register_writable_root(
        &self,
        ctx: &Context,
        session: &Session,
        directory: impl Into<String>,
    ) -> Result<Disposer, BoxError> {
        let directory = directory.into();
        resolve_additional_writable_roots([directory.clone()])?;
        let registrations = self.writable_root_registrations.clone();
        let registered_session = session.clone();
        let token = Arc::new(());
        let remove_token = token.clone();
        Ok(ctx.effect("sandboxPolicy.registerWritableRoot()", move || {
            registrations.insert(
                registered_session.clone(),
                WritableRegistration { token, directory },
            );
            Undo::sync(move || registrations.remove(&registered_session, &remove_token))
        })?)
    }

    /// Read only the last durable session override.
    pub fn override_of(&self, session: &Session) -> Result<Option<SandboxMode>, BoxError> {
        Ok(self.sandbox_mode_projection.state_of(session)?.0)
    }
}

fn render_policy_context(policy: &SandboxExecutionPolicy) -> String {
    match policy.mode {
        SandboxMode::ReadOnly => "Current DSH file policy: read-only. Any available operation enforced by the DSH file sandbox cannot modify files in the standing mode. Do not refuse a required modification from this policy alone: try an available tool normally and follow any denial and escalation guidance it returns.".into(),
        SandboxMode::WorkspaceWrite => {
            let extra = policy.additional_writable_roots.as_ref().map_or_else(
                String::new,
                |roots| format!(
                    " Additional writable directories: {}. On Windows, these additional grants apply to file tools only, not sandboxed processes.",
                    js_json_stringify(&Value::Array(
                        roots.iter().cloned().map(Value::String).collect()
                    ))
                ),
            );
            format!(
                "Current DSH file policy: workspace-write. Any available operation enforced by the DSH file sandbox may modify files under the session workspace: {}.{extra} Some platform temporary areas may also be writable.",
                js_json_stringify(&Value::String(policy.workspace_root.clone()))
            )
        }
        SandboxMode::DangerFullAccess => "Current DSH file policy: danger-full-access. The DSH file sandbox does not restrict file modifications by available operations.".into(),
    }
}

#[derive(Inject)]
pub struct SandboxPolicyDeps {
    projections: Service<SessionProjections>,
}

#[derive(Inject)]
struct PromptDeps {
    prompt: Service<SystemPromptKey>,
}

impl Plugin for SandboxPolicyService {
    const NAME: &'static str = "SandboxPolicyService";
    type Config = Config;
    type Deps = SandboxPolicyDeps;

    async fn apply(
        ctx: Context,
        deps: SandboxPolicyDeps,
        config: Cfg<Config>,
    ) -> Result<(), BoxError> {
        // Constructor validation precedes projection registration, as in the source plugin.
        let config = (*config.get()).clone();
        let workspace_root = match config.workspace_root {
            Some(root) => root,
            None => std::env::current_dir()?.to_string_lossy().into_owned(),
        };
        if !is_execution_world_path(&workspace_root) {
            return Err(PolicyError::WorkspaceRoot.into());
        }
        let additional_writable_roots =
            resolve_additional_writable_roots(config.additional_writable_roots)?;

        let projection = deps
            .projections
            .register(&ctx, sandbox_mode_definition())
            .await?;
        if ctx.ensure_active().is_err() {
            return Ok(());
        }
        let service = Self::from_resolved_config(
            config.mode,
            workspace_root,
            additional_writable_roots,
            projection,
        );
        let for_prompt = service.clone();
        let prompt = ctx.inject::<PromptDeps, _>(move |scope, deps| {
            let service = for_prompt.clone();
            async move {
                deps.prompt.context(
                    &scope,
                    PromptContext {
                        name: "sandbox:policy".into(),
                        order: SystemPrompt::get_context_order(
                            PromptContextOrderName::SandboxPolicy,
                        ),
                        text: PromptText::provider(move |context| {
                            let Some(agent) = context.extension::<AssembleAgent>() else {
                                return Ok(String::new());
                            };
                            Ok(render_policy_context(&service.resolve(
                                SandboxPolicyRequest {
                                    session: Some(agent.0.session()),
                                    mode: None,
                                },
                            )?))
                        }),
                        ..Default::default()
                    },
                )?;
                Ok(())
            }
        })?;
        if ctx.has("systemPrompt") {
            prompt.settled().await?;
        }
        ctx.provide::<SandboxPolicy>(Arc::new(service)).await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/policy.rs"]
mod policy_tests;
