//! The session driven by an external harness: an [`Agent`] whose turns run in the harness. Its
//! events are written to the session log in the agent loop's shapes, its permission requests go
//! through the destructive classifier and the approval service, its questions through the
//! user-questions service.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentAssistantStream, AgentError, AgentOptions, AgentStatus, AgentStatusChanged,
    AssistantStreamFrame, AssistantStreamOutcome, CancelCauseError, CancelOptions, Inbox,
    InboxState, InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
    emit_agent_event,
};
use hoh_destructive_classifier::{Question, command_reach};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Disposer};
use hoh_llm::{
    AssistantProviderMetadata, ContentBlock, LlmAttemptId, MessageId, NewAssistantMessage,
    StreamChunk, ToolCallBlock, ToolCallId, ToolResultMessageInput, UserMessage,
    create_assistant_message, create_tool_result_message,
};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, Session, SessionId, SessionSeq, SurfaceIntent, SurfaceOp,
    append_plugin_record,
};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_user_approval::{ApprovalOutcome, ApprovalRequest, DisplayReason};
use hoh_user_questions::{AskUserQuestionItem, AskUserQuestionOption, AskUserQuestionRequest};
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::contract::{
    AccessMode, ApprovalDecision, ApprovalMode, EndReason, FileChange, FileChangeKind,
    HarnessAction, HarnessBackgroundTask, HarnessEvent, HarnessFailure, HarnessPolicy,
    HarnessQuestion, HarnessResponse, HarnessTelemetry, HarnessTelemetryRecord,
    HarnessWorkflowProgress, InteractionMode, ItemOutcome, NativeSessionId, NetworkAccess,
    PromptImage, PromptParams, QuestionAnswer, RespondParams, SandboxMode, StartParams,
    TELEMETRY_RECORD, code,
};
use crate::executor::{BINDING_RECORD, Launched, Shared};
use crate::harness::HarnessSession;
use crate::redact::{ready_to_show, redact, redact_value};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

const MAX_OUTPUT_CHARS: usize = 64 * 1024;
const MAX_HEADLINE_CHARS: usize = 600;
const MAX_DETAILS_CHARS: usize = 3600;
/// Telemetry is durable and provider-controlled: bound every display string, every repeated
/// collection, and the final encoded record before it reaches the session log.
const MAX_TELEMETRY_TEXT_BYTES: usize = 4 * 1024;
const MAX_TELEMETRY_ITEMS: usize = 256;
const MAX_TELEMETRY_RECORD_BYTES: usize = 64 * 1024;

fn telemetry_text(text: &str) -> String {
    let mut text = redact(text);
    if text.len() > MAX_TELEMETRY_TEXT_BYTES {
        let mut end = MAX_TELEMETRY_TEXT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

fn scrub_background_task(task: &mut HarnessBackgroundTask) {
    task.task_type = task.task_type.as_deref().map(telemetry_text);
    task.description = task.description.as_deref().map(telemetry_text);
}

fn scrub_workflow_progress(progress: &mut HarnessWorkflowProgress) {
    progress.kind = telemetry_text(&progress.kind);
    progress.title = progress.title.as_deref().map(telemetry_text);
    progress.label = progress.label.as_deref().map(telemetry_text);
    progress.state = progress.state.as_deref().map(telemetry_text);
    progress.phase_title = progress.phase_title.as_deref().map(telemetry_text);
    progress.model = progress.model.as_deref().map(telemetry_text);
    progress.prompt_preview = progress.prompt_preview.as_deref().map(telemetry_text);
    progress.result_preview = progress.result_preview.as_deref().map(telemetry_text);
}

/// Redact only payload display/free-text: provenance and task/message/run ids are identities and
/// must remain byte-for-byte provider values. A repeated collection over the limit is rejected
/// whole, so a complete roster can never become a believable partial snapshot.
fn prepare_telemetry(record: &mut HarnessTelemetryRecord) -> Result<(), &'static str> {
    match &mut record.payload {
        HarnessTelemetry::AssistantUsage { .. } => {}
        HarnessTelemetry::ResultUsage {
            model_usage,
            user_message_uuids,
            origin,
            ..
        } => {
            if model_usage
                .as_ref()
                .is_some_and(|models| models.len() > MAX_TELEMETRY_ITEMS)
            {
                return Err("too many model usage entries");
            }
            if user_message_uuids.len() > MAX_TELEMETRY_ITEMS {
                return Err("too many echoed user message ids");
            }
            if let Some(models) = model_usage {
                for model in models {
                    model.model = telemetry_text(&model.model);
                }
            }
            *origin = origin.as_deref().map(telemetry_text);
        }
        HarnessTelemetry::McpServers { servers } => {
            if servers.len() > MAX_TELEMETRY_ITEMS {
                return Err("too many MCP servers");
            }
            for server in servers {
                server.name = telemetry_text(&server.name);
                server.status = server.status.as_deref().map(telemetry_text);
            }
        }
        HarnessTelemetry::BackgroundTask { update } => {
            if update.workflow_progress.len() > MAX_TELEMETRY_ITEMS {
                return Err("too many workflow progress entries");
            }
            update.subtype = telemetry_text(&update.subtype);
            update.status = update.status.as_deref().map(telemetry_text);
            update.summary = update.summary.as_deref().map(telemetry_text);
            scrub_background_task(&mut update.task);
            for progress in &mut update.workflow_progress {
                scrub_workflow_progress(progress);
            }
        }
        HarnessTelemetry::BackgroundTaskUpdated { patch, .. } => {
            patch.status = patch.status.as_deref().map(telemetry_text);
        }
        HarnessTelemetry::BackgroundTasksSnapshot { tasks } => {
            if tasks.len() > MAX_TELEMETRY_ITEMS {
                return Err("too many background tasks");
            }
            for task in tasks {
                scrub_background_task(task);
            }
        }
    }
    let encoded = serde_json::to_vec(record).map_err(|_| "cannot encode telemetry")?;
    if encoded.len() > MAX_TELEMETRY_RECORD_BYTES {
        return Err("telemetry record is too large");
    }
    Ok(())
}

/// `approval: never` is built in (feature `never-mode`). Off by default until it passes its own
/// review: the slot then refuses to start a native session under `never`, and nothing in the slot
/// answers a permission request by itself. Full access is a different thing: the user's choice in
/// the window ([`AccessMode::Full`]), under which the CLI itself asks nothing.
const NEVER_MODE: bool = cfg!(feature = "never-mode");

/// What the window shows when `approval: never` is refused in this build (CEO, 20.11).
pub(crate) fn never_off(executor: &str) -> String {
    format!(
        "Этот режим пока недоступен для {}: действия идут через подтверждение",
        display_name(executor)
    )
}

/// How each access mode is put to the user, word for word (CEO, 20.11).
pub(crate) fn access_text(executor: &str, access: AccessMode) -> String {
    let name = display_name(executor);
    match access {
        AccessMode::Confirm => format!(
            "С подтверждением: команды, правки файлов и сеть {name} выполняет после вашего \
             подтверждения"
        ),
        AccessMode::AcceptEdits => format!(
            "Правки без вопросов: файлы в папке разговора {name} меняет сам, команды и сеть — \
             после вашего подтверждения"
        ),
        AccessMode::Auto => format!(
            "Автоматически: что безопасно, {name} решает и выполняет сам, остальное — после \
             вашего подтверждения"
        ),
        AccessMode::Full => format!("Полный доступ: {name} выполняет команды без вопросов"),
    }
}

/// The policy a native session is started with under `access`. [`AccessMode::Confirm`] caps the
/// configured sandbox and network to the safe default (the folder, no network): the
/// configuration does not widen what the user chose; the two modes between it and full access
/// (accept-edits, auto) cap it the same way. [`AccessMode::Full`] runs without the
/// CLI's sandbox and with the network, except a session configured read-only: it stays read-only
/// with its own network (review 232500, item 4; as `hoh-harness-claude` launches it).
pub(crate) fn launch_policy(configured: &HarnessPolicy, access: AccessMode) -> HarnessPolicy {
    match access {
        AccessMode::Full if configured.sandbox == SandboxMode::ReadOnly => HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            approval: ApprovalMode::OnRequest,
            network: configured.network,
        },
        AccessMode::Full => HarnessPolicy {
            sandbox: SandboxMode::FullAccess,
            approval: ApprovalMode::OnRequest,
            network: NetworkAccess::Allowed,
        },
        AccessMode::Confirm | AccessMode::AcceptEdits | AccessMode::Auto => HarnessPolicy {
            sandbox: match configured.sandbox {
                SandboxMode::FullAccess => SandboxMode::WorkspaceWrite,
                other => other,
            },
            approval: configured.approval,
            network: NetworkAccess::Denied,
        },
    }
}

