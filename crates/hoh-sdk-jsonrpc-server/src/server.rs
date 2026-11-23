//! packages/sdk/server/src/server.ts. One SDK owner over an existing harness runtime.
use crate::{
    HohRpcMethods,
    error::{ForeignErrorValue, SdkServerError, error, type_error},
    lock,
    query_activity::{QUERY_INVOCATION, QueryActivity},
    query_content::{query_command_images, query_content},
    query_input::query_command,
    shim::{ScheduleResolveAgent, SubagentRunEndInfo, Subagents, directory_agent},
    transport::JsonRpcTransportPeer,
    user_questions::SdkUserQuestionBridge,
};
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared, join_all},
};
use hoh_agent::{
    Agent, AgentAssistantStream, AgentError, AgentHandle, AgentOptions, AgentSessionMeta,
    AgentStatus, AgentStatusChanged, Agents, AssistantStreamFrame as AgentAssistantStreamFrame,
    AssistantStreamOutcome as AgentAssistantStreamOutcome, CreateAgentOptions, Live,
    ModelSelection, ResumeAgentOptions,
};
use hoh_attachment::{
    AttachmentError, AttachmentErrorCode, Attachments, EncodedImageAttachment, ImageMediaType,
    admit_encoded_images,
};
use hoh_commands::{
    CommandExecution as NativeCommandExecution, CommandResult as NativeCommandResult, Commands,
};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_goal::Goals;
use hoh_kernel::{Context, Disposer, Fiber, Inject, Service, Undo};
use hoh_llm::{
    ContentBlock, Llm, LlmCallConfig, LlmCallControls, MessageSource, NewUserMessage,
    ReasoningEffortId, StreamChunk, create_user_message,
};
use hoh_protocol::{
    ApprovalHost, HohInitializeResult, InitializeParams, InitializeResult, SecretString,
    ServerInfo, SessionAssistantStreamChunk, SessionAssistantStreamFrame,
    SessionAssistantStreamNotification, SessionAssistantStreamOutcome, SessionPromptParams,
    SessionPromptResult, SessionQueryParams, SessionQueryResult, SessionResumeParams,
    SessionWaitParams, SessionWorkingDirectoryParams, SessionWorkingDirectoryResult,
    SessionWorkingDirectorySetParams,
};
use hoh_session::{SessionCreated, SessionEventEmitted, SessionId, SessionLogOffset};
use hoh_session_persistence::SessionPersistenceService;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_util_values::js_trim;
use hoh_working_directory::WorkingDirectoryKey;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

mod session_rpc;

const HOH_PROTOCOL_VERSION: u32 = 1;

/// The validated host-approval part of `initialize`. The key is never logged or echoed.
struct HostHandshake {
    key: SecretString,
}
impl HostHandshake {
    /// `Ok(None)` is the plain SDK handshake; every host-mode rule fails before any state change.
    fn from_params(params: &InitializeParams) -> Result<Option<Self>, BoxError> {
        let host = params
            .capabilities
            .as_ref()
            .is_some_and(|c| c.approvals.is_some());
        let hoh = params.hoh.as_ref();
        if hoh.is_some_and(|h| h.protocol_version != HOH_PROTOCOL_VERSION) {
            return Err(error(format!(
                "unsupported hoh.protocolVersion; this server speaks {HOH_PROTOCOL_VERSION}"
            )));
        }
        let key = hoh.and_then(|h| h.approval_key.as_ref());
        if !host {
            return if key.is_some() {
                Err(error("approvalKey requires host approvals"))
            } else {
                Ok(None)
            };
        }
        if hoh.is_none() {
            return Err(error(format!(
                "host approvals require hoh.protocolVersion {HOH_PROTOCOL_VERSION}"
            )));
        }
        let key = key.ok_or_else(|| error("host approvals require approvalKey"))?;
        hoh_protocol::approval::key_from_hex(key.expose()).map_err(error)?;
        Ok(Some(Self { key: key.clone() }))
    }
}

/// Lets the approval service reach the window through this server's transport without a
/// dependency from `hoh-user-approval` back to this crate.
struct TransportApprovalPeer(Arc<dyn JsonRpcTransportPeer>);
impl hoh_user_approval::HostApprovalPeer for TransportApprovalPeer {
    fn request(
        &self,
        method: &'static str,
        params: Value,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'static, Result<Value, BoxError>> {
        self.0.request(method, params, signal)
    }
    fn notify(&self, method: &'static str, params: Value) -> Result<(), BoxError> {
        self.0.notify(method, Some(params))
    }
}

fn protocol_command_execution(execution: NativeCommandExecution) -> hoh_protocol::CommandExecution {
    let result = match execution.result {
        NativeCommandResult::Success {
            text,
            source_event_seq,
        } => hoh_protocol::CommandResult::Success {
            text,
            source_event_seq: source_event_seq.map(|seq| seq.get()),
        },
        NativeCommandResult::Error { text } => hoh_protocol::CommandResult::Error { text },
    };
    hoh_protocol::CommandExecution {
        command_id: execution.command_id.to_string(),
        result,
    }
}

fn assistant_stream_frame(
    frame: &AgentAssistantStreamFrame,
) -> Option<SessionAssistantStreamFrame> {
    Some(match frame {
        AgentAssistantStreamFrame::Start {
            attempt_id,
            revision,
            turn,
            step,
        } => SessionAssistantStreamFrame::Start {
            attempt_id: attempt_id.to_string(),
            revision: *revision,
            turn: *turn,
            step: *step,
        },
        AgentAssistantStreamFrame::Chunk {
            attempt_id,
            revision,
            index,
            time,
            chunk: StreamChunk::TextDelta { text, .. },
        } => SessionAssistantStreamFrame::Chunk {
            attempt_id: attempt_id.to_string(),
            revision: *revision,
            index: *index,
            time: *time,
            chunk: SessionAssistantStreamChunk::TextDelta { text: text.clone() },
        },
        AgentAssistantStreamFrame::Chunk { .. } => return None,
        AgentAssistantStreamFrame::End {
            attempt_id,
            revision,
            index,
            outcome,
        } => SessionAssistantStreamFrame::End {
            attempt_id: attempt_id.to_string(),
            revision: *revision,
            index: *index,
            outcome: match outcome {
                AgentAssistantStreamOutcome::Committed { .. } => {
                    SessionAssistantStreamOutcome::Committed
                }
                AgentAssistantStreamOutcome::Abandoned => SessionAssistantStreamOutcome::Abandoned,
            },
        },
    })
}

