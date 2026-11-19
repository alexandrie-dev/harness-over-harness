//! Workspace instruction discovery, rendering, and per-session reconciliation.

pub mod config;
pub mod digest;
pub mod files;
pub mod render;
pub mod state;

pub use config::Config;
pub use files::{
    InstructionFile, LoadedInstructionFile, discover_baseline_instruction_files,
    load_baseline_instructions,
};
pub use render::{RenderedAgentInstructions, TruncatedInstruction, render_agent_instructions};
pub use state::NAME;

use crate::{
    config::{ResolvedConfig, resolve_config, workspace_baseline_identity},
    digest::trimmed_instruction_digest,
    files::{DiscoverOptions, LoadOptions, find_project_root, load_baseline_instruction_set},
    render::{
        AgentInstructionChange, InstructionAction, instruction_candidate_group,
        instruction_scope_key, scope_for_display_path,
    },
    state::{
        InstructionVersionCache, ReconcileOptions, apply_instruction_version_updates,
        baseline_context_message, baseline_instruction_state, reconcile_instruction_context,
        workspace_instruction_changes,
    },
};
use hoh_agent::{Agent, AgentPreStep, Agents, InboxTarget, PreStepDecision, spawn_inherited};
use hoh_error::BoxError;
use hoh_fs::Fs;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{ContentBlock, MessageSource, UserMessage};
use hoh_session::{Session, SessionDisposed, SessionEventEmitted, SessionId};
use hoh_timeout::AbortSignal;
use hoh_tools::{ToolExecutionToken, ToolsResult};
use hoh_working_directory::WorkingDirectoryKey;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    future::Future,
    sync::{Arc, Mutex},
};

#[derive(Inject)]
pub struct Deps {
    agents: Service<Agents>,
    working_directory: Service<WorkingDirectoryKey>,
}

pub struct AgentInstructions;

fn baseline_source(message: &UserMessage) -> Option<&MessageSource> {
    (message.source.kind == NAME
        && message
            .source
            .data
            .get("baseline")
            .and_then(serde_json::Value::as_bool)
            == Some(true))
    .then_some(&message.source)
}

fn visible_baseline_source(
    agent: &dyn Agent,
    authority: &[UserMessage],
) -> Result<Option<MessageSource>, BoxError> {
    if let Some(source) = authority.iter().rev().find_map(baseline_source) {
        return Ok(Some(source.clone()));
    }
    for seq in agent.session().surface()?.nodes()?.into_iter().rev() {
        let Some(event) = agent.session().event_at(seq) else {
            continue;
        };
        if event.event_type() != "user/message" {
            continue;
        }
        let Ok(message) = serde_json::from_value::<UserMessage>(event.data().clone()) else {
            continue;
        };
        if let Some(source) = baseline_source(&message) {
            return Ok(Some(source.clone()));
        }
    }
    Ok(None)
}

fn is_workspace_context(message: &UserMessage) -> bool {
    message.source.kind == NAME
}

fn same_context_payload(left: &UserMessage, right: &UserMessage) -> bool {
    left.content == right.content && left.source == right.source
}

fn touched_file_path(event: &ToolsResult) -> Option<String> {
    if event.result.is_error()
        || !matches!(event.exec.name.as_str(), "read" | "write" | "edit")
        || event.exec.signal().aborted()
    {
        return None;
    }
    event
        .exec
        .arguments
        .as_object()?
        .get("file_path")?
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}

#[derive(Clone)]
struct ProjectionTouch {
    agent: Arc<dyn Agent>,
    path: String,
}

#[derive(Default)]
struct ProjectionRuntime {
    execution_touches: Mutex<HashMap<ToolExecutionToken, Vec<ProjectionTouch>>>,
    step_touches: Mutex<HashMap<SessionId, Vec<ProjectionTouch>>>,
    queues: Mutex<HashMap<SessionId, Arc<ProjectionQueue>>>,
}

#[derive(Default)]
struct ProjectionQueueState {
    issued: u64,
    serving: u64,
    completed: BTreeSet<u64>,
}

struct ProjectionQueue {
    state: Mutex<ProjectionQueueState>,
    progress: tokio::sync::watch::Sender<u64>,
}

impl Default for ProjectionQueue {
    fn default() -> Self {
        let (progress, _) = tokio::sync::watch::channel(0);
        Self {
            state: Mutex::new(ProjectionQueueState::default()),
            progress,
        }
    }
}

impl ProjectionQueue {
    fn issue(self: &Arc<Self>) -> ProjectionTicket {
        let value = {
            let mut state = lock(&self.state);
            let value = state.issued;
            state.issued += 1;
            value
        };
        ProjectionTicket {
            queue: self.clone(),
            value,
        }
    }

    async fn wait_turn(&self, value: u64) {
        let mut progress = self.progress.subscribe();
        loop {
            if lock(&self.state).serving == value {
                return;
            }
            if progress.changed().await.is_err() {
                return;
            }
        }
    }

    async fn wait_idle(&self) {
        let mut progress = self.progress.subscribe();
        loop {
            let idle = {
                let state = lock(&self.state);
                state.serving == state.issued
            };
            if idle {
                return;
            }
            if progress.changed().await.is_err() {
                return;
            }
        }
    }

    fn complete(&self, value: u64) {
        let serving = {
            let mut state = lock(&self.state);
            state.completed.insert(value);
            loop {
                let serving = state.serving;
                if !state.completed.remove(&serving) {
                    break;
                }
                state.serving += 1;
            }
            state.serving
        };
        self.progress.send_replace(serving);
    }
}

struct ProjectionTicket {
    queue: Arc<ProjectionQueue>,
    value: u64,
}

impl ProjectionTicket {
    async fn wait_turn(&self) {
        self.queue.wait_turn(self.value).await;
    }
}

impl Drop for ProjectionTicket {
    fn drop(&mut self) {
        self.queue.complete(self.value);
    }
}

#[derive(Clone)]
struct ProjectionEnvironment {
    ctx: Context,
    resolved: Arc<ResolvedConfig>,
    versions: Arc<InstructionVersionCache>,
    preparations: Arc<BaselinePreparations>,
    working_directory: Service<WorkingDirectoryKey>,
    agents: Service<Agents>,
    runtime: Arc<ProjectionRuntime>,
}

fn projection_queue(runtime: &ProjectionRuntime, session: &Session) -> Arc<ProjectionQueue> {
    lock(&runtime.queues)
        .entry(session.id().clone())
        .or_default()
        .clone()
}