/// The executor's name as a word in a sentence: `claude` → `Claude`.
pub(crate) fn display_name(executor: &str) -> String {
    let mut chars = executor.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// What `approval: never` lets pass without a question, word for word as the consent question
/// says it (see [`ExternalAgent::within_workspace`] and the classifier).
const NEVER_RULE: &str = "Без вопроса пройдут только: команда, у которой рабочая папка и все \
    пути в аргументах, значениях флагов и перенаправлениях лежат в папке сессии, без сетевых \
    программ (curl, ssh, git push, npm install и т. п.), без переменных, подстановок, шаблонов, \
    cd, обёрток (sudo, env, xargs) и кода в аргументах (sh -c, python -c, awk), и которую \
    классификатор не считает удаляющей или перезаписывающей файлы; создание и правка файлов в \
    папке сессии без удаления и без переноса поверх существующего файла. Удаления, пути вне \
    папки, сеть, другие инструменты исполнителя и запросы прав он спросит";
/// How long the end of a turn waits for its approval and question tasks to settle.
const TASK_SETTLE: Duration = Duration::from_secs(10);
/// The `turn/end` error code of a turn whose opening events could not be written.
const TURN_OPEN_FAILED: &str = "turn-open-failed";
/// The model an answer is attributed to when none was selected: the harness's own default.
const DEFAULT_MODEL: &str = "default";

pub(crate) struct ExternalAgent {
    me: Weak<ExternalAgent>,
    pub(crate) shared: Arc<Shared>,
    id: SessionId,
    session: Session,
    ctx: Context,
    scope_key: ScopeKey,
    options: AgentOptions,
    model: ExternalModel,
    inbox: ExternalInbox,
    status: watch::Sender<AgentStatus>,
    wake: Notify,
    state: Mutex<State>,
    revision: AtomicU64,
}

#[derive(Default)]
struct State {
    last_turn: u64,
    native: Option<NativeSessionId>,
    /// The model the last binding record names.
    recorded_model: Option<String>,
    /// The running native session, for teardown.
    control: Option<Arc<dyn HarnessSession>>,
    /// The open turn.
    abort: Option<Arc<AbortController>>,
    cause: Option<AgentCancelCause>,
    /// A wake asked for work that sits only in the next-step list.
    wake_pending: bool,
    /// The input of a turn that could not be opened went back to the inbox: it waits for the
    /// next wake instead of being retried at once.
    held: bool,
    /// The session is being unloaded: no turn starts any more.
    closed: bool,
    /// The effort level of the turns (HOH-40): the one the last prompt named, when the harness
    /// has it; a turn without a prompt of its own keeps it.
    effort: Option<String>,
    /// T3's interaction mode for the current turn. A wake without a prompt keeps it.
    interaction_mode: InteractionMode,
}

pub(crate) struct NewAgent {
    pub shared: Arc<Shared>,
    pub session: Session,
    pub ctx: Context,
    pub scope_key: ScopeKey,
    pub options: AgentOptions,
    /// The conversation's own model (`None`: the harness's default), never another provider's.
    pub model: Option<String>,
    pub last_turn: u64,
    pub native: Option<NativeSessionId>,
}

impl ExternalAgent {
    pub(crate) fn new(init: NewAgent) -> Arc<ExternalAgent> {
        init.shared
            .choices
            .serve_efforts(init.session.id().as_ref());
        let model = ExternalModel {
            provider: init.shared.name.clone(),
            model: Mutex::new(init.model.clone()),
            restart: AtomicBool::new(false),
            log: init.ctx.logger(),
        };
        Arc::new_cyclic(|me| ExternalAgent {
            me: me.clone(),
            id: init.session.id().clone(),
            shared: init.shared,
            session: init.session,
            ctx: init.ctx,
            scope_key: init.scope_key,
            options: init.options,
            model,
            inbox: ExternalInbox::default(),
            status: watch::channel(AgentStatus::Idle).0,
            wake: Notify::new(),
            state: Mutex::new(State {
                last_turn: init.last_turn,
                native: init.native,
                recorded_model: init.model,
                ..State::default()
            }),
            revision: AtomicU64::new(0),
        })
    }

    fn as_dyn(&self) -> Option<Arc<dyn Agent>> {
        self.me.upgrade().map(|me| me as Arc<dyn Agent>)
    }

    fn emit_status(&self, status: AgentStatus) {
        let changed = self.status.send_if_modified(|current| {
            if *current == status {
                false
            } else {
                *current = status;
                true
            }
        });
        if changed && let Some(agent) = self.as_dyn() {
            emit_agent_event(&self.shared.ctx, &agent, |agent| AgentStatusChanged {
                agent,
                status,
            });
        }
    }

    fn emit_frame(&self, frame: AssistantStreamFrame) {
        if let Some(agent) = self.as_dyn() {
            emit_agent_event(&self.shared.ctx, &agent, |agent| AgentAssistantStream {
                agent,
                frame,
            });
        }
    }

    /// The next chunk of a live text; nothing for an empty piece.
    fn emit_frame_of(&self, live: &mut LiveText, piece: &str) {
        if piece.is_empty() {
            return;
        }
        let Some(chunk) = text_delta(piece) else {
            return;
        };
        self.emit_frame(AssistantStreamFrame::Chunk {
            attempt_id: live.attempt_id.clone(),
            revision: live.revision,
            index: live.chunks,
            time: now_ms(),
            chunk,
        });
        live.chunks += 1;
    }

    fn emit_error(&self, turn: u64, step: u64, error: BoxError) {
        if let Some(agent) = self.as_dyn() {
            let error = SharedError::new(error);
            emit_agent_event(&self.shared.ctx, &agent, |agent| AgentError {
                agent,
                turn,
                step,
                error,
            });
        }
    }

    fn interrupt_grace(&self) -> Duration {
        Duration::from_millis(self.shared.config.interrupt_grace_ms)
    }

    fn terminate_grace(&self) -> Duration {
        Duration::from_millis(self.shared.config.terminate_grace_ms)
    }

    /// The conversation's folder, read where it is used: a new chat may still change it; once the
    /// conversation has worked there, a change is refused (`Shared::started_in_folder`).
    fn cwd(&self) -> String {
        crate::executor::session_cwd(&self.shared, &self.session)
    }

    /// The running native session, if any (teardown terminates it).
    pub(crate) fn control(&self) -> Option<Arc<dyn HarnessSession>> {
        lock(&self.state).control.clone()
    }

    /// Record the native id durably with the model it runs, when either is new (`replaces`: a
    /// lost id it replaces).
    fn record_native(&self, id: &NativeSessionId, replaces: Option<&NativeSessionId>) {
        let model = lock(&self.model.model).clone();
        {
            let state = lock(&self.state);
            if state.native.as_ref() == Some(id)
                && state.recorded_model == model
                && replaces.is_none()
            {
                return;
            }
        }
        let mut data = json!({"harness": self.shared.name, "nativeSessionId": id});
        if let Some(model) = &model {
            data["model"] = json!(model);
        }
        if let Some(lost) = replaces {
            data["replaces"] = json!(lost);
        }
        // Not under the state lock: listeners of the append may call back into the agent.
        match append_plugin_record(&self.session, BINDING_RECORD, data) {
            Ok(_) => {
                let mut state = lock(&self.state);
                state.native = Some(id.clone());
                state.recorded_model = model;
            }
            Err(error) => self
                .ctx
                .logger()
                .warn(format_args!("cannot record native session id: {error}")),
        }
    }

    /// Preserve bounded, redacted provider telemetry as an ignorable session record. The
    /// concrete adapter selects typed-safe fields; this common boundary enforces durable-log
    /// safety for every adapter without rewriting provider identities.
    fn record_telemetry(&self, mut record: HarnessTelemetryRecord) {
        if let Err(reason) = prepare_telemetry(&mut record) {
            self.ctx
                .logger()
                .warn(format_args!("dropping harness telemetry: {reason}"));
            return;
        }
        let data = json!({"harness": self.shared.name, "record": record});
        if let Err(error) = append_plugin_record(&self.session, TELEMETRY_RECORD, data) {
            self.ctx
                .logger()
                .warn(format_args!("cannot record harness telemetry: {error}"));
        }
    }

    // ---- driver --------------------------------------------------------------------------

    /// Take the next turn's input and open its abort under one lock.
    fn begin_turn(&self) -> Option<(TurnInput, Arc<AbortController>)> {
        let mut state = lock(&self.state);
        if state.closed || (state.held && !state.wake_pending) {
            return None;
        }
        let input = self.inbox.take_turn(state.wake_pending);
        state.wake_pending = false;
        if input.messages.is_empty() {
            return None;
        }
        state.held = false;
        let abort = Arc::new(AbortController::new());
        state.abort = Some(abort.clone());
        state.cause = None;
        Some((input, abort))
    }

    /// Stop starting turns and stop the running one (the session is being unloaded); queued
    /// input stays in the inbox.
    pub(crate) fn close(&self) {
        lock(&self.state).closed = true;
        let _ = self.cancel(
            AgentCancelCause::Disposed,
            CancelOptions { keep_inbox: true },
        );
    }

    fn end_turn(&self) {
        let mut state = lock(&self.state);
        state.abort = None;
        state.cause = None;
    }

    fn cause(&self) -> AgentCancelCause {
        lock(&self.state)
            .cause
            .clone()
            .unwrap_or(AgentCancelCause::User)
    }

    pub(crate) async fn drive(self: Arc<Self>, token: CancellationToken) {
        let mut live: Option<LiveSession> = None;
        loop {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if token.is_cancelled() {
                break;
            }
            let Some((input, abort)) = self.begin_turn() else {
                self.emit_status(AgentStatus::Idle);
                tokio::select! {
                    () = &mut notified => continue,
                    () = token.cancelled() => break,
                }
            };
            self.emit_status(AgentStatus::Running);
            self.run_turn(&mut live, input, &abort).await;
            self.end_turn();
        }
        if let Some(live) = live.take() {
            live.control.terminate(self.terminate_grace()).await;
        }
        lock(&self.state).control = None;
        self.emit_status(AgentStatus::Idle);
    }

    /// Write the opening events of a turn; on failure, also how many of `turn/start` and
    /// `step/start` were written.
    fn open_turn(
        &self,
        turn: u64,
        step: u64,
        messages: &[UserMessage],
    ) -> Result<(), (BoxError, u8)> {
        self.session
            .append("turn/start", json!({"turn": turn}), None)
            .map_err(|error| (BoxError::from(error), 0))?;
        self.session
            .append("step/start", json!({"turn": turn, "step": step}), None)
            .map_err(|error| (BoxError::from(error), 1))?;
        for message in messages {
            let data = serde_json::to_value(message).map_err(|error| (error.into(), 2))?;
            self.session
                .append("user/message", data, Some(SurfaceIntent::append()))
                .map_err(|error| (BoxError::from(error), 2))?;
        }
        Ok(())
    }

    async fn run_turn(
        self: &Arc<Self>,
        live: &mut Option<LiveSession>,
        input: TurnInput,
        abort: &Arc<AbortController>,
    ) {
        let signal = abort.signal();
        let turn = {
            let mut state = lock(&self.state);
            state.last_turn += 1;
            state.last_turn
        };
        let step = 1;
        if let Err((error, written)) = self.open_turn(turn, step, &input.messages) {
            let message = format!("cannot open turn {turn}: {error}");
            self.ctx.logger().error(format_args!("{message}"));
            // The harness never saw this input: it waits in the inbox for the next wake.
            self.inbox.give_back(input);
            let mut state = lock(&self.state);
            state.held = true;
            if written == 0 {
                state.last_turn -= 1;
            }
            drop(state);
            let closed = (|| -> Result<(), BoxError> {
                if written >= 2 {
                    self.session
                        .append("step/end", json!({"turn": turn, "step": step}), None)?;
                }
                if written >= 1 {
                    let reason = json!({"kind": "error",
                        "error": {"message": message, "code": TURN_OPEN_FAILED}});
                    self.session.append(
                        "turn/end",
                        json!({"turn": turn, "reason": reason}),
                        None,
                    )?;
                }
                Ok(())
            })();
            if let Err(close_error) = closed {
                self.ctx
                    .logger()
                    .error(format_args!("cannot close turn {turn}: {close_error}"));
            }
            self.emit_error(turn, step, error);
            return;
        }
        self.take_prompt_choices(&input);
        let messages = input.messages;
        let mut log = TurnLog::new(self.clone(), turn, step, signal.clone());
        let outcome = self.native_turn(live, &mut log, &messages, &signal).await;
        log.finish(matches!(outcome, TurnOutcome::Completed)).await;
        // The harness's own words may carry a secret (a key in an upstream error): masked
        // before they reach the log, the window and the kernel log.
        let outcome = match outcome {
            TurnOutcome::Failed(failure) => TurnOutcome::Failed(HarnessFailure {
                message: redact(&failure.message),
                code: failure.code,
            }),
            other => other,
        };
        let reason = match &outcome {
            TurnOutcome::Completed => json!({"kind": "completed"}),
            TurnOutcome::Aborted => json!({"kind": "aborted", "reason": self.cause()}),
            TurnOutcome::Failed(failure) => {
                json!({"kind": "error", "error": {"message": failure.message, "code": failure.code}})
            }
        };
        let mut end = json!({"turn": turn, "reason": reason});
        if let Some(usage) = log.usage {
            end["usage"] = json!(usage);
        }
        let closed = self
            .session
            .append("step/end", json!({"turn": turn, "step": step}), None)
            .and_then(|_| self.session.append("turn/end", end, None));
        if let Err(error) = closed {
            self.ctx
                .logger()
                .error(format_args!("cannot close turn {turn}: {error}"));
        }
        if let TurnOutcome::Failed(failure) = outcome {
            self.emit_error(turn, step, Box::new(failure));
        }
    }

    async fn native_turn(
        self: &Arc<Self>,
        live: &mut Option<LiveSession>,
        log: &mut TurnLog,
        messages: &[UserMessage],
        signal: &AbortSignal,
    ) -> TurnOutcome {
        if signal.aborted() {
            return TurnOutcome::Aborted;
        }
        let started = tokio::select! {
            biased;
            _ = signal.cancelled() => return TurnOutcome::Aborted,
            started = self.ensure_live(live, signal) => started,
        };
        if let Err(failure) = started {
            return TurnOutcome::Failed(failure);
        }
        let Some(session) = live.as_mut() else {
            return TurnOutcome::Failed(HarnessFailure::new(code::CLOSED, "no harness session"));
        };
        let turn_id = format!("{}#{}", self.id, log.turn);
        let images = match prompt_images(&self.ctx, messages, signal).await {
            Ok(images) => images,
            Err(failure) => return TurnOutcome::Failed(failure),
        };
        // The answer to the prompt is awaited together with the events: a harness may stream
        // the turn before it answers, and a stop before the answer must not be lost.
        let mut prompt = session.control.prompt(PromptParams {
            session_id: self.id.to_string(),
            turn_id: turn_id.clone(),
            text: prompt_text(messages),
            images,
        });
        let mut prompted = false;
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let stopping = deadline.is_some();
            let expiry = tokio::time::sleep_until(deadline.unwrap_or_else(|| {
                tokio::time::Instant::now() + Duration::from_secs(365 * 24 * 3600)
            }));
            tokio::select! {
                biased;
                result = &mut prompt, if !prompted => {
                    prompted = true;
                    if let Err(failure) = result {
                        session.dead = true;
                        return if signal.aborted() {
                            TurnOutcome::Aborted
                        } else {
                            TurnOutcome::Failed(failure)
                        };
                    }
                    if stopping {
                        // Stopped while the harness took the prompt: its turn exists now.
                        self.interrupt(&session.control);
                    }
                }
                event = session.events.recv() => {
                    let Some(event) = event else {
                        session.dead = true;
                        return TurnOutcome::Failed(HarnessFailure::new(
                            code::CLOSED,
                            "the harness event stream ended",
                        ));
                    };
                    if let HarnessEvent::SessionEnded { .. } = &event {
                        session.dead = true;
                    }
                    if let Some(outcome) = log.apply(event, session, &turn_id) {
                        if log.broken {
                            // It broke the contract: it does not run on (the next turn starts
                            // a new process, asking for a wider policy again).
                            session.control.terminate(self.terminate_grace()).await;
                            session.dead = true;
                        }
                        return outcome;
                    }
                }
                _ = signal.cancelled(), if !stopping => {
                    log.withdraw_requests();
                    if self.shared.harness.stop_ends_process() {
                        // Nothing of the stopped turn runs on or reaches the next one: that turn
                        // starts the process again and continues the native session. Until then
                        // the window shows no running process.
                        session.control.terminate(self.terminate_grace()).await;
                        *live = None;
                        lock(&self.state).control = None;
                        return TurnOutcome::Aborted;
                    }
                    deadline = Some(tokio::time::Instant::now() + self.interrupt_grace());
                    // Before the harness accepts the prompt it has no turn to interrupt: the
                    // interrupt goes out with its answer.
                    if prompted {
                        self.interrupt(&session.control);
                    }
                }
                () = expiry, if stopping => {
                    // The native interrupt did not end the turn in time: stop the process.
                    session.control.terminate(self.terminate_grace()).await;
                    session.dead = true;
                    return TurnOutcome::Aborted;
                }
            }
        }
    }

    /// Ask the harness to stop its turn (bounded; the caller's deadline decides what follows).
    fn interrupt(&self, control: &Arc<dyn HarnessSession>) {
        let control = control.clone();
        let grace = self.interrupt_grace();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(grace, control.interrupt()).await;
        });
    }

    /// A live native session for the next prompt: the running one, or a new one that continues
    /// the recorded native id (a fresh one when the harness lost it).
    async fn ensure_live(
        self: &Arc<Self>,
        live: &mut Option<LiveSession>,
        signal: &AbortSignal,
    ) -> Result<(), HarnessFailure> {
        let (effort, interaction_mode) = {
            let state = lock(&self.state);
            (state.effort.clone(), state.interaction_mode)
        };
        if let Some(session) = live.as_mut() {
            while let Ok(event) = session.events.try_recv() {
                match event {
                    HarnessEvent::SessionEnded { .. } => session.dead = true,
                    HarnessEvent::NativeSession { native_session_id } => {
                        self.record_native(&native_session_id, None)
                    }
                    HarnessEvent::Telemetry { record } => self.record_telemetry(record),
                    // Outside a turn nobody can be asked: the request is withdrawn, not left
                    // waiting.
                    HarnessEvent::ApprovalRequested { request_id, .. }
                    | HarnessEvent::QuestionAsked { request_id, .. } => {
                        let control = session.control.clone();
                        let withdrawn =
                            respond(self, &control, request_id, HarnessResponse::Cancelled);
                        let _ = tokio::time::timeout(self.interrupt_grace(), withdrawn).await;
                    }
                    // Masked like everything else from the harness: it may quote keys.
                    other => self.ctx.logger().debug(format_args!(
                        "dropped a harness event between turns: {}",
                        js_json_stringify(&redact_value(&json!(other)))
                    )),
                }
            }
            let chosen = session.access != self.shared.access();
            // The window's mode choice ends an idle process outside this driver and forgets its
            // launch (`Shared::retire_idle`); a terminate may say nothing on the events, so the
            // mode being back to this session's does not make the process alive again.
            let retired = self.shared.running_launch_of(self.id.as_ref()).is_none();
            // Another effort level is another process on the same native session, as in T3.
            let other_effort = session.effort != effort;
            let other_interaction = session.interaction_mode != interaction_mode;
            if session.dead
                || chosen
                || retired
                || other_effort
                || other_interaction
                || self.model.restart.swap(false, Ordering::SeqCst)
            {
                if let Some(old) = live.take() {
                    old.control.terminate(self.terminate_grace()).await;
                }
                lock(&self.state).control = None;
            }
        }
        if live.is_some() {
            return Ok(());
        }
        self.model.restart.store(false, Ordering::SeqCst);
        // Read at every start, from the user's choice in the window only.
        let access = self.shared.access();
        let policy = self.policy_for(access)?;
        // `never` (built only with `never-mode`) is put to the user once per process; full
        // access was already the user's own action in the window, so it is not asked again.
        let escalated = if access.asks() && policy.approval == ApprovalMode::Never {
            self.ask_escalation(&policy, signal).await?;
            true
        } else {
            false
        };
        let flags = self.shared.harness.launch_flags(&policy, access);
        let resume = lock(&self.state).native.clone();
        let mut params = StartParams {
            session_id: self.id.to_string(),
            cwd: self.cwd(),
            model: lock(&self.model.model).clone(),
            effort: effort.clone(),
            interaction_mode,
            policy,
            resume: resume.clone(),
        };
        let harness = &self.shared.harness;
        let (started, lost) = match harness.start_with_access(params.clone(), access).await {
            Err(failure) if failure.code == code::SESSION_NOT_FOUND && params.resume.is_some() => {
                self.ctx.logger().warn(format_args!(
                    "harness \"{}\" lost native session {:?}: starting a new one",
                    self.shared.name, params.resume
                ));
                params.resume = None;
                (harness.start_with_access(params, access).await?, resume)
            }
            started => {
                let started = started?;
                // A resume the harness did not confirm: it started a new native session itself.
                let lost = resume.filter(|_| !started.result.resumed);
                if let Some(lost) = &lost {
                    self.ctx.logger().warn(format_args!(
                        "harness \"{}\" did not continue native session {lost:?}: it started {:?}",
                        self.shared.name, started.result.native_session_id
                    ));
                }
                (started, lost)
            }
        };
        self.record_native(&started.result.native_session_id, lost.as_ref());
        self.shared.record_launch(
            self.id.as_ref(),
            Launched {
                access,
                flags,
                session: Arc::downgrade(&started.session),
            },
        );
        lock(&self.state).control = Some(started.session.clone());
        *live = Some(LiveSession {
            control: started.session,
            events: started.events,
            escalated,
            dead: false,
            access,
            effort,
            interaction_mode,
        });
        Ok(())
    }

    /// The effort level of this turn (HOH-40): the one its own prompt named in `session/prompt`
    /// (none: the harness's own), so a prompt queued behind it does not decide it. A level the
    /// harness does not have is not passed on, and said in the log. A turn whose prompt named
    /// nothing through `session/prompt` (a wake, another method) keeps the last level.
    fn take_prompt_choices(&self, input: &TurnInput) {
        let Some(prompt) = input.messages.last().filter(|_| input.last_from_next_turn) else {
            return;
        };
        if let Some(named) = self
            .shared
            .choices
            .take_effort(self.id.as_ref(), prompt.id.as_ref())
        {
            let effort = named.filter(|effort| {
                let known = self
                    .shared
                    .harness
                    .effort_levels()
                    .contains(&effort.as_str());
                if !known {
                    self.ctx.logger().warn(format_args!(
                        "harness \"{}\" has no effort level {}: the turn runs without one",
                        self.shared.name,
                        js_json_stringify(&json!(effort))
                    ));
                }
                known
            });
            lock(&self.state).effort = effort;
        }
        if let Some(mode) = self
            .shared
            .choices
            .take_interaction(self.id.as_ref(), prompt.id.as_ref())
        {
            lock(&self.state).interaction_mode = mode;
        }
    }

    /// [`launch_policy`], said in the log when it caps the configuration; `never` without its
    /// build is refused.
    fn policy_for(&self, access: AccessMode) -> Result<HarnessPolicy, HarnessFailure> {
        let configured = self.shared.config.policy;
        if configured.approval == ApprovalMode::Never && !NEVER_MODE {
            // Before any process: checked at every start.
            return Err(HarnessFailure::new(
                code::POLICY_UNAVAILABLE,
                never_off(&self.shared.name),
            ));
        }
        let policy = launch_policy(&configured, access);
        if access.asks() && policy != configured {
            self.ctx.logger().warn(format_args!(
                "the configured policy {} of harness \"{}\" is wider than the access mode the \
                 user chose ({}): started with {}",
                js_json_stringify(&json!(configured)),
                self.shared.name,
                access.as_str(),
                js_json_stringify(&json!(policy))
            ));
        }
        Ok(policy)
    }

    /// Put `approval: never` to the user (a build with `never-mode` only); only `allowed-once`
    /// starts it.
    async fn ask_escalation(
        &self,
        policy: &HarnessPolicy,
        signal: &AbortSignal,
    ) -> Result<(), HarnessFailure> {
        let Some(agent) = self.as_dyn() else {
            return Err(HarnessFailure::new(code::CLOSED, "agent is gone"));
        };
        let headline = format!(
            "Разрешить исполнителю «{}» работать без вопросов только о командах и правках файлов, \
             которые остаются в папке сессии: всё, что удаляет или перезаписывает файлы, касается \
             путей вне папки или сети, он спросит",
            self.shared.name
        );
        let mut request = ApprovalRequest::new(agent, "external-harness");
        request.reason = Some(format!(
            "{headline}. {NEVER_RULE}. Разрешение действует до конца этого запуска исполнителя"
        ));
        request.details = Some(format!(
            "Режим исполнителя: {}",
            js_json_stringify(&json!(*policy))
        ));
        request.display_reason = Some(DisplayReason::new(one_line(&headline)));
        request.signal = Some(signal.clone());
        match self.shared.approval.request(request).await {
            Ok(ApprovalOutcome::AllowedOnce) => Ok(()),
            Ok(outcome) => Err(HarnessFailure::new(
                code::ESCALATION_DENIED,
                format!(
                    "the wider harness policy was not allowed ({})",
                    outcome.as_str()
                ),
            )),
            Err(error) => Err(HarnessFailure::new(
                code::ESCALATION_DENIED,
                format!("the wider harness policy could not be asked: {error}"),
            )),
        }
    }

    /// The step stays inside the session folder: a command run there whose every path-like word
    /// (arguments, option values, assignments, redirections, the program's own path) lies there
    /// and that runs no network program, or adding and changing files there. A command line the
    /// classifier's `command_reach` does not understand, a request for a wider permission or a
    /// tool call never is. Links in the part of a path that exists are followed (on a blocking
    /// thread).
    async fn within_workspace(&self, action: HarnessAction) -> bool {
        let base = PathBuf::from(self.cwd());
        let inside_all = move || match &action {
            HarnessAction::Command { command, cwd } => {
                let dir = cwd
                    .as_deref()
                    .map_or_else(|| base.clone(), |cwd| base.join(cwd));
                within(&base, &dir)
                    && command_reach(command).is_some_and(|reach| {
                        reach.network.is_empty()
                            && reach.paths.iter().all(|path| {
                                !path.starts_with('~') && within(&base, &dir.join(path))
                            })
                    })
            }
            HarnessAction::FileChange { changes } => {
                !changes.is_empty()
                    && changes.iter().all(|change| {
                        change.kind != FileChangeKind::Delete
                            && inside(&base, &change.path)
                            && change
                                .move_path
                                .as_deref()
                                .is_none_or(|path| inside(&base, path))
                    })
            }
            HarnessAction::Tool { .. } | HarnessAction::Permission { .. } => false,
        };
        tokio::task::spawn_blocking(inside_all)
            .await
            .unwrap_or(false)
    }

    /// The classifier's question for an action; `None` when it is not dangerous.
    async fn danger(&self, action: &HarnessAction) -> Option<Question> {
        let base = PathBuf::from(self.cwd());
        let classifier = &self.shared.classifier;
        match action {
            HarnessAction::Command { command, cwd } => {
                let cwd = cwd
                    .as_ref()
                    .map_or_else(|| base.clone(), |cwd| base.join(cwd));
                classifier.command(command, Some(cwd)).await
            }
            HarnessAction::FileChange { changes } => {
                let deleted: Vec<String> = changes
                    .iter()
                    .filter(|change| change.kind == FileChangeKind::Delete)
                    .map(|change| change.path.clone())
                    .collect();
                let targets: Vec<String> = changes
                    .iter()
                    .filter_map(|change| change.move_path.clone())
                    .collect();
                // `overwrite` looks at the file system: on a blocking thread, like the command
                // classifier (review 220312, item 6). The question is the same either way.
                let decide = {
                    let classifier = classifier.clone();
                    move || {
                        classifier
                            .deletion(&deleted, Some(&base))
                            .or_else(|| classifier.overwrite(&targets, Some(&base)))
                    }
                };
                let retry = decide.clone();
                match tokio::task::spawn_blocking(decide).await {
                    Ok(question) => question,
                    // The blocking pool refused the task: decide here rather than not at all.
                    Err(_) => retry(),
                }
            }
            HarnessAction::Tool { .. } | HarnessAction::Permission { .. } => None,
        }
    }
}

