//! One-shot direct Agent driver.

use crate::{
    json_stream::{MAX_STRING_BYTES, bound_json_event_line, project_json_run},
    startup::{HeadlessRuntime, HeadlessRuntimeKey, HeadlessStartupKey},
};
use hoh_agent::{
    Agent, AgentAssistantStream, AgentError, AgentOptions, Agents, AssistantStreamFrame,
    CreateAgentOptions, Live, ResumeAgentOptions, install_model_selection,
};
use hoh_agent_default_model::AgentDefaultModel;
use hoh_error::{BoxError, HarnessError};
use hoh_fs::{Fs, FsResolveOptions};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, StreamChunk, create_user_message};
use hoh_loader::{LoaderKey, cmdline::AppExitKey};
use hoh_permission_presets::PermissionPresets;
use hoh_sandbox_policy::SandboxPolicy;
use hoh_session::{
    Session, SessionEventEmitted, SessionId, SessionLogOffset, SessionSeq, Sessions,
};
use hoh_session_persistence::SessionPersistenceService;
use hoh_session_query::{SessionObservationOptions, SessionQuery, SessionQueryErrorCode};
use hoh_subagent::Subagents;
use hoh_user_approval::{Approval, ApprovalPolicy, set_approval_policy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Runner configuration resolved from the startup provider.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub json: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
}

/// Durable outcome of the owned interval.
#[derive(Clone, Debug, PartialEq)]
pub struct RunOutcome {
    pub text: String,
    pub reason: Option<Value>,
}

/// Aggregate only the final turn after `first_seq`.
pub fn summarize(session: &Session, first_seq: SessionLogOffset) -> Result<RunOutcome, BoxError> {
    let mut started = false;
    let mut text = String::new();
    let mut reason = None;
    let length = session.seq();
    for raw in u64::from(first_seq)..u64::from(length) {
        let seq = SessionSeq::new(raw)?;
        let event = session.event_at(seq).ok_or_else(|| {
            format!(
                "headless summary cannot read seq {raw} below captured length {}",
                u64::from(length)
            )
        })?;
        match event.event_type() {
            "turn/start" => {
                started = true;
                reason = None;
            }
            "assistant/message" if started => {
                let joined: String = event
                    .data()
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect();
                if !joined.is_empty() {
                    text = joined;
                }
            }
            "turn/end" if started => reason = event.data().get("reason").cloned(),
            _ => {}
        }
    }
    Ok(RunOutcome { text, reason })
}

fn current_preset(session: &Session) -> Result<Option<String>, BoxError> {
    let mut preset = session
        .header()
        .get("agentPreset")
        .and_then(Value::as_str)
        .map(str::to_owned);
    for event in session.snapshot_events(None, None).iter() {
        if event.event_type() != "agent-preset/selected" {
            continue;
        }
        let selected = event.data().get("agentPreset").and_then(Value::as_str);
        match selected {
            Some(selected) if !selected.is_empty() => preset = Some(selected.into()),
            _ => {
                return Err(format!(
                    "session \"{}\" records a malformed agent-preset/selected event and cannot be adopted",
                    session.id()
                )
                .into());
            }
        }
    }
    Ok(preset)
}

fn assert_adoptable(session: &Session, cwd: &str) -> Result<(), BoxError> {
    if let Some(preset) = current_preset(session)? {
        return Err(format!(
            "session \"{}\" runs under agent preset \"{preset}\", which the one-shot runner does not compose",
            session.id()
        )
        .into());
    }
    let header = session.header();
    if header.get("origin").and_then(Value::as_str) == Some("subagent") {
        return Err(format!(
            "session \"{}\" is a subagent session and cannot be driven directly",
            session.id()
        )
        .into());
    }
    let Some(recorded_cwd) = header.get("cwd").and_then(Value::as_str) else {
        return Err(format!(
            "session \"{}\" recorded no working directory, so it cannot be adopted",
            session.id()
        )
        .into());
    };
    if recorded_cwd != cwd {
        return Err(format!(
            "session \"{}\" was recorded in \"{recorded_cwd}\", not \"{cwd}\"",
            session.id()
        )
        .into());
    }
    Ok(())
}

async fn resolve_agent(
    ctx: &Context,
    agents: &hoh_agent::AgentRegistry,
    session_id: SessionId,
    options: AgentOptions,
    selection: hoh_agent::ModelSelection,
    cwd: &str,
) -> Result<Arc<dyn Agent>, BoxError> {
    if ctx.get::<SessionPersistenceService>().is_none() {
        return Err("headless --session-id requires the sessionPersistence service; the Session would not survive this process".into());
    }
    let query = ctx
        .get::<SessionQuery>()
        .ok_or("headless --session-id requires the sessionQuery service; dsh-base provides it")?;
    if let Some(live) = agents.get(&session_id) {
        assert_adoptable(live.session(), cwd)?;
        return Err(format!(
            "session \"{session_id}\" is live in this process, so the one-shot runner cannot own an exclusive run interval"
        )
        .into());
    }
    match query
        .observe_session(&session_id, SessionObservationOptions::default())
        .await
    {
        Ok(observation) => {
            let observed = Session::from_restore(
                session_id.clone(),
                observation.events(),
                serde_json::to_value(observation.header())?,
                observation.inherited_event_count(),
                hoh_session::SessionSeedEventState::Detached,
                None,
            )?;
            assert_adoptable(&observed, cwd)?;
        }
        Err(error) if is_session_not_found(error.as_ref()) => {
            return Err(missing_session_error(&session_id));
        }
        Err(error) => return Err(error),
    }
    let setup = Arc::new({
        move |agent_ctx: Context,
              agent: Arc<dyn Agent>|
              -> futures_util::future::BoxFuture<'static, _> {
            let mut configured = selection.clone();
            Box::pin(async move {
                if configured.reasoning_effort.is_none()
                    && let Some(previous) = agent.model().current()
                    && previous.provider == configured.provider
                    && previous.model == configured.model
                {
                    configured.reasoning_effort = previous.reasoning_effort;
                }
                agent.model().select(&configured)?;
                install_model_selection(&agent_ctx)?;
                Ok(None)
            })
        }
    });
    let mut resume = ResumeAgentOptions::new(session_id.clone());
    resume.agent_options = Some(options);
    resume.setup = Some(setup);
    let handle = agents
        .resume(ctx, resume)
        .await
        .map_err(|error| map_resume_error(error, &session_id))?;
    assert_adoptable(handle.agent.session(), cwd)?;
    Ok(handle.agent)
}