fn spawn_projection_task<F, Fut>(ctx: &Context, task: F) -> Result<(), BoxError>
where
    F: FnOnce(AbortSignal) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), BoxError>> + Send + 'static,
{
    let logger = ctx.logger();
    let (completed, completion) = tokio::sync::oneshot::channel();
    let disposer = ctx.spawn(
        "agent-instructions file projection",
        move |cancel| async move {
            let signal = AbortSignal::from_token(cancel);
            let outcome = task(signal.clone()).await;
            if let Err(error) = outcome
                && !signal.aborted()
            {
                logger.warn(format_args!(
                    "workspace instruction refresh failed: {error}"
                ));
            }
            let _ = completed.send(());
            Ok(())
        },
    )?;
    // A Context task is an owned effect. Retire it as soon as the finite projection
    // finishes so completed file touches do not accumulate until plugin unload.
    drop(spawn_inherited(async move {
        let _ = completion.await;
        disposer.dispose().await;
    }));
    Ok(())
}

fn boundary_has_open_step(boundary: &hoh_agent::TurnBoundaryProjection) -> bool {
    boundary.open_turn_start_seq.is_some()
        && boundary.last_step_boundary.as_ref().is_some_and(|step| {
            step.kind == hoh_agent::StepBoundaryKind::Start
                && boundary
                    .open_turn_start_seq
                    .is_some_and(|turn_start| step.seq > turn_start)
        })
}

fn step_is_open(environment: &ProjectionEnvironment, session: &Session) -> Result<bool, BoxError> {
    let boundary = environment.agents.turn_boundary_of(session)?;
    Ok(boundary_has_open_step(&boundary))
}

fn propagate_projection_touches<K>(
    executions: &Mutex<HashMap<K, Vec<ProjectionTouch>>>,
    token: K,
    parent: Option<K>,
    touch: Option<ProjectionTouch>,
) -> Vec<ProjectionTouch>
where
    K: Copy + Eq + std::hash::Hash,
{
    let mut touches = lock(executions).remove(&token).unwrap_or_default();
    if let Some(touch) = touch {
        touches.push(touch);
    }
    if let Some(parent) = parent {
        if !touches.is_empty() {
            lock(executions).entry(parent).or_default().extend(touches);
        }
        return vec![];
    }
    touches
}

fn queue_projection(
    environment: &ProjectionEnvironment,
    touch: ProjectionTouch,
) -> Result<(), BoxError> {
    let file_system = environment.ctx.get::<Fs>();
    let queue = projection_queue(&environment.runtime, touch.agent.session());
    // Register before spawning. A following pre-step therefore observes the
    // pending projection even when the executor has not polled its task yet.
    let ticket = queue.issue();
    let environment = environment.clone();
    let task_context = environment.ctx.clone();
    spawn_projection_task(&task_context, move |signal| async move {
        ticket.wait_turn().await;
        if signal.aborted() {
            return Ok(());
        }
        let Some(file_system) = file_system else {
            return Ok(());
        };
        let pending: Vec<_> = touch
            .agent
            .inbox()
            .next_step()
            .into_iter()
            .filter(is_workspace_context)
            .collect();
        let desired = compose_context(
            touch.agent.clone(),
            environment.resolved,
            environment.versions,
            environment.preparations,
            file_system,
            environment.working_directory,
            ComposeRequest {
                claimed: vec![],
                pending,
                touched_paths: vec![touch.path],
                signal: signal.clone(),
            },
        )
        .await?;
        signal.throw_if_aborted()?;
        sync_inbox(touch.agent.as_ref(), &[], desired)?;
        Ok(())
    })
}

fn project_touch(
    environment: &ProjectionEnvironment,
    touch: ProjectionTouch,
) -> Result<(), BoxError> {
    let session = touch.agent.session();
    if !step_is_open(environment, session)? {
        return queue_projection(environment, touch);
    }
    lock(&environment.runtime.step_touches)
        .entry(session.id().clone())
        .or_default()
        .push(touch);
    Ok(())
}

fn sync_inbox(
    agent: &dyn Agent,
    claimed: &[UserMessage],
    desired: Option<UserMessage>,
) -> Result<(), BoxError> {
    let pending: Vec<_> = agent
        .inbox()
        .next_step()
        .into_iter()
        .filter(is_workspace_context)
        .collect();
    let already_supplied = if let Some(desired) = &desired {
        let claimed_has = claimed
            .iter()
            .any(|message| same_context_payload(message, desired));
        let mut surface_has = false;
        if !claimed_has {
            for seq in agent.session().surface()?.nodes()? {
                let Some(event) = agent.session().event_at(seq) else {
                    continue;
                };
                if event.event_type() != "user/message" {
                    continue;
                }
                let Ok(message) = serde_json::from_value::<UserMessage>(event.data().clone())
                else {
                    continue;
                };
                if same_context_payload(&message, desired) {
                    surface_has = true;
                    break;
                }
            }
        }
        claimed_has || surface_has
    } else {
        false
    };
    if desired.is_none() || already_supplied {
        for message in pending {
            agent.inbox().remove(&message.id)?;
        }
        return Ok(());
    }
    let desired = desired.expect("checked above");
    if let Some(reusable) = pending
        .iter()
        .find(|message| same_context_payload(message, &desired))
    {
        for message in &pending {
            if message.id != reusable.id {
                agent.inbox().remove(&message.id)?;
            }
        }
        return Ok(());
    }
    if let Some(first) = pending.first() {
        agent.inbox().replace(&first.id, desired)?;
    } else {
        agent.inbox().prepend(InboxTarget::NextStep, desired)?;
    }
    for message in pending.iter().skip(1) {
        agent.inbox().remove(&message.id)?;
    }
    Ok(())
}

fn reconcile_pre_step_decision(
    agent: &dyn Agent,
    step: u64,
    claimed: &[UserMessage],
    pending: &[UserMessage],
    decision: PreStepDecision,
    desired: Option<UserMessage>,
) -> Result<PreStepDecision, BoxError> {
    let PreStepDecision::Enter {
        mut messages,
        starts_request_series,
    } = decision
    else {
        sync_inbox(agent, claimed, desired)?;
        return Ok(PreStepDecision::Reject);
    };
    if step == 1 && messages.is_empty() {
        sync_inbox(agent, claimed, desired)?;
        return Ok(PreStepDecision::Enter {
            messages,
            starts_request_series,
        });
    }
    for message in pending {
        agent.inbox().remove(&message.id)?;
    }
    if let Some(desired) = desired
        && !messages
            .iter()
            .any(|message| same_context_payload(message, &desired))
    {
        let insert_at = messages
            .iter()
            .rposition(|message| claimed.iter().any(|item| item.id == message.id))
            .map_or(0, |index| index + 1);
        messages.insert(insert_at, desired);
    }
    Ok(PreStepDecision::Enter {
        messages,
        starts_request_series,
    })
}