/// The text the harness gets for a turn: the text blocks of its messages.
fn prompt_text(messages: &[UserMessage]) -> String {
    messages
        .iter()
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(ContentBlock::text_value)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The pictures of a turn's messages (HOH-41), read back from the engine's attachment store: the
/// copy the store made when the window sent the bytes (`session/prompt`, checked there against the
/// store's limits and types), never a file the window named.
async fn prompt_images(
    ctx: &Context,
    messages: &[UserMessage],
    signal: &AbortSignal,
) -> Result<Vec<PromptImage>, HarnessFailure> {
    use base64::Engine as _;
    let mut images = Vec::new();
    for image in messages
        .iter()
        .flat_map(|message| message.content.iter().filter_map(ContentBlock::image))
    {
        let store = ctx.get::<hoh_attachment::Attachments>().ok_or_else(|| {
            HarnessFailure::new(code::HARNESS, "the engine has no attachment store")
        })?;
        let stored = store
            .read_image(&image.attachment, Some(signal))
            .await
            .map_err(|error| {
                HarnessFailure::new(code::HARNESS, format!("an attached picture: {error}"))
            })?;
        images.push(PromptImage {
            media_type: stored.r#ref.media_type.as_str().to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(&stored.data),
            name: stored.r#ref.name.clone(),
        });
    }
    Ok(images)
}

/// One line the window shows as it is, at most [`MAX_HEADLINE_CHARS`] characters: every character
/// the protocol refuses in `humanText` (a control, spacing other than a plain space, an invisible
/// mark such as U+FE0F, U+00A0 or U+200B) becomes a space, and runs of spaces become one. A
/// refused headline would be replaced by the host's fallback without the command.
fn one_line(text: &str) -> String {
    let mut buffer = [0; 4];
    let cleaned: String = text
        .chars()
        .map(|c| {
            let refused =
                hoh_protocol::approval::validate_human_text(c.encode_utf8(&mut buffer)).is_err();
            if refused { ' ' } else { c }
        })
        .collect();
    let words: Vec<&str> = cleaned.split(' ').filter(|word| !word.is_empty()).collect();
    truncate_chars(&words.join(" "), MAX_HEADLINE_CHARS)
}

/// `path` (relative to `base`, or absolute) lies within `base`, as the system would open it.
fn inside(base: &Path, path: &str) -> bool {
    within(base, &base.join(path))
}

/// `path` lies within `base`, as the system would open it.
fn within(base: &Path, path: &Path) -> bool {
    match (resolved(base), resolved(path)) {
        (Some(base), Some(path)) => path.starts_with(base),
        _ => false,
    }
}

/// `path` the way the system walks it: each existing part with its links followed before the
/// next, so `..` leaves the real folder a link points to. The part that does not exist yet is
/// kept as written; `..` there, a broken link or an unreadable part give `None` (not judged).
fn resolved(path: &Path) -> Option<PathBuf> {
    let mut real = PathBuf::new();
    let mut exists = true;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !exists || !real.pop() {
                    return None;
                }
            }
            other => {
                real.push(other);
                if exists {
                    match real.symlink_metadata() {
                        Ok(_) => real = real.canonicalize().ok()?,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            exists = false;
                        }
                        Err(_) => return None,
                    }
                }
            }
        }
    }
    Some(real)
}