fn is_session_not_found(error: &(dyn std::error::Error + 'static)) -> bool {
    HarnessError::of(error)
        .is_some_and(|error| error.code() == SessionQueryErrorCode::SessionNotFound.as_str())
}

fn missing_session_error(session_id: &SessionId) -> BoxError {
    format!("session \"{session_id}\" does not exist; omit --session-id to start a new Session")
        .into()
}

fn map_resume_error(error: BoxError, session_id: &SessionId) -> BoxError {
    if is_session_not_found(error.as_ref()) {
        missing_session_error(session_id)
    } else {
        error
    }
}

fn model_options(selection: &hoh_agent::ModelSelection) -> AgentOptions {
    AgentOptions {
        provider: Some(Live::Value(selection.provider.clone())),
        model: Some(Live::Value(selection.model.clone())),
        reasoning_effort: selection.reasoning_effort.clone().map(Live::Value),
        max_tokens: None,
    }
}

fn terminal_completed(reason: Option<&Value>) -> bool {
    reason
        .and_then(|reason| reason.get("kind"))
        .and_then(Value::as_str)
        == Some("completed")
}

#[derive(Default)]
struct ReasoningState {
    open: bool,
    ends_with_newline: bool,
}

fn close_reasoning(state: &mut ReasoningState, runtime: &HeadlessRuntime) {
    if !state.open {
        return;
    }
    if !state.ends_with_newline {
        runtime.stderr.write("\n");
    }
    state.open = false;
    state.ends_with_newline = true;
}

fn write_reasoning_chunk(
    state: &mut ReasoningState,
    runtime: &HeadlessRuntime,
    chunk: &StreamChunk,
) {
    match chunk {
        StreamChunk::ReasoningDelta { text, .. } => {
            if text.is_empty() {
                return;
            }
            if !state.open {
                runtime.stderr.write("dsh: reasoning:\n");
                state.open = true;
            }
            runtime.stderr.write(text);
            state.ends_with_newline = text.ends_with('\n');
        }
        StreamChunk::BlockStart { block_type, .. } if block_type == "reasoning" => {}
        StreamChunk::BlockEnd { block, .. } if block.r#type == "reasoning" => {}
        StreamChunk::Usage { .. } => {}
        _ => close_reasoning(state, runtime),
    }
}

struct ReasoningGuard {
    state: Arc<Mutex<ReasoningState>>,
    active: Arc<AtomicBool>,
    runtime: HeadlessRuntime,
}

impl ReasoningGuard {
    fn stop(&self) {
        self.active.store(false, Ordering::Release);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        close_reasoning(&mut state, &self.runtime);
    }
}

impl Drop for ReasoningGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

struct TrackingGuard(Arc<AtomicBool>);

impl Drop for TrackingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

type TrackedFailure = Option<(hoh_error::SharedError, SessionLogOffset)>;

fn record_agent_failure(
    failure: &Mutex<TrackedFailure>,
    expected: &Arc<dyn Agent>,
    event: &AgentError,
) {
    if Arc::ptr_eq(&event.agent, expected) {
        *failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((event.error.clone(), event.agent.session().seq()));
    }
}

fn clear_terminal_failure(
    failure: &Mutex<TrackedFailure>,
    expected: &Session,
    session: &Session,
    event: &hoh_session::SessionEvent,
) {
    if session != expected || event.event_type() != "turn/end" {
        return;
    }
    let mut failure = failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if failure
        .as_ref()
        .is_some_and(|(_, offset)| u64::from(event.seq()) >= u64::from(*offset))
    {
        *failure = None;
    }
}

fn stream_reasoning(
    ctx: &Context,
    agent: Arc<dyn Agent>,
    runtime: HeadlessRuntime,
) -> Result<ReasoningGuard, BoxError> {
    let state = Arc::new(Mutex::new(ReasoningState {
        open: false,
        ends_with_newline: true,
    }));
    let active = Arc::new(AtomicBool::new(true));
    let listener_state = state.clone();
    let listener_active = active.clone();
    let listener_runtime = runtime.clone();
    let expected = agent.clone();
    ctx.on_contained::<AgentAssistantStream>(move |event| {
        if !listener_active.load(Ordering::Acquire) || !Arc::ptr_eq(&event.agent, &expected) {
            return Ok(());
        }
        let mut state = listener_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &event.frame {
            AssistantStreamFrame::Start { .. } | AssistantStreamFrame::End { .. } => {
                close_reasoning(&mut state, &listener_runtime);
            }
            AssistantStreamFrame::Chunk { chunk, .. } => {
                write_reasoning_chunk(&mut state, &listener_runtime, chunk);
            }
        }
        Ok(())
    })?;
    Ok(ReasoningGuard {
        state,
        active,
        runtime,
    })
}

async fn run(
    ctx: Context,
    config: Config,
    deps: Deps,
    runtime: HeadlessRuntime,
) -> Result<(), BoxError> {
    if let Some(loader) = ctx.get::<LoaderKey>() {
        loader.settled().await;
    }
    if config
        .session_id
        .as_deref()
        .is_some_and(|id| id.trim().is_empty())
    {
        return Err("headless-runner: sessionId must not be blank".into());
    }
    let startup = ctx.get::<HeadlessStartupKey>();
    let permission_mode = startup
        .as_ref()
        .and_then(|startup| startup.permission_mode.clone())
        .or(config.permission_mode.clone());
    let approval_policy = startup
        .as_ref()
        .and_then(|startup| startup.approval_policy.as_deref())
        .map(ApprovalPolicy::try_from)
        .transpose()?
        .or(config.approval_policy);
    let permissions = ctx.get::<PermissionPresets>();
    if let Some(mode) = permission_mode.as_deref() {
        permissions
            .as_ref()
            .ok_or("headless --permission-mode requires the permissionPresets service")?
            .resolve(mode)?;
    }
    let approval = ctx.get::<Approval>();
    if approval_policy.is_some() && approval.is_none() {
        return Err("headless --approval-policy requires the approval service".into());
    }
    if let Some(roots) = startup
        .as_ref()
        .and_then(|startup| startup.additional_writable_roots.as_ref())
        && !roots.is_empty()
    {
        let allowed = ctx
            .get::<SandboxPolicy>()
            .and_then(|policy| policy.additional_writable_roots.clone())
            .is_some_and(|allowed| {
                roots
                    .iter()
                    .all(|root| allowed.iter().any(|item| item == root))
            });
        if !allowed {
            return Err("headless --add-dir requires the sandbox-policy row to include headlessStartup.additionalWritableRoots; check the profile overlays".into());
        }
    }
    let configured_task = startup
        .as_ref()
        .and_then(|startup| startup.task.clone())
        .or(config.task.clone());
    let task = if configured_task.as_deref().is_none_or(|task| task == "-") {
        (runtime.read_stdin)().await?
    } else {
        configured_task.expect("checked above")
    };
    if task.trim().is_empty() {
        return Err(
            "a task is required, for example: dsh --profile headless \"run the tests\"".into(),
        );
    }
    let selection = deps.default_model.current_selection();
    let options = model_options(&selection);
    let cwd = if let Some(fs) = ctx.get::<Fs>() {
        let target = fs.resolve(".", FsResolveOptions::default()).await?;
        fs.process_path(&target)
    } else {
        std::env::current_dir()?.to_string_lossy().into_owned()
    };
    let requested_id = startup
        .as_ref()
        .and_then(|startup| startup.session_id.clone())
        .or(config.session_id.clone());
    let agent = if let Some(id) = requested_id.as_deref() {
        resolve_agent(
            &ctx,
            &deps.agents,
            id.to_owned().into(),
            options,
            selection.clone(),
            &cwd,
        )
        .await?
    } else {
        let session_id: SessionId = format!("session-{}", hoh_util_crypto::random_uuid()).into();
        let setup = Arc::new(
            |agent_ctx: Context,
             _agent: Arc<dyn Agent>|
             -> futures_util::future::BoxFuture<'static, _> {
                Box::pin(async move {
                    install_model_selection(&agent_ctx)?;
                    Ok(None)
                })
            },
        );
        let mut create = CreateAgentOptions::new(session_id);
        create.meta = Some(hoh_agent::AgentSessionMeta {
            cwd: Some(cwd.clone()),
            ..Default::default()
        });
        create.agent_options = Some(options);
        create.setup = Some(setup);
        deps.agents.create(&ctx, create).await?.agent
    };
    agent.when_idle().await;
    if requested_id.is_some() {
        assert_adoptable(agent.session(), &cwd)?;
    }
    if let Some(mode) = permission_mode.as_deref()
        && let Some(permissions) = permissions
    {
        permissions.set(agent.session(), mode)?;
    }
    if let Some(policy) = approval_policy
        && let Some(approval) = approval
        && approval.override_of(agent.session())? != Some(policy)
    {
        set_approval_policy(agent.session(), policy)?;
    }
    let first_seq = agent.session().seq();
    let json_mode = startup.as_ref().map_or(config.json, |startup| startup.json);
    let projection = if json_mode {
        Some(project_json_run(
            &ctx,
            agent.clone(),
            runtime.stdout.clone(),
            &cwd,
            MAX_STRING_BYTES,
        )?)
    } else {
        None
    };
    let failure = Arc::new(Mutex::new(
        None::<(hoh_error::SharedError, SessionLogOffset)>,
    ));
    let tracking = Arc::new(AtomicBool::new(true));
    let _tracking_guard = TrackingGuard(tracking.clone());
    let error_failure = failure.clone();
    let error_tracking = tracking.clone();
    let error_agent = agent.clone();
    ctx.on_contained::<AgentError>(move |event| {
        if error_tracking.load(Ordering::Acquire) {
            record_agent_failure(&error_failure, &error_agent, event);
        }
        Ok(())
    })?;
    let terminal_failure = failure.clone();
    let terminal_tracking = tracking.clone();
    let terminal_session = agent.session().clone();
    ctx.on_contained::<SessionEventEmitted>(move |emitted| {
        if !terminal_tracking.load(Ordering::Acquire) {
            return Ok(());
        }
        clear_terminal_failure(
            &terminal_failure,
            &terminal_session,
            &emitted.session,
            &emitted.event,
        );
        Ok(())
    })?;
    let stop_reasoning = if projection.is_none() {
        Some(stream_reasoning(&ctx, agent.clone(), runtime.clone())?)
    } else {
        None
    };
    let settlement = async {
        agent.followup(create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(task)],
            source: MessageSource::new("user"),
        }))?;
        loop {
            agent.when_idle().await;
            let idle_seq = agent.session().seq();
            let child_work = match ctx.get::<Subagents>() {
                Some(subagents) => subagents.wait_for_tree_work(&agent).await?,
                None => false,
            };
            if !child_work
                && agent.status() == hoh_agent::AgentStatus::Idle
                && agent.session().seq() == idle_seq
            {
                break;
            }
        }
        Ok::<(), BoxError>(())
    }
    .await;
    drop(stop_reasoning);
    if let Err(error) = settlement {
        tracking.store(false, Ordering::Release);
        if let Some(projection) = &projection {
            projection.dispose();
        }
        return Err(error);
    }
    deps.sessions.flush(agent.session()).await?;
    tracking.store(false, Ordering::Release);
    if let Some((error, _)) = failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        if let Some(projection) = &projection {
            projection.dispose();
        }
        return Err(Box::new(error));
    }
    let outcome = summarize(agent.session(), first_seq)?;
    if let Some(projection) = &projection {
        projection.finish(&outcome.text, outcome.reason.as_ref());
        projection.dispose();
    } else {
        runtime.stdout.write(&format!("{}\n", outcome.text));
    }
    if outcome
        .reason
        .as_ref()
        .and_then(|reason| reason.get("kind"))
        .and_then(Value::as_str)
        == Some("error")
    {
        let code = outcome
            .reason
            .as_ref()
            .and_then(|reason| reason.pointer("/error/code"))
            .and_then(Value::as_str)
            .unwrap_or("ERROR");
        let message = outcome
            .reason
            .as_ref()
            .and_then(|reason| reason.pointer("/error/message"))
            .and_then(Value::as_str)
            .unwrap_or("agent failed");
        runtime.stderr.write(&format!("dsh: {code}: {message}\n"));
    }
    let exit = ctx
        .get::<AppExitKey>()
        .ok_or("headless-runner: the launcher must provide ctx.appExit before the tree mounts")?;
    exit.request(if terminal_completed(outcome.reason.as_ref()) {
        0
    } else {
        1
    });
    Ok(())
}