fn is_current_owned_agent<T: ?Sized>(
    owned: Option<&Arc<T>>,
    registered: Option<&Arc<T>>,
    event: &Arc<T>,
) -> bool {
    owned.is_some_and(|agent| Arc::ptr_eq(agent, event))
        && registered.is_some_and(|agent| Arc::ptr_eq(agent, event))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSdkJsonRpcServerOptions {
    #[serde(default)]
    pub max_tokens_as_success: bool,
}

struct SessionRecord {
    handle: AgentHandle,
    failure: Mutex<Option<(SharedError, SessionLogOffset)>>,
}
type Creation = Shared<BoxFuture<'static, Result<Arc<SessionRecord>, SharedError>>>;
type Shutdown = Shared<BoxFuture<'static, Result<Value, SharedError>>>;
#[derive(Clone)]
struct Route {
    cwd: String,
    provider: String,
    model: String,
    reasoning_effort: Option<ReasoningEffortId>,
    max_tokens: Option<u64>,
}
struct State {
    route: Route,
    initialized: bool,
    initializing: bool,
    host_approvals: bool,
    shutting_down: bool,
    approval_requests: bool,
    sessions: IndexMap<SessionId, Arc<SessionRecord>>,
    creations: IndexMap<SessionId, Creation>,
    scheduled: IndexMap<SessionId, Creation>,
    activities: IndexMap<u64, Arc<QueryActivity>>,
    command_tasks: IndexMap<u64, Shared<BoxFuture<'static, ()>>>,
    next_task: u64,
    llm_fiber: Option<Fiber>,
    shutdown: Option<Shutdown>,
}
struct Inner {
    ctx: Context,
    transport: Arc<dyn JsonRpcTransportPeer>,
    options: HarnessSdkJsonRpcServerOptions,
    state: Mutex<State>,
    lifetime: AbortController,
    initialization: tokio::sync::watch::Sender<bool>,
    disposers: Mutex<Vec<Disposer>>,
    questions: Mutex<Option<SdkUserQuestionBridge>>,
    goal_lifetime: Mutex<Option<Fiber>>,
}
#[derive(Clone)]
pub struct HarnessSdkJsonRpcServer {
    inner: Arc<Inner>,
}
#[derive(Inject)]
struct GoalDeps {
    _goals: Service<Goals>,
}

impl HarnessSdkJsonRpcServer {
    pub fn new(
        ctx: Context,
        transport: Arc<dyn JsonRpcTransportPeer>,
        options: HarnessSdkJsonRpcServerOptions,
    ) -> Result<Self, BoxError> {
        let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
        let this = Self {
            inner: Arc::new(Inner {
                ctx: ctx.clone(),
                transport: transport.clone(),
                options,
                state: Mutex::new(State {
                    route: Route {
                        cwd,
                        provider: "deepseek-official".into(),
                        model: "deepseek-official".into(),
                        reasoning_effort: None,
                        max_tokens: None,
                    },
                    initialized: false,
                    initializing: false,
                    host_approvals: false,
                    shutting_down: false,
                    approval_requests: false,
                    sessions: IndexMap::new(),
                    creations: IndexMap::new(),
                    scheduled: IndexMap::new(),
                    activities: IndexMap::new(),
                    command_tasks: IndexMap::new(),
                    next_task: 0,
                    llm_fiber: None,
                    shutdown: None,
                }),
                lifetime: AbortController::new(),
                initialization: tokio::sync::watch::channel(false).0,
                disposers: Mutex::new(Vec::new()),
                questions: Mutex::new(None),
                goal_lifetime: Mutex::new(None),
            }),
        };
        let weak = Arc::downgrade(&this.inner);
        *lock(&this.inner.goal_lifetime) =
            Some(ctx.inject(move |ctx: Context, _deps: GoalDeps| {
                let weak = weak.clone();
                async move {
                    ctx.effect("jsonrpc.goalLifetime", move || {
                        Undo::sync(move || {
                            if let Some(inner) = weak.upgrade() {
                                let activities = lock(&inner.state)
                                    .activities
                                    .values()
                                    .cloned()
                                    .collect::<Vec<_>>();
                                for activity in activities {
                                    if let Err(e) = activity.goal_unavailable() {
                                        inner.ctx.logger().warn(e.to_string());
                                    }
                                }
                            }
                        })
                    })?;
                    Ok(())
                }
            })?);
        let weak = Arc::downgrade(&this.inner);
        lock(&this.inner.disposers).push(ctx.on_waterfall::<hoh_user_approval::ApprovalRequest, Value, _>(move |request, next| {
            let inner = weak.upgrade();
            async move {
                let Some(inner) = inner else { return next.run().await; };
                let this = Self { inner };
                let approval_requests = lock(&this.inner.state).approval_requests;
                if !approval_requests || !this.owns_agent(&request.agent)? { return next.run().await; }
                let params = hoh_protocol::ApprovalRequestParams {
                    session_id:request.agent.session().id().to_string(), tool_name:request.tool_name.clone(), call_id:request.call_id.as_ref().map(ToString::to_string), reason:request.reason.clone(), approval_id:None, risk:None, human_text:None, action_hash:None,
                };
                let signal = request.signal.as_ref().map_or_else(|| this.inner.lifetime.signal(), |s| AbortSignal::any([s.clone(), this.inner.lifetime.signal()]));
                let result = this.inner.transport.request("approval/request", serde_json::to_value(params)?, Some(signal)).await?;
                let outcome = result.as_object().and_then(|v| v.get("outcome")).and_then(Value::as_str);
                match outcome {
                    Some("allowed-once" | "rejected" | "cancelled") => Ok(Value::String(outcome.unwrap_or_default().into())),
                    _ => Err(type_error("approval/request response requires an allowed-once, rejected, or cancelled outcome")),
                }
            }
        })?);
        let weak = Arc::downgrade(&this.inner);
        *lock(&this.inner.questions) =
            Some(SdkUserQuestionBridge::new(&ctx, transport, move |agent| {
                weak.upgrade().is_some_and(|inner| {
                    lock(&inner.state)
                        .sessions
                        .get(agent.session().id())
                        .is_some_and(|r| Arc::ptr_eq(&r.handle.agent, agent))
                })
            })?);
        let weak = Arc::downgrade(&this.inner);
        lock(&this.inner.disposers).push(
            ctx.on_waterfall::<ScheduleResolveAgent, Arc<dyn Agent>, _>(move |event, _next| {
                let inner = weak.upgrade();
                async move {
                    let this = Self {
                        inner: inner.ok_or_else(|| error("SDK server is shutting down"))?,
                    };
                    let mut initialized = this.inner.initialization.subscribe();
                    initialized
                        .wait_for(|ready| *ready)
                        .await
                        .map_err(|_| error("SDK server is shutting down"))?;
                    let creation = {
                        let mut state = lock(&this.inner.state);
                        if state.shutting_down {
                            return Err(error("SDK server is shutting down"));
                        }
                        if let Some(record) = state.sessions.get(&event.session_id) {
                            let record = record.clone();
                            drop(state);
                            this.assert_live_agent(&record, &event.session_id)?;
                            return Ok(record.handle.agent.clone());
                        }
                        if let Some(pending) = state.creations.get(&event.session_id) {
                            pending.clone()
                        } else {
                            let creation = this.open_session_locked(
                                &mut state,
                                event.session_id.clone(),
                                true,
                            );
                            state
                                .scheduled
                                .insert(event.session_id.clone(), creation.clone());
                            creation
                        }
                    };
                    match creation.await {
                        Ok(rec) => Ok(rec.handle.agent.clone()),
                        Err(e) => {
                            lock(&this.inner.state)
                                .scheduled
                                .shift_remove(&event.session_id);
                            Err(e.into())
                        }
                    }
                }
            })?,
        );
        let weak = Arc::downgrade(&this.inner);
        lock(&this.inner.disposers).push(ctx.on_contained::<SessionEventEmitted>(
            move |event| {
                let Some(inner) = weak.upgrade() else {
                    return Ok(());
                };
                let rec = lock(&inner.state).sessions.get(event.session.id()).cloned();
                if event.event.event_type() == "turn/end"
                    && let Some(rec) = rec
                    && rec.handle.agent.session() == &event.session
                {
                    let mut failure = lock(&rec.failure);
                    if failure
                        .as_ref()
                        .is_some_and(|(_, offset)| event.event.seq().get() >= offset.get())
                    {
                        *failure = None;
                    }
                }
                inner.transport.notify(
                    "session.event",
                    Some(json!({"sessionId":event.session.id(), "event":event.event})),
                )
            },
        )?);
        let weak = Arc::downgrade(&this.inner);
        lock(&this.inner.disposers).push(ctx.on_contained::<AgentAssistantStream>(
            move |event| {
                let Some(inner) = weak.upgrade() else {
                    return Ok(());
                };
                let session_id = event.agent.session().id();
                let owned_agent = lock(&inner.state)
                    .sessions
                    .get(session_id)
                    .map(|rec| rec.handle.agent.clone());
                let registered_agent = inner
                    .ctx
                    .get::<Agents>()
                    .and_then(|agents| agents.get(event.agent.id()));
                if !is_current_owned_agent(
                    owned_agent.as_ref(),
                    registered_agent.as_ref(),
                    &event.agent,
                ) {
                    return Ok(());
                }
                let Some(frame) = assistant_stream_frame(&event.frame) else {
                    return Ok(());
                };
                inner.transport.notify(
                    "session.assistant-stream",
                    Some(serde_json::to_value(SessionAssistantStreamNotification {
                        session_id: session_id.to_string(),
                        frame,
                    })?),
                )
            },
        )?);
        let weak = Arc::downgrade(&this.inner);
        lock(&this.inner.disposers).push(ctx.on_contained::<AgentError>(move |event| {
            if let Some(inner) = weak.upgrade()
                && let Some(rec) = lock(&inner.state).sessions.get(event.agent.session().id())
                && Arc::ptr_eq(&rec.handle.agent, &event.agent)
            {
                *lock(&rec.failure) = Some((event.error.clone(), event.agent.session().seq()));
            }
            Ok(())
        })?);
        let weak = Arc::downgrade(&this.inner);
        lock(&this.inner.disposers).push(ctx.on_contained::<AgentStatusChanged>(move |event| {
            if let Some(inner) = weak.upgrade() { inner.transport.notify("session.status", Some(json!({"sessionId":event.agent.session().id(), "status":event.status.as_str()})))?; }
            Ok(())
        })?);
        let weak = Arc::downgrade(&this.inner);
        lock(&this.inner.disposers).push(ctx.on_contained::<SessionCreated>(move |event| {
            let header = event.session.header();
            if header["origin"] != "subagent" {
                return Ok(());
            }
            if let Some(parent) = header.get("parentSession")
                && let Some(inner) = weak.upgrade()
            {
                inner.transport.notify(
                    "subagent.started",
                    Some(json!({"parentSessionId":parent, "childSessionId":event.session.id()})),
                )?;
            }
            Ok(())
        })?);
        let weak = Arc::downgrade(&this.inner);
        lock(&this.inner.disposers).push(ctx.on_contained::<SubagentRunEndInfo>(move |event| {
            if !event.local { return Ok(()); }
            let Some(inner) = weak.upgrade() else { return Ok(()); };
            let mut payload = json!({"provider":event.provider, "agentId":event.id, "parentSessionId":event.parent_session_id, "childSessionId":event.id, "status":success_status(&event.stop_reason, &inner.options), "stopReason":event.stop_reason});
            if let Some(message) = &event.last_assistant_message { payload["lastAssistantMessage"] = serde_json::to_value(message)?; }
            inner.transport.notify("subagent.finished", Some(payload))
        })?);
        Ok(this)
    }

    /// The one-time handshake. A second call is an error, whatever the first one carried:
    /// the host approval key is set once (docs/window.md §4).
    pub async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult, BoxError> {
        {
            let mut state = lock(&self.inner.state);
            if state.initialized || state.initializing {
                return Err(error("SDK server is already initialized"));
            }
            state.initializing = true;
        }
        let result = self.initialize_once(params).await;
        if result.is_err() {
            lock(&self.inner.state).initializing = false;
        }
        result
    }
    async fn initialize_once(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResult, BoxError> {
        let host = HostHandshake::from_params(&params)?;
        let approval = match &host {
            Some(_) => Some(
                self.inner
                    .ctx
                    .get::<hoh_user_approval::Approval>()
                    .ok_or_else(|| error("host approvals require the approval service"))?,
            ),
            None => None,
        };
        if params
            .reasoning_effort
            .as_ref()
            .is_some_and(String::is_empty)
        {
            return Err(type_error(
                "initialize reasoningEffort must be a non-empty string",
            ));
        }
        let cwd = hoh_util_workspace_path::node_path::resolve(
            &std::env::current_dir()?.to_string_lossy(),
            &[&params.cwd],
        );
        let has_adapter = self
            .inner
            .ctx
            .get::<Llm>()
            .is_some_and(|llm| llm.list_providers().iter().any(|p| p.id == params.provider));
        if !has_adapter {
            if params.provider != "deepseek-official" {
                return Err(error(format!(
                    "no adapter registered for provider \"{}\"",
                    params.provider
                )));
            }
            let fiber = self
                .inner
                .ctx
                .plugin::<hoh_llm_deepseek_api_key::DeepSeekApiKey>(json!({}))?;
            fiber.settled().await?;
            lock(&self.inner.state).llm_fiber = Some(fiber);
        }
        let llm = self
            .inner
            .ctx
            .get::<Llm>()
            .ok_or_else(|| error("llm service is unavailable"))?;
        let reasoning_effort = params.reasoning_effort.map(ReasoningEffortId::from);
        let max_tokens = params.max_tokens.map(|n| n.get());
        llm.resolve_call_config(
            LlmCallConfig {
                provider: params.provider.clone(),
                model: params.model.clone(),
                controls: LlmCallControls {
                    reasoning_effort: reasoning_effort.clone(),
                    max_tokens,
                    ..LlmCallControls::default()
                },
            },
            None,
        )
        .await?;
        // Last fallible step: a failed initialize must leave no key behind for a retry.
        if let (Some(host), Some(approval)) = (&host, &approval) {
            approval.install_host_mode(
                host.key.expose(),
                Arc::new(TransportApprovalPeer(self.inner.transport.clone())),
            )?;
        }
        if let Some(working_directory) = self.inner.ctx.get::<WorkingDirectoryKey>() {
            working_directory.set_initial_default_directory(cwd.clone());
        }
        {
            let mut state = lock(&self.inner.state);
            state.route = Route {
                cwd,
                provider: params.provider,
                model: params.model,
                reasoning_effort,
                max_tokens,
            };
            state.approval_requests = params.approval_requests == Some(true);
            state.host_approvals = host.is_some();
            state.initializing = false;
            state.initialized = true;
            if let Some(questions) = lock(&self.inner.questions).as_ref() {
                questions
                    .set_enabled(params.capabilities.and_then(|c| c.user_questions) == Some(true));
            }
        }
        self.inner.initialization.send_replace(true);
        Ok(InitializeResult {
            server_info: ServerInfo {
                name: "deepseek-harness-sdk-runtime".into(),
                version: "0.0.1".into(),
            },
            hoh: host.map(|_| HohInitializeResult {
                protocol_version: HOH_PROTOCOL_VERSION,
                approvals: ApprovalHost::Host,
            }),
        })
    }
    pub async fn prompt(
        &self,
        params: SessionPromptParams,
    ) -> Result<SessionPromptResult, BoxError> {
        let blocks = params
            .content_blocks
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        self.prompt_blocks(SessionId::from(params.session_id), blocks, None)
            .await
    }
    /// `params`: those of a `session/prompt` request, for its listener (HOH-40).
    async fn prompt_blocks(
        &self,
        session_id: SessionId,
        blocks: Vec<Value>,
        params: Option<&Value>,
    ) -> Result<SessionPromptResult, BoxError> {
        self.assert_initialized()?;
        let rec = self.get_or_create_session(session_id.clone()).await?;
        self.inner.lifetime.signal().throw_if_aborted()?;
        self.assert_live_agent(&rec, &session_id)?;
        let content = durable_prompt_content(&self.inner.ctx, blocks).await?;
        self.inner.lifetime.signal().throw_if_aborted()?;
        self.assert_live_agent(&rec, &session_id)?;
        let message = create_user_message(NewUserMessage {
            content,
            source: MessageSource::new("user"),
        });
        let message_id = message.id.to_string();
        // HOH-40: the rest of the params (the window's `effort`) to the plugin that serves it,
        // with the message they belong to, before the message can start a turn.
        if let (Some(params), Some(methods)) = (params, self.inner.ctx.get::<HohRpcMethods>()) {
            methods.prompted(session_id.as_ref(), &message_id, params);
        }
        rec.handle.agent.followup(message)?;
        Ok(SessionPromptResult { message_id })
    }
    pub async fn query(&self, params: SessionQueryParams) -> Result<SessionQueryResult, BoxError> {
        self.query_blocks(
            SessionId::from(params.session_id),
            params
                .content_blocks
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?,
        )
        .await
    }
    async fn query_blocks(
        &self,
        session_id: SessionId,
        blocks: Vec<Value>,
    ) -> Result<SessionQueryResult, BoxError> {
        self.assert_initialized()?;
        self.assert_accepting()?;
        if let Some((text_index, text)) =
            blocks.iter().enumerate().find(|(_, b)| b["type"] == "text")
            && let Some(text) = text["text"].as_str()
            && js_trim(text).starts_with('/')
            && let Some(commands) = self.inner.ctx.get::<Commands>()
        {
            let rec = self.get_or_create_session(session_id.clone()).await?;
            self.inner.lifetime.signal().throw_if_aborted()?;
            self.assert_live_agent(&rec, &session_id)?;
            if let Some(line) = query_command(text, &commands.list(&rec.handle.agent)) {
                let images = query_command_images(&blocks, text_index)?;
                let weak = Arc::downgrade(&self.inner);
                let activity = QueryActivity::new(
                    self.inner.ctx.clone(),
                    rec.handle.agent.clone(),
                    self.inner.transport.clone(),
                    self.inner.lifetime.signal(),
                    move |id| {
                        if let Some(inner) = weak.upgrade() {
                            lock(&inner.state).activities.shift_remove(&id);
                        }
                    },
                )?;
                lock(&self.inner.state)
                    .activities
                    .insert(activity.id(), activity.clone());
                let id = {
                    let mut state = lock(&self.inner.state);
                    state.next_task += 1;
                    state.next_task
                };
                let signal = self.inner.lifetime.signal();
                let agent = rec.handle.agent.clone();
                let server = self.clone();
                let command_activity = activity.clone();
                // Source promises are eager and remain owned by shutdown even if the
                // JSON-RPC request that started one is abandoned during plugin unload.
                let job = QUERY_INVOCATION
                    .scope(activity.id(), async move {
                        let result = commands.execute(agent, line, images, signal).await;
                        let outcome = match result {
                            Ok(Some(execution)) => {
                                command_activity.accept(execution.clone()).map_or_else(
                                    |e| {
                                        command_activity.close();
                                        Err(e)
                                    },
                                    |()| Ok(execution),
                                )
                            }
                            Ok(None) => {
                                command_activity.close();
                                Err(error(
                                    "The selected command is unavailable in this SDK profile",
                                ))
                            }
                            Err(error) => {
                                command_activity.close();
                                Err(error)
                            }
                        };
                        lock(&server.inner.state).command_tasks.shift_remove(&id);
                        outcome.map_err(SharedError::new)
                    })
                    .boxed()
                    .shared();
                let completed = job.clone().map(|_| ()).boxed().shared();
                // Source: the shutting-down check and the registration of the task are one
                // synchronous step, so shutdown either sees the task or the command never
                // starts. `job` is lazy: dropping it unpolled never calls the handler.
                {
                    let mut state = lock(&self.inner.state);
                    if state.shutting_down {
                        drop(state);
                        activity.close();
                        return Err(error("SDK server is shutting down"));
                    }
                    state.command_tasks.insert(id, completed);
                }
                let drive = job.clone();
                tokio::spawn(async move {
                    let _ = drive.await;
                });
                let execution = job.await.map_err(|error| Box::new(error) as BoxError)?;
                return Ok(SessionQueryResult::Command {
                    execution: protocol_command_execution(execution),
                });
            }
        }
        let receipt = self.prompt_blocks(session_id, blocks, None).await?;
        Ok(SessionQueryResult::Prompt {
            message_id: receipt.message_id,
        })
    }
    pub async fn resume(&self, params: SessionResumeParams) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        let id = SessionId::from(params.session_id);
        let (creation, scheduled) = {
            let mut state = lock(&self.inner.state);
            if state.shutting_down {
                return Err(error("SDK server is shutting down"));
            }
            if let Some(pending) = state.scheduled.shift_remove(&id) {
                (pending, true)
            } else {
                if state.sessions.contains_key(&id) || state.creations.contains_key(&id) {
                    return Err(error(format!(
                        "SDK session is already open or opening: {id}"
                    )));
                }
                (
                    self.open_session_locked(&mut state, id.clone(), true),
                    false,
                )
            }
        };
        let rec = creation.await?;
        if scheduled {
            self.assert_live_agent(&rec, &id)?;
        }
        Ok(json!({}))
    }
    pub async fn wait(&self, params: SessionWaitParams) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        let id = SessionId::from(params.session_id);
        let rec = lock(&self.inner.state)
            .sessions
            .get(&id)
            .cloned()
            .ok_or_else(|| error(format!("unknown SDK session: {id}")))?;
        let agent = &rec.handle.agent;
        let subagents = self.inner.ctx.get::<Subagents>();
        loop {
            self.assert_live_agent(&rec, &id)?;
            agent.when_idle().await;
            let idle_seq = agent.session().seq();
            let children = match &subagents {
                Some(s) => s.wait_for_tree_work(agent.clone()).await?,
                None => false,
            };
            self.assert_live_agent(&rec, &id)?;
            if !children && agent.status() == AgentStatus::Idle && agent.session().seq() == idle_seq
            {
                if let Some((e, _)) = lock(&rec.failure).as_ref() {
                    return Err(e.clone().into());
                }
                return Ok(json!({}));
            }
        }
    }
    fn owns_agent(&self, agent: &Arc<dyn Agent>) -> Result<bool, BoxError> {
        let agents = self
            .inner
            .ctx
            .get::<Agents>()
            .ok_or_else(|| error("agents service is unavailable"))?;
        if !agents
            .get(agent.id())
            .is_some_and(|a| Arc::ptr_eq(&a, agent))
        {
            return Ok(false);
        }
        if lock(&self.inner.state)
            .sessions
            .get(agent.session().id())
            .is_some_and(|r| Arc::ptr_eq(&r.handle.agent, agent))
        {
            return Ok(true);
        }
        let Some(subagents) = self.inner.ctx.get::<Subagents>() else {
            return Ok(false);
        };
        let root = match subagents.root_of(agent) {
            Ok(root) => root,
            Err(e) if is_unauthorized_subagent_error(e.as_ref()) => return Ok(false),
            Err(e) => return Err(e),
        };
        Ok(lock(&self.inner.state)
            .sessions
            .get(root.session().id())
            .is_some_and(|r| Arc::ptr_eq(&r.handle.agent, &root)))
    }
    fn assert_live_agent(
        &self,
        rec: &SessionRecord,
        session_id: &SessionId,
    ) -> Result<(), BoxError> {
        if self
            .inner
            .ctx
            .get::<Agents>()
            .and_then(|a| a.get(rec.handle.agent.id()))
            .is_some_and(|a| Arc::ptr_eq(&a, &rec.handle.agent))
        {
            return Ok(());
        }
        Err(error(format!(
            "session agent was disposed outside the server: {session_id}"
        )))
    }
    pub async fn get_working_directory(
        &self,
        params: SessionWorkingDirectoryParams,
    ) -> Result<SessionWorkingDirectoryResult, BoxError> {
        let agent = self
            .working_directory_agent(SessionId::from(params.session_id))
            .await?;
        let directory = self
            .inner
            .ctx
            .get::<WorkingDirectoryKey>()
            .ok_or_else(|| error("workingDirectory service is unavailable"))?;
        Ok(SessionWorkingDirectoryResult {
            cwd: directory.ensure(&directory_agent(agent), None).await?,
        })
    }
    pub async fn set_working_directory(
        &self,
        params: SessionWorkingDirectorySetParams,
    ) -> Result<SessionWorkingDirectoryResult, BoxError> {
        let agent = self
            .working_directory_agent(SessionId::from(params.session_id))
            .await?;
        let directory = self
            .inner
            .ctx
            .get::<WorkingDirectoryKey>()
            .ok_or_else(|| error("workingDirectory service is unavailable"))?;
        Ok(SessionWorkingDirectoryResult {
            cwd: directory
                .set(&directory_agent(agent), &params.path, None)
                .await?,
        })
    }
    async fn working_directory_agent(&self, id: SessionId) -> Result<Arc<dyn Agent>, BoxError> {
        self.assert_initialized()?;
        let rec = self.get_or_create_session(id.clone()).await?;
        self.assert_live_agent(&rec, &id)?;
        Ok(rec.handle.agent.clone())
    }
    fn assert_initialized(&self) -> Result<(), BoxError> {
        if lock(&self.inner.state).initialized {
            Ok(())
        } else {
            Err(error("SDK server is not initialized"))
        }
    }
    fn assert_accepting(&self) -> Result<(), BoxError> {
        if lock(&self.inner.state).shutting_down {
            Err(error("SDK server is shutting down"))
        } else {
            Ok(())
        }
    }
    async fn get_or_create_session(&self, id: SessionId) -> Result<Arc<SessionRecord>, BoxError> {
        let pending = {
            let mut state = lock(&self.inner.state);
            if state.shutting_down {
                return Err(error("SDK server is shutting down"));
            }
            state.scheduled.shift_remove(&id);
            if let Some(rec) = state.sessions.get(&id) {
                return Ok(rec.clone());
            }
            if let Some(pending) = state.creations.get(&id) {
                pending.clone()
            } else {
                self.open_session_locked(&mut state, id, false)
            }
        };
        pending.await.map_err(Into::into)
    }
    fn open_session_locked(&self, state: &mut State, id: SessionId, resume: bool) -> Creation {
        let this = self.clone();
        let route = state.route.clone();
        let key = id.clone();
        let pending = async move {
            let result = this
                .create_session(id.clone(), resume, route)
                .await
                .map_err(SharedError::new);
            lock(&this.inner.state).creations.shift_remove(&id);
            result
        }
        .boxed()
        .shared();
        state.creations.insert(key, pending.clone());
        // Source Promises start eagerly and settle even if their caller abandons the waiter.
        // A shared driver is owned by the server's creations set and drained by shutdown.
        let drive = pending.clone();
        tokio::spawn(async move {
            let _ = drive.await;
        });
        pending
    }
    async fn create_session(
        &self,
        id: SessionId,
        resume: bool,
        route: Route,
    ) -> Result<Arc<SessionRecord>, BoxError> {
        if resume
            && let Some(persistence) = self.inner.ctx.get::<SessionPersistenceService>()
            && let Some(snapshot) = persistence.stat(&id, None).await?
            && snapshot.header.origin.as_deref() == Some("subagent")
        {
            return Err(error(format!(
                "subagent session \"{id}\" cannot be resumed directly; resume its root session instead"
            )));
        }
        let options = AgentOptions {
            provider: Some(Live::Value(route.provider.clone())),
            model: Some(Live::Value(route.model.clone())),
            reasoning_effort: route.reasoning_effort.clone().map(Live::Value),
            max_tokens: route.max_tokens.map(Live::Value),
        };
        let agents = self
            .inner
            .ctx
            .get::<Agents>()
            .ok_or_else(|| error("agents service is unavailable"))?;
        let handle = if resume {
            let selection = ModelSelection {
                provider: route.provider,
                model: route.model,
                reasoning_effort: route.reasoning_effort,
            };
            agents
                .resume(
                    &self.inner.ctx,
                    ResumeAgentOptions {
                        resume_session_id: id.clone(),
                        agent_options: Some(options),
                        signal: None,
                        setup: Some(Arc::new(move |_ctx, agent| {
                            let mut selection = selection.clone();
                            Box::pin(async move {
                                if selection.reasoning_effort.is_none()
                                    && let Some(previous) = agent.model().current()
                                    && previous.provider == selection.provider
                                    && previous.model == selection.model
                                {
                                    selection.reasoning_effort = previous.reasoning_effort;
                                }
                                agent.model().select(&selection)?;
                                Ok(None)
                            })
                        })),
                    },
                )
                .await?
        } else {
            agents
                .create(
                    &self.inner.ctx,
                    CreateAgentOptions {
                        session_id: Some(id.clone()),
                        meta: Some(AgentSessionMeta {
                            cwd: Some(route.cwd),
                            ..AgentSessionMeta::default()
                        }),
                        agent_options: Some(options),
                        ..CreateAgentOptions::default()
                    },
                )
                .await?
        };
        let record = Arc::new(SessionRecord {
            handle,
            failure: Mutex::new(None),
        });
        lock(&self.inner.state).sessions.insert(id, record.clone());
        Ok(record)
    }
    pub fn shutdown(&self) -> BoxFuture<'static, Result<Value, BoxError>> {
        let shutdown = {
            let mut state = lock(&self.inner.state);
            if let Some(shutdown) = &state.shutdown {
                shutdown.clone()
            } else {
                state.shutting_down = true;
                let this = self.clone();
                let task = async move { this.perform_shutdown().await.map_err(SharedError::new) }
                    .boxed()
                    .shared();
                state.shutdown = Some(task.clone());
                task
            }
        };
        self.inner
            .lifetime
            .abort_with(error("SDK server is shutting down"));
        // Abort listeners run in kernel tasks; close synchronously as the source's
        // AbortSignal callback does, before any agent teardown can emit failures.
        let activities = lock(&self.inner.state)
            .activities
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for activity in activities {
            activity.close();
        }
        if let Some(q) = lock(&self.inner.questions).as_ref() {
            q.close();
        }
        self.inner.initialization.send_replace(true);
        let drive = shutdown.clone();
        tokio::spawn(async move {
            let _ = drive.await;
        });
        Box::pin(async move { shutdown.await.map_err(Into::into) })
    }
    async fn perform_shutdown(&self) -> Result<Value, BoxError> {
        let pending = lock(&self.inner.state)
            .creations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let _ = join_all(pending).await;
        let commands = lock(&self.inner.state)
            .command_tasks
            .values()
            .cloned()
            .collect::<Vec<_>>();
        join_all(commands).await;
        let records = {
            let mut state = lock(&self.inner.state);
            state.creations.clear();
            state.scheduled.clear();
            std::mem::take(&mut state.sessions)
                .into_values()
                .collect::<Vec<_>>()
        };
        let mut failures = Vec::new();
        let disposers = std::mem::take(&mut *lock(&self.inner.disposers));
        for disposer in disposers.into_iter().rev() {
            disposer.dispose().await;
        }
        if let Some(subagents) = self.inner.ctx.get::<Subagents>()
            && let Err(e) = subagents
                .drain_descendants(records.iter().map(|r| r.handle.agent.clone()).collect())
                .await
        {
            failures.push(SharedError::new(e));
        }
        let mut tasks: Vec<BoxFuture<'static, Result<(), BoxError>>> = Vec::new();
        if let Some(goal) = lock(&self.inner.goal_lifetime).take() {
            tasks.push(
                async move {
                    goal.dispose().await;
                    Ok(())
                }
                .boxed(),
            );
        }
        for rec in records {
            tasks.push(async move { rec.handle.dispose().await }.boxed());
        }
        if let Some(llm) = lock(&self.inner.state).llm_fiber.take() {
            tasks.push(
                async move {
                    llm.dispose().await;
                    Ok(())
                }
                .boxed(),
            );
        }
        for result in join_all(tasks).await {
            if let Err(e) = result {
                failures.push(SharedError::new(e));
            }
        }
        match failures.len() {
            0 => Ok(json!({})),
            1 => Err(failures.remove(0).into()),
            _ => Err(Box::new(SdkServerError::Aggregate { errors: failures })),
        }
    }

    pub async fn handle_request(&self, method: &str, params: Value) -> Result<Value, BoxError> {
        match method {
            "initialize" => {
                if params
                    .get("approvalRequests")
                    .is_some_and(|v| !v.is_boolean())
                {
                    return Err(type_error("initialize approvalRequests must be a boolean"));
                }
                if params
                    .get("reasoningEffort")
                    .is_some_and(|v| !v.as_str().is_some_and(|s| !s.is_empty()))
                {
                    return Err(type_error(
                        "initialize reasoningEffort must be a non-empty string",
                    ));
                }
                if let Some(n) = params.get("maxTokens") {
                    let safe = n.as_f64().is_some_and(|n| {
                        n > 0.0 && n <= 9_007_199_254_740_991.0 && n.fract() == 0.0
                    });
                    if !n.is_number() || !safe {
                        return Err(type_error(
                            "initialize maxTokens must be a positive safe integer",
                        ));
                    }
                }
                if !params.get("cwd").is_some_and(Value::is_string) {
                    return Err(type_error(node_path_resolve_argument_error(
                        params.get("cwd"),
                    )));
                }
                // Source: a provider that is no string matches no adapter id and is not
                // "deepseek-official", so the failure names `String(provider)`.
                if !params.get("provider").is_some_and(Value::is_string) {
                    let shown = params
                        .get("provider")
                        .map_or_else(|| "undefined".to_owned(), hoh_util_values::js_string);
                    return Err(error(format!(
                        "no adapter registered for provider \"{shown}\""
                    )));
                }
                let mut normalized = params;
                // Source: `params.capabilities?.userQuestions === true` reads any other shape as
                // "not enabled" instead of rejecting the request.
                match normalized.get_mut("capabilities") {
                    Some(Value::Object(capabilities)) => {
                        if capabilities
                            .get("userQuestions")
                            .is_some_and(|v| !v.is_boolean())
                        {
                            capabilities.remove("userQuestions");
                        }
                    }
                    Some(_) => {
                        normalized
                            .as_object_mut()
                            .map(|object| object.remove("capabilities"));
                    }
                    None => {}
                }
                if let Some(n) = normalized.get("maxTokens").and_then(Value::as_f64) {
                    normalized["maxTokens"] = Value::from(n as u64);
                }
                serde_json::to_value(
                    self.initialize(
                        serde_json::from_value(normalized)
                            .map_err(|e| type_error(e.to_string()))?,
                    )
                    .await?,
                )
                .map_err(Into::into)
            }
            "session/prompt" => {
                self.assert_initialized()?;
                // Preserve the SDK-compatible diagnostics for the two required fields while the
                // typed decode below enforces the rest of the closed request contract.
                string_field(
                    &params,
                    "sessionId",
                    "session/prompt requires a sessionId string",
                )?;
                params
                    .get("contentBlocks")
                    .and_then(Value::as_array)
                    .ok_or_else(|| type_error("session/prompt requires contentBlocks"))?;
                // `effort` is private glue consumed by the prompt listener, not the public SDK
                // struct. Everything else crosses the typed protocol boundary before a session,
                // message, mode choice, or followup can be created.
                let mut typed = params.clone();
                if let Some(object) = typed.as_object_mut() {
                    object.remove("effort");
                }
                let typed: SessionPromptParams =
                    serde_json::from_value(typed).map_err(|error| type_error(error.to_string()))?;
                let blocks = typed
                    .content_blocks
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<Vec<_>, _>>()?;
                serde_json::to_value(
                    self.prompt_blocks(SessionId::from(typed.session_id), blocks, Some(&params))
                        .await?,
                )
                .map_err(Into::into)
            }
            "session/query" => {
                let valid = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
                    && params.as_object().is_some_and(|o| {
                        o.keys().all(|k| k == "sessionId" || k == "contentBlocks")
                    });
                if !valid {
                    return Err(type_error(
                        "session/query requires only a non-empty sessionId and contentBlocks",
                    ));
                }
                let blocks = query_content(params.get("contentBlocks").unwrap_or(&Value::Null))?;
                serde_json::to_value(
                    self.query_blocks(
                        SessionId::from(params["sessionId"].as_str().unwrap_or_default()),
                        blocks,
                    )
                    .await?,
                )
                .map_err(Into::into)
            }
            "session/resume" => {
                let id = string_field(
                    &params,
                    "sessionId",
                    "session/resume requires a non-empty sessionId string",
                )?;
                if id.is_empty() {
                    return Err(type_error(
                        "session/resume requires a non-empty sessionId string",
                    ));
                }
                self.resume(SessionResumeParams {
                    session_id: id.into(),
                })
                .await
            }
            "session/wait" => {
                self.wait(SessionWaitParams {
                    session_id: string_field(
                        &params,
                        "sessionId",
                        "session/wait requires a sessionId string",
                    )?
                    .into(),
                })
                .await
            }
            "session/working-directory/get" | "session/working-directory/set" => {
                let id = string_field(
                    &params,
                    "sessionId",
                    "working-directory requests require a non-empty sessionId",
                )?;
                if id.is_empty() {
                    return Err(type_error(
                        "working-directory requests require a non-empty sessionId",
                    ));
                }
                let result = if method == "session/working-directory/get" {
                    self.get_working_directory(SessionWorkingDirectoryParams {
                        session_id: id.into(),
                    })
                    .await?
                } else {
                    self.approve_privileged(method, &params).await?;
                    self.set_working_directory(SessionWorkingDirectorySetParams {
                        session_id: id.into(),
                        path: string_field(
                            &params,
                            "path",
                            "working-directory set requires a path string",
                        )?
                        .into(),
                    })
                    .await?
                };
                serde_json::to_value(result).map_err(Into::into)
            }
            "session/list" => self.session_list(params).await,
            "session/events" => self.session_events(params).await,
            "session/cancel" => self.session_cancel(params),
            "shutdown" => self.shutdown().await,
            method if method.starts_with("hoh/") => self.hoh_method(method, params).await,
            _ => Err(error(format!(
                "unknown DeepSeek Harness SDK runtime method: {method}"
            ))),
        }
    }
}