/// A question that cannot show the whole action says so where it is always seen: the user must
/// not approve a command whose hidden tail was not read.
fn flag_incomplete(mut question: Question, action: &HarnessAction) -> Question {
    let (complete, size) = match action {
        HarnessAction::Command { command, .. } => (
            question.details.contains(command.as_str()),
            format!("{} знаков", command.chars().count()),
        ),
        HarnessAction::Tool { input, .. } => {
            let input = js_json_stringify(input);
            (
                question.details.contains(&input),
                format!("{} знаков", input.chars().count()),
            )
        }
        HarnessAction::Permission { description } => (
            question.details.contains(description.as_str()),
            format!("{} знаков", description.chars().count()),
        ),
        HarnessAction::FileChange { changes } => (
            question.details.contains(&change_list(changes)),
            format!("изменений в файлах: {}", changes.len()),
        ),
    };
    if complete {
        return question;
    }
    const SUFFIX: &str = " (показано не полностью)";
    let room = MAX_HEADLINE_CHARS - SUFFIX.chars().count();
    question.headline = format!("{}{SUFFIX}", truncate_chars(&question.headline, room));
    question.details.push_str(&format!(
        "\n[Показано не полностью, всего {size}. Одобряйте, только если доверяете исполнителю \
         целиком.]"
    ));
    question
}

fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Keep the last [`MAX_OUTPUT_CHARS`] characters of `buffer`.
fn keep_tail(buffer: &mut String) {
    let count = buffer.chars().count();
    if count > MAX_OUTPUT_CHARS {
        let skip = count - MAX_OUTPUT_CHARS;
        let start = buffer
            .char_indices()
            .nth(skip)
            .map_or(buffer.len(), |(index, _)| index);
        buffer.replace_range(..start, "");
    }
}

/// How a step appears in the log: the tool name and arguments the window already knows.
fn step_call(action: &HarnessAction) -> (String, Value) {
    match action {
        HarnessAction::Command { command, cwd } => {
            let mut arguments = json!({"command": command});
            if let Some(cwd) = cwd {
                arguments["workdir"] = json!(cwd);
            }
            ("bash".to_owned(), arguments)
        }
        HarnessAction::FileChange { changes } => (
            "edit_file".to_owned(),
            json!({
                "path": changes.first().map(|change| change.path.clone()),
                "changes": changes,
            }),
        ),
        HarnessAction::Tool { name, input } => (name.clone(), input.clone()),
        HarnessAction::Permission { description } => {
            ("permission".to_owned(), json!({"description": description}))
        }
    }
}

/// The question for an action the classifier finds harmless (or cannot judge).
fn plain_question(harness: &str, action: &HarnessAction, reason: Option<&str>) -> Question {
    // The executor by its name as a word (`Claude`), and what it asks for by what it does: the
    // words of a person who never saw a command line.
    let who = display_name(harness);
    let (headline, details) = match action {
        HarnessAction::Command { command, .. } => (
            format!("{who} просит выполнить команду: {command}"),
            format!("Команда: {command}"),
        ),
        HarnessAction::FileChange { changes } => {
            let shown = match changes.as_slice() {
                [only] => match (&only.move_path, only.kind) {
                    (Some(_), _) => format!("переместить файл {}", only.path),
                    (None, FileChangeKind::Add) => format!("создать файл {}", only.path),
                    (None, FileChangeKind::Update) => format!("изменить файл {}", only.path),
                    (None, FileChangeKind::Delete) => format!("удалить файл {}", only.path),
                },
                _ => {
                    let names: Vec<&str> =
                        changes.iter().map(|change| change.path.as_str()).collect();
                    let more = if names.len() > 4 {
                        " и другие"
                    } else {
                        ""
                    };
                    format!(
                        "изменить файлы: {}{more}",
                        names.iter().take(4).copied().collect::<Vec<_>>().join(", ")
                    )
                }
            };
            (format!("{who} просит {shown}"), change_list(changes))
        }
        HarnessAction::Tool { name, input } => (
            format!("{who} просит разрешения на действие «{name}»"),
            format!("Данные: {}", js_json_stringify(input)),
        ),
        HarnessAction::Permission { description } => (
            format!("{who} просит дополнительное разрешение"),
            description.clone(),
        ),
    };
    let details = match reason {
        Some(reason) => format!("{details}\nОбоснование исполнителя: {reason}"),
        None => details,
    };
    // Nothing to add to the headline: the explanation stays empty (the window shows no block).
    Question {
        explanation: String::new(),
        headline: one_line(&headline),
        details: truncate_chars(&details, MAX_DETAILS_CHARS),
    }
}

/// Every change of a file change, one per line: the user approves them all at once.
fn change_list(changes: &[FileChange]) -> String {
    let lines: Vec<String> = changes
        .iter()
        .map(|change| match (&change.move_path, change.kind) {
            (Some(target), _) => format!("переместить: {} → {target}", change.path),
            (None, FileChangeKind::Add) => format!("создать: {}", change.path),
            (None, FileChangeKind::Update) => format!("изменить: {}", change.path),
            (None, FileChangeKind::Delete) => format!("удалить: {}", change.path),
        })
        .collect();
    format!("Изменения:\n{}", lines.join("\n"))
}