#[derive(Inject, Clone)]
pub struct Deps {
    agents: Service<Agents>,
    default_model: Service<AgentDefaultModel>,
    sessions: Service<Sessions>,
}

pub struct HeadlessRunner;

impl Plugin for HeadlessRunner {
    const NAME: &'static str = "headless-runner";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let exit = ctx.get::<AppExitKey>().ok_or(
            "headless-runner: the launcher must provide ctx.appExit before the tree mounts",
        )?;
        let runtime = ctx
            .get::<HeadlessRuntimeKey>()
            .map(|runtime| (*runtime).clone())
            .unwrap_or_default();
        let config = (*config.get()).clone();
        let fallback_json = config.json;
        let run_ctx = ctx.clone();
        let catch_ctx = ctx.clone();
        ctx.spawn("headless run", move |cancel| async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                result = run(run_ctx, config, deps, runtime.clone()) => {
                    if let Err(error) = result {
                        let message = error.to_string();
                        let json = catch_ctx
                            .get::<HeadlessStartupKey>()
                            .map_or(fallback_json, |startup| startup.json);
                        if json {
                            runtime.stdout.write(&format!(
                                "{}\n",
                                bound_json_event_line(&json!({"type":"error", "message":message}))
                            ));
                        }
                        runtime.stderr.write(&format!("dsh: {message}\n"));
                        exit.request(1);
                    }
                }
            }
            Ok(())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::BoxFuture;
    use hoh_agent::{
        AgentFactory, AgentHandle, AgentRegistryPlugin, AgentStatus, CancelOptions, Inbox,
        InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
    };
    use hoh_agent_default_model::AgentDefaultModelConfig;
    use hoh_commander::Output;
    use hoh_llm::{MessageId, UserMessage};
    use hoh_loader::cmdline::{AppExit, AppExitKey};
    use hoh_scope::ScopeKey;
    use hoh_session::{
        AgentCancelCause, CreateSessionOptions, SessionHeader, SessionPlugin, SessionStore,
        SurfaceIntent,
    };
    use hoh_session_persistence::{
        SessionAccess, SessionHandle, SessionPersistence, SessionPersistenceCreateOptions,
        SessionPersistenceIdentity, SessionPersistenceListOptions, SessionPersistenceOpenOptions,
        SessionPersistenceSnapshot, SessionPersistenceStatOptions,
    };
    use hoh_session_projection::SessionProjectionRegistry;
    use hoh_session_query::{
        Config as SessionQueryConfig, SessionEventSearchPage, SessionEventSearchRequest,
        SessionSearchExecContext, SessionSearchHit, SessionSearchPage, SessionSearchProvider,
        SessionSearchRequest, mount_session_query,
    };

    #[derive(Default)]
    struct TestModel(Mutex<Option<ModelSelection>>);

    impl Model for TestModel {
        fn current(&self) -> Option<ModelSelection> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn select(&self, selection: &ModelSelection) -> Result<(), BoxError> {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(selection.clone());
            Ok(())
        }
    }

    struct TestInbox;

    impl Inbox for TestInbox {
        fn next_turn(&self) -> Vec<UserMessage> {
            Vec::new()
        }
        fn next_step(&self) -> Vec<UserMessage> {
            Vec::new()
        }
        fn clear(&self) -> Result<(), BoxError> {
            Ok(())
        }
        fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
            Ok(())
        }
        fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
            Ok(())
        }
        fn replace(&self, _: &MessageId, _: UserMessage) -> Result<bool, BoxError> {
            Ok(false)
        }
        fn remove(&self, _: &MessageId) -> Result<bool, BoxError> {
            Ok(false)
        }
        fn splice(
            &self,
            _: InboxTarget,
            _: f64,
            _: f64,
            _: Vec<UserMessage>,
        ) -> Result<Vec<UserMessage>, BoxError> {
            Ok(Vec::new())
        }
    }

    struct TestAgent {
        id: SessionId,
        session: Session,
        ctx: Context,
        scope: ScopeKey,
        options: AgentOptions,
        model: TestModel,
        inbox: TestInbox,
        order: Option<Arc<Mutex<Vec<String>>>>,
        followup_error: bool,
    }

    impl Agent for TestAgent {
        fn id(&self) -> &SessionId {
            &self.id
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
            &self.model
        }
        fn inbox(&self) -> &dyn Inbox {
            &self.inbox
        }
        fn status(&self) -> AgentStatus {
            AgentStatus::Idle
        }
        fn ctx(&self) -> &Context {
            &self.ctx
        }
        fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
            Ok(())
        }
        fn when_idle(&self) -> BoxFuture<'static, ()> {
            let order = self.order.clone();
            Box::pin(async move {
                if let Some(order) = order {
                    order
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push("idle".into());
                }
            })
        }
        fn run_maintenance_erased(
            &self,
            task: MaintenanceTask,
        ) -> Result<MaintenanceFuture, BoxError> {
            Ok(task(hoh_timeout::AbortController::new().signal()))
        }
        fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
            Ok(())
        }
        fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
            if let Some(order) = &self.order {
                order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("followup".into());
            }
            if self.followup_error {
                return Err("followup failed".into());
            }
            self.session.append("turn/start", json!({"turn":1}), None)?;
            self.session.append(
                "assistant/message",
                json!({"turn":1,"step":1,"message":{"id":"answer","role":"assistant","content":[{"type":"text","text":"done"}],"source":{"kind":"model","provider":"mock","model":"mock"}},"stream":[]}),
                Some(SurfaceIntent::append()),
            )?;
            self.session.append(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
                None,
            )?;
            Ok(())
        }
        fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
            Ok(())
        }
        fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
            Ok(())
        }
    }

    struct TestFactory {
        order: Option<Arc<Mutex<Vec<String>>>>,
        followup_error: bool,
    }

    impl AgentFactory for TestFactory {
        fn create_agent(
            &self,
            owner_ctx: Context,
            options: CreateAgentOptions,
        ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
            let order = self.order.clone();
            let followup_error = self.followup_error;
            Box::pin(async move {
                let id = options
                    .session_id
                    .ok_or("test factory requires a session id")?;
                let meta = options.meta.unwrap_or_default();
                let mut session_meta = serde_json::Map::new();
                if let Some(cwd) = meta.cwd {
                    session_meta.insert("cwd".into(), Value::String(cwd));
                }
                if let Some(parent) = meta.parent_session {
                    session_meta.insert("parentSession".into(), Value::String(parent.to_string()));
                }
                if let Some(is_seeded) = meta.is_seeded {
                    session_meta.insert("isSeeded".into(), Value::Bool(is_seeded));
                }
                if let Some(origin) = meta.origin {
                    session_meta.insert("origin".into(), Value::String(origin));
                }
                if let Some(depth) = meta.delegation_depth {
                    session_meta.insert("delegationDepth".into(), Value::from(depth));
                }
                if let Some(preset) = meta.agent_preset {
                    session_meta.insert("agentPreset".into(), Value::String(preset));
                }
                let session = owner_ctx
                    .get::<Sessions>()
                    .ok_or("test factory requires sessions")?
                    .create(
                        &owner_ctx,
                        Some(id.clone()),
                        CreateSessionOptions {
                            meta: Some(Value::Object(session_meta)),
                            ..CreateSessionOptions::default()
                        },
                    )?;
                let agent: Arc<dyn Agent> = Arc::new(TestAgent {
                    id,
                    session,
                    ctx: owner_ctx,
                    scope: ScopeKey::new(),
                    options: options.agent_options.unwrap_or_default(),
                    model: TestModel::default(),
                    inbox: TestInbox,
                    order,
                    followup_error,
                });
                Ok(AgentHandle::new(agent, || Box::pin(async { Ok(()) })))
            })
        }

        fn resume(
            &self,
            _: Context,
            _: ResumeAgentOptions,
        ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
            Box::pin(async { Err("resume is unused by this fixture".into()) })
        }
    }

    struct PresencePersistence(SessionPersistenceIdentity);

    impl PresencePersistence {
        fn new() -> Self {
            Self(SessionPersistenceIdentity::new())
        }
    }

    impl SessionPersistence for PresencePersistence {
        fn identity(&self) -> SessionPersistenceIdentity {
            self.0.clone()
        }

        fn create<'a>(
            &'a self,
            _: &'a SessionHeader,
            _: Option<&'a SessionPersistenceCreateOptions>,
        ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
            Box::pin(async { Err("test persistence create is unused".into()) })
        }

        fn open<'a>(
            &'a self,
            _: &'a SessionId,
            _: SessionAccess,
            _: Option<&'a SessionPersistenceOpenOptions>,
        ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
            Box::pin(async { Err("test persistence open is unused".into()) })
        }

        fn flush(&self) -> BoxFuture<'_, Result<(), BoxError>> {
            Box::pin(async { Ok(()) })
        }

        fn stat<'a>(
            &'a self,
            _: &'a SessionId,
            _: Option<&'a SessionPersistenceStatOptions>,
        ) -> BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>, BoxError>> {
            Box::pin(async { Ok(None) })
        }

        fn list<'a>(
            &'a self,
            _: Option<&'a SessionPersistenceListOptions>,
        ) -> BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>, BoxError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    struct EmptySearchProvider;

    impl SessionSearchProvider for EmptySearchProvider {
        fn search_sessions<'a>(
            &'a self,
            _: SessionSearchRequest,
            _: SessionSearchExecContext,
        ) -> BoxFuture<'a, Result<SessionSearchPage<SessionSearchHit>, BoxError>> {
            Box::pin(async {
                Ok(SessionSearchPage {
                    items: Vec::new(),
                    next_cursor: None,
                })
            })
        }

        fn search_events<'a>(
            &'a self,
            request: SessionEventSearchRequest,
            _: SessionSearchExecContext,
        ) -> BoxFuture<'a, Result<SessionEventSearchPage, BoxError>> {
            Box::pin(async move {
                Ok(SessionEventSearchPage {
                    session: SessionHeader::new(request.session_id, 0),
                    items: Vec::new(),
                    next_cursor: None,
                })
            })
        }
    }

    struct ResumeFactory {
        session: Session,
        calls: Arc<Mutex<Vec<SessionId>>>,
        not_found: bool,
    }

    impl AgentFactory for ResumeFactory {
        fn create_agent(
            &self,
            _: Context,
            _: CreateAgentOptions,
        ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
            Box::pin(async { Err("create is unused by the resume fixture".into()) })
        }

        fn resume(
            &self,
            owner_ctx: Context,
            options: ResumeAgentOptions,
        ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(options.resume_session_id.clone());
            let session = self.session.clone();
            let not_found = self.not_found;
            Box::pin(async move {
                if not_found {
                    return Err(Box::new(hoh_session_query::SessionQueryError::new(
                        "gone during resume",
                        SessionQueryErrorCode::SessionNotFound,
                    )) as BoxError);
                }
                let agent: Arc<dyn Agent> = Arc::new(TestAgent {
                    id: session.id().clone(),
                    session,
                    ctx: owner_ctx.clone(),
                    scope: ScopeKey::new(),
                    options: options.agent_options.unwrap_or_default(),
                    model: TestModel::default(),
                    inbox: TestInbox,
                    order: None,
                    followup_error: false,
                });
                if let Some(setup) = options.setup
                    && let Some(commit) = setup(owner_ctx, agent.clone()).await?
                {
                    commit()?;
                }
                Ok(AgentHandle::new(agent, || Box::pin(async { Ok(()) })))
            })
        }
    }

    async fn mount_agent_registry(ctx: &Context) -> Service<Agents> {
        for fiber in [
            ctx.plugin::<SessionPlugin>(json!({})).unwrap(),
            ctx.plugin::<SessionProjectionRegistry>(json!({})).unwrap(),
        ] {
            fiber.settled().await.unwrap();
        }
        ctx.plugin::<AgentRegistryPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.get::<Agents>().unwrap()
    }

    async fn mount_resolve_services(ctx: &Context) -> (Service<Agents>, SessionStore) {
        let agents = mount_agent_registry(ctx).await;
        ctx.provide::<SessionPersistenceService>(Arc::new(PresencePersistence::new()))
            .await
            .unwrap();
        let sessions = ctx.get::<Sessions>().unwrap();
        mount_session_query(
            ctx,
            sessions.clone(),
            Arc::new(EmptySearchProvider),
            SessionQueryConfig::default(),
        )
        .await
        .unwrap();
        (agents, (*sessions).clone())
    }

    async fn mount_run_deps(ctx: &Context, factory: Arc<dyn AgentFactory>) -> Deps {
        let agents = mount_agent_registry(ctx).await;
        ctx.plugin::<AgentDefaultModelConfig>(json!({
            "provider":"mock",
            "model":"mock"
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();
        let _factory = agents.set_factory(ctx, factory).unwrap();
        Deps {
            agents,
            default_model: ctx.get::<AgentDefaultModel>().unwrap(),
            sessions: ctx.get::<Sessions>().unwrap(),
        }
    }

    fn captured_runtime(stderr: Arc<Mutex<String>>) -> HeadlessRuntime {
        HeadlessRuntime {
            output: Output::default(),
            stdout: Arc::new(|_: &str| {}),
            stderr: Arc::new(move |text: &str| {
                stderr
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(text);
            }),
            stdin_is_tty: Arc::new(|| false),
            read_stdin: Arc::new(|| {
                Box::pin(async { Ok(String::new()) }) as BoxFuture<'static, std::io::Result<String>>
            }),
        }
    }

    #[test]
    fn config_schema_rejects_unknown_fields_and_bad_policy() {
        let schema = schemars::schema_for!(Config);
        let value = serde_json::to_value(schema).unwrap();
        assert_eq!(
            value.pointer("/additionalProperties"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            serde_json::from_value::<Config>(json!({"approvalPolicy":"sometimes"}))
                .unwrap_err()
                .to_string(),
            "unknown variant `sometimes`, expected `ask` or `never`"
        );
    }

    #[test]
    fn empty_reasoning_delta_does_not_close_the_open_line() {
        let stderr = Arc::new(Mutex::new(String::new()));
        let runtime = captured_runtime(stderr.clone());
        let mut state = ReasoningState {
            open: false,
            ends_with_newline: true,
        };
        for text in ["abc", "", "def"] {
            write_reasoning_chunk(
                &mut state,
                &runtime,
                &StreamChunk::ReasoningDelta {
                    index: 0,
                    text: text.into(),
                    wire: Default::default(),
                },
            );
        }
        assert_eq!(
            *stderr
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "dsh: reasoning:\nabcdef"
        );
    }

    #[test]
    fn resume_not_found_uses_the_public_headless_message() {
        let id = SessionId::from("missing");
        let error: BoxError = Box::new(hoh_session_query::SessionQueryError::new(
            "gone during resume",
            SessionQueryErrorCode::SessionNotFound,
        ));
        assert_eq!(
            map_resume_error(error, &id).to_string(),
            "session \"missing\" does not exist; omit --session-id to start a new Session"
        );
    }

    fn test_selection() -> ModelSelection {
        ModelSelection {
            provider: "mock-provider".into(),
            model: "mock-model".into(),
            reasoning_effort: None,
        }
    }

    #[tokio::test]
    async fn resolve_agent_requires_persistence_before_query() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let agents = mount_agent_registry(&ctx).await;
        let id = SessionId::from("required-services");
        let selection = test_selection();

        let error = resolve_agent(
            &ctx,
            &agents,
            id.clone(),
            model_options(&selection),
            selection.clone(),
            "/work",
        )
        .await
        .map(|_| ())
        .expect_err("missing persistence must fail");
        assert_eq!(
            error.to_string(),
            "headless --session-id requires the sessionPersistence service; the Session would not survive this process"
        );

        ctx.provide::<SessionPersistenceService>(Arc::new(PresencePersistence::new()))
            .await
            .unwrap();
        let error = resolve_agent(
            &ctx,
            &agents,
            id,
            model_options(&selection),
            selection,
            "/work",
        )
        .await
        .map(|_| ())
        .expect_err("missing query must fail");
        assert_eq!(
            error.to_string(),
            "headless --session-id requires the sessionQuery service; dsh-base provides it"
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn resolve_agent_rejects_live_and_observed_incompatible_sessions_before_resume() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let (agents, store) = mount_resolve_services(&ctx).await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let returned = standalone_session("returned", Some("/work"));
        let _factory = agents
            .set_factory(
                &ctx,
                Arc::new(ResumeFactory {
                    session: returned,
                    calls: calls.clone(),
                    not_found: false,
                }),
            )
            .unwrap();
        let selection = test_selection();

        let live = store
            .create(
                &ctx,
                Some(SessionId::from("live")),
                CreateSessionOptions {
                    meta: Some(json!({"cwd":"/work"})),
                    ..CreateSessionOptions::default()
                },
            )
            .unwrap();
        let detach = agents.enter(&ctx, test_agent(&ctx, live)).unwrap();
        let error = resolve_agent(
            &ctx,
            &agents,
            SessionId::from("live"),
            model_options(&selection),
            selection.clone(),
            "/work",
        )
        .await
        .map(|_| ())
        .expect_err("live session must fail");
        assert_eq!(
            error.to_string(),
            "session \"live\" is live in this process, so the one-shot runner cannot own an exclusive run interval"
        );
        detach.detach();

        store
            .create(
                &ctx,
                Some(SessionId::from("foreign")),
                CreateSessionOptions {
                    meta: Some(json!({"cwd":"/elsewhere"})),
                    ..CreateSessionOptions::default()
                },
            )
            .unwrap();
        let error = resolve_agent(
            &ctx,
            &agents,
            SessionId::from("foreign"),
            model_options(&selection),
            selection,
            "/work",
        )
        .await
        .map(|_| ())
        .expect_err("foreign observed session must fail");
        assert_eq!(
            error.to_string(),
            "session \"foreign\" was recorded in \"/elsewhere\", not \"/work\""
        );
        assert!(
            calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn resolve_agent_maps_missing_observation_and_resume_race() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let (agents, store) = mount_resolve_services(&ctx).await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let _factory = agents
            .set_factory(
                &ctx,
                Arc::new(ResumeFactory {
                    session: standalone_session("race", Some("/work")),
                    calls: calls.clone(),
                    not_found: true,
                }),
            )
            .unwrap();
        let selection = test_selection();

        let error = resolve_agent(
            &ctx,
            &agents,
            SessionId::from("missing"),
            model_options(&selection),
            selection.clone(),
            "/work",
        )
        .await
        .map(|_| ())
        .expect_err("missing observation must fail");
        assert_eq!(
            error.to_string(),
            "session \"missing\" does not exist; omit --session-id to start a new Session"
        );
        assert!(
            calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );

        store
            .create(
                &ctx,
                Some(SessionId::from("race")),
                CreateSessionOptions {
                    meta: Some(json!({"cwd":"/work"})),
                    ..CreateSessionOptions::default()
                },
            )
            .unwrap();
        let error = resolve_agent(
            &ctx,
            &agents,
            SessionId::from("race"),
            model_options(&selection),
            selection,
            "/work",
        )
        .await
        .map(|_| ())
        .expect_err("resume race must fail");
        assert_eq!(
            error.to_string(),
            "session \"race\" does not exist; omit --session-id to start a new Session"
        );
        assert_eq!(
            *calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [SessionId::from("race")]
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn resolve_agent_configures_resumed_model_and_rechecks_adoption() {
        for (returned_cwd, expected_error) in [(Some("/elsewhere"), true), (Some("/work"), false)] {
            let kernel = hoh_kernel::Kernel::start();
            let ctx = kernel.root();
            let (agents, store) = mount_resolve_services(&ctx).await;
            store
                .create(
                    &ctx,
                    Some(SessionId::from("resumed")),
                    CreateSessionOptions {
                        meta: Some(json!({"cwd":"/work"})),
                        ..CreateSessionOptions::default()
                    },
                )
                .unwrap();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let _factory = agents
                .set_factory(
                    &ctx,
                    Arc::new(ResumeFactory {
                        session: standalone_session("resumed", returned_cwd),
                        calls: calls.clone(),
                        not_found: false,
                    }),
                )
                .unwrap();
            let selection = test_selection();
            let result = resolve_agent(
                &ctx,
                &agents,
                SessionId::from("resumed"),
                model_options(&selection),
                selection.clone(),
                "/work",
            )
            .await;
            if expected_error {
                assert_eq!(
                    result
                        .map(|_| ())
                        .expect_err("post-resume adoption must fail")
                        .to_string(),
                    "session \"resumed\" was recorded in \"/elsewhere\", not \"/work\""
                );
            } else {
                let agent = result.unwrap();
                assert_eq!(agent.model().current(), Some(selection));
            }
            assert_eq!(
                *calls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                [SessionId::from("resumed")]
            );
            kernel.shutdown().await;
        }
    }

    fn standalone_session(id: &str, cwd: Option<&str>) -> Session {
        let id = SessionId::from(id);
        let mut header = SessionHeader::new(id.clone(), 0);
        header.cwd = cwd.map(str::to_owned);
        Session::create(
            id,
            None,
            Some(serde_json::to_value(header).unwrap()),
            None,
            None,
        )
        .unwrap()
    }

    fn test_agent(ctx: &Context, session: Session) -> Arc<dyn Agent> {
        Arc::new(TestAgent {
            id: session.id().clone(),
            session,
            ctx: ctx.clone(),
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            model: TestModel::default(),
            inbox: TestInbox,
            order: None,
            followup_error: false,
        })
    }

    #[test]
    fn adoption_rejects_every_incompatible_session_fact() {
        let preset = standalone_session("preset", Some("/work"));
        let mut preset_header: Value = (*preset.header()).clone();
        preset_header["agentPreset"] = Value::String("reviewer".into());
        let preset = Session::create(
            SessionId::from("preset-header"),
            None,
            Some({
                preset_header["id"] = Value::String("preset-header".into());
                preset_header
            }),
            None,
            None,
        )
        .unwrap();
        assert!(
            assert_adoptable(&preset, "/work")
                .unwrap_err()
                .to_string()
                .contains("agent preset \"reviewer\"")
        );

        let selected = standalone_session("selected", Some("/work"));
        selected
            .append(
                "agent-preset/selected",
                json!({"agentPreset":"strict"}),
                None,
            )
            .unwrap();
        assert!(
            assert_adoptable(&selected, "/work")
                .unwrap_err()
                .to_string()
                .contains("agent preset \"strict\"")
        );

        let malformed = standalone_session("malformed", Some("/work"));
        malformed
            .append("agent-preset/selected", json!({"agentPreset":""}), None)
            .unwrap();
        assert!(
            assert_adoptable(&malformed, "/work")
                .unwrap_err()
                .to_string()
                .contains("malformed agent-preset/selected")
        );

        let subagent = standalone_session("subagent", Some("/work"));
        let mut header: Value = (*subagent.header()).clone();
        header["id"] = Value::String("subagent-header".into());
        header["origin"] = Value::String("subagent".into());
        let subagent = Session::create(
            SessionId::from("subagent-header"),
            None,
            Some(header),
            None,
            None,
        )
        .unwrap();
        assert!(
            assert_adoptable(&subagent, "/work")
                .unwrap_err()
                .to_string()
                .contains("subagent session")
        );

        let missing = standalone_session("missing-cwd", None);
        assert!(
            assert_adoptable(&missing, "/work")
                .unwrap_err()
                .to_string()
                .contains("recorded no working directory")
        );
        let foreign = standalone_session("foreign-cwd", Some("/elsewhere"));
        assert!(
            assert_adoptable(&foreign, "/work")
                .unwrap_err()
                .to_string()
                .contains("not \"/work\"")
        );
        assert!(assert_adoptable(&standalone_session("valid", Some("/work")), "/work").is_ok());
    }

    #[tokio::test]
    async fn terminal_failure_tracking_is_agent_scoped_and_recovers_only_after_turn_end() {
        let kernel = hoh_kernel::Kernel::start();
        let root_session = standalone_session("root-failure", Some("/work"));
        let other_session = standalone_session("other-failure", Some("/work"));
        let root_agent = test_agent(&kernel.root(), root_session.clone());
        let other_agent = test_agent(&kernel.root(), other_session.clone());
        let failure = Mutex::new(None);

        record_agent_failure(
            &failure,
            &root_agent,
            &AgentError {
                agent: other_agent,
                turn: 1,
                step: 1,
                error: hoh_error::SharedError::new("other failed"),
            },
        );
        assert!(
            failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );

        record_agent_failure(
            &failure,
            &root_agent,
            &AgentError {
                agent: root_agent.clone(),
                turn: 1,
                step: 1,
                error: hoh_error::SharedError::new("root failed"),
            },
        );
        let other_end = other_session
            .append(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
                None,
            )
            .unwrap();
        clear_terminal_failure(&failure, &root_session, &other_session, &other_end);
        assert!(
            failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
        );

        let root_end = root_session
            .append(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
                None,
            )
            .unwrap();
        clear_terminal_failure(&failure, &root_session, &root_session, &root_end);
        assert!(
            failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );

        record_agent_failure(
            &failure,
            &root_agent,
            &AgentError {
                agent: root_agent.clone(),
                turn: 2,
                step: 1,
                error: hoh_error::SharedError::new("late failure"),
            },
        );
        clear_terminal_failure(&failure, &root_session, &root_session, &root_end);
        assert!(
            failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
        );
        let recovered = root_session
            .append(
                "turn/end",
                json!({"turn":2,"reason":{"kind":"completed"}}),
                None,
            )
            .unwrap();
        clear_terminal_failure(&failure, &root_session, &root_session, &recovered);
        assert!(
            failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn summary_ignores_prefix_and_uses_last_started_turn() {
        let kernel = hoh_kernel::Kernel::start();
        let store = SessionStore::new();
        let session = store
            .create(
                &kernel.root(),
                Some("session-test".into()),
                CreateSessionOptions::default(),
            )
            .unwrap();
        session
            .append(
                "assistant/message",
                json!({"turn":0,"step":0,"message":{"id":"a-old","role":"assistant","content":[{"type":"text","text":"old"}],"source":{"kind":"model","provider":"mock","model":"mock"}},"stream":[]}),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        let first = session.seq();
        session
            .append("turn/start", json!({"turn":1}), None)
            .unwrap();
        session
            .append(
                "assistant/message",
                json!({"turn":1,"step":1,"message":{"id":"a-answer","role":"assistant","content":[{"type":"text","text":"answer"}],"source":{"kind":"model","provider":"mock","model":"mock"}},"stream":[]}),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        session
            .append(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
                None,
            )
            .unwrap();
        let outcome = summarize(&session, first).unwrap();
        assert_eq!(outcome.text, "answer");
        assert_eq!(outcome.reason, Some(json!({"kind":"completed"})));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn summary_does_not_reuse_an_earlier_terminal_reason() {
        let kernel = hoh_kernel::Kernel::start();
        let session = SessionStore::new()
            .create(
                &kernel.root(),
                Some("session-reset".into()),
                CreateSessionOptions::default(),
            )
            .unwrap();
        let first = session.seq();
        for (turn, text) in [(1, "first"), (2, "latest")] {
            session
                .append("turn/start", json!({"turn":turn}), None)
                .unwrap();
            session
                .append(
                    "assistant/message",
                    json!({"turn":turn,"step":1,"message":{"id":format!("a-{turn}"),"role":"assistant","content":[{"type":"text","text":text}],"source":{"kind":"model","provider":"mock","model":"mock"}},"stream":[]}),
                    Some(SurfaceIntent::append()),
                )
                .unwrap();
            if turn == 1 {
                session
                    .append(
                        "turn/end",
                        json!({"turn":turn,"reason":{"kind":"completed"}}),
                        None,
                    )
                    .unwrap();
            }
        }
        assert_eq!(
            summarize(&session, first).unwrap(),
            RunOutcome {
                text: "latest".into(),
                reason: None,
            }
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn run_exercises_permission_approval_and_add_dir_guards() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let deps = mount_run_deps(
            &ctx,
            Arc::new(TestFactory {
                order: None,
                followup_error: false,
            }),
        )
        .await;
        let runtime = captured_runtime(Arc::new(Mutex::new(String::new())));

        let error = run(
            ctx.clone(),
            Config {
                task: Some("work".into()),
                permission_mode: Some("workspace-write".into()),
                ..Config::default()
            },
            deps.clone(),
            runtime.clone(),
        )
        .await
        .map(|_| ())
        .expect_err("permission mode without its service must fail");
        assert_eq!(
            error.to_string(),
            "headless --permission-mode requires the permissionPresets service"
        );

        let error = run(
            ctx.clone(),
            Config {
                task: Some("work".into()),
                approval_policy: Some(ApprovalPolicy::Never),
                ..Config::default()
            },
            deps.clone(),
            runtime.clone(),
        )
        .await
        .map(|_| ())
        .expect_err("approval policy without its service must fail");
        assert_eq!(
            error.to_string(),
            "headless --approval-policy requires the approval service"
        );

        ctx.provide::<HeadlessStartupKey>(Arc::new(crate::startup::HeadlessStartup {
            task: Some("work".into()),
            session_id: None,
            json: false,
            permission_mode: None,
            approval_policy: None,
            additional_writable_roots: Some(vec!["/extra".into()]),
        }))
        .await
        .unwrap();
        let error = run(ctx.clone(), Config::default(), deps, runtime)
            .await
            .map(|_| ())
            .expect_err("additional directory without sandbox policy must fail");
        assert_eq!(
            error.to_string(),
            "headless --add-dir requires the sandbox-policy row to include headlessStartup.additionalWritableRoots; check the profile overlays"
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn run_json_mode_projects_session_and_final_before_success_exit() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let deps = mount_run_deps(
            &ctx,
            Arc::new(TestFactory {
                order: None,
                followup_error: false,
            }),
        )
        .await;
        let exits = Arc::new(Mutex::new(Vec::new()));
        let observed_exits = exits.clone();
        ctx.provide::<AppExitKey>(Arc::new(AppExit::new(move |code| {
            observed_exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(code);
        })))
        .await
        .unwrap();
        let stdout = Arc::new(Mutex::new(String::new()));
        let observed_stdout = stdout.clone();
        let runtime = HeadlessRuntime {
            output: Output::default(),
            stdout: Arc::new(move |text: &str| {
                observed_stdout
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(text);
            }),
            stderr: Arc::new(|_: &str| {}),
            stdin_is_tty: Arc::new(|| false),
            read_stdin: Arc::new(|| {
                Box::pin(async { Ok(String::new()) }) as BoxFuture<'static, std::io::Result<String>>
            }),
        };

        run(
            ctx.clone(),
            Config {
                task: Some("work".into()),
                json: true,
                ..Config::default()
            },
            deps,
            runtime,
        )
        .await
        .unwrap();

        let records: Vec<Value> = stdout
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.first().unwrap()["type"], "session");
        assert_eq!(records.last().unwrap()["type"], "final");
        assert_eq!(records.last().unwrap()["text"], "done");
        assert_eq!(
            *exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [0]
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn run_stops_before_flush_output_and_exit_when_followup_fails() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let order = Arc::new(Mutex::new(Vec::new()));
        let deps = mount_run_deps(
            &ctx,
            Arc::new(TestFactory {
                order: Some(order.clone()),
                followup_error: true,
            }),
        )
        .await;
        let stdout = Arc::new(Mutex::new(String::new()));
        let observed_stdout = stdout.clone();
        let runtime = HeadlessRuntime {
            output: Output::default(),
            stdout: Arc::new(move |text: &str| {
                observed_stdout
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(text);
            }),
            stderr: Arc::new(|_: &str| {}),
            stdin_is_tty: Arc::new(|| false),
            read_stdin: Arc::new(|| {
                Box::pin(async { Ok(String::new()) }) as BoxFuture<'static, std::io::Result<String>>
            }),
        };

        let error = run(
            ctx.clone(),
            Config {
                task: Some("work".into()),
                ..Config::default()
            },
            deps,
            runtime,
        )
        .await
        .map(|_| ())
        .expect_err("followup failure must escape run");
        assert_eq!(error.to_string(), "followup failed");
        assert!(
            stdout
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["idle", "followup"]
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn runner_reports_run_failure_and_requests_error_exit() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        mount_run_deps(
            &ctx,
            Arc::new(TestFactory {
                order: None,
                followup_error: true,
            }),
        )
        .await;
        let exits = Arc::new(Mutex::new(Vec::new()));
        let observed_exits = exits.clone();
        ctx.provide::<AppExitKey>(Arc::new(AppExit::new(move |code| {
            observed_exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(code);
        })))
        .await
        .unwrap();
        let stdout = Arc::new(Mutex::new(String::new()));
        let stderr = Arc::new(Mutex::new(String::new()));
        let observed_stdout = stdout.clone();
        let observed_stderr = stderr.clone();
        ctx.provide::<HeadlessRuntimeKey>(Arc::new(HeadlessRuntime {
            output: Output::default(),
            stdout: Arc::new(move |text: &str| {
                observed_stdout
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(text);
            }),
            stderr: Arc::new(move |text: &str| {
                observed_stderr
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(text);
            }),
            stdin_is_tty: Arc::new(|| false),
            read_stdin: Arc::new(|| {
                Box::pin(async { Ok(String::new()) }) as BoxFuture<'static, std::io::Result<String>>
            }),
        }))
        .await
        .unwrap();

        let runner = ctx
            .plugin::<HeadlessRunner>(json!({"task":"work"}))
            .unwrap();
        runner.settled().await.unwrap();
        for _ in 0..100 {
            if !exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            *exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [1]
        );
        assert!(
            stdout
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        assert_eq!(
            *stderr
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "dsh: followup failed\n"
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn run_orders_idle_followup_settlement_flush_output_and_exit() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        for fiber in [
            ctx.plugin::<SessionPlugin>(json!({})).unwrap(),
            ctx.plugin::<SessionProjectionRegistry>(json!({})).unwrap(),
        ] {
            fiber.settled().await.unwrap();
        }
        ctx.plugin::<AgentRegistryPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<AgentDefaultModelConfig>(json!({
            "provider":"mock",
            "model":"mock"
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();
        let agents = ctx.get::<Agents>().unwrap();
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let _factory = agents
            .set_factory(
                &ctx,
                Arc::new(TestFactory {
                    order: Some(order.clone()),
                    followup_error: false,
                }),
            )
            .unwrap();
        let flush_order = order.clone();
        SessionStore::on_flush(&ctx, move |_| {
            let order = flush_order.clone();
            async move {
                order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("flush".into());
                Ok(())
            }
        })
        .unwrap();
        let exits = Arc::new(Mutex::new(Vec::new()));
        let observed_exits = exits.clone();
        let exit_order = order.clone();
        ctx.provide::<AppExitKey>(Arc::new(AppExit::new(move |code| {
            exit_order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("exit".into());
            observed_exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(code);
        })))
        .await
        .unwrap();
        let stdout = Arc::new(Mutex::new(String::new()));
        let observed_stdout = stdout.clone();
        let stdout_order = order.clone();
        let runtime = HeadlessRuntime {
            output: Output::default(),
            stdout: Arc::new(move |text: &str| {
                stdout_order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("stdout".into());
                observed_stdout
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(text);
            }),
            stderr: Arc::new(|_: &str| {}),
            stdin_is_tty: Arc::new(|| false),
            read_stdin: Arc::new(|| {
                Box::pin(async { Ok(String::new()) }) as BoxFuture<'static, std::io::Result<String>>
            }),
        };
        let deps = Deps {
            agents,
            default_model: ctx.get::<AgentDefaultModel>().unwrap(),
            sessions: ctx.get::<Sessions>().unwrap(),
        };

        run(
            ctx.clone(),
            Config {
                task: Some("work".into()),
                ..Config::default()
            },
            deps,
            runtime,
        )
        .await
        .unwrap();

        assert_eq!(
            *stdout
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "done\n"
        );
        assert_eq!(
            *exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [0]
        );
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["idle", "followup", "idle", "flush", "stdout", "exit"]
        );
        kernel.shutdown().await;
    }
}