fn is_unauthorized_subagent_error(error: &(dyn std::error::Error + 'static)) -> bool {
    if HarnessError::of(error)
        .is_some_and(|error| error.name() == "SubagentError" && error.code() == "UNAUTHORIZED")
    {
        return true;
    }
    let error: &(dyn std::error::Error + 'static) = match SharedError::of(error) {
        Some(shared) => shared.get_ref(),
        None => error,
    };
    error
        .downcast_ref::<ForeignErrorValue>()
        .is_some_and(|error| {
            error.field("name") == Some("SubagentError")
                && error.field("code") == Some("UNAUTHORIZED")
        })
}
fn string_field<'a>(params: &'a Value, name: &str, message: &str) -> Result<&'a str, BoxError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| type_error(message))
}
fn success_status(reason: &str, options: &HarnessSdkJsonRpcServerOptions) -> &'static str {
    if reason == "completed" || reason == "max-tokens" && options.max_tokens_as_success {
        "ok"
    } else {
        "error"
    }
}
async fn durable_prompt_content(
    ctx: &Context,
    blocks: Vec<Value>,
) -> Result<Vec<ContentBlock>, BoxError> {
    let encoded = blocks
        .iter()
        .filter(|block| block["type"] == "image" && block.get("data").is_some())
        .collect::<Vec<_>>();
    let refs = if encoded.is_empty() {
        Vec::new()
    } else {
        let attachments = ctx
            .get::<Attachments>()
            .ok_or_else(|| error("SDK image prompt requires an attachment store"))?;
        let images = encoded
            .into_iter()
            .map(|block| {
                let data = block["data"].as_str().ok_or_else(|| {
                    type_error(
                        "The first argument must be of type string or an instance of Buffer, \
                         ArrayBuffer, or Array or an Array-like Object.",
                    )
                })?;
                let media_type = match block["mimeType"].as_str() {
                    Some("image/png") => ImageMediaType::Png,
                    Some("image/jpeg") => ImageMediaType::Jpeg,
                    Some("image/webp") => ImageMediaType::Webp,
                    Some("image/gif") => ImageMediaType::Gif,
                    other => {
                        let received = other.unwrap_or("undefined");
                        return Err(AttachmentError::new(
                            format!("Image type {received} is not accepted by this deployment."),
                            AttachmentErrorCode::UnsupportedImageType,
                        )
                        .into());
                    }
                };
                Ok(EncodedImageAttachment {
                    data: data.into(),
                    media_type,
                    name: block.get("name").and_then(Value::as_str).map(str::to_owned),
                })
            })
            .collect::<Result<Vec<_>, BoxError>>()?;
        admit_encoded_images(&*attachments, &images).await?
    };
    let mut refs = refs.into_iter();
    blocks
        .into_iter()
        .map(|block| {
            let value = if block["type"] == "image" && block.get("data").is_some() {
                json!({"type":"image", "attachment":refs.next()})
            } else {
                block
            };
            serde_json::from_value(value).map_err(Into::into)
        })
        .collect()
}

fn node_path_resolve_argument_error(value: Option<&Value>) -> String {
    let received = match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(value)) => format!("type boolean ({value})"),
        Some(Value::Number(value)) => format!("type number ({value})"),
        Some(Value::Array(_)) => "an instance of Array".into(),
        Some(Value::Object(_)) => "an instance of Object".into(),
        Some(Value::String(_)) => unreachable!("strings are accepted above"),
    };
    format!("The \"paths[0]\" argument must be of type string. Received {received}")
}

#[cfg(test)]
mod assistant_stream_ownership_tests {
    use super::is_current_owned_agent;
    use std::sync::Arc;

    #[test]
    fn requires_the_event_to_be_both_owned_and_current_by_arc_identity() {
        let old = Arc::new(());
        let replacement = Arc::new(());
        assert!(is_current_owned_agent(Some(&old), Some(&old), &old));
        assert!(!is_current_owned_agent(
            Some(&old),
            Some(&replacement),
            &old
        ));
        assert!(!is_current_owned_agent(
            Some(&old),
            Some(&replacement),
            &replacement
        ));
        assert!(!is_current_owned_agent(None, Some(&old), &old));
    }
}