/// The tool name an approval request carries for an action.
fn approval_tool(action: &HarnessAction) -> String {
    step_call(action).0
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn text_delta(text: &str) -> Option<StreamChunk> {
    serde_json::from_value(json!({"type": "text-delta", "index": 0, "text": text})).ok()
}

pub(crate) struct LiveSession {
    control: Arc<dyn HarnessSession>,
    events: mpsc::Receiver<HarnessEvent>,
    escalated: bool,
    dead: bool,
    /// The access mode it was started under: another choice in the window restarts it.
    access: AccessMode,
    /// The effort level it was started with: another one in a prompt restarts it.
    effort: Option<String>,
    /// The interaction mode it was started with: another prompt mode restarts it.
    interaction_mode: InteractionMode,
}

enum TurnOutcome {
    Completed,
    Aborted,
    Failed(HarnessFailure),
}

struct LiveText {
    message_id: String,
    attempt_id: LlmAttemptId,
    revision: u64,
    chunks: u64,
    /// The text as the harness sent it; masked when written or shown.
    text: String,
    /// How much of `text` the chunks showed (masked): the rest may still grow into a secret.
    shown: usize,
}

struct OpenItem {
    item_id: String,
    call_seq: SessionSeq,
    parent_item_id: Option<String>,
    command: bool,
    /// A command or a file change: under `approval: never` it runs only after `allow`.
    guarded: bool,
    /// The action it started as (`None`: completed without a start).
    action: Option<HarnessAction>,
    output: String,
}

type OpenQuestions = Arc<Mutex<HashMap<String, (SessionSeq, Option<String>)>>>;

/// Everything one turn writes to the log.
struct TurnLog {
    agent: Arc<ExternalAgent>,
    turn: u64,
    step: u64,
    signal: AbortSignal,
    live: Vec<LiveText>,
    reasoning: Vec<(String, String, Option<String>)>,
    items: Vec<OpenItem>,
    requests: HashMap<String, Arc<AbortController>>,
    tasks: Vec<Disposer>,
    /// Question calls of this turn still without a result, by call id. A question task writes
    /// its result only while the call is here; the end of the turn closes the rest.
    questions: OpenQuestions,
    /// Native message and item ids this turn already settled: a repeated final or completion
    /// is dropped.
    settled: HashSet<String>,
    /// `approval: never`: commands and file changes run only after an allowed request.
    never: bool,
    /// Steps HoH allowed in this turn, by item id, with the action that was allowed (inserted
    /// before the answer is sent).
    allowed: Arc<Mutex<HashMap<String, HarnessAction>>>,
    /// The harness broke the contract: the turn ends and the process is stopped.
    broken: bool,
    /// The harness opened this turn (`turn-started` with its id). A request before that was made
    /// between turns, whenever it reaches HoH: it is withdrawn, nobody is asked.
    opened: bool,
    usage: Option<HarnessUsage>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessUsage {
    input_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    output_tokens: u64,
    max_tokens: Option<u64>,
}

impl TurnLog {
    fn new(agent: Arc<ExternalAgent>, turn: u64, step: u64, signal: AbortSignal) -> Self {
        // Under full access the CLI asks nothing: `never`'s rule applies only with confirmation.
        let never = agent.shared.config.policy.approval == ApprovalMode::Never
            && agent.shared.access().asks();
        Self {
            agent,
            turn,
            step,
            signal,
            live: Vec::new(),
            reasoning: Vec::new(),
            items: Vec::new(),
            requests: HashMap::new(),
            tasks: Vec::new(),
            questions: Arc::default(),
            settled: HashSet::new(),
            never,
            allowed: Arc::default(),
            broken: false,
            opened: false,
            usage: None,
        }
    }

    /// A step that ran without the allowed request `approval: never` requires: no `allow` for its
    /// id, or the step started as another action than the one allowed (`ls` asked, `rm` run).
    fn unasked(&self, item_id: &str, guarded: bool, started: Option<&HarnessAction>) -> bool {
        if !self.never || !guarded {
            return false;
        }
        match lock(&self.allowed).get(item_id) {
            None => true,
            Some(allowed) => started.is_some_and(|started| started != allowed),
        }
    }

    /// Record a step the harness ran without asking, and end the turn with the harness stopped.
    fn bypassed(&mut self, item_id: &str, output: Option<String>) -> TurnOutcome {
        self.broken = true;
        if self.settled.insert(format!("item:{item_id}")) {
            let item = match self.items.iter().position(|item| item.item_id == item_id) {
                Some(index) => Some(self.items.remove(index)),
                None => self
                    .write_call(item_id, "step", &json!({}), None)
                    .map(|call_seq| OpenItem {
                        item_id: item_id.to_owned(),
                        call_seq,
                        parent_item_id: None,
                        command: false,
                        guarded: true,
                        action: None,
                        output: String::new(),
                    }),
            };
            if let Some(item) = item {
                let mut text = redact(&output.unwrap_or(item.output));
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str("[The harness ran this step without asking for permission.]");
                self.write_result(
                    item_id,
                    item.call_seq,
                    text,
                    Some(code::APPROVAL_BYPASSED),
                    item.parent_item_id.as_deref(),
                );
            }
        }
        TurnOutcome::Failed(HarnessFailure::new(
            code::APPROVAL_BYPASSED,
            format!(
                "the harness ran step {item_id} without asking for permission (approval: never); \
                 it was stopped"
            ),
        ))
    }

    /// A request id used twice in a turn: the harness would wait for an answer that is not
    /// coming, or take one meant for another request.
    fn repeated(&mut self, request_id: &str) -> TurnOutcome {
        self.broken = true;
        TurnOutcome::Failed(HarnessFailure::new(
            code::PROTOCOL,
            format!("the harness repeated request id {request_id}"),
        ))
    }

    fn warn(&self, what: &str, error: impl std::fmt::Display) {
        self.agent
            .ctx
            .logger()
            .warn(format_args!("turn {}: {what}: {error}", self.turn));
    }

    /// Apply one event; `Some` ends the turn.
    fn apply(
        &mut self,
        event: HarnessEvent,
        session: &LiveSession,
        turn_id: &str,
    ) -> Option<TurnOutcome> {
        match event {
            HarnessEvent::NativeSession { native_session_id } => {
                self.agent.record_native(&native_session_id, None);
            }
            HarnessEvent::TurnStarted { turn_id: id } => {
                if id == turn_id {
                    self.opened = true;
                }
            }
            HarnessEvent::Status { .. } => {}
            HarnessEvent::Telemetry { record } => self.agent.record_telemetry(record),
            HarnessEvent::Usage {
                input_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
                output_tokens,
                max_tokens,
            } => {
                let usage = HarnessUsage {
                    input_tokens,
                    cache_creation_input_tokens,
                    cache_read_input_tokens,
                    output_tokens,
                    max_tokens,
                };
                // Usage is a live projection, not merely turn-end accounting. Persisting the
                // snapshot also broadcasts it through session.event so T3 can repaint its native
                // context component while Claude is still streaming.
                if let Err(error) = self.agent.session.append(
                    "turn/usage",
                    json!({"turn": self.turn, "usage": usage}),
                    None,
                ) {
                    self.warn("cannot record context usage", error);
                }
                self.usage = Some(usage);
            }
            // Sent before the harness took this turn's prompt: the request belongs to the time
            // between turns (after the last turn ended), even when it reaches HoH only now. It is
            // withdrawn like one found before the prompt, never put to the user as this turn's.
            HarnessEvent::ApprovalRequested { request_id, .. }
            | HarnessEvent::QuestionAsked { request_id, .. }
                if !self.opened =>
            {
                return self.refuse(session, request_id, "it came before the turn started");
            }
            HarnessEvent::TurnCompleted { turn_id: id } if id == turn_id => {
                return Some(TurnOutcome::Completed);
            }
            HarnessEvent::TurnAborted { turn_id: id } if id == turn_id => {
                return Some(if self.signal.aborted() {
                    TurnOutcome::Aborted
                } else {
                    TurnOutcome::Failed(HarnessFailure::new(
                        code::HARNESS,
                        "the harness stopped the turn on its own",
                    ))
                });
            }
            HarnessEvent::TurnFailed { turn_id: id, error } if id == turn_id => {
                return Some(TurnOutcome::Failed(error));
            }
            HarnessEvent::TurnCompleted { .. }
            | HarnessEvent::TurnAborted { .. }
            | HarnessEvent::TurnFailed { .. } => {
                // A stale end of an earlier turn.
            }
            HarnessEvent::TextDelta {
                message_id,
                text,
                parent_item_id,
            } => self.delta(message_id, &text, parent_item_id.as_deref()),
            HarnessEvent::TextFinal {
                message_id,
                text,
                parent_item_id,
            } => self.final_text(&message_id, text, parent_item_id.as_deref()),
            HarnessEvent::ReasoningDelta {
                message_id,
                text,
                parent_item_id,
            } => {
                match self.reasoning.iter_mut().find(|(id, _, parent)| {
                    *id == message_id && parent.as_deref() == parent_item_id.as_deref()
                }) {
                    Some((_, buffer, _)) => {
                        buffer.push_str(&text);
                        keep_tail(buffer);
                    }
                    None => self.reasoning.push((message_id, text, parent_item_id)),
                }
            }
            HarnessEvent::ReasoningFinal {
                message_id,
                text,
                parent_item_id,
            } => {
                self.reasoning.retain(|(id, _, parent)| {
                    *id != message_id || parent.as_deref() != parent_item_id.as_deref()
                });
                self.reasoning.push((message_id, text, parent_item_id));
            }
            HarnessEvent::ItemStarted {
                item_id,
                action,
                parent_item_id,
            } => self.start_item(item_id, &action, parent_item_id),
            HarnessEvent::ItemOutput {
                item_id,
                text,
                parent_item_id: _,
            } => {
                if let Some(item) = self.items.iter_mut().find(|item| item.item_id == item_id) {
                    item.output.push_str(&text);
                    keep_tail(&mut item.output);
                    let (guarded, started) = (item.guarded, item.action.clone());
                    if self.unasked(&item_id, guarded, started.as_ref()) {
                        return Some(self.bypassed(&item_id, None));
                    }
                }
            }
            HarnessEvent::ItemCompleted {
                item_id,
                outcome,
                output,
                exit_code,
                parent_item_id,
            } => {
                let ran = matches!(outcome, ItemOutcome::Completed | ItemOutcome::Failed);
                let (guarded, started) =
                    match self.items.iter().find(|item| item.item_id == item_id) {
                        Some(item) => (item.guarded, item.action.clone()),
                        // A completion without a start: a command when it has an exit code.
                        None => (exit_code.is_some(), None),
                    };
                if ran
                    && !self.settled.contains(&format!("item:{item_id}"))
                    && self.unasked(&item_id, guarded, started.as_ref())
                {
                    return Some(self.bypassed(&item_id, output));
                }
                self.complete_item(&item_id, outcome, output, exit_code, parent_item_id);
            }
            HarnessEvent::ApprovalRequested {
                request_id,
                item_id,
                action,
                reason,
                parent_item_id: _,
            } => return self.spawn_approval(session, request_id, item_id, action, reason),
            HarnessEvent::QuestionAsked {
                request_id,
                questions,
                parent_item_id,
            } => return self.spawn_question(session, request_id, questions, parent_item_id),
            HarnessEvent::RequestResolved { request_id } => {
                if let Some(controller) = self.requests.remove(&request_id) {
                    controller.abort();
                }
            }
            HarnessEvent::SessionEnded {
                reason,
                message,
                exit_code,
            } => {
                let what = match reason {
                    EndReason::Exited => "the harness process exited",
                    EndReason::Crashed => "the harness process crashed",
                    EndReason::StreamClosed => "the harness output stream broke",
                    EndReason::ProtocolError => "the harness broke the contract",
                    EndReason::Terminated => "the harness process was stopped",
                };
                let mut text = what.to_owned();
                if let Some(exit_code) = exit_code {
                    text.push_str(&format!(" (exit code {exit_code})"));
                }
                if let Some(message) = message.filter(|message| !message.is_empty()) {
                    text.push_str(": ");
                    text.push_str(&message);
                }
                return Some(TurnOutcome::Failed(HarnessFailure::new(code::CLOSED, text)));
            }
        }
        None
    }

    fn delta(&mut self, message_id: String, text: &str, parent_item_id: Option<&str>) {
        if self.settled.contains(&format!("text:{message_id}")) {
            return;
        }
        // The window-only live stream has no parent field. Never flatten a subagent delta into
        // the root answer; its authoritative `text-final` is recorded below with `parentCallId`.
        if parent_item_id.is_some() {
            return;
        }
        let index = match self
            .live
            .iter()
            .position(|live| live.message_id == message_id)
        {
            Some(index) => index,
            None => {
                let attempt_id =
                    LlmAttemptId::from(format!("{}:{}:{}", self.agent.id, self.turn, message_id));
                let revision = self.agent.revision.fetch_add(1, Ordering::Relaxed) + 1;
                self.agent.emit_frame(AssistantStreamFrame::Start {
                    attempt_id: attempt_id.clone(),
                    revision,
                    turn: self.turn,
                    step: self.step,
                });
                self.live.push(LiveText {
                    message_id,
                    attempt_id,
                    revision,
                    chunks: 0,
                    text: String::new(),
                    shown: 0,
                });
                self.live.len() - 1
            }
        };
        let live = &mut self.live[index];
        live.text.push_str(text);
        // A tail that may still be the start of a secret waits for the next chunk or the end.
        let cut = ready_to_show(&live.text, live.shown);
        let piece = redact(&live.text[live.shown..cut]);
        live.shown = cut;
        self.agent.emit_frame_of(live, &piece);
    }

    fn final_text(&mut self, message_id: &str, text: String, parent_item_id: Option<&str>) {
        if !self.settled.insert(format!("text:{message_id}")) {
            return;
        }
        let live = parent_item_id
            .is_none()
            .then(|| {
                self.live
                    .iter()
                    .position(|live| live.message_id == message_id)
                    .map(|index| self.live.remove(index))
                    .map(|live| self.shown_whole(live))
            })
            .flatten();
        let seq = self.write_assistant(text, false, Vec::new(), parent_item_id);
        if let Some(live) = live {
            self.end_live(live, seq);
        }
    }

    /// Append one `assistant/message`: the pending reasoning, `text`, then the tool calls it
    /// advertises; `None` when there is nothing to write.
    fn write_assistant(
        &mut self,
        text: String,
        interrupted: bool,
        calls: Vec<ContentBlock>,
        parent_item_id: Option<&str>,
    ) -> Option<SessionSeq> {
        let mut content = Vec::new();
        let mut reasoning = Vec::new();
        let mut other_reasoning = Vec::new();
        for (message_id, text, parent) in self.reasoning.drain(..) {
            if parent.as_deref() == parent_item_id {
                if !text.is_empty() {
                    reasoning.push(text);
                }
            } else {
                other_reasoning.push((message_id, text, parent));
            }
        }
        self.reasoning = other_reasoning;
        if !reasoning.is_empty() {
            content.push(ContentBlock::reasoning(redact(&reasoning.join("\n\n"))));
        }
        if !text.is_empty() {
            content.push(ContentBlock::text(redact(&text)));
        }
        content.extend(calls);
        if content.is_empty() {
            return None;
        }
        let message = create_assistant_message(NewAssistantMessage {
            content,
            source: AssistantProviderMetadata {
                provider: self.agent.shared.name.clone(),
                model: lock(&self.agent.model.model)
                    .clone()
                    .filter(|model| !model.is_empty())
                    .unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
                replay_state: None,
            },
        });
        // `stream` is the loop's record of model chunks (a settlement field the store checks on
        // reload); the harness's own stream is not kept.
        let mut data = json!({"turn": self.turn, "step": self.step, "stream": []});
        match serde_json::to_value(&message) {
            Ok(message) => data["message"] = message,
            Err(error) => {
                self.warn("cannot encode the answer", error);
                return None;
            }
        }
        if interrupted {
            data["interrupted"] = json!(true);
        }
        if let Some(parent_item_id) = parent_item_id {
            data["parentCallId"] = json!(parent_item_id);
        }
        match self
            .agent
            .session
            .append("assistant/message", data, Some(SurfaceIntent::append()))
        {
            Ok(event) => Some(event.seq()),
            Err(error) => {
                self.warn("cannot record the answer", error);
                None
            }
        }
    }

    /// The rest of a live text the wait for a whole word held back, shown before the answer is
    /// recorded: the window puts the recorded answer in the place of the live text it has seen, so
    /// a piece after the record would show the answer twice (pilot 22.11: a one-word «ok1» is held
    /// back whole until its final).
    fn shown_whole(&self, mut live: LiveText) -> LiveText {
        let rest = redact(&live.text[live.shown..]);
        self.agent.emit_frame_of(&mut live, &rest);
        live.shown = live.text.len();
        live
    }

    /// The end of a live text, [`shown_whole`](Self::shown_whole) already.
    fn end_live(&self, live: LiveText, seq: Option<SessionSeq>) {
        self.agent.emit_frame(AssistantStreamFrame::End {
            attempt_id: live.attempt_id.clone(),
            revision: live.revision,
            index: live.chunks,
            outcome: match seq {
                Some(seq) => AssistantStreamOutcome::Committed {
                    event_type: "assistant/message",
                    seq,
                },
                None => AssistantStreamOutcome::Abandoned,
            },
        });
    }

    fn start_item(
        &mut self,
        item_id: String,
        action: &HarnessAction,
        parent_item_id: Option<String>,
    ) {
        if self.settled.contains(&format!("item:{item_id}"))
            || self.items.iter().any(|item| item.item_id == item_id)
        {
            return;
        }
        let (name, arguments) = step_call(action);
        let Some(call_seq) =
            self.write_call(&item_id, &name, &arguments, parent_item_id.as_deref())
        else {
            return;
        };
        self.items.push(OpenItem {
            item_id,
            call_seq,
            parent_item_id,
            command: matches!(action, HarnessAction::Command { .. }),
            guarded: matches!(
                action,
                HarnessAction::Command { .. } | HarnessAction::FileChange { .. }
            ),
            action: Some(action.clone()),
            output: String::new(),
        });
    }

    /// Record a step as the loop does: an `assistant/message` advertising the call (with the
    /// reasoning that led to it), then `tool/call`. The stored log accepts no call it was not
    /// told of; every advertised call gets its `tool/result` before `step/end`.
    fn write_call(
        &mut self,
        call_id: &str,
        name: &str,
        arguments: &Value,
        parent_item_id: Option<&str>,
    ) -> Option<SessionSeq> {
        let arguments = js_json_stringify(&redact_value(arguments));
        let call = ContentBlock::from(ToolCallBlock {
            id: ToolCallId::from(call_id),
            name: name.to_owned(),
            arguments: arguments.clone(),
        });
        self.write_assistant(String::new(), false, vec![call], parent_item_id)?;
        let mut data = json!({
            "turn": self.turn,
            "step": self.step,
            "callId": call_id,
            "name": name,
            "arguments": arguments,
        });
        if let Some(parent_item_id) = parent_item_id {
            data["parentCallId"] = json!(parent_item_id);
        }
        match self.agent.session.append("tool/call", data, None) {
            Ok(event) => Some(event.seq()),
            Err(error) => {
                self.warn("cannot record a step", error);
                None
            }
        }
    }

    fn complete_item(
        &mut self,
        item_id: &str,
        outcome: ItemOutcome,
        output: Option<String>,
        exit_code: Option<i64>,
        parent_item_id: Option<String>,
    ) {
        if !self.settled.insert(format!("item:{item_id}")) {
            return;
        }
        let item = match self.items.iter().position(|item| item.item_id == item_id) {
            Some(index) => self.items.remove(index),
            None => {
                // A completion without a start: record the step it completes.
                let Some(call_seq) =
                    self.write_call(item_id, "step", &json!({}), parent_item_id.as_deref())
                else {
                    return;
                };
                OpenItem {
                    item_id: item_id.to_owned(),
                    call_seq,
                    parent_item_id,
                    command: exit_code.is_some(),
                    guarded: exit_code.is_some(),
                    action: None,
                    output: String::new(),
                }
            }
        };
        let mut text = redact(&output.unwrap_or(item.output));
        if item.command
            && let Some(exit_code) = exit_code
        {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&format!("[Command finished with exit code {exit_code}]"));
        }
        if text.is_empty() {
            text = match outcome {
                ItemOutcome::Completed => String::new(),
                ItemOutcome::Failed => "The action failed.".to_owned(),
                ItemOutcome::Declined => "The action was not allowed.".to_owned(),
                ItemOutcome::Interrupted => "The action was interrupted.".to_owned(),
            };
        }
        let code = match outcome {
            ItemOutcome::Completed => None,
            ItemOutcome::Failed => Some("failed"),
            ItemOutcome::Declined => Some("declined"),
            ItemOutcome::Interrupted => Some("interrupted"),
        };
        self.write_result(
            item_id,
            item.call_seq,
            text,
            code,
            item.parent_item_id.as_deref(),
        );
    }

    fn write_result(
        &self,
        call_id: &str,
        call_seq: SessionSeq,
        text: String,
        error: Option<&str>,
        parent_item_id: Option<&str>,
    ) {
        let message = create_tool_result_message(ToolResultMessageInput {
            call_id: ToolCallId::from(call_id),
            content: vec![ContentBlock::text(text)],
            is_error: error.is_some(),
        });
        let mut data = json!({"turn": self.turn, "step": self.step});
        match serde_json::to_value(&message) {
            Ok(message) => data["message"] = message,
            Err(error) => {
                self.warn("cannot encode a step result", error);
                return;
            }
        }
        if let Some(code) = error {
            data["error"] = json!({"name": "ExternalHarnessError", "code": code});
        }
        if let Some(parent_item_id) = parent_item_id {
            data["parentCallId"] = json!(parent_item_id);
        }
        let intent = SurfaceIntent {
            surface_op: Some(SurfaceOp::append()),
            source_event_seqs: Some(vec![call_seq]),
        };
        if let Err(error) = self.agent.session.append("tool/result", data, Some(intent)) {
            self.warn("cannot record a step result", error);
        }
    }

    /// A per-request signal that also follows the turn.
    fn open_request(&mut self, request_id: &str) -> Option<AbortSignal> {
        if self.requests.contains_key(request_id) {
            return None;
        }
        let controller = Arc::new(AbortController::new());
        if self.signal.aborted() {
            controller.abort();
        }
        let signal = AbortSignal::any([controller.signal(), self.signal.clone()]);
        self.requests.insert(request_id.to_owned(), controller);
        Some(signal)
    }

    /// Answer a permission request in a task of its own; `Some` ends the turn (a repeated id).
    fn spawn_approval(
        &mut self,
        session: &LiveSession,
        request_id: String,
        item_id: Option<String>,
        action: HarnessAction,
        reason: Option<String>,
    ) -> Option<TurnOutcome> {
        let Some(signal) = self.open_request(&request_id) else {
            return Some(self.repeated(&request_id));
        };
        let agent = self.agent.clone();
        let control = session.control.clone();
        // Off in this build unless `never-mode` is built in (and then only for a session the
        // user let run without questions).
        let no_questions = NEVER_MODE && session.escalated && self.never;
        let allowed = self.allowed.clone();
        let id = request_id.clone();
        let label = format!("externalHarness.approval({request_id})");
        let task = self.agent.ctx.spawn(label, move |_| async move {
            let request = PermissionRequest {
                request_id,
                item_id,
                action,
                reason,
            };
            answer_approval(agent, control, no_questions, allowed, request, signal).await;
            Ok(())
        });
        match task {
            Ok(task) => {
                self.tasks.push(task);
                None
            }
            Err(error) => Some(self.unserved(&id, error)),
        }
    }

    /// Put a question to the user in a task of its own; `Some` ends the turn (a repeated id).
    fn spawn_question(
        &mut self,
        session: &LiveSession,
        request_id: String,
        questions: Vec<HarnessQuestion>,
        parent_item_id: Option<String>,
    ) -> Option<TurnOutcome> {
        let Some(signal) = self.open_request(&request_id) else {
            return Some(self.repeated(&request_id));
        };
        let call_id = format!("question-{request_id}");
        let Some(call_seq) = self.write_call(
            &call_id,
            "user_question",
            &json!({"questions": questions}),
            parent_item_id.as_deref(),
        ) else {
            // An unrecorded question is not asked; the harness hears so instead of waiting.
            return self.refuse(session, request_id, "the question could not be recorded");
        };
        lock(&self.questions).insert(call_id.clone(), (call_seq, parent_item_id));
        let asked = AskedQuestion {
            agent: self.agent.clone(),
            control: session.control.clone(),
            turn: self.turn,
            step: self.step,
            request_id: request_id.clone(),
            call_id,
            open: self.questions.clone(),
        };
        let label = format!("externalHarness.question({request_id})");
        let task = self.agent.ctx.spawn(label, move |_| async move {
            answer_question(asked, questions, signal).await;
            Ok(())
        });
        match task {
            Ok(task) => {
                self.tasks.push(task);
                None
            }
            Err(error) => Some(self.unserved(&request_id, error)),
        }
    }

    /// Withdraw a request the turn cannot serve (`cancelled`, in a task of its own); `Some` ends
    /// the turn when not even that task can start.
    fn refuse(
        &mut self,
        session: &LiveSession,
        request_id: String,
        why: &str,
    ) -> Option<TurnOutcome> {
        self.warn(&format!("request {request_id} is withdrawn"), why);
        let (agent, control) = (self.agent.clone(), session.control.clone());
        let id = request_id.clone();
        let label = format!("externalHarness.withdraw({request_id})");
        let task = self.agent.ctx.spawn(label, move |_| async move {
            respond(&agent, &control, request_id, HarnessResponse::Cancelled).await;
            Ok(())
        });
        match task {
            Ok(task) => {
                self.tasks.push(task);
                None
            }
            Err(error) => Some(self.unserved(&id, error)),
        }
    }

    /// A request no task can serve (the agent's context is going away): the turn ends with the
    /// harness stopped, so it does not wait for an answer that never comes.
    fn unserved(&mut self, request_id: &str, error: impl std::fmt::Display) -> TurnOutcome {
        self.broken = true;
        TurnOutcome::Failed(HarnessFailure::new(
            code::CLOSED,
            format!("cannot serve request {request_id}: {error}"),
        ))
    }

    /// Withdraw every open request (the turn is stopping).
    fn withdraw_requests(&mut self) {
        for (_, controller) in self.requests.drain() {
            controller.abort();
        }
    }

    /// Close what the turn left open: requests, answers being written, steps without results.
    /// After a completed turn a text without its final is the whole text; after a stopped or
    /// failed one it is marked interrupted.
    async fn finish(&mut self, completed: bool) {
        self.withdraw_requests();
        let tasks = std::mem::take(&mut self.tasks);
        let settle = futures_util::future::join_all(tasks.into_iter().map(Disposer::dispose));
        if tokio::time::timeout(TASK_SETTLE, settle).await.is_err() {
            self.warn("requests did not settle", "timeout");
        }
        // Questions still unanswered are closed here, before `step/end`; a late answer finds
        // its call gone and writes nothing.
        let questions = self.questions.clone();
        let mut open = lock(&questions);
        for (call_id, (call_seq, parent_item_id)) in open.drain() {
            self.write_result(
                &call_id,
                call_seq,
                "The question was withdrawn before it was answered.".to_owned(),
                Some("interrupted"),
                parent_item_id.as_deref(),
            );
        }
        drop(open);
        for live in std::mem::take(&mut self.live) {
            self.settled.insert(format!("text:{}", live.message_id));
            let live = self.shown_whole(live);
            let seq = self.write_assistant(live.text.clone(), !completed, Vec::new(), None);
            self.end_live(live, seq);
        }
        // Reasoning no text followed (a turn of steps only) is kept as a message of its own.
        while let Some(parent_item_id) = self
            .reasoning
            .first()
            .map(|(_, _, parent_item_id)| parent_item_id.clone())
        {
            self.write_assistant(
                String::new(),
                !completed,
                Vec::new(),
                parent_item_id.as_deref(),
            );
        }
        for item in std::mem::take(&mut self.items) {
            self.settled.insert(format!("item:{}", item.item_id));
            self.write_result(
                &item.item_id,
                item.call_seq,
                "The action was interrupted before the harness reported its outcome.".to_owned(),
                Some("interrupted"),
                item.parent_item_id.as_deref(),
            );
        }
    }
}

/// A native permission request, as [`HarnessEvent::ApprovalRequested`] carries it.
struct PermissionRequest {
    request_id: String,
    item_id: Option<String>,
    action: HarnessAction,
    reason: Option<String>,
}

/// Decide a permission request and send the answer. An allowed step's item id is recorded in
/// `allowed` with its action before the answer goes out: the harness may run it, and only it,
/// after the answer.
async fn answer_approval(
    agent: Arc<ExternalAgent>,
    control: Arc<dyn HarnessSession>,
    no_questions: bool,
    allowed: Arc<Mutex<HashMap<String, HarnessAction>>>,
    asked: PermissionRequest,
    signal: AbortSignal,
) {
    let PermissionRequest {
        request_id,
        item_id,
        action,
        reason,
    } = asked;
    let danger = agent.danger(&action).await;
    // The user let this native session run without questions (`approval: never`, allowed in
    // the window): it still asks for anything the classifier finds dangerous, for any request
    // of a wider permission or tool, and for anything outside the session folder. Widening only
    // the sandbox or the network answers nothing by itself.
    let by_itself =
        no_questions && danger.is_none() && agent.within_workspace(action.clone()).await;
    let decision = if by_itself {
        Some(ApprovalDecision::Allow)
    } else {
        // A question the classifier raised is a judged one; a plain one is not judged at all.
        let assessed = danger.is_some();
        let question = match (danger, &action) {
            // The classifier names only the dangerous files; the question shows every change.
            (Some(mut question), HarnessAction::FileChange { changes }) => {
                question.details = truncate_chars(&change_list(changes), MAX_DETAILS_CHARS);
                question
            }
            (Some(question), _) => question,
            (None, _) => plain_question(&agent.shared.name, &action, reason.as_deref()),
        };
        let question = flag_incomplete(question, &action);
        match agent.as_dyn() {
            // The agent is going away: the request is withdrawn, not left unanswered.
            None => None,
            Some(subject) => {
                let mut request = ApprovalRequest::new(subject, approval_tool(&action));
                request.call_id = item_id.clone().map(ToolCallId::from);
                request.reason = Some(question.explanation).filter(|text| !text.is_empty());
                request.details = Some(question.details);
                let mut headline = DisplayReason::new(question.headline);
                if !assessed {
                    // No judgment of the risk behind this question: the window says so. The key is
                    // that of `hoh-user-approval`'s `host.rs` (`UNASSESSED_RISK_KEY`).
                    headline.insert("x-risk", "unassessed");
                }
                request.display_reason = Some(headline);
                request.signal = Some(signal.clone());
                match agent.shared.approval.request(request).await {
                    Ok(ApprovalOutcome::AllowedOnce) => Some(ApprovalDecision::Allow),
                    Ok(ApprovalOutcome::Rejected | ApprovalOutcome::Unavailable) => {
                        Some(ApprovalDecision::Deny)
                    }
                    Ok(ApprovalOutcome::Cancelled) => None,
                    Err(error) => {
                        agent
                            .ctx
                            .logger()
                            .warn(format_args!("permission request {request_id}: {error}"));
                        Some(ApprovalDecision::Deny)
                    }
                }
            }
        }
    };
    if signal.aborted() {
        // The harness withdrew the request, or the turn is stopping: nobody waits for it.
        return;
    }
    if decision == Some(ApprovalDecision::Allow)
        && let Some(item_id) = item_id
    {
        lock(&allowed).insert(item_id, action);
    }
    let response = match decision {
        Some(decision) => HarnessResponse::Approval { decision },
        None => HarnessResponse::Cancelled,
    };
    respond(&agent, &control, request_id, response).await;
}

/// A question of the harness on its way to the user.
struct AskedQuestion {
    agent: Arc<ExternalAgent>,
    control: Arc<dyn HarnessSession>,
    turn: u64,
    step: u64,
    request_id: String,
    call_id: String,
    open: OpenQuestions,
}

async fn answer_question(
    asked: AskedQuestion,
    questions: Vec<HarnessQuestion>,
    signal: AbortSignal,
) {
    let AskedQuestion {
        agent,
        control,
        turn,
        step,
        request_id,
        call_id,
        open,
    } = asked;
    let log = TurnLog::new(agent.clone(), turn, step, signal.clone());
    let items = questions
        .iter()
        .map(|question| AskUserQuestionItem {
            id: question.id.clone(),
            question: redact(&question.question),
            detail: None,
            header: question.header.as_deref().map(redact),
            options: (!question.options.is_empty()).then(|| {
                question
                    .options
                    .iter()
                    .map(|option| AskUserQuestionOption {
                        label: option.label.clone(),
                        description: option.description.as_deref().map(redact),
                    })
                    .collect()
            }),
            multi_select: question.multi_select.then_some(true),
            intent: None,
        })
        .collect();
    let asked = agent
        .shared
        .questions
        .ask(AskUserQuestionRequest {
            questions: items,
            agent: agent.as_dyn(),
            signal: Some(signal.clone()),
            wait: None,
        })
        .await;
    let (text, error, response) = match asked {
        Ok(answer) => {
            let text = serde_json::to_value(&answer)
                .map(|value| js_json_stringify(&redact_value(&value)))
                .unwrap_or_default();
            let response = HarnessResponse::Answer {
                answers: answer
                    .answers
                    .into_iter()
                    .map(|item| QuestionAnswer {
                        id: item.id,
                        selected: item.selected,
                        custom: item.custom,
                    })
                    .collect(),
            };
            (text, None, response)
        }
        Err(error) => (
            redact(&format!("The question was not answered: {error}")),
            Some("unanswered"),
            HarnessResponse::Cancelled,
        ),
    };
    {
        // Under the lock: the end of the turn either waits for this result or has closed the
        // call already (then nothing is written after `step/end`).
        let mut open = lock(&open);
        let Some((call_seq, parent_item_id)) = open.remove(&call_id) else {
            return;
        };
        log.write_result(&call_id, call_seq, text, error, parent_item_id.as_deref());
    }
    if signal.aborted() {
        return;
    }
    respond(&agent, &control, request_id, response).await;
}

async fn respond(
    agent: &ExternalAgent,
    control: &Arc<dyn HarnessSession>,
    request_id: String,
    response: HarnessResponse,
) {
    let params = RespondParams {
        session_id: agent.id.to_string(),
        request_id: request_id.clone(),
        response,
    };
    if let Err(error) = control.respond(params).await {
        agent
            .ctx
            .logger()
            .warn(format_args!("cannot answer request {request_id}: {error}"));
    }
}

// ---- the Agent face ------------------------------------------------------------------------

impl Agent for ExternalAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn scope_key(&self) -> ScopeKey {
        self.scope_key.clone()
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
        *self.status.borrow()
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, cause: AgentCancelCause, options: CancelOptions) -> Result<(), BoxError> {
        if !options.keep_inbox {
            self.inbox.clear()?;
            lock(&self.state).wake_pending = false;
        }
        let abort = {
            let mut state = lock(&self.state);
            match (&state.abort, &state.cause) {
                (Some(abort), None) => {
                    let abort = abort.clone();
                    state.cause = Some(cause.clone());
                    Some(abort)
                }
                _ => None,
            }
        };
        if let Some(abort) = abort {
            abort.abort_with(CancelCauseError(cause));
        }
        // An idle driver re-reads the inbox and reports idle.
        self.wake.notify_one();
        Ok(())
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        let mut status = self.status.subscribe();
        Box::pin(async move {
            let _ = status.wait_for(|status| *status == AgentStatus::Idle).await;
        })
    }

    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        if self.status() == AgentStatus::Running {
            return Err(format!("agent \"{}\" is running a turn", self.id).into());
        }
        Ok(task(AbortController::new().signal()))
    }

    fn send(
        &self,
        message: UserMessage,
        target: InboxTarget,
        wakeup: bool,
    ) -> Result<(), BoxError> {
        self.inbox.append(target, message)?;
        if wakeup {
            lock(&self.state).wake_pending = true;
            self.emit_status(AgentStatus::Running);
            self.wake.notify_one();
        }
        Ok(())
    }

    fn followup(&self, message: UserMessage) -> Result<(), BoxError> {
        self.send(message, InboxTarget::NextTurn, true)
    }

    /// Steering a running native turn is not part of the first slice: the message becomes the
    /// next turn.
    fn steer(&self, message: UserMessage) -> Result<(), BoxError> {
        self.send(message, InboxTarget::NextTurn, true)
    }

    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        self.send(message, InboxTarget::NextStep, false)
    }
}