struct ComposeRequest {
    claimed: Vec<UserMessage>,
    pending: Vec<UserMessage>,
    touched_paths: Vec<String>,
    signal: hoh_timeout::AbortSignal,
}

#[derive(Clone)]
struct BaselinePreparation {
    identity: String,
    excluded_scopes: HashSet<String>,
}

type BaselinePreparations = Mutex<HashMap<SessionId, BaselinePreparation>>;

fn may_reuse_pending(
    touched_paths: &[String],
    pending: &[UserMessage],
    prepared: Option<&BaselinePreparation>,
    identity: &str,
) -> bool {
    touched_paths.is_empty()
        && !pending.is_empty()
        && prepared.is_some_and(|prepared| prepared.identity == identity)
}

fn refresh_excluded_baseline_scopes(
    excluded: &mut HashSet<String>,
    included: &[LoadedInstructionFile],
    observed: &[LoadedInstructionFile],
    deduped: &[LoadedInstructionFile],
) {
    let (included_changes, _) = baseline_instruction_state(included);
    let (observed_changes, _) = baseline_instruction_state(observed);
    let mut included_digests_by_group: HashMap<String, HashSet<String>> = HashMap::new();
    for file in included {
        included_digests_by_group
            .entry(instruction_candidate_group(&scope_for_display_path(
                &file.display_path,
            )))
            .or_default()
            .insert(trimmed_instruction_digest(&file.content));
    }
    excluded.clear();
    excluded.extend(observed_changes.keys().cloned());
    for scope in included_changes.keys() {
        excluded.remove(scope);
    }
    for file in deduped {
        let group = instruction_candidate_group(&scope_for_display_path(&file.display_path));
        if included_digests_by_group
            .get(&group)
            .is_some_and(|digests| digests.contains(&trimmed_instruction_digest(&file.content)))
        {
            excluded.remove(&instruction_scope_key(&file.display_path));
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn compose_context(
    agent: Arc<dyn Agent>,
    resolved: Arc<ResolvedConfig>,
    versions: Arc<InstructionVersionCache>,
    preparations: Arc<BaselinePreparations>,
    file_system: Service<Fs>,
    working_directory: Service<WorkingDirectoryKey>,
    request: ComposeRequest,
) -> Result<Option<UserMessage>, BoxError> {
    let ComposeRequest {
        claimed,
        pending,
        touched_paths,
        signal,
    } = request;
    signal.throw_if_aborted()?;
    if resolved.max_bytes <= 0.0 || !resolved.max_bytes.is_finite() {
        return Ok(None);
    }
    let cwd = working_directory.ensure(&agent, Some(&signal)).await?;
    let project_root = find_project_root(
        &cwd,
        &resolved.project_root_markers,
        Some(&*file_system),
        Some(&signal),
    )
    .await?;
    let identity = workspace_baseline_identity(&resolved, &cwd, &project_root);
    if may_reuse_pending(
        &touched_paths,
        &pending,
        lock(&preparations).get(agent.session().id()),
        &identity,
    ) {
        return Ok(pending.first().cloned());
    }
    let visible_baseline = visible_baseline_source(agent.as_ref(), &claimed)?;
    let baseline_present = visible_baseline.is_some();
    let keep_baseline = visible_baseline.as_ref().is_some_and(|source| {
        source
            .data
            .get("baselineIdentity")
            .and_then(serde_json::Value::as_str)
            == Some(identity.as_str())
    });

    let mut content: Vec<ContentBlock> = Vec::new();
    let mut changes = Vec::new();
    let mut authority = claimed;
    let mut desired_baseline = false;
    let prepared = lock(&preparations)
        .get(agent.session().id())
        .filter(|prepared| prepared.identity == identity)
        .cloned();
    let mut excluded = prepared
        .as_ref()
        .map(|prepared| prepared.excluded_scopes.clone())
        .unwrap_or_default();
    let mut next_preparation = None;
    if !baseline_present || !keep_baseline || prepared.is_none() {
        let loaded = load_baseline_instruction_set(
            &LoadOptions {
                discovery: DiscoverOptions {
                    cwd: cwd.clone(),
                    dsh_home: Some(resolved.dsh_home.clone()),
                    agents_home: Some(resolved.agents_home.clone()),
                    project_root_markers: Some(resolved.project_root_markers.clone()),
                    instruction_file_candidates: Some(resolved.instruction_file_candidates.clone()),
                    local_instruction_file_candidates: Some(
                        resolved.local_instruction_file_candidates.clone(),
                    ),
                    project_root: Some(project_root.clone()),
                    signal: Some(signal.clone()),
                },
                max_bytes: resolved.max_bytes,
                max_source_bytes: resolved.max_source_bytes,
                replace_previous_baseline: baseline_present && !keep_baseline,
            },
            Some(&*file_system),
        )
        .await?;
        let included = loaded
            .as_ref()
            .map_or(&[][..], |set| set.included.as_slice());
        let observed = loaded
            .as_ref()
            .map_or(&[][..], |set| set.observed.as_slice());
        let (baseline_changes, baseline_versions) = baseline_instruction_state(included);
        refresh_excluded_baseline_scopes(
            &mut excluded,
            included,
            observed,
            loaded
                .as_ref()
                .map_or(&[][..], |set| set.deduped.as_slice()),
        );
        versions.merge_states(agent.session(), baseline_versions);
        next_preparation = Some(BaselinePreparation {
            identity: identity.clone(),
            excluded_scopes: excluded.clone(),
        });
        if let Some(loaded) = loaded
            && !keep_baseline
            && !loaded.rendered.text.is_empty()
        {
            content.push(ContentBlock::text(loaded.rendered.text));
            let replacement_scopes: HashSet<_> = baseline_changes.keys().cloned().collect();
            if baseline_present {
                for old in visible_baseline
                    .as_ref()
                    .map(workspace_instruction_changes)
                    .unwrap_or_default()
                {
                    if old.action != InstructionAction::Remove
                        && !replacement_scopes.contains(&old.scope)
                    {
                        changes.push(AgentInstructionChange {
                            action: InstructionAction::Remove,
                            scope: old.scope,
                            path: old.path,
                            digest: None,
                        });
                    }
                }
            }
            changes.extend(baseline_changes.into_values());
            let baseline_message = baseline_context_message(content.clone(), &changes, &identity);
            authority.push(baseline_message);
            desired_baseline = true;
        }
    }

    let update = reconcile_instruction_context(
        agent.as_ref(),
        &resolved,
        &versions,
        &*file_system,
        ReconcileOptions {
            cwd: &cwd,
            authority_messages: &authority,
            scope_messages: &pending,
            touched_paths: &touched_paths,
            include_baseline_scopes: keep_baseline,
            excluded_baseline_scopes: keep_baseline.then_some(&excluded),
            project_root: Some(&project_root),
            signal: Some(&signal),
        },
    )
    .await?;
    if let Some(update) = update {
        content.extend(update.context.content);
        changes.extend(workspace_instruction_changes(&update.context.source));
        apply_instruction_version_updates(agent.session(), &update.version_updates, &versions);
    }
    if let Some(preparation) = next_preparation {
        lock(&preparations).insert(agent.session().id().clone(), preparation);
    }
    if content.is_empty() {
        return Ok(None);
    }
    Ok(Some(if desired_baseline {
        baseline_context_message(content, &changes, &identity)
    } else {
        state::workspace_context_hook(
            content
                .into_iter()
                .filter_map(|block| block.text_value().map(str::to_owned))
                .collect::<Vec<_>>()
                .join("\n\n"),
            &changes,
        )
    }))
}

impl Plugin for AgentInstructions {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(
        ctx: Context,
        deps: Self::Deps,
        config: Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        let resolved = Arc::new(resolve_config(&config.get())?);
        let versions = Arc::new(InstructionVersionCache::default());
        let preparations = Arc::new(BaselinePreparations::default());
        let runtime = Arc::new(ProjectionRuntime::default());
        let listener_ctx = ctx.clone();
        let pre_resolved = resolved.clone();
        let pre_versions = versions.clone();
        let pre_preparations = preparations.clone();
        let pre_working_directory = deps.working_directory.clone();
        let pre_runtime = runtime.clone();
        ctx.on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let resolved = pre_resolved.clone();
            let versions = pre_versions.clone();
            let preparations = pre_preparations.clone();
            let working_directory = pre_working_directory.clone();
            let file_system = listener_ctx.get::<Fs>();
            let queue = projection_queue(&pre_runtime, event.agent.session());
            async move {
                let decision = next.run().await?;
                queue.wait_idle().await;
                let pending: Vec<_> = event
                    .agent
                    .inbox()
                    .next_step()
                    .into_iter()
                    .filter(is_workspace_context)
                    .collect();
                let desired = match file_system {
                    Some(file_system) => {
                        compose_context(
                            event.agent.clone(),
                            resolved,
                            versions,
                            preparations,
                            file_system,
                            working_directory,
                            ComposeRequest {
                                claimed: event.messages.clone(),
                                pending: pending.clone(),
                                touched_paths: vec![],
                                signal: event.signal.clone(),
                            },
                        )
                        .await?
                    }
                    None => None,
                };
                event.signal.throw_if_aborted()?;
                reconcile_pre_step_decision(
                    event.agent.as_ref(),
                    event.step,
                    &event.messages,
                    &pending,
                    decision,
                    desired,
                )
            }
        })?;

        let environment = ProjectionEnvironment {
            ctx: ctx.clone(),
            resolved,
            versions,
            preparations,
            working_directory: deps.working_directory,
            agents: deps.agents,
            runtime,
        };
        let event_environment = environment.clone();
        ctx.on_contained::<SessionEventEmitted>(move |emitted| {
            let session_id = emitted.session.id().clone();
            if emitted.event.event_type() == "step/end" {
                let touches = lock(&event_environment.runtime.step_touches)
                    .remove(&session_id)
                    .unwrap_or_default();
                for touch in touches {
                    queue_projection(&event_environment, touch)?;
                }
            }
            Ok(())
        })?;

        let dispose_environment = environment.clone();
        ctx.on_contained::<SessionDisposed>(move |event| {
            let session_id = event.session.id();
            dispose_environment.versions.remove_session(&event.session);
            lock(&dispose_environment.preparations).remove(session_id);
            lock(&dispose_environment.runtime.step_touches).remove(session_id);
            lock(&dispose_environment.runtime.queues).remove(session_id);
            Ok(())
        })?;

        ctx.on_contained::<ToolsResult>(move |event| {
            let touch = touched_file_path(event)
                .zip(event.exec.agent.clone())
                .map(|(path, agent)| ProjectionTouch { agent, path });
            let touches = propagate_projection_touches(
                &environment.runtime.execution_touches,
                event.exec.token,
                event.exec.parent,
                touch,
            );
            for touch in touches {
                project_touch(&environment, touch)?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use futures_util::future::BoxFuture;
    use hoh_agent::{
        AgentOptions, AgentRegistryPlugin, AgentStatus, CancelOptions, Inbox, MaintenanceFuture,
        MaintenanceTask, Model, ModelSelection,
    };
    use hoh_agent_loop_testkit::provide_working_directory_fixture;
    use hoh_fs::{
        FileSystem, FsByteRange, FsDeleteOutcome, FsDirEntry, FsEditOutcome, FsEditRequest,
        FsFuture, FsInfo, FsMoveOptions, FsMoveOutcome, FsPathInfo, FsResolveOptions, FsTarget,
        FsTextStream, FsVersionGuard, FsWriteIntent, FsWriteOutcome,
    };
    use hoh_fs_local::{Config as LocalFsConfig, LocalFileSystem};
    use hoh_llm::MessageId;
    use hoh_scope::ScopeKey;
    use hoh_session::{
        AgentCancelCause, DispatchValidation, SessionEventEmitted, SessionSeq, SurfaceIntent,
    };
    use hoh_timeout::AbortController;
    use hoh_tool_testkit::{
        ContentToolFixtureOptions, define_content_tool_fixture, execute_tool_fixture,
    };
    use hoh_tools::{ToolExecutionInput, ToolRegistrationOptions, ToolRuntime, Tools};
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::oneshot;

    struct StreamGate {
        entered: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    }

    struct GatedFs {
        inner: LocalFileSystem,
        gate: Mutex<Option<StreamGate>>,
        stream_calls: AtomicU64,
    }

    macro_rules! delegate_fs {
        ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
            $(fn $name<'a>(&'a self, $($arg: $ty),*) -> FsFuture<'a, $ret> {
                self.inner.$name($($arg),*)
            })*
        };
    }

    impl FileSystem for GatedFs {
        fn resolve<'a>(
            &'a self,
            path: &'a str,
            options: FsResolveOptions,
        ) -> FsFuture<'a, FsTarget> {
            self.inner.resolve(path, options)
        }

        fn process_path(&self, target: &FsTarget) -> String {
            self.inner.process_path(target)
        }

        fn process_path_from_host_path(&self, path: &str) -> Option<String> {
            self.inner.process_path_from_host_path(path)
        }

        fn file_url(&self, target: &FsTarget) -> String {
            self.inner.file_url(target)
        }

        fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
            self.inner.contains(parent, child)
        }

        fn stat<'a>(
            &'a self,
            target: &'a FsTarget,
            signal: Option<AbortSignal>,
        ) -> FsFuture<'a, Option<FsInfo>> {
            self.inner.stat(target, signal)
        }

        fn lstat<'a>(
            &'a self,
            path: &'a str,
            cwd: Option<&'a str>,
            signal: Option<AbortSignal>,
        ) -> FsFuture<'a, Option<FsPathInfo>> {
            self.inner.lstat(path, cwd, signal)
        }

        fn stream_text<'a>(
            &'a self,
            target: &'a FsTarget,
            signal: Option<AbortSignal>,
        ) -> FsFuture<'a, FsTextStream> {
            Box::pin(async move {
                self.stream_calls.fetch_add(1, Ordering::Relaxed);
                let stream = self.inner.stream_text(target, signal).await?;
                let gate = lock(&self.gate).take();
                if let Some(gate) = gate {
                    let _ = gate.entered.send(());
                    let _ = gate.release.await;
                }
                Ok(stream)
            })
        }

        delegate_fs! {
            read_text(target: &'a FsTarget, signal: Option<AbortSignal>) -> String;
            read_bytes(target: &'a FsTarget, signal: Option<AbortSignal>, max_bytes: u64) -> Vec<u8>;
            read_byte_range(target: &'a FsTarget, range: FsByteRange, signal: Option<AbortSignal>) -> Vec<u8>;
            list_dir(target: &'a FsTarget, signal: Option<AbortSignal>) -> Vec<FsDirEntry>;
            write_text(target: &'a FsTarget, content: &'a str, expected: Option<FsWriteIntent>, signal: Option<AbortSignal>, policy: Option<hoh_fs::SandboxExecutionPolicy>) -> FsWriteOutcome;
            edit_text(target: &'a FsTarget, edit: FsEditRequest, expected: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<hoh_fs::SandboxExecutionPolicy>) -> FsEditOutcome;
            patch_text(target: &'a FsTarget, hunks: &'a [hoh_fs_patch::PatchHunk], expected: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<hoh_fs::SandboxExecutionPolicy>) -> FsEditOutcome;
            delete_file(target: &'a FsTarget, expected: Option<FsVersionGuard>, signal: Option<AbortSignal>, policy: Option<hoh_fs::SandboxExecutionPolicy>) -> FsDeleteOutcome;
            move_file(source: &'a FsTarget, destination: &'a FsTarget, options: FsMoveOptions, signal: Option<AbortSignal>, policy: Option<hoh_fs::SandboxExecutionPolicy>) -> FsMoveOutcome;
        }
    }

    struct NoopModel;

    impl Model for NoopModel {
        fn current(&self) -> Option<ModelSelection> {
            None
        }

        fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordedInbox(Mutex<Vec<UserMessage>>);

    impl Inbox for RecordedInbox {
        fn next_turn(&self) -> Vec<UserMessage> {
            vec![]
        }

        fn next_step(&self) -> Vec<UserMessage> {
            lock(&self.0).clone()
        }

        fn clear(&self) -> Result<(), BoxError> {
            lock(&self.0).clear();
            Ok(())
        }

        fn append(&self, _: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
            lock(&self.0).push(message);
            Ok(())
        }

        fn prepend(&self, _: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
            lock(&self.0).insert(0, message);
            Ok(())
        }

        fn replace(&self, id: &MessageId, message: UserMessage) -> Result<bool, BoxError> {
            let mut messages = lock(&self.0);
            let Some(index) = messages.iter().position(|item| &item.id == id) else {
                return Ok(false);
            };
            messages[index] = message;
            Ok(true)
        }

        fn remove(&self, id: &MessageId) -> Result<bool, BoxError> {
            let mut messages = lock(&self.0);
            let Some(index) = messages.iter().position(|item| &item.id == id) else {
                return Ok(false);
            };
            messages.remove(index);
            Ok(true)
        }

        fn splice(
            &self,
            _: InboxTarget,
            start: f64,
            delete_count: f64,
            inserted: Vec<UserMessage>,
        ) -> Result<Vec<UserMessage>, BoxError> {
            let mut messages = lock(&self.0);
            let start = start.max(0.0) as usize;
            let end = start
                .saturating_add(delete_count.max(0.0) as usize)
                .min(messages.len());
            Ok(messages.splice(start..end, inserted).collect())
        }
    }

    struct TestAgent {
        session: Session,
        context: Context,
        scope: ScopeKey,
        options: AgentOptions,
        inbox: RecordedInbox,
    }

    impl Agent for TestAgent {
        fn id(&self) -> &SessionId {
            self.session.id()
        }

        fn scope_key(&self) -> ScopeKey {
            self.scope.clone()
        }

        fn options(&self) -> &AgentOptions {
            &self.options
        }

        fn session(&self) -> &Session {
            &self.session
        }

        fn model(&self) -> &dyn Model {
            &NoopModel
        }

        fn inbox(&self) -> &dyn Inbox {
            &self.inbox
        }

        fn status(&self) -> AgentStatus {
            AgentStatus::Idle
        }

        fn ctx(&self) -> &Context {
            &self.context
        }

        fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
            Ok(())
        }

        fn when_idle(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }

        fn run_maintenance_erased(
            &self,
            _: MaintenanceTask,
        ) -> Result<MaintenanceFuture, BoxError> {
            Err("unused maintenance".into())
        }

        fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
            Ok(())
        }

        fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
            Ok(())
        }

        fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
            Ok(())
        }

        fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
            Ok(())
        }
    }

    fn test_agent(context: Context, pending: Vec<UserMessage>) -> TestAgent {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id: SessionId = format!(
            "agent-instructions-review-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
        .into();
        TestAgent {
            session: Session::create(id, None, None, None, None).unwrap(),
            context,
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            inbox: RecordedInbox(Mutex::new(pending)),
        }
    }

    fn test_agent_with_id(context: Context, id: SessionId) -> TestAgent {
        TestAgent {
            session: Session::create(id, None, None, None, None).unwrap(),
            context,
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            inbox: RecordedInbox::default(),
        }
    }

    fn loaded(path: &str, content: &str) -> LoadedInstructionFile {
        LoadedInstructionFile {
            absolute_path: format!("/repo/{path}"),
            display_path: path.into(),
            content: content.into(),
            version: None,
        }
    }

    fn context_message(text: &str, identity: &str) -> UserMessage {
        baseline_context_message(vec![ContentBlock::text(text)], &[], identity)
    }

    async fn mounted_plugin(
        root_text: &str,
        gate: Option<StreamGate>,
    ) -> (hoh_kernel::Kernel, Context, Arc<GatedFs>) {
        let fs = Arc::new(GatedFs {
            inner: LocalFileSystem::new(LocalFsConfig {
                cwd: Some(root_text.to_owned()),
                ..LocalFsConfig::default()
            })
            .unwrap(),
            gate: Mutex::new(gate),
            stream_calls: AtomicU64::new(0),
        });
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let provided = fs.clone();
        ctx.inject(move |scope, (): ()| {
            let provided = provided.clone();
            async move {
                scope.provide::<Fs>(provided).await?;
                Ok(())
            }
        })
        .unwrap()
        .settled()
        .await
        .unwrap();
        provide_working_directory_fixture(&ctx, Some(root_text))
            .await
            .unwrap();
        ctx.plugin::<AgentRegistryPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<AgentInstructions>(json!({
            "projectRootMarkers":[".git"],
            "maxBytes":65536,
            "maxSourceBytes":65536,
            "instructionFileCandidates":["AGENTS.md"],
            "localInstructionFileCandidates":[]
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();
        let fixture = define_content_tool_fixture(ContentToolFixtureOptions::new(
            "read",
            "read fixture",
            json!({}),
            |_, _| async { Ok(vec![ContentBlock::text("read completed")]) },
        ))
        .unwrap();
        ctx.get::<Tools>()
            .unwrap()
            .register(&ctx, fixture, ToolRegistrationOptions::default())
            .unwrap();
        (kernel, ctx, fs)
    }

    async fn execute_read(ctx: &Context, agent: Arc<dyn Agent>) {
        let mut input = ToolExecutionInput::new(
            "read-instructions".into(),
            "read",
            json!({"file_path":"pkg/AGENTS.md"}),
            AbortController::new().signal(),
        );
        input.agent = Some(agent);
        let result = execute_tool_fixture(ctx, input).await.unwrap();
        assert!(!result.is_error());
    }

    async fn run_pre_step(ctx: Context, agent: Arc<dyn Agent>) -> PreStepDecision {
        ctx.waterfall(
            AgentPreStep {
                agent,
                messages: vec![],
                turn: 1,
                step: 2,
                signal: AbortController::new().signal(),
                model_selection: ModelSelection {
                    provider: "mock".into(),
                    model: "mock".into(),
                    reasoning_effort: None,
                },
            },
            |_| async {
                Ok(PreStepDecision::Enter {
                    messages: vec![],
                    starts_request_series: false,
                })
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn provider_known_size_is_rejected_before_streaming() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "too large").unwrap();
        let root_text = root.path().to_string_lossy().into_owned();
        let fs = GatedFs {
            inner: LocalFileSystem::new(LocalFsConfig {
                cwd: Some(root_text.clone()),
                ..LocalFsConfig::default()
            })
            .unwrap(),
            gate: Mutex::new(None),
            stream_calls: AtomicU64::new(0),
        };
        let options = LoadOptions {
            discovery: DiscoverOptions {
                cwd: root_text.clone(),
                dsh_home: Some(root_text.clone()),
                agents_home: Some(root_text),
                project_root_markers: Some(vec![".git".into()]),
                instruction_file_candidates: Some(vec!["AGENTS.md".into()]),
                local_instruction_file_candidates: Some(vec![]),
                project_root: None,
                signal: None,
            },
            max_bytes: 65_536.0,
            max_source_bytes: 4,
            replace_previous_baseline: false,
        };
        let loaded = load_baseline_instructions(&options, Some(&fs))
            .await
            .unwrap();
        assert!(loaded.is_none());
        assert_eq!(fs.stream_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn tools_result_projection_is_visible_to_the_immediately_following_pre_step() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::create_dir(root.path().join("pkg")).unwrap();
        std::fs::write(
            root.path().join("pkg/AGENTS.md"),
            "projection must reach this step",
        )
        .unwrap();
        let root_text = root.path().to_string_lossy().into_owned();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (kernel, ctx, _fs) = mounted_plugin(
            &root_text,
            Some(StreamGate {
                entered: entered_tx,
                release: release_rx,
            }),
        )
        .await;
        let agent: Arc<dyn Agent> = Arc::new(test_agent(ctx.clone(), vec![]));
        execute_read(&ctx, agent.clone()).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), entered_rx)
            .await
            .expect("the tools/result projection should begin reading the touched instruction")
            .expect("the projection gate should remain connected");

        let pre_ctx = ctx.clone();
        let pre_agent = agent.clone();
        let pre_step = tokio::spawn(async move { run_pre_step(pre_ctx, pre_agent).await });
        tokio::task::yield_now().await;
        assert!(
            !pre_step.is_finished(),
            "pre-step must wait for the already-issued tools/result projection"
        );
        release_tx.send(()).unwrap();
        let decision = tokio::time::timeout(std::time::Duration::from_secs(2), pre_step)
            .await
            .expect("pre-step should finish after the projection gate opens")
            .unwrap();
        let PreStepDecision::Enter { messages, .. } = decision else {
            panic!("pre-step should be admitted")
        };
        assert!(messages.iter().any(|message| {
            message.content.iter().any(|block| {
                block.text_value().is_some_and(|text| {
                    text.contains("pkg/AGENTS.md")
                        && text.contains("projection must reach this step")
                })
            })
        }));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn step_end_listener_releases_a_deferred_tools_result_projection() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::create_dir(root.path().join("pkg")).unwrap();
        std::fs::write(root.path().join("pkg/AGENTS.md"), "released at step end").unwrap();
        let root_text = root.path().to_string_lossy().into_owned();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (kernel, ctx, fs) = mounted_plugin(
            &root_text,
            Some(StreamGate {
                entered: entered_tx,
                release: release_rx,
            }),
        )
        .await;
        let agent: Arc<dyn Agent> = Arc::new(test_agent(ctx.clone(), vec![]));
        agent
            .session()
            .append("turn/start", json!({"turn":1}), None)
            .unwrap();
        agent
            .session()
            .append("step/start", json!({"turn":1,"step":1}), None)
            .unwrap();
        execute_read(&ctx, agent.clone()).await;
        assert_eq!(
            fs.stream_calls.load(Ordering::Relaxed),
            0,
            "an open step must defer the projection"
        );
        let step_end = agent
            .session()
            .append("step/end", json!({"turn":1,"step":1}), None)
            .unwrap();
        ctx.dispatch::<SessionEventEmitted>(Arc::new(SessionEventEmitted {
            session: agent.session().clone(),
            event: step_end,
            validation: DispatchValidation::default(),
        }))
        .invoke(|_| {});
        tokio::time::timeout(std::time::Duration::from_secs(2), entered_rx)
            .await
            .expect("step/end must start the deferred projection")
            .expect("the stream gate should remain connected");
        release_tx.send(()).unwrap();
        let decision = run_pre_step(ctx.clone(), agent).await;
        let PreStepDecision::Enter { messages, .. } = decision else {
            panic!("pre-step should be admitted")
        };
        assert!(messages.iter().any(|message| {
            message.content.iter().any(|block| {
                block
                    .text_value()
                    .is_some_and(|text| text.contains("released at step end"))
            })
        }));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn session_disposed_listener_detaches_a_reused_id_from_the_old_projection_queue() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::create_dir(root.path().join("pkg")).unwrap();
        std::fs::write(root.path().join("pkg/AGENTS.md"), "old session projection").unwrap();
        let root_text = root.path().to_string_lossy().into_owned();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (kernel, ctx, _fs) = mounted_plugin(
            &root_text,
            Some(StreamGate {
                entered: entered_tx,
                release: release_rx,
            }),
        )
        .await;
        let old_agent: Arc<dyn Agent> = Arc::new(test_agent(ctx.clone(), vec![]));
        let reused_id = old_agent.id().clone();
        execute_read(&ctx, old_agent.clone()).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), entered_rx)
            .await
            .expect("the old projection should reach its gate")
            .expect("the old projection gate should remain connected");
        ctx.dispatch::<SessionDisposed>(Arc::new(SessionDisposed {
            session: old_agent.session().clone(),
            validation: DispatchValidation::default(),
        }))
        .invoke(|_| {});
        let new_agent: Arc<dyn Agent> = Arc::new(test_agent_with_id(ctx.clone(), reused_id));
        let decision = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run_pre_step(ctx.clone(), new_agent),
        )
        .await
        .expect("a reused id must not wait for the disposed session's projection queue");
        assert!(matches!(decision, PreStepDecision::Enter { .. }));
        release_tx.send(()).unwrap();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn non_abort_projection_failure_is_logged_as_a_warning() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        spawn_projection_task(&ctx, |_| async { Err("projection boom".into()) }).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if ctx.logger().buffer().iter().any(|record| {
                    record
                        .message
                        .contains("workspace instruction refresh failed: projection boom")
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a failed projection should emit its warning");
        kernel.shutdown().await;
    }

    #[test]
    fn pending_baseline_is_not_reused_after_identity_changes() {
        let pending = vec![context_message("rules", "old")];
        let prepared = BaselinePreparation {
            identity: "old".into(),
            excluded_scopes: HashSet::new(),
        };
        assert!(may_reuse_pending(&[], &pending, Some(&prepared), "old"));
        assert!(!may_reuse_pending(&[], &pending, Some(&prepared), "new"));
    }

    #[test]
    fn represented_duplicate_is_reenabled_for_later_promotion() {
        let winner = loaded("AGENTS.md", "same");
        let duplicate = loaded("CLAUDE.md", " same\n");
        let mut excluded = HashSet::new();
        refresh_excluded_baseline_scopes(
            &mut excluded,
            std::slice::from_ref(&winner),
            &[winner.clone(), duplicate.clone()],
            &[duplicate],
        );
        assert!(excluded.is_empty());
    }

    #[test]
    fn recomputed_exclusions_do_not_retain_stale_scopes() {
        let mut excluded = HashSet::from([instruction_scope_key("old/AGENTS.md")]);
        refresh_excluded_baseline_scopes(&mut excluded, &[], &[], &[]);
        assert!(excluded.is_empty());
    }

    #[tokio::test]
    async fn surface_payload_removes_the_same_pending_context() {
        let kernel = hoh_kernel::Kernel::start();
        let desired = context_message("desired", "identity");
        let pending = context_message("stale", "identity");
        let agent = test_agent(kernel.root(), vec![pending]);
        agent
            .session
            .append(
                "user/message",
                serde_json::to_value(&desired).unwrap(),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        sync_inbox(&agent, &[], Some(desired)).unwrap();
        assert!(agent.inbox.next_step().is_empty());
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn synchronously_issued_projection_blocks_the_pre_step_barrier() {
        let queue = Arc::new(ProjectionQueue::default());
        let ticket = queue.issue();

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), queue.wait_idle())
                .await
                .is_err(),
            "a ticket must be visible before its task is first polled"
        );

        drop(ticket);
        tokio::time::timeout(std::time::Duration::from_secs(1), queue.wait_idle())
            .await
            .expect("dropping the projection ticket releases the barrier");
    }

    #[tokio::test]
    async fn projection_tickets_run_in_issue_order_not_poll_order() {
        let queue = Arc::new(ProjectionQueue::default());
        let first = queue.issue();
        let second = queue.issue();
        let second_wait = tokio::spawn(async move {
            second.wait_turn().await;
            drop(second);
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), second_wait)
                .await
                .is_err(),
            "polling the second task first must not reorder projections"
        );
        drop(first);
        tokio::time::timeout(std::time::Duration::from_secs(1), queue.wait_idle())
            .await
            .expect("the second ticket advances after the first completes");
    }

    #[test]
    fn step_boundary_is_open_only_between_start_and_end() {
        let turn_start = SessionSeq::new(4).unwrap();
        let step_start = SessionSeq::new(5).unwrap();
        let mut boundary = hoh_agent::TurnBoundaryProjection {
            open_turn_start_seq: Some(turn_start),
            last_step_start_seq: Some(step_start),
            last_step_boundary: Some(hoh_agent::StepBoundary {
                kind: hoh_agent::StepBoundaryKind::Start,
                seq: step_start,
            }),
            last_turn: 1,
        };
        assert!(boundary_has_open_step(&boundary));

        boundary.last_step_boundary = Some(hoh_agent::StepBoundary {
            kind: hoh_agent::StepBoundaryKind::End,
            seq: SessionSeq::new(6).unwrap(),
        });
        assert!(!boundary_has_open_step(&boundary));

        boundary.open_turn_start_seq = None;
        boundary.last_step_boundary = Some(hoh_agent::StepBoundary {
            kind: hoh_agent::StepBoundaryKind::Start,
            seq: step_start,
        });
        assert!(!boundary_has_open_step(&boundary));
    }

    #[tokio::test]
    async fn nested_tool_touches_are_released_only_by_the_root_execution() {
        let kernel = hoh_kernel::Kernel::start();
        let agent: Arc<dyn Agent> = Arc::new(test_agent(kernel.root(), vec![]));
        let executions = Mutex::new(HashMap::<u64, Vec<ProjectionTouch>>::new());
        let touch = ProjectionTouch {
            agent,
            path: "pkg/AGENTS.md".into(),
        };

        assert!(propagate_projection_touches(&executions, 3, Some(2), Some(touch)).is_empty());
        assert!(propagate_projection_touches(&executions, 2, Some(1), None).is_empty());
        let released = propagate_projection_touches(&executions, 1, None, None);

        assert_eq!(released.len(), 1);
        assert_eq!(released[0].path, "pkg/AGENTS.md");
        assert!(lock(&executions).is_empty());
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn sync_inbox_reuses_an_identical_pending_message() {
        let kernel = hoh_kernel::Kernel::start();
        let reusable = context_message("desired", "identity");
        let reusable_id = reusable.id.clone();
        let stale = context_message("stale", "identity");
        let desired = context_message("desired", "identity");
        let agent = test_agent(kernel.root(), vec![reusable, stale]);

        sync_inbox(&agent, &[], Some(desired)).unwrap();

        let pending = agent.inbox.next_step();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, reusable_id);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn sync_inbox_replaces_the_first_stale_message() {
        let kernel = hoh_kernel::Kernel::start();
        let stale = context_message("stale", "identity");
        let desired = context_message("desired", "identity");
        let desired_id = desired.id.clone();
        let agent = test_agent(kernel.root(), vec![stale]);

        sync_inbox(&agent, &[], Some(desired)).unwrap();

        let pending = agent.inbox.next_step();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, desired_id);
        assert_eq!(pending[0].content, vec![ContentBlock::text("desired")]);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn sync_inbox_prepends_when_no_context_is_pending() {
        let kernel = hoh_kernel::Kernel::start();
        let desired = context_message("desired", "identity");
        let desired_id = desired.id.clone();
        let agent = test_agent(kernel.root(), vec![]);

        sync_inbox(&agent, &[], Some(desired)).unwrap();

        let pending = agent.inbox.next_step();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, desired_id);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejected_pre_step_keeps_desired_context_in_the_inbox() {
        let kernel = hoh_kernel::Kernel::start();
        let desired = context_message("desired", "identity");
        let desired_id = desired.id.clone();
        let agent = test_agent(kernel.root(), vec![]);

        let decision = reconcile_pre_step_decision(
            &agent,
            2,
            &[],
            &[],
            PreStepDecision::Reject,
            Some(desired),
        )
        .unwrap();

        assert_eq!(decision, PreStepDecision::Reject);
        assert_eq!(agent.inbox.next_step()[0].id, desired_id);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn empty_first_step_defers_desired_context_to_the_inbox() {
        let kernel = hoh_kernel::Kernel::start();
        let desired = context_message("desired", "identity");
        let desired_id = desired.id.clone();
        let agent = test_agent(kernel.root(), vec![]);

        let decision = reconcile_pre_step_decision(
            &agent,
            1,
            &[],
            &[],
            PreStepDecision::Enter {
                messages: vec![],
                starts_request_series: false,
            },
            Some(desired),
        )
        .unwrap();

        assert_eq!(
            decision,
            PreStepDecision::Enter {
                messages: vec![],
                starts_request_series: false,
            }
        );
        assert_eq!(agent.inbox.next_step()[0].id, desired_id);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn pre_step_inserts_context_after_the_last_claimed_message() {
        let kernel = hoh_kernel::Kernel::start();
        let prefix = context_message("prefix", "prefix");
        let claimed = context_message("claimed", "claimed");
        let suffix = context_message("suffix", "suffix");
        let desired = context_message("desired", "desired");
        let expected_ids = [
            prefix.id.clone(),
            claimed.id.clone(),
            desired.id.clone(),
            suffix.id.clone(),
        ];
        let agent = test_agent(kernel.root(), vec![]);

        let decision = reconcile_pre_step_decision(
            &agent,
            2,
            std::slice::from_ref(&claimed),
            &[],
            PreStepDecision::Enter {
                messages: vec![prefix, claimed.clone(), suffix],
                starts_request_series: true,
            },
            Some(desired),
        )
        .unwrap();

        let PreStepDecision::Enter { messages, .. } = decision else {
            panic!("expected an admitted step")
        };
        assert_eq!(
            messages
                .iter()
                .map(|message| message.id.clone())
                .collect::<Vec<_>>(),
            expected_ids
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn projection_signal_is_aborted_during_context_shutdown() {
        let kernel = hoh_kernel::Kernel::start();
        let context = kernel.root();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (aborted_tx, aborted_rx) = tokio::sync::oneshot::channel();
        spawn_projection_task(&context, move |signal| async move {
            let _ = started_tx.send(());
            signal.cancelled().await;
            let _ = aborted_tx.send(signal.aborted());
            Ok(())
        })
        .unwrap();
        started_rx.await.unwrap();
        kernel.shutdown().await;
        assert!(aborted_rx.await.unwrap());
    }
}