/// The model choice of an external session: the provider is the harness itself.
struct ExternalModel {
    provider: String,
    model: Mutex<Option<String>>,
    /// The model changed: the next turn starts the native session anew (continuing it).
    restart: AtomicBool,
    log: hoh_kernel::Logger,
}

impl Model for ExternalModel {
    fn current(&self) -> Option<ModelSelection> {
        Some(ModelSelection {
            provider: self.provider.clone(),
            model: lock(&self.model)
                .clone()
                .filter(|model| !model.is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
            reasoning_effort: None,
        })
    }

    fn select(&self, selection: &ModelSelection) -> Result<(), BoxError> {
        if selection.provider != self.provider {
            self.log.warn(format_args!(
                "external harness \"{}\" ignores a model selection for provider \"{}\"",
                self.provider, selection.provider
            ));
            return Ok(());
        }
        let mut model = lock(&self.model);
        if model.as_deref() != Some(selection.model.as_str()) {
            *model = Some(selection.model.clone());
            self.restart.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// The pending input of an external session (process-local).
#[derive(Default)]
struct ExternalInbox {
    state: Mutex<InboxState>,
}

/// The input one turn took from the inbox.
pub(crate) struct TurnInput {
    messages: Vec<UserMessage>,
    /// The last message came from the next-turn list (the others from the next-step list).
    last_from_next_turn: bool,
}

impl ExternalInbox {
    /// The next turn's input: all next-step input plus the first next-turn prompt; next-step
    /// input alone starts a turn only after a wake.
    fn take_turn(&self, woken: bool) -> TurnInput {
        let mut state = lock(&self.state);
        if state.next_turn.is_empty() && !woken {
            return TurnInput {
                messages: Vec::new(),
                last_from_next_turn: false,
            };
        }
        let mut messages: Vec<UserMessage> = state.next_step.drain(..).collect();
        let last_from_next_turn = !state.next_turn.is_empty();
        if last_from_next_turn {
            messages.push(state.next_turn.remove(0));
        }
        TurnInput {
            messages,
            last_from_next_turn,
        }
    }

    /// Put a turn's input back where it was taken from, ahead of what came since.
    fn give_back(&self, input: TurnInput) {
        let mut state = lock(&self.state);
        let mut messages = input.messages;
        if input.last_from_next_turn
            && let Some(last) = messages.pop()
        {
            state.next_turn.insert(0, last);
        }
        state.next_step.splice(0..0, messages);
    }
}

fn list_of(state: &mut InboxState, target: InboxTarget) -> &mut Vec<UserMessage> {
    match target {
        InboxTarget::NextTurn => &mut state.next_turn,
        InboxTarget::NextStep => &mut state.next_step,
    }
}

/// `ToIntegerOrInfinity` of JavaScript.
fn js_integer(value: f64) -> f64 {
    if value.is_nan() { 0.0 } else { value.trunc() }
}

impl Inbox for ExternalInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        lock(&self.state).next_turn.clone()
    }
    fn next_step(&self) -> Vec<UserMessage> {
        lock(&self.state).next_step.clone()
    }
    fn clear(&self) -> Result<(), BoxError> {
        let mut state = lock(&self.state);
        state.next_step.clear();
        state.next_turn.clear();
        Ok(())
    }
    fn append(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
        list_of(&mut lock(&self.state), target).push(message);
        Ok(())
    }
    fn prepend(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
        list_of(&mut lock(&self.state), target).insert(0, message);
        Ok(())
    }
    fn replace(&self, message_id: &MessageId, new_message: UserMessage) -> Result<bool, BoxError> {
        let mut state = lock(&self.state);
        for target in [InboxTarget::NextStep, InboxTarget::NextTurn] {
            if let Some(slot) = list_of(&mut state, target)
                .iter_mut()
                .find(|message| &message.id == message_id)
            {
                *slot = new_message;
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn remove(&self, message_id: &MessageId) -> Result<bool, BoxError> {
        let mut state = lock(&self.state);
        for target in [InboxTarget::NextStep, InboxTarget::NextTurn] {
            let list = list_of(&mut state, target);
            if let Some(index) = list.iter().position(|message| &message.id == message_id) {
                list.remove(index);
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn splice(
        &self,
        target: InboxTarget,
        start: f64,
        delete_count: f64,
        inserted: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        let mut state = lock(&self.state);
        let list = list_of(&mut state, target);
        let len = list.len() as f64;
        let start = js_integer(start);
        let from = if start < 0.0 {
            (len + start).max(0.0)
        } else {
            start.min(len)
        };
        let count = js_integer(delete_count).max(0.0).min(len - from);
        // Both are whole numbers within `0..=len` here.
        let (from, count) = (from as usize, count as usize);
        Ok(list.splice(from..from + count, inserted).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::DEFAULT_ACCESS;
    use hoh_llm::{MessageSource, NewUserMessage, create_user_message};

    /// CEO 20.11: the words the window shows for Claude, exactly.
    #[test]
    fn the_window_gets_the_ceos_words_for_claude() {
        assert_eq!(
            never_off("claude"),
            "Этот режим пока недоступен для Claude: действия идут через подтверждение"
        );
        assert_eq!(
            access_text("claude", AccessMode::Full),
            "Полный доступ: Claude выполняет команды без вопросов"
        );
        assert_eq!(
            access_text("claude", AccessMode::Confirm),
            "С подтверждением: команды, правки файлов и сеть Claude выполняет после вашего \
             подтверждения"
        );
        assert_eq!(DEFAULT_ACCESS, AccessMode::Confirm);
        assert_eq!(AccessMode::default(), DEFAULT_ACCESS);
    }

    /// `confirm` caps a configured sandbox and network; `full` runs without them.
    #[test]
    fn the_launch_policy_follows_the_users_mode() {
        let wide = HarnessPolicy {
            sandbox: SandboxMode::FullAccess,
            approval: ApprovalMode::OnRequest,
            network: NetworkAccess::Allowed,
        };
        assert_eq!(
            launch_policy(&wide, AccessMode::Confirm),
            HarnessPolicy::default()
        );
        let read_only = HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            ..HarnessPolicy::default()
        };
        assert_eq!(launch_policy(&read_only, AccessMode::Confirm), read_only);
        // Full access does not widen a read-only session (review 232500, item 4).
        assert_eq!(launch_policy(&read_only, AccessMode::Full), read_only);
        let reviewer = HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            approval: ApprovalMode::OnRequest,
            network: NetworkAccess::Allowed,
        };
        assert_eq!(launch_policy(&reviewer, AccessMode::Full), reviewer);
        assert_eq!(
            launch_policy(&HarnessPolicy::default(), AccessMode::Full),
            wide
        );
        // The two modes between them (CEO 23.11 00:07) cap the configuration as confirm does.
        for between in [AccessMode::AcceptEdits, AccessMode::Auto] {
            assert_eq!(launch_policy(&wide, between), HarnessPolicy::default());
            assert_eq!(
                launch_policy(&reviewer, between),
                launch_policy(&reviewer, AccessMode::Confirm)
            );
        }
    }

    /// The four modes in their order from the narrowest, by the window's names; only full access
    /// stops the questions.
    #[test]
    fn the_four_modes_keep_their_order_and_names() {
        let modes = [
            AccessMode::Confirm,
            AccessMode::AcceptEdits,
            AccessMode::Auto,
            AccessMode::Full,
        ];
        assert!(modes.windows(2).all(|pair| pair[0] < pair[1]));
        for mode in modes {
            let wire = serde_json::to_value(mode).unwrap();
            assert_eq!(wire, mode.as_str());
            assert_eq!(serde_json::from_value::<AccessMode>(wire).unwrap(), mode);
            assert_eq!(mode.asks(), mode != AccessMode::Full);
            assert!(access_text("claude", mode).contains("Claude"));
        }
        assert_eq!(
            modes.map(AccessMode::as_str),
            ["confirm", "acceptEdits", "auto", "full"]
        );
    }

    fn message(text: &str) -> UserMessage {
        create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(text)],
            source: MessageSource::new("user"),
        })
    }

    fn texts(list: &[UserMessage]) -> Vec<String> {
        list.iter()
            .map(|message| {
                message
                    .content
                    .iter()
                    .filter_map(ContentBlock::text_value)
                    .collect()
            })
            .collect()
    }

    fn inbox(next_step: &[&str], next_turn: &[&str]) -> ExternalInbox {
        let inbox = ExternalInbox::default();
        for text in next_step {
            inbox
                .append(InboxTarget::NextStep, message(text))
                .expect("append");
        }
        for text in next_turn {
            inbox
                .append(InboxTarget::NextTurn, message(text))
                .expect("append");
        }
        inbox
    }

    #[test]
    fn a_turn_takes_all_step_input_and_one_turn_prompt() {
        let inbox = inbox(&["s1", "s2"], &["t1", "t2"]);
        let input = inbox.take_turn(false);
        assert_eq!(texts(&input.messages), ["s1", "s2", "t1"]);
        assert!(input.last_from_next_turn);
        assert_eq!(texts(&inbox.next_turn()), ["t2"]);
        // Step input alone waits for a wake.
        let inbox = self::inbox(&["s1"], &[]);
        assert!(inbox.take_turn(false).messages.is_empty());
        let input = inbox.take_turn(true);
        assert_eq!(texts(&input.messages), ["s1"]);
        assert!(!input.last_from_next_turn);
    }

    #[test]
    fn given_back_input_returns_ahead_of_what_came_since() {
        let inbox = inbox(&["s1"], &["t1", "t2"]);
        let input = inbox.take_turn(false);
        inbox
            .append(InboxTarget::NextStep, message("s-new"))
            .expect("append");
        inbox.give_back(input);
        assert_eq!(texts(&inbox.next_step()), ["s1", "s-new"]);
        assert_eq!(texts(&inbox.next_turn()), ["t1", "t2"]);
    }

    #[test]
    fn splice_follows_javascript_semantics() {
        let inbox = inbox(&[], &["a", "b", "c", "d"]);
        let removed = inbox
            .splice(InboxTarget::NextTurn, -2.0, 1.0, vec![message("x")])
            .expect("splice");
        assert_eq!(texts(&removed), ["c"]);
        assert_eq!(texts(&inbox.next_turn()), ["a", "b", "x", "d"]);
        // Start past the end appends; a negative count removes nothing; NaN counts as 0.
        inbox
            .splice(InboxTarget::NextTurn, 99.0, -5.0, vec![message("e")])
            .expect("splice");
        let removed = inbox
            .splice(InboxTarget::NextTurn, f64::NAN, 2.5, Vec::new())
            .expect("splice");
        assert_eq!(texts(&removed), ["a", "b"]);
        assert_eq!(texts(&inbox.next_turn()), ["x", "d", "e"]);
    }

    #[test]
    fn replace_and_remove_find_a_message_in_either_list() {
        let inbox = inbox(&["s1"], &["t1"]);
        let step_id = inbox.next_step()[0].id.clone();
        let turn_id = inbox.next_turn()[0].id.clone();
        assert!(inbox.replace(&turn_id, message("t1b")).expect("replace"));
        assert_eq!(texts(&inbox.next_turn()), ["t1b"]);
        assert!(inbox.remove(&step_id).expect("remove"));
        assert!(!inbox.remove(&step_id).expect("remove"));
        assert!(inbox.next_step().is_empty());
        inbox
            .prepend(InboxTarget::NextTurn, message("t0"))
            .expect("prepend");
        assert_eq!(texts(&inbox.next_turn()), ["t0", "t1b"]);
        inbox.clear().expect("clear");
        assert!(inbox.next_turn().is_empty());
    }
}
